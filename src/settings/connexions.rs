//! Réglages › Connexions : les accès du compte donnés à des programmes.
//!
//! Pour l'instant, les jetons créés à la main (voir `crate::auth::tokens`).
//! Chaque action remplace la carte (htmx) : le jeton entier ne s'affiche
//! qu'une fois, dans la réponse à sa création.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::Response;
use chrono::Datelike;
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::AppState;
use crate::auth::session::Ctx;
use crate::auth::tokens;
use crate::time;
use crate::web::{self, AppError, AppResult};

/// Jetons créés à la main au plus par compte, expirés compris.
pub const MAX_TOKENS: i64 = 20;
pub const NAME_MAX_CHARS: usize = 60;
/// Durées proposées : (valeur du formulaire, jours, libellé).
pub const DURATIONS: [(&str, i64, &str); 3] = [
    ("30", 30, "30 jours"),
    ("90", 90, "90 jours"),
    ("365", 365, "1 an"),
];
pub const DEFAULT_DURATION: &str = "90";

pub struct TokenRow {
    pub id: i64,
    pub name: String,
    /// « créé le 23 novembre »
    pub created: String,
    /// « utilisé il y a 5 min », « jamais utilisé »
    pub used: String,
    /// « expire le 21 février 2027 », ou vide s'il est expiré.
    pub expires: String,
    pub expired: bool,
}

#[derive(Template)]
#[template(path = "settings/_connexions.html")]
pub struct ConnexionsCard {
    pub tokens: Vec<TokenRow>,
    /// Le jeton qui vient d'être créé, en entier : affiché une seule fois.
    pub new_token: Option<String>,
    pub new_name: String,
    /// Adresse de l'API, pour l'exemple d'appel.
    pub api_url: String,
    /// Adresse du connecteur Claude, pour la commande de Claude Code.
    pub mcp_url: String,
    pub error: Option<String>,
    pub durations: [(&'static str, i64, &'static str); 3],
    pub default_duration: &'static str,
    pub name_max: usize,
    pub max_tokens: i64,
}

/// « 21 février », avec l'année si ce n'est pas celle d'aujourd'hui.
fn date_label(ts: i64, now: i64) -> String {
    let date = time::paris_date(ts);
    let day = time::day_month(date);
    if date.year() == time::paris_date(now).year() {
        day
    } else {
        format!("{day} {}", date.year())
    }
}

impl ConnexionsCard {
    pub async fn load(state: &AppState, user: i64) -> AppResult<Self> {
        let now = time::now();
        let tokens = list(&state.db, user)
            .await?
            .into_iter()
            .map(|t| TokenRow {
                id: t.id,
                name: t.name,
                created: format!("créé le {}", date_label(t.created_at, now)),
                used: match t.last_used_at {
                    Some(at) => format!("utilisé {}", time::ago(at, now)),
                    None => "jamais utilisé".into(),
                },
                expires: if t.expires_at > now {
                    format!("expire le {}", date_label(t.expires_at, now))
                } else {
                    String::new()
                },
                expired: t.expires_at <= now,
            })
            .collect();
        Ok(Self {
            tokens,
            new_token: None,
            new_name: String::new(),
            api_url: format!("{}/api/v1", state.config.base_url.trim_end_matches('/')),
            mcp_url: format!("{}/mcp", state.config.base_url.trim_end_matches('/')),
            error: None,
            durations: DURATIONS,
            default_duration: DEFAULT_DURATION,
            name_max: NAME_MAX_CHARS,
            max_tokens: MAX_TOKENS,
        })
    }

    /// La carte rendue, pour l'insérer dans la page Réglages.
    pub async fn render_html(state: &AppState, user: i64) -> AppResult<String> {
        Self::load(state, user)
            .await?
            .render()
            .map_err(|e| AppError::Internal(e.into()))
    }
}

