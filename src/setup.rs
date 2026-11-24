//! Premier lancement dans le navigateur (SPEC, « Installation simple »,
//! étape 4).
//!
//! Tant qu'aucun compte n'existe, toute page mène à `/setup`. L'assistant
//! demande d'abord le **code d'installation** que dashu écrit dans son
//! journal au démarrage (`journalctl -u dashu`, `docker logs`) : sans lui, le
//! premier venu qui tomberait sur la page ne prend pas l'instance. Puis, sur
//! une seule page en trois parties : le compte (qui devient l'administrateur),
//! l'adresse publique et, si on veut, la connexion Google. À la fin, dashu
//! enregistre, ouvre la session du compte et se relance pour prendre la
//! nouvelle configuration.
//!
//! Dès qu'un compte existe, `/setup` répond 404, pour de bon.

use std::sync::atomic::Ordering;
use std::time::Duration;

use askama::Template;
use axum::Json;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tower_sessions::cookie::time::Duration as CookieDuration;
use tower_sessions::{Expiry, Session};

use crate::AppState;
use crate::auth::password;
use crate::auth::session::{ANONYMOUS_HOURS, sign_in};
use crate::instance;
use crate::web::{self, AppError, AppResult, csrf, see_other};

/// Le code a été donné, dans cette session.
const CODE_OK: &str = "setup_ok";

/// Code d'installation : 12 caractères faciles à lire et à taper (ni 0, O, 1
/// ni I), en trois groupes : `K7QM-2XPA-9RTD`. 32 symboles, soit 60 bits ;
/// les essais sont limités comme sur `/login`. Nouveau à chaque démarrage,
/// gardé en mémoire seulement.
pub fn new_code() -> String {
    const ALPHABET: &[u8; 32] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
    let bytes: [u8; 12] = rand::random();
    let mut code = String::with_capacity(14);
    for (i, byte) in bytes.iter().enumerate() {
        if i > 0 && i % 4 == 0 {
            code.push('-');
        }
        // 256 est un multiple de 32 : chaque symbole a la même chance.
        code.push(char::from(ALPHABET[usize::from(*byte) % ALPHABET.len()]));
    }
    code
}

/// Le code tel qu'on le compare : lettres et chiffres en majuscules (les
/// tirets et les espaces tapés ou non n'y changent rien).
fn normalize(code: &str) -> String {
    code.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Vrai dès qu'un compte existe. Lu en base tant que ce n'est pas le cas
/// (un compte peut aussi naître par `dashu set-password`), puis gardé.
pub async fn installed(state: &AppState) -> sqlx::Result<bool> {
    if state.installed.load(Ordering::Relaxed) {
        return Ok(true);
    }
    let any = instance::has_users(&state.db).await?;
    if any {
        state.installed.store(true, Ordering::Relaxed);
    }
    Ok(any)
}

/// Au démarrage : sans compte, écrit dans le journal où aller et le code.
pub async fn announce(state: &AppState) -> anyhow::Result<()> {
    if !installed(state).await? {
        // Sans adresse réglée, celle de la machine, sur le vrai port.
        let address = if state.config.base_url == crate::config::DEFAULT_BASE_URL {
            format!("http://localhost:{}", state.config.bind_addr.port())
        } else {
            state.config.base_url.clone()
        };
        tracing::warn!(
            "premier lancement : ouvre {address}/setup dans un navigateur et tape le code d'installation {}",
            state.setup_code
        );
    }
    Ok(())
}

/// Middleware : sans compte, tout mène à `/setup`, sauf `/health` et les
/// fichiers statiques ; l'API, le connecteur Claude et OAuth répondent 503
/// en JSON.
pub async fn guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    match installed(&state).await {
        Ok(true) => return next.run(request).await,
        Ok(false) => {}
        Err(e) => return AppError::from(e).into_response(),
    }
    let path = request.uri().path();
    if path == "/health"
        || path.starts_with("/static/")
        || path == "/setup"
        || path.starts_with("/setup/")
    {
        return next.run(request).await;
    }
    if path.starts_with("/api/")
        || path == "/mcp"
        || path.starts_with("/oauth/")
        || path.starts_with("/.well-known/")
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "not_installed",
                "message": "dashu n'est pas encore installé : ouvre /setup dans un navigateur."
            })),
        )
            .into_response();
    }
    see_other("/setup")
}

#[derive(Template)]
#[template(path = "setup.html")]
struct SetupPage<'a> {
    csrf: &'a str,
    /// « code », « form » ou « done ».
    step: &'a str,
    error: Option<&'a str>,
    email: &'a str,
    base_url: &'a str,
    base_url_locked: bool,
    google_locked: bool,
    google_ready: bool,
    allowed_locked: bool,
    /// Après l'installation : l'adresse ne répond pas depuis le serveur.
    warning: Option<&'a str>,
}

