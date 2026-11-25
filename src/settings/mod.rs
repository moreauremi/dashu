//! Réglages : thème, espaces (gérés par `crate::spaces`), raccourcis du
//! téléphone, mot de passe, compte Google lié, sources iCal, connexions.

pub mod connexions;
pub mod instance;
pub mod profile;
pub mod space_tools;

use std::collections::HashMap;

use askama::Template;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::Deserialize;

use crate::AppState;
use crate::auth::password;
use crate::auth::session::{Ctx, Shell};
use crate::calendar::sync;
use crate::registry::{self, Shown};
use crate::spaces::{self, person_name};
use crate::tabbar::{self, Tab};
use crate::time;
use crate::web::{self, AppError, AppResult, local_path, see_other};

const SOURCE_NAME_MAX_CHARS: usize = 60;

struct SourceRow {
    id: i64,
    name: String,
    /// Domaine seul : le chemin de l'URL porte un jeton d'accès et reste caché.
    masked_url: String,
    space_name: String,
    space_color: String,
    status: String,
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "settings/page.html")]
struct SettingsPage {
    shell: Shell,
    has_password: bool,
    google_linked: bool,
    google_enabled: bool,
    /// Google Agenda relié : l'adresse du compte, et la dernière erreur.
    google_calendar: Option<crate::calendar::google::Linked>,
    /// Événements pas encore arrivés dans Google Agenda.
    google_pending: i64,
    sources: Vec<SourceRow>,
    min_password: usize,
    /// Toutes les entrées possibles de la barre du téléphone, et si elles y sont.
    tab_choices: Vec<(&'static Tab, bool)>,
    tab_max: usize,
    first_name: String,
    last_name: String,
    profile_max: usize,
    /// Couleur et icône présélectionnées pour un nouvel espace.
    new_color: &'static str,
    new_icon: &'static str,
    max_spaces: usize,
    name_max: usize,
    /// Réglage « espace du moment » : l'espace des jours sans cours, 0 si
    /// le réglage est désactivé.
    moment_id: i64,
    /// Invitations reçues, en attente.
    invitations: Vec<InvitationRow>,
    /// Mention de partage de chaque espace (« Partagé par Alice »).
    shared: HashMap<i64, String>,
    /// Carte Connexions, déjà rendue (settings/_connexions.html).
    connexions_html: String,
    /// Carte Instance (settings/_instance.html), déjà rendue ; vide sauf
    /// pour l'administrateur.
    instance_html: String,
    /// Éléments dans la corbeille.
    trash_count: usize,
    /// Outils de chaque espace en service : réglables, et affichés ou non.
    space_tools: HashMap<i64, Shown>,
}

pub(crate) struct InvitationRow {
    pub(crate) id: i64,
    pub(crate) space_name: String,
    pub(crate) space_color: String,
    pub(crate) space_icon: String,
    /// Qui invite.
    pub(crate) from: String,
}

/// Invitations reçues par un compte, en attente. Aussi pour l'API.
pub(crate) async fn received_invitations(
    db: &sqlx::SqlitePool,
    user: i64,
) -> AppResult<Vec<InvitationRow>> {
    let invitations = sqlx::query!(
        r#"SELECT i.id AS "id!", s.name AS space_name, s.color, s.icon,
                  u.email, u.first_name, u.last_name
           FROM space_invitations i
           JOIN spaces s ON s.id = i.space_id
           JOIN users u ON u.id = i.invited_by
           WHERE i.user_id = ? ORDER BY i.id"#,
        user
    )
    .fetch_all(db)
    .await?
    .into_iter()
    .map(|i| InvitationRow {
        id: i.id,
        space_name: i.space_name,
        space_color: i.color,
        space_icon: i.icon,
        from: person_name(&i.first_name, &i.last_name, &i.email),
    })
    .collect();
    Ok(invitations)
}

/// `GET /settings`
pub async fn page(ctx: Ctx, State(state): State<AppState>) -> AppResult<Response> {
    let now = time::now();
    let sources = sqlx::query!(
        r#"SELECT id AS "id!", space_id, name, ical_url, last_fetched_at, last_error
           FROM calendar_sources WHERE user_id = ? ORDER BY name"#,
        ctx.user.id
    )
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|s| {
        let space = ctx.space_by_id(s.space_id);
        SourceRow {
            id: s.id,
            name: s.name,
            masked_url: sync::mask_url(&s.ical_url),
            space_name: space.map(|sp| sp.name.clone()).unwrap_or_default(),
            space_color: space.map(|sp| sp.color.clone()).unwrap_or_default(),
            status: match s.last_fetched_at {
                Some(ts) => format!("Synchronisé {}", time::ago(ts, now)),
                None => "Jamais synchronisé".into(),
            },
            error: s.last_error,
        }
    })
    .collect();

