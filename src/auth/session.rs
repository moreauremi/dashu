//! Contexte d'une requête authentifiée.
//!
//! [`Ctx`] est l'extracteur que prennent tous les handlers protégés : il
//! garantit qu'un utilisateur est connecté et charge ce dont chaque page a
//! besoin (utilisateur, espaces, espace actif, jeton CSRF). Un handler qui ne
//! le demande pas n'a accès à aucune donnée utilisateur ; toutes les requêtes
//! SQL filtrent ensuite sur `ctx.user.id`.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderName, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use tower_sessions::cookie::time::Duration;
use tower_sessions::{Expiry, Session};

use crate::AppState;
use crate::spaces::{self, Space, SpaceTag, url};
use crate::tabbar::{self, Tab};
use crate::web::{self, AppError, AppResult, csrf};

/// Clés du contenu de la session (stocké côté serveur, dans SQLite).
pub const USER_ID: &str = "uid";
pub const ACTIVE_SPACE: &str = "space";
const FLASH: &str = "flash";

/// Durée d'une session connectée : 30 jours glissants (repoussés à chaque visite).
pub const LOGGED_IN_DAYS: i64 = 30;
/// Une session anonyme ne sert qu'à porter le jeton CSRF du formulaire de
/// connexion : elle expire vite pour ne pas remplir la table.
pub const ANONYMOUS_HOURS: i64 = 1;

#[derive(Debug, Clone)]
pub struct User {
    pub id: i64,
    pub email: String,
    /// `dark`, `light`, ou `None` pour suivre le réglage du système.
    pub theme: Option<String>,
    pub has_password: bool,
    pub google_linked: bool,
    /// Raccourcis du téléphone (voir [`crate::tabbar`]), `None` = par défaut.
    pub mobile_tabs: Option<String>,
    pub first_name: String,
    pub last_name: String,
    /// Fond de l'avatar par défaut : un accent (voir `spaces::COLORS`).
    pub avatar_color: String,
    pub has_avatar: bool,
    pub avatar_version: i64,
    /// `small`, `large`, ou `None` pour la taille normale.
    pub ui_size: Option<String>,
    pub sidebar_floating: bool,
}

impl User {
    /// « Prénom Nom », ou l'email tant que le profil est vide.
    pub fn display_name(&self) -> String {
        let name = format!("{} {}", self.first_name, self.last_name);
        match name.trim() {
            "" => self.email.clone(),
            name => name.to_string(),
        }
    }

    /// Initiales de l'avatar par défaut : celles du prénom et du nom, ou la
    /// première lettre de l'email.
    pub fn initials(&self) -> String {
        let first = |s: &str| s.trim().chars().next();
        let letters: String = [first(&self.first_name), first(&self.last_name)]
            .into_iter()
            .flatten()
            .collect();
        let letters = if letters.is_empty() {
            first(&self.email).map(String::from).unwrap_or_default()
        } else {
            letters
        };
        letters.to_uppercase()
    }

    /// Adresse de la photo de profil, avec sa version (cache d'un an).
    pub fn avatar_url(&self) -> Option<String> {
        self.has_avatar
            .then(|| format!("/profile/avatar?v={}", self.avatar_version))
    }
}

/// Message affiché une seule fois sur la page suivante (après une redirection).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flash {
    /// `ok` ou `error`.
    pub kind: String,
    pub text: String,
}

pub struct Ctx {
    pub user: User,
    pub spaces: Vec<Space>,
    pub active: Space,
    pub csrf: String,
    pub session: Session,
    /// Chemin + query de la requête, pour y revenir après un changement d'espace.
    pub path: String,
    /// Chargement complet de la page (adresse tapée, rechargement, ouverture
    /// de l'app), par opposition à une requête htmx : navigation, fragment.
    pub launch: bool,
    /// Conversions en cours (pages seulement, pas les fragments), et si l'une
    /// part d'un fichier de l'onglet Outils.
    pub jobs: (i64, bool),
}