struct Stored {
    id: i64,
    name: String,
    created_at: i64,
    expires_at: i64,
    last_used_at: Option<i64>,
}

/// Jetons créés à la main, du plus récent au plus ancien.
async fn list(db: &SqlitePool, user: i64) -> AppResult<Vec<Stored>> {
    Ok(sqlx::query_as!(
        Stored,
        r#"SELECT id AS "id!", name, created_at, expires_at, last_used_at
           FROM access_tokens WHERE user_id = ? AND grant_id IS NULL
           ORDER BY created_at DESC, id DESC"#,
        user
    )
    .fetch_all(db)
    .await?)
}

#[derive(Deserialize)]
pub struct CreateForm {
    name: String,
    duree: String,
}

/// `POST /settings/tokens` : crée un jeton. Répond par la carte, qui montre
/// le jeton entier cette fois-ci seulement.
pub async fn create(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<CreateForm>,
) -> AppResult<Response> {
    let user = ctx.user.id;
    let mut card = ConnexionsCard::load(&state, user).await?;
    let name = form.name.trim().to_string();
    let duration = DURATIONS.iter().find(|(key, _, _)| *key == form.duree);
    let count = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!: i64" FROM access_tokens WHERE user_id = ? AND grant_id IS NULL"#,
        user
    )
    .fetch_one(&state.db)
    .await?;

    let refused = if name.is_empty() {
        Some("Donne un nom au jeton, pour le reconnaître dans la liste.".to_string())
    } else if name.chars().count() > NAME_MAX_CHARS {
        Some(format!(
            "Nom trop long ({NAME_MAX_CHARS} caractères au plus)."
        ))
    } else if duration.is_none() {
        Some("Durée inconnue.".to_string())
    } else if count >= MAX_TOKENS {
        Some(format!(
            "{MAX_TOKENS} jetons au plus : supprime-en un avant d'en créer un autre."
        ))
    } else {
        None
    };
    if let Some(message) = refused {
        card.error = Some(message);
        return Ok(web::html(card));
    }
    let (_, days, _) = duration.copied().unwrap_or(DURATIONS[1]);

    let token = tokens::generate();
    let token_hash = tokens::hash(&token);
    let now = time::now();
    let expires_at = now + days * 24 * 3600;
    sqlx::query!(
        "INSERT INTO access_tokens (user_id, name, token_hash, created_at, expires_at) VALUES (?, ?, ?, ?, ?)",
        user,
        name,
        token_hash,
        now,
        expires_at
    )
    .execute(&state.db)
    .await?;
    tracing::info!(user, name, "jeton créé");

    let mut card = ConnexionsCard::load(&state, user).await?;
    card.new_token = Some(token);
    card.new_name = name;
    Ok(web::html(card))
}

/// `POST /settings/tokens/{id}/revoke` : coupe le jeton à la requête suivante
/// (ou efface un jeton expiré de la liste).
pub async fn revoke(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let user = ctx.user.id;
    let deleted = sqlx::query!(
        "DELETE FROM access_tokens WHERE id = ? AND user_id = ? AND grant_id IS NULL",
        id,
        user
    )
    .execute(&state.db)
    .await?
    .rows_affected();
    if deleted == 0 {
        return Err(AppError::NotFound);
    }
    tracing::info!(user, id, "jeton révoqué");
    Ok(web::html(ConnexionsCard::load(&state, user).await?))
}

/// `POST /settings/tokens/revoke-all` : coupe d'un coup tous les accès du compte.
pub async fn revoke_all(ctx: Ctx, State(state): State<AppState>) -> AppResult<Response> {
    let user = ctx.user.id;
    let deleted = sqlx::query!("DELETE FROM access_tokens WHERE user_id = ?", user)
        .execute(&state.db)
        .await?
        .rows_affected();
    tracing::info!(user, deleted, "tous les accès révoqués");
    Ok(web::html(ConnexionsCard::load(&state, user).await?))
}