    let current = tabbar::selected(ctx.user.mobile_tabs.as_deref());
    let tab_choices = tabbar::ALL
        .iter()
        .map(|tab| (tab, current.contains(&tab)))
        .collect();

    let invitations = received_invitations(&state.db, ctx.user.id).await?;

    // Espaces rejoints : qui les partage. Espaces à soi : avec combien de personnes.
    let mut shared = HashMap::new();
    for space in ctx.all_spaces().iter().filter(|s| !s.mine) {
        let owner = sqlx::query!(
            r#"SELECT email, first_name, last_name FROM users WHERE id = ?"#,
            space.owner_id
        )
        .fetch_one(&state.db)
        .await?;
        shared.insert(
            space.id,
            format!(
                "Partagé par {}",
                person_name(&owner.first_name, &owner.last_name, &owner.email)
            ),
        );
    }
    let counts = sqlx::query!(
        r#"SELECT m.space_id AS "space_id!", count(*) AS "n!: i64"
           FROM space_members m JOIN spaces s ON s.id = m.space_id
           WHERE s.user_id = ? GROUP BY m.space_id"#,
        ctx.user.id
    )
    .fetch_all(&state.db)
    .await?;
    for count in counts {
        let label = if count.n == 1 {
            "Partagé avec 1 personne".to_string()
        } else {
            format!("Partagé avec {} personnes", count.n)
        };
        shared.insert(count.space_id, label);
    }

    let mut space_tools = HashMap::new();
    for space in &ctx.spaces {
        space_tools.insert(
            space.id,
            registry::load(&state.db, ctx.user.id, space.id).await?,
        );
    }

    let (new_color, new_icon) = spaces::suggestion(&ctx.spaces);
    let page = SettingsPage {
        space_tools,
        first_name: ctx.user.first_name.clone(),
        last_name: ctx.user.last_name.clone(),
        profile_max: profile::NAME_MAX_CHARS,
        tab_choices,
        tab_max: tabbar::MAX,
        new_color,
        new_icon,
        max_spaces: spaces::MAX_SPACES,
        name_max: spaces::NAME_MAX_CHARS,
        moment_id: ctx.user.moment_space.unwrap_or(0),
        invitations,
        shared,
        connexions_html: connexions::ConnexionsCard::render_html(&state, ctx.user.id).await?,
        instance_html: instance::InstanceCard::render_html(&state, &ctx).await?,
        trash_count: crate::trash::page::count(&state.db, ctx.user.id).await?,
        has_password: ctx.user.has_password,
        google_linked: ctx.user.google_linked,
        google_enabled: state.google.is_some(),
        google_calendar: crate::calendar::google::linked(&state, ctx.user.id).await?,
        google_pending: sqlx::query_scalar!(
            r#"SELECT count(*) AS "n!: i64" FROM events WHERE user_id = ? AND google = 1 AND google_dirty > 0"#,
            ctx.user.id
        )
        .fetch_one(&state.db)
        .await?,
        sources,
        min_password: password::MIN_LENGTH,
        shell: ctx.shell("settings").await?,
    };
    Ok(web::html(page))
}

#[derive(Deserialize)]
pub struct ThemeForm {
    /// `dark`, `light` ou `system`.
    pub(crate) theme: String,
    pub(crate) next: Option<String>,
}

/// Valeur enregistrée pour un thème : `dark`, `light`, ou rien pour `system`.
pub(crate) fn parse_theme(raw: &str) -> Result<Option<&'static str>, String> {
    match raw {
        "dark" => Ok(Some("dark")),
        "light" => Ok(Some("light")),
        "system" => Ok(None),
        _ => Err("Thème inconnu.".into()),
    }
}

