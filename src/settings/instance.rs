//! Réglages › Instance (SPEC, « Installation simple », étape 5) : l'adresse
//! publique, la connexion avec Google et les adresses autorisées, pour
//! l'administrateur seulement (le compte créé au premier lancement, ou le
//! plus ancien).
//!
//! Une valeur donnée par l'environnement (`BASE_URL`, `GOOGLE_CLIENT_ID`,
//! `ALLOWED_EMAILS`) s'affiche verrouillée : elle se change sur le serveur.
//! Un changement est rangé en base (`crate::instance`), puis dashu se relance
//! pour le prendre (`crate::restart`) : quelques secondes de coupure.

use askama::Template;
use axum::extract::State;
use axum::response::Response;
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::AppState;
use crate::auth::session::Ctx;
use crate::instance;
use crate::web::{AppError, AppResult, see_other};

const BACK: &str = "/settings#instance";
const RESTARTING: &str =
    "Enregistré. Hermes Dashboard redémarre pour l'appliquer (quelques secondes).";
const LOCKED: &str = "Ce réglage est défini sur le serveur (dashu.env) : il se change là-bas.";

pub struct EmailRow {
    pub email: String,
    /// Un compte existe déjà pour cette adresse.
    pub has_account: bool,
    /// Pourquoi elle ne se retire pas, sinon `None`.
    pub kept: Option<&'static str>,
}

#[derive(Template)]
#[template(path = "settings/_instance.html")]
pub struct InstanceCard {
    pub base_url: String,
    pub base_url_locked: bool,
    /// Identifiant du client Google, s'il y en a un.
    pub google_id: Option<String>,
    pub google_locked: bool,
    pub emails: Vec<EmailRow>,
    pub emails_locked: bool,
}

/// Le compte est-il l'administrateur de l'instance ?
pub async fn is_admin(db: &SqlitePool, user: i64) -> AppResult<bool> {
    let admin = sqlx::query_scalar!(
        r#"SELECT is_admin AS "admin!: bool" FROM users WHERE id = ?"#,
        user
    )
    .fetch_optional(db)
    .await?;
    Ok(admin.unwrap_or(false))
}

async fn require_admin(state: &AppState, ctx: &Ctx) -> AppResult<()> {
    if is_admin(&state.db, ctx.user.id).await? {
        Ok(())
    } else {
        Err(AppError::NotFound)
    }
}