impl<'a> SetupPage<'a> {
    fn new(state: &'a AppState, csrf: &'a str, step: &'a str) -> Self {
        Self {
            csrf,
            step,
            error: None,
            email: "",
            base_url: "",
            base_url_locked: state.config.from_env.base_url,
            google_locked: state.config.from_env.google,
            google_ready: state.google.is_some(),
            allowed_locked: state.config.from_env.allowed_emails,
            warning: None,
        }
    }
}

/// Jeton CSRF d'une session anonyme courte, comme la page de connexion.
async fn anonymous_csrf(session: &Session) -> AppResult<String> {
    if session.get::<String>(csrf::SESSION_KEY).await?.is_none() {
        session.set_expiry(Some(Expiry::OnInactivity(CookieDuration::hours(
            ANONYMOUS_HOURS,
        ))));
    }
    csrf::token(session).await
}

/// Adresse proposée : celle que le navigateur utilise (le JavaScript de la
/// page la corrige avec `location.origin`, plus sûr), ou celle de l'instance
/// si elle est fixée par l'environnement.
fn suggested_base_url(state: &AppState, headers: &HeaderMap) -> String {
    if state.config.from_env.base_url {
        return state.config.base_url.clone();
    }
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.split(',').next().unwrap_or("").trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let host = header("x-forwarded-host").or_else(|| header(header::HOST.as_str()));
    let proto = header("x-forwarded-proto").unwrap_or_else(|| "http".into());
    host.and_then(|host| instance::normalize_base_url(&format!("{proto}://{host}")))
        .unwrap_or_else(|| state.config.base_url.clone())
}

/// `GET /setup`
pub async fn page(
    State(state): State<AppState>,
    session: Session,
    headers: HeaderMap,
) -> AppResult<Response> {
    if installed(&state).await? {
        return Err(AppError::NotFound);
    }
    let csrf = anonymous_csrf(&session).await?;
    let code_ok = session.get::<bool>(CODE_OK).await?.unwrap_or(false);
    let base_url = suggested_base_url(&state, &headers);
    let mut page = SetupPage::new(&state, &csrf, if code_ok { "form" } else { "code" });
    page.base_url = &base_url;
    Ok(web::html(page))
}

#[derive(Deserialize)]
pub struct CodeForm {
    code: String,
}

/// `POST /setup/code` : le code d'installation (essais limités comme sur
/// `/login`).
pub async fn check_code(
    State(state): State<AppState>,
    session: Session,
    axum::Form(form): axum::Form<CodeForm>,
) -> AppResult<Response> {
    if installed(&state).await? {
        return Err(AppError::NotFound);
    }
    let expected = normalize(&state.setup_code);
    let given = normalize(&form.code);
    if csrf::constant_time_eq(given.as_bytes(), expected.as_bytes()) {
        session.insert(CODE_OK, true).await?;
        csrf::rotate(&session).await?;
        tracing::info!("premier lancement : code d'installation accepté");
        return Ok(see_other("/setup"));
    }
    tracing::warn!("premier lancement : code d'installation refusé");
    let csrf = anonymous_csrf(&session).await?;
    let mut page = SetupPage::new(&state, &csrf, "code");
    page.error = Some(
        "Ce n'est pas le bon code. Il est dans le journal de dashu (journalctl -u dashu, ou docker logs), et change à chaque démarrage.",
    );
    Ok((StatusCode::UNAUTHORIZED, web::html(page)).into_response())
}

#[derive(Deserialize)]
pub struct SetupForm {
    email: String,
    password: String,
    password2: String,
    #[serde(default)]
    base_url: String,
    #[serde(default)]
    google_client_id: String,
    #[serde(default)]
    google_client_secret: String,
}

/// Ce que l'assistant enregistre, une fois la saisie vérifiée.
struct Valid {
    email: String,
    base_url: Option<String>,
    google: Option<(String, String)>,
}

fn validate(state: &AppState, form: &SetupForm) -> Result<Valid, &'static str> {
    let email = form.email.trim().to_lowercase();
    if !instance::valid_email(&email) {
        return Err("Adresse email invalide.");
    }
    if state.config.from_env.allowed_emails && !state.config.is_allowed(&email) {
        return Err(
            "Cette adresse n'est pas autorisée par la configuration du serveur (ALLOWED_EMAILS).",
        );
    }
    if form.password.chars().count() < password::MIN_LENGTH {
        return Err("Le mot de passe doit faire au moins 12 caractères.");
    }
    if form.password != form.password2 {
        return Err("Les deux mots de passe ne sont pas identiques.");
    }
    let base_url = if state.config.from_env.base_url {
        None
    } else {
        Some(instance::normalize_base_url(&form.base_url).ok_or(
            "Adresse invalide : http:// ou https://, puis le nom ou l'adresse IP, sans chemin (par exemple https://dashu.exemple.fr).",
        )?)
    };
    let id = form.google_client_id.trim();
    let secret = form.google_client_secret.trim();
    let google = match (
        state.config.from_env.google,
        id.is_empty(),
        secret.is_empty(),
    ) {
        (true, _, _) | (false, true, true) => None,
        (false, false, false) => Some((id.to_string(), secret.to_string())),
        _ => return Err("Pour Google, il faut l'identifiant et le secret, ou aucun des deux."),
    };
    Ok(Valid {
        email,
        base_url,
        google,
    })
}