/// Ce que le gabarit `layout.html` affiche autour de chaque page.
pub struct Shell {
    /// Entrée de menu active : `dashboard`, `notes`, `calendar`, `settings`.
    pub nav: &'static str,
    pub email: String,
    /// « Prénom Nom », ou l'email si le profil est vide.
    pub name: String,
    /// Avatar : photo s'il y en a une, sinon initiales sur un fond coloré.
    pub initials: String,
    pub avatar_color: String,
    pub avatar_url: Option<String>,
    /// Taille de l'interface : `small`, `large`, ou vide (normale).
    pub ui_size: String,
    /// Barre latérale détachée (cachée à gauche) plutôt qu'ancrée.
    pub sidebar_floating: bool,
    /// `dark`, `light` ou chaîne vide (suivre le système).
    pub theme: String,
    pub spaces: Vec<Space>,
    pub active: Space,
    pub csrf: String,
    pub path: String,
    pub flash: Option<Flash>,
    /// Barre de raccourcis du téléphone.
    pub tabs: Vec<&'static Tab>,
    /// Écran de lancement (logo sur fond noir) : seulement au chargement
    /// complet, jamais pendant une navigation htmx.
    pub launch: bool,
    /// Début des adresses de l'espace actif : `/e/ecole`.
    pub base: String,
    /// Conversions en cours : l'indicateur de la barre latérale.
    pub jobs_active: i64,
    /// L'une d'elles part de l'onglet Outils : le lien y mène.
    pub jobs_tools: bool,
}

impl Ctx {
    /// Prépare l'habillage d'une page entière. Consomme le message flash en
    /// attente : à n'appeler que pour une page, pas pour un fragment.
    pub async fn shell(&self, nav: &'static str) -> AppResult<Shell> {
        let flash = self.session.remove::<Flash>(FLASH).await?;
        Ok(Shell {
            nav,
            email: self.user.email.clone(),
            name: self.user.display_name(),
            initials: self.user.initials(),
            avatar_color: self.user.avatar_color.clone(),
            avatar_url: self.user.avatar_url(),
            ui_size: self.user.ui_size.clone().unwrap_or_default(),
            sidebar_floating: self.user.sidebar_floating,
            theme: self.user.theme.clone().unwrap_or_default(),
            spaces: self.spaces.clone(),
            base: format!("/e/{}", self.active.slug),
            active: self.active.clone(),
            csrf: self.csrf.clone(),
            path: self.path.clone(),
            flash,
            tabs: tabbar::selected(self.user.mobile_tabs.as_deref()),
            launch: self.launch,
            jobs_active: self.jobs.0,
            jobs_tools: self.jobs.1,
        })
    }

    pub async fn flash_ok(&self, text: impl Into<String>) -> AppResult<()> {
        set_flash(&self.session, "ok", text).await
    }

    pub async fn flash_error(&self, text: impl Into<String>) -> AppResult<()> {
        set_flash(&self.session, "error", text).await
    }

    /// Espace de l'utilisateur portant cet identifiant, s'il existe.
    pub fn space_by_id(&self, id: i64) -> Option<&Space> {
        self.spaces.iter().find(|s| s.id == id)
    }

    /// Espace dont montrer le contenu : `None` pour tous (Perso, vue
    /// d'ensemble), sauf si `only` demande le seul contenu propre de Perso.
    pub fn scope(&self, only: bool) -> Option<i64> {
        (only || !self.active.is_overview()).then_some(self.active.id)
    }

    /// Pastille d'un élément dont l'espace n'est pas l'espace actif.
    pub fn tag_of(&self, space_id: i64) -> Option<SpaceTag> {
        if space_id == self.active.id {
            return None;
        }
        self.space_by_id(space_id).map(|s| SpaceTag {
            name: s.name.clone(),
            color: s.color.clone(),
        })
    }
}

pub async fn set_flash(session: &Session, kind: &str, text: impl Into<String>) -> AppResult<()> {
    session
        .insert(
            FLASH,
            Flash {
                kind: kind.into(),
                text: text.into(),
            },
        )
        .await?;
    Ok(())
}