impl InstanceCard {
    /// La carte de l'administrateur, déjà rendue ; vide pour les autres.
    pub async fn render_html(state: &AppState, ctx: &Ctx) -> AppResult<String> {
        if !is_admin(&state.db, ctx.user.id).await? {
            return Ok(String::new());
        }
        // Ce qui est en base, plutôt que la configuration du démarrage : un
        // changement s'affiche avant même la relance.
        let stored = instance::load(&state.db, &state.config.session_secret).await?;
        let from_env = &state.config.from_env;
        let base_url = if from_env.base_url {
            state.config.base_url.clone()
        } else {
            stored
                .base_url
                .unwrap_or_else(|| state.config.base_url.clone())
        };
        let google_id = if from_env.google {
            state.config.google.as_ref().map(|g| g.client_id.clone())
        } else {
            stored.google.map(|(id, _)| id)
        };
        let listed = if from_env.allowed_emails {
            state.config.allowed_emails.clone()
        } else {
            stored.allowed_emails
        };
        let accounts: Vec<String> =
            sqlx::query_scalar!(r#"SELECT lower(email) AS "email!: String" FROM users"#)
                .fetch_all(&state.db)
                .await?;
        let api = state.config.api.as_ref().map(|a| a.user_email.clone());
        let only_one = listed.len() == 1;
        let me = ctx.user.email.to_lowercase();
        let emails = listed
            .iter()
            .map(|email| EmailRow {
                has_account: accounts.contains(email),
                kept: if *email == me {
                    Some("ton adresse")
                } else if api.as_deref() == Some(email.as_str()) {
                    Some("compte de l'API")
                } else if only_one {
                    Some("la seule adresse")
                } else {
                    None
                },
                email: email.clone(),
            })
            .collect();
        let card = Self {
            base_url,
            base_url_locked: from_env.base_url,
            google_id,
            google_locked: from_env.google,
            emails,
            emails_locked: from_env.allowed_emails,
        };
        Ok(card.render()?)
    }
}

#[derive(Deserialize)]
pub struct AddressForm {
    base_url: String,
}

/// `POST /settings/instance/address`
pub async fn set_address(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<AddressForm>,
) -> AppResult<Response> {
    require_admin(&state, &ctx).await?;
    if state.config.from_env.base_url {
        ctx.flash_error(LOCKED).await?;
        return Ok(see_other(BACK));
    }
    let Some(base_url) = instance::normalize_base_url(&form.base_url) else {
        ctx.flash_error(
            "Adresse invalide : http:// ou https://, puis le nom ou l'adresse IP, sans chemin.",
        )
        .await?;
        return Ok(see_other(BACK));
    };
    instance::set(&state.db, instance::BASE_URL, &base_url).await?;
    tracing::info!(user_id = ctx.user.id, %base_url, "instance : adresse changée");
    restart(&state, &ctx).await
}

#[derive(Deserialize)]
pub struct GoogleForm {
    client_id: String,
    client_secret: String,
}

/// `POST /settings/instance/google`
pub async fn set_google(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<GoogleForm>,
) -> AppResult<Response> {
    require_admin(&state, &ctx).await?;
    if state.config.from_env.google {
        ctx.flash_error(LOCKED).await?;
        return Ok(see_other(BACK));
    }
    let (id, secret) = (form.client_id.trim(), form.client_secret.trim());
    if id.is_empty() || secret.is_empty() || id.len() > 300 || secret.len() > 300 {
        ctx.flash_error("Il faut l'identifiant et le secret du client Google.")
            .await?;
        return Ok(see_other(BACK));
    }
    let mut db = state.db.acquire().await?;
    instance::set_google(&mut db, &state.config.session_secret, id, secret).await?;
    tracing::info!(user_id = ctx.user.id, "instance : connexion Google réglée");
    restart(&state, &ctx).await
}

/// `POST /settings/instance/google/remove`
pub async fn remove_google(ctx: Ctx, State(state): State<AppState>) -> AppResult<Response> {
    require_admin(&state, &ctx).await?;
    if state.config.from_env.google {
        ctx.flash_error(LOCKED).await?;
        return Ok(see_other(BACK));
    }
    let mut db = state.db.acquire().await?;
    instance::remove_google(&mut db).await?;
    tracing::info!(user_id = ctx.user.id, "instance : connexion Google retirée");
    restart(&state, &ctx).await
}

#[derive(Deserialize)]
pub struct EmailForm {
    email: String,
}

/// `POST /settings/instance/emails` : autorise une adresse.
pub async fn add_email(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<EmailForm>,
) -> AppResult<Response> {
    require_admin(&state, &ctx).await?;
    if state.config.from_env.allowed_emails {
        ctx.flash_error(LOCKED).await?;
        return Ok(see_other(BACK));
    }
    let email = form.email.trim().to_lowercase();
    if !instance::valid_email(&email) {
        ctx.flash_error("Adresse email invalide.").await?;
        return Ok(see_other(BACK));
    }
    let stored = instance::load(&state.db, &state.config.session_secret).await?;
    if stored.allowed_emails.contains(&email) {
        ctx.flash_ok(format!("{email} est déjà autorisée.")).await?;
        return Ok(see_other(BACK));
    }
    instance::allow(&state.db, &email, Some(ctx.user.id)).await?;
    tracing::info!(
        user_id = ctx.user.id,
        "instance : adresse autorisée ajoutée"
    );
    restart(&state, &ctx).await
}

/// `POST /settings/instance/emails/remove` : coupe l'accès d'une adresse.
/// Les données du compte restent dans la base.
pub async fn remove_email(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<EmailForm>,
) -> AppResult<Response> {
    require_admin(&state, &ctx).await?;
    if state.config.from_env.allowed_emails {
        ctx.flash_error(LOCKED).await?;
        return Ok(see_other(BACK));
    }
    let email = form.email.trim().to_lowercase();
    let stored = instance::load(&state.db, &state.config.session_secret).await?;
    let refusal = if !stored.allowed_emails.contains(&email) {
        Some("Cette adresse n'est pas dans la liste.")
    } else if email == ctx.user.email.to_lowercase() {
        Some("Tu ne peux pas retirer ta propre adresse : tu perdrais l'accès.")
    } else if state.config.api.as_ref().map(|a| a.user_email.as_str()) == Some(email.as_str()) {
        Some("C'est le compte de l'API (API_USER_EMAIL) : dashu ne démarrerait plus sans lui.")
    } else if stored.allowed_emails.len() == 1 {
        Some("C'est la seule adresse autorisée : plus personne ne pourrait se connecter.")
    } else {
        None
    };
    if let Some(message) = refusal {
        ctx.flash_error(message).await?;
        return Ok(see_other(BACK));
    }
    instance::disallow(&state.db, &email).await?;
    tracing::info!(
        user_id = ctx.user.id,
        "instance : adresse autorisée retirée"
    );
    restart(&state, &ctx).await
}

async fn restart(state: &AppState, ctx: &Ctx) -> AppResult<Response> {
    ctx.flash_ok(RESTARTING).await?;
    state.restart.ask();
    Ok(see_other(BACK))
}
