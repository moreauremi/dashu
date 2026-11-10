//! Authentification : identifiants, Google, sessions, anti-bruteforce.
//!
//! Il n'y a pas de page d'inscription. dashu est un outil personnel : un
//! compte naît soit à la première connexion Google d'un email autorisé (Google
//! prouve que l'adresse appartient bien au visiteur), soit en ligne de
//! commande sur le serveur (`dashu set-password <email>`). Un formulaire
//! d'inscription public, sans vérification d'email, laisserait n'importe qui
//! connaissant une adresse autorisée créer le compte à la place de son
//! propriétaire.

pub mod google;
pub mod password;
pub mod ratelimit;
pub mod session;

use askama::Template;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tower_sessions::cookie::time::Duration;
use tower_sessions::{Expiry, Session};

use crate::AppState;
use crate::web::{self, AppResult, csrf, see_other};
use session::{ANONYMOUS_HOURS, USER_ID, sign_in};

/// Un seul message, que l'email existe ou non, que le mot de passe soit faux
/// ou que l'adresse ne soit pas autorisée : le formulaire ne révèle rien.
const BAD_CREDENTIALS: &str = "Email ou mot de passe incorrect.";

#[derive(Template)]
#[template(path = "login.html")]
struct LoginPage<'a> {
    csrf: &'a str,
    email: &'a str,
    error: Option<&'a str>,
    google_enabled: bool,
}

#[derive(Deserialize)]
pub struct LoginQuery {
    /// Code d'erreur posé par le retour de Google.
    e: Option<String>,
}

/// `GET /login`
pub async fn login_page(
    State(state): State<AppState>,
    session: Session,
    Query(query): Query<LoginQuery>,
) -> AppResult<Response> {
    if session.get::<i64>(USER_ID).await?.is_some() {
        return Ok(see_other("/"));
    }
    let error = match query.e.as_deref() {
        Some("refuse") => Some(BAD_CREDENTIALS),
        Some("google") => Some("La connexion avec Google n'a pas abouti. Réessaie."),
        _ => None,
    };
    render_login(&state, &session, "", error, StatusCode::OK).await
}

async fn render_login(
    state: &AppState,
    session: &Session,
    email: &str,
    error: Option<&str>,
    status: StatusCode,
) -> AppResult<Response> {
    // Le formulaire a besoin d'un jeton CSRF, donc d'une session anonyme.
    if session.get::<String>(csrf::SESSION_KEY).await?.is_none() {
        session.set_expiry(Some(Expiry::OnInactivity(Duration::hours(ANONYMOUS_HOURS))));
    }
    let csrf = csrf::token(session).await?;
    let page = LoginPage {
        csrf: &csrf,
        email,
        error,
        google_enabled: state.google.is_some(),
    };
    Ok((status, web::html(page)).into_response())
}

#[derive(Deserialize)]
pub struct LoginForm {
    email: String,
    password: String,
}

/// `POST /login`
pub async fn login(
    State(state): State<AppState>,
    session: Session,
    axum::Form(form): axum::Form<LoginForm>,
) -> AppResult<Response> {
    let email = form.email.trim().to_lowercase();

    let user = sqlx::query!(
        r#"SELECT id AS "id!", password_hash FROM users WHERE email = ?"#,
        email
    )
    .fetch_optional(&state.db)
    .await?;
    let stored_hash = user.as_ref().and_then(|u| u.password_hash.clone());

    // La vérification tourne toujours, même pour un compte inconnu (voir password::verify).
    let password_ok = password::verify(form.password, stored_hash).await;
    let allowed = state.config.is_allowed(&email);

    match user {
        Some(user) if password_ok && allowed => {
            sign_in(&state, &session, user.id).await?;
            tracing::info!(user_id = user.id, "connexion par identifiants");
            Ok(see_other("/"))
        }
        _ => {
            tracing::warn!("tentative de connexion refusée");
            render_login(
                &state,
                &session,
                &email,
                Some(BAD_CREDENTIALS),
                StatusCode::UNAUTHORIZED,
            )
            .await
        }
    }
}

/// `POST /logout`
pub async fn logout(session: Session) -> AppResult<Response> {
    // Supprime la session côté serveur et le cookie.
    session.flush().await?;
    Ok(see_other("/login"))
}

/// Crée un compte ou remplace son mot de passe (commande `dashu set-password`).
pub async fn set_password(
    db: &sqlx::SqlitePool,
    email: &str,
    new_password: String,
) -> anyhow::Result<()> {
    if new_password.chars().count() < password::MIN_LENGTH {
        anyhow::bail!(
            "le mot de passe doit faire au moins {} caractères",
            password::MIN_LENGTH
        );
    }
    let email = email.trim().to_lowercase();
    let hash = password::hash(new_password).await?;
    let now = crate::time::now();
    sqlx::query!(
        "INSERT INTO users (email, password_hash, created_at) VALUES (?, ?, ?)
         ON CONFLICT (email) DO UPDATE SET password_hash = excluded.password_hash",
        email,
        hash,
        now
    )
    .execute(db)
    .await?;
    Ok(())
}