/// Tire au hasard (par SQLite) la couleur de l'avatar par défaut et la garde.
async fn draw_avatar_color(db: &sqlx::SqlitePool, user_id: i64) -> AppResult<String> {
    let draw: i64 = sqlx::query_scalar!(r#"SELECT abs(random() % 1000) AS "n!: i64""#)
        .fetch_one(db)
        .await?;
    let index = usize::try_from(draw).unwrap_or(0) % spaces::COLORS.len();
    let color = spaces::COLORS[index].0;
    sqlx::query!(
        "UPDATE users SET avatar_color = ? WHERE id = ? AND avatar_color = ''",
        color,
        user_id
    )
    .execute(db)
    .await?;
    Ok(color.to_string())
}

/// Ouvre la session d'un utilisateur qui vient de s'authentifier.
pub async fn sign_in(state: &AppState, session: &Session, user_id: i64) -> AppResult<()> {
    // Nouvel identifiant de session et nouveau jeton CSRF : un identifiant
    // connu avant la connexion (fixation de session) ne vaut plus rien.
    session.cycle_id().await?;
    session.insert(USER_ID, user_id).await?;
    session.remove::<String>(ACTIVE_SPACE).await?;
    csrf::rotate(session).await?;
    session.set_expiry(Some(Expiry::OnInactivity(Duration::days(LOGGED_IN_DAYS))));
    // Les trois espaces existent dès la première connexion.
    spaces::ensure_defaults(&state.db, user_id).await?;
    Ok(())
}

impl FromRequestParts<AppState> for Ctx {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let session = Session::from_request_parts(parts, state)
            .await
            .map_err(IntoResponse::into_response)?;

        let internal = |e: AppError| e.into_response();

        let user_id = session
            .get::<i64>(USER_ID)
            .await
            .map_err(|e| internal(e.into()))?;
        let Some(user_id) = user_id else {
            return Err(login_redirect(parts));
        };

        let user = sqlx::query_as!(
            User,
            r#"SELECT id AS "id!", email, theme,
                      password_hash IS NOT NULL AS "has_password!: bool",
                      google_sub IS NOT NULL AS "google_linked!: bool",
                      mobile_tabs, first_name, last_name, avatar_color,
                      avatar IS NOT NULL AS "has_avatar!: bool", avatar_version,
                      ui_size, sidebar_floating AS "sidebar_floating!: bool"
               FROM users WHERE id = ?"#,
            user_id
        )
        .fetch_optional(&state.db)
        .await
        .map_err(|e| internal(e.into()))?;

        // Compte supprimé, ou email retiré de ALLOWED_EMAILS : la session ne
        // vaut plus rien, même si son cookie est encore valide.
        let mut user = match user {
            Some(user) if state.config.is_allowed(&user.email) => user,
            _ => {
                let _ = session.flush().await;
                return Err(login_redirect(parts));
            }
        };

        // Fond de l'avatar par défaut : tiré au hasard la première fois, puis gardé.
        if user.avatar_color.is_empty() {
            user.avatar_color = draw_avatar_color(&state.db, user.id)
                .await
                .map_err(internal)?;
        }

        let spaces = spaces::list(&state.db, user.id).await.map_err(internal)?;
        let wanted = session
            .get::<String>(ACTIVE_SPACE)
            .await
            .map_err(|e| internal(e.into()))?;
        // L'espace actif : celui de l'adresse (`/e/<slug>/…`, retenu comme
        // dernier espace utilisé), sinon celui de l'onglet (en-tête des
        // requêtes htmx), sinon celui de la session.
        let from_url = parts.extensions.get::<url::UrlSpace>().map(|s| s.0.clone());
        let from_tab = parts
            .headers
            .get(url::HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let by_slug = |slug: &str| spaces.iter().find(|s| s.slug == slug);
        let active = if let Some(slug) = from_url {
            // Un espace inconnu (ou d'un autre compte) dans l'adresse : 404.
            let space = by_slug(&slug)
                .cloned()
                .ok_or_else(|| AppError::NotFound.into_response())?;
            if wanted.as_deref() != Some(slug.as_str()) {
                session
                    .insert(ACTIVE_SPACE, &slug)
                    .await
                    .map_err(|e| internal(e.into()))?;
            }
            space
        } else {
            from_tab
                .as_deref()
                .and_then(by_slug)
                .or_else(|| wanted.as_deref().and_then(by_slug))
                .or_else(|| spaces.first())
                .cloned()
                .ok_or_else(|| {
                    internal(AppError::Internal(anyhow::anyhow!(
                        "utilisateur sans espace"
                    )))
                })?
        };

        let csrf = csrf::token(&session).await.map_err(internal)?;
        let path = parts
            .uri
            .path_and_query()
            .map(|p| p.as_str().to_string())
            .unwrap_or_else(|| "/".into());
        // htmx joint HX-Request à tout ce qu'il demande (navigation hx-boost,
        // retour arrière, fragment) : sans cet en-tête, la page se lance.
        let launch = !parts.headers.contains_key("hx-request");
        // Les conversions en cours ne servent qu'à l'habillage d'une page.
        let jobs = if state.config.work_dir.is_some() && !web::is_fragment_request(&parts.headers) {
            crate::tools::active_jobs(&state.db, user.id)
                .await
                .map_err(internal)?
        } else {
            (0, false)
        };

        Ok(Self {
            user,
            spaces,
            active,
            csrf,
            session,
            path,
            launch,
            jobs,
        })
    }
}

/// Renvoie vers la page de connexion.
///
/// Pour un fragment htmx, une redirection classique insérerait la page de
/// connexion au milieu d'un widget : on répond donc 401 avec l'en-tête
/// `HX-Redirect`, que htmx traduit en vraie navigation.
fn login_redirect(parts: &Parts) -> Response {
    if web::is_fragment_request(&parts.headers) {
        (
            StatusCode::UNAUTHORIZED,
            [(HeaderName::from_static("hx-redirect"), "/login")],
        )
            .into_response()
    } else {
        web::see_other("/login")
    }
}