/// Enregistre le thème sur le compte. Aussi pour l'API.
pub(crate) async fn store_theme(
    db: &sqlx::SqlitePool,
    user: i64,
    theme: Option<&str>,
) -> AppResult<()> {
    sqlx::query!("UPDATE users SET theme = ? WHERE id = ?", theme, user)
        .execute(db)
        .await?;
    Ok(())
}

/// `POST /settings/theme` : le thème est enregistré sur le compte, il suit
/// donc l'utilisateur d'un appareil à l'autre.
pub async fn set_theme(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<ThemeForm>,
) -> AppResult<Response> {
    let theme = parse_theme(&form.theme).map_err(AppError::BadRequest)?;
    store_theme(&state.db, ctx.user.id, theme).await?;
    Ok(see_other(local_path(form.next.as_deref())))
}

/// Enregistre les raccourcis du téléphone (valeur de `tabbar::encode`).
/// Aussi pour l'API.
pub(crate) async fn store_tabs(db: &sqlx::SqlitePool, user: i64, value: &str) -> AppResult<()> {
    sqlx::query!("UPDATE users SET mobile_tabs = ? WHERE id = ?", value, user)
        .execute(db)
        .await?;
    Ok(())
}

/// `POST /settings/tabs` : choix des raccourcis du téléphone. Une case
/// cochée arrive sous la forme `tab_<identifiant>=on`.
pub async fn set_tabs(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<HashMap<String, String>>,
) -> AppResult<Response> {
    let keys: Vec<&str> = form
        .keys()
        .filter_map(|name| name.strip_prefix("tab_"))
        .collect();
    match tabbar::encode(&keys) {
        Ok(value) => {
            store_tabs(&state.db, ctx.user.id, &value).await?;
            ctx.flash_ok("Raccourcis enregistrés").await?;
        }
        Err(message) => ctx.flash_error(message).await?,
    }
    Ok(see_other("/settings#raccourcis"))
}

#[derive(Deserialize)]
pub struct PasswordForm {
    current: Option<String>,
    new: String,
    confirm: String,
}

/// `POST /settings/password` : définit ou change le mot de passe.
pub async fn set_password(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<PasswordForm>,
) -> AppResult<Response> {
    let back = || see_other("/settings");

    // Changer un mot de passe existant demande de le connaître : une session
    // laissée ouverte sur un poste ne suffit pas à prendre le compte.
    if ctx.user.has_password {
        let stored =
            sqlx::query_scalar!("SELECT password_hash FROM users WHERE id = ?", ctx.user.id)
                .fetch_one(&state.db)
                .await?;
        if !password::verify(form.current.unwrap_or_default(), stored).await {
            ctx.flash_error("Le mot de passe actuel est incorrect.")
                .await?;
            return Ok(back());
        }
    }
    if form.new.chars().count() < password::MIN_LENGTH {
        ctx.flash_error(format!(
            "Le nouveau mot de passe doit faire au moins {} caractères.",
            password::MIN_LENGTH
        ))
        .await?;
        return Ok(back());
    }
    if form.new != form.confirm {
        ctx.flash_error("Les deux saisies du nouveau mot de passe ne correspondent pas.")
            .await?;
        return Ok(back());
    }

    let hash = password::hash(form.new).await?;
    sqlx::query!(
        "UPDATE users SET password_hash = ? WHERE id = ?",
        hash,
        ctx.user.id
    )
    .execute(&state.db)
    .await?;
    ctx.flash_ok("Mot de passe enregistré").await?;
    Ok(back())
}

/// `POST /settings/google/unlink`
pub async fn unlink_google(ctx: Ctx, State(state): State<AppState>) -> AppResult<Response> {
    // Sans mot de passe, Google est le seul moyen d'entrer : on ne le retire pas.
    if !ctx.user.has_password {
        ctx.flash_error("Définis d'abord un mot de passe, sinon tu ne pourrais plus te connecter.")
            .await?;
        return Ok(see_other("/settings"));
    }
    sqlx::query!(
        "UPDATE users SET google_sub = NULL WHERE id = ?",
        ctx.user.id
    )
    .execute(&state.db)
    .await?;
    ctx.flash_ok("Compte Google délié").await?;
    Ok(see_other("/settings"))
}

#[derive(Deserialize)]
pub struct CalendarForm {
    pub(crate) name: String,
    pub(crate) url: String,
    pub(crate) space_id: i64,
}