/// `POST /setup` : crée le compte administrateur, enregistre l'adresse et
/// Google, ouvre la session, puis relance dashu.
pub async fn finish(
    State(state): State<AppState>,
    session: Session,
    axum::Form(form): axum::Form<SetupForm>,
) -> AppResult<Response> {
    if installed(&state).await? {
        return Err(AppError::NotFound);
    }
    if !session.get::<bool>(CODE_OK).await?.unwrap_or(false) {
        return Ok(see_other("/setup"));
    }
    let valid = match validate(&state, &form) {
        Ok(valid) => valid,
        Err(message) => {
            let csrf = anonymous_csrf(&session).await?;
            let mut page = SetupPage::new(&state, &csrf, "form");
            page.error = Some(message);
            page.email = form.email.trim();
            page.base_url = form.base_url.trim();
            return Ok((StatusCode::UNPROCESSABLE_ENTITY, web::html(page)).into_response());
        }
    };

    let hash = password::hash(form.password).await?;
    let now = crate::time::now();
    let mut tx = state.db.begin().await?;
    // Deux onglets, ou un compte créé entre-temps par dashu set-password :
    // le premier compte seulement.
    if instance::has_users(&mut *tx).await? {
        return Err(AppError::NotFound);
    }
    let user_id = sqlx::query_scalar!(
        r#"INSERT INTO users (email, password_hash, created_at, is_admin) VALUES (?, ?, ?, 1)
           RETURNING id AS "id!""#,
        valid.email,
        hash,
        now
    )
    .fetch_one(&mut *tx)
    .await?;
    if !state.config.from_env.allowed_emails {
        instance::allow(&mut *tx, &valid.email, Some(user_id)).await?;
    }
    if let Some(base_url) = &valid.base_url {
        instance::set(&mut *tx, instance::BASE_URL, base_url).await?;
    }
    if let Some((id, secret)) = &valid.google {
        instance::set_google(&mut tx, &state.config.session_secret, id, secret).await?;
    }
    tx.commit().await?;
    state.installed.store(true, Ordering::Relaxed);

    sign_in(&state, &session, user_id).await?;
    session.remove::<bool>(CODE_OK).await?;
    tracing::info!(
        user_id,
        google = valid.google.is_some(),
        "premier lancement : compte administrateur créé"
    );

    // L'adresse mène-t-elle bien à dashu ? On prévient, sans bloquer : depuis
    // le serveur, le réseau ou le DNS peuvent différer de ceux du navigateur.
    let warning = match &valid.base_url {
        Some(base_url) if !responds(&state, base_url).await => Some(
            "Cette adresse ne répond pas depuis le serveur. Si elle marche depuis ton navigateur, rien à faire ; sinon, corrige-la dans Réglages › Instance.",
        ),
        _ => None,
    };

    state.restart.ask();
    let mut page = SetupPage::new(&state, "", "done");
    page.email = &valid.email;
    page.base_url = valid.base_url.as_deref().unwrap_or(&state.config.base_url);
    page.warning = warning;
    Ok(web::html(page))
}

/// `GET <adresse>/health` répond-il `ok` ?
async fn responds(state: &AppState, base_url: &str) -> bool {
    let request = state
        .http
        .get(format!("{base_url}/health"))
        .timeout(Duration::from_secs(5))
        .send();
    match request.await {
        Ok(response) if response.status().is_success() => {
            response.text().await.is_ok_and(|body| body.trim() == "ok")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_code_se_lit_facilement() {
        let code = new_code();
        assert_eq!(code.len(), 14);
        assert_eq!(code.matches('-').count(), 2);
        assert!(
            code.chars()
                .all(|c| c == '-' || "23456789ABCDEFGHJKLMNPQRSTUVWXYZ".contains(c))
        );
        assert_ne!(new_code(), new_code());
    }

    #[test]
    fn le_code_se_compare_sans_tirets_ni_casse() {
        assert_eq!(normalize(" k7qm 2xpa-9rtd "), "K7QM2XPA9RTD");
    }
}