/// Ajoute une source iCal à un espace (déjà vérifié), après les mêmes
/// vérifications que Réglages : un nom, une adresse HTTPS publique. Rend
/// son identifiant, son nom et son adresse, ou le refus à montrer. Aussi
/// pour l'API.
pub(crate) async fn add_source(
    db: &sqlx::SqlitePool,
    user: i64,
    space: i64,
    raw_name: &str,
    raw_url: &str,
) -> AppResult<Result<(i64, String, String), String>> {
    let name: String = raw_name
        .trim()
        .chars()
        .take(SOURCE_NAME_MAX_CHARS)
        .collect();
    if name.is_empty() {
        return Ok(Err("Donne un nom à cet agenda.".into()));
    }
    let url = match sync::validate_url(raw_url) {
        Ok(url) => url,
        Err(message) => return Ok(Err(message.into())),
    };
    let now = time::now();
    let id = sqlx::query!(
        "INSERT INTO calendar_sources (user_id, space_id, name, ical_url, created_at) VALUES (?, ?, ?, ?, ?)",
        user,
        space,
        name,
        url,
        now
    )
    .execute(db)
    .await?
    .last_insert_rowid();
    Ok(Ok((id, name, url)))
}

/// `POST /settings/calendars` : ajoute une source iCal et la synchronise aussitôt.
pub async fn add_calendar(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<CalendarForm>,
) -> AppResult<Response> {
    let back = || see_other("/settings#agendas");
    let Some(space) = ctx.space_by_id(form.space_id) else {
        return Err(AppError::NotFound);
    };
    let (id, name, url) =
        match add_source(&state.db, ctx.user.id, space.id, &form.name, &form.url).await? {
            Ok(added) => added,
            Err(message) => {
                ctx.flash_error(message).await?;
                return Ok(back());
            }
        };

    match sync::sync_source(&state, id, &url).await {
        Ok(count) => {
            ctx.flash_ok(format!(
                "Agenda « {name} » ajouté : {count} événements importés"
            ))
            .await?
        }
        Err(error) => {
            ctx.flash_error(format!(
                "Agenda « {name} » ajouté, mais la première synchro a échoué ({error}). Nouvel essai dans l'heure."
            ))
            .await?;
        }
    }
    Ok(back())
}

/// `POST /settings/calendars/{id}/delete` : supprime la source et son cache.
pub async fn delete_calendar(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let deleted = sqlx::query!(
        "DELETE FROM calendar_sources WHERE id = ? AND user_id = ?",
        id,
        ctx.user.id
    )
    .execute(&state.db)
    .await?
    .rows_affected();
    if deleted == 0 {
        return Err(AppError::NotFound);
    }
    ctx.flash_ok("Agenda supprimé").await?;
    Ok(see_other("/settings#agendas"))
}

#[derive(Deserialize)]
pub struct MomentForm {
    /// Identifiant de l'espace des jours sans cours ; vide pour désactiver.
    space: String,
}

/// Enregistre l'espace « moment » (déjà vérifié), ou le désactive. Aussi
/// pour l'API.
pub(crate) async fn store_moment(
    db: &sqlx::SqlitePool,
    user: i64,
    space: Option<i64>,
) -> AppResult<()> {
    sqlx::query!(
        "UPDATE users SET moment_space = ? WHERE id = ?",
        space,
        user
    )
    .execute(db)
    .await?;
    Ok(())
}

/// `POST /settings/moment` : réglage « espace du moment » à l'ouverture.
pub async fn set_moment(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<MomentForm>,
) -> AppResult<Response> {
    let space = match form.space.trim() {
        "" => None,
        id => {
            let id: i64 = id.parse().map_err(|_| AppError::NotFound)?;
            // Un espace en service de l'utilisateur.
            Some(
                ctx.spaces
                    .iter()
                    .find(|s| s.id == id)
                    .ok_or(AppError::NotFound)?
                    .id,
            )
        }
    };
    store_moment(&state.db, ctx.user.id, space).await?;
    ctx.flash_ok(match space {
        Some(_) => "L'app s'ouvrira sur l'espace du moment",
        None => "L'app s'ouvrira sur le dernier espace utilisé",
    })
    .await?;
    Ok(see_other("/settings#ouverture"))
}
