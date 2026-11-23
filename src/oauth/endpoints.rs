//! Points d'entrée OAuth sans session ni cookie : découverte, enregistrement,
//! échange de jetons, révocation. Ils répondent en JSON, et ceux qui
//! écrivent ont leur propre limiteur par adresse IP, large comme celui des
//! liens de partage : tous les appels de claude.ai partent des mêmes
//! adresses.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use tower_governor::GovernorError;
use tower_governor::GovernorLayer;
use tower_governor::governor::GovernorConfigBuilder;

use super::{
    ACCESS_SECONDS, REFRESH_SECONDS, UNUSED_CLIENT_SECONDS, base_url, no_store, oauth_error,
    pkce_matches, redirect_allowed, resource,
};
use crate::AppState;
use crate::auth::ratelimit::ClientIp;
use crate::auth::tokens;
use crate::time;

/// Longueur maximale du nom d'une application.
const NAME_MAX_CHARS: usize = 100;
/// Adresses de retour au plus par application.
const MAX_REDIRECTS: usize = 10;

pub fn router() -> Router<AppState> {
    let config = Arc::new(
        GovernorConfigBuilder::default()
            .period(Duration::from_secs(3))
            .burst_size(20)
            .key_extractor(ClientIp)
            .finish()
            .expect("configuration du limiteur valide"),
    );
    let limiter = config.limiter().clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(300));
        loop {
            tick.tick().await;
            limiter.retain_recent();
        }
    });
    let limit = || GovernorLayer::new(config.clone()).error_handler(too_many_requests);
    Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(protected_resource),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp",
            get(protected_resource),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(authorization_server),
        )
        .route("/oauth/register", post(register).layer(limit()))
        .route("/oauth/token", post(token).layer(limit()))
        .route("/oauth/revoke", post(revoke).layer(limit()))
}

fn too_many_requests(error: GovernorError) -> Response {
    let wait = match error {
        GovernorError::TooManyRequests { wait_time, .. } => wait_time,
        _ => 3,
    };
    oauth_error(
        StatusCode::TOO_MANY_REQUESTS,
        "slow_down",
        &format!("trop de demandes : réessaie dans {wait} s"),
    )
}

// ---------------------------------------------------------------------
// Découverte
// ---------------------------------------------------------------------

/// `GET /.well-known/oauth-protected-resource` (RFC 9728) : `/mcp` est
/// protégé, et dashu en est lui-même le serveur d'autorisation.
async fn protected_resource(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "resource": resource(&state),
        "authorization_servers": [base_url(&state)],
        "bearer_methods_supported": ["header"],
        "resource_name": crate::assets::APP_NAME,
    }))
}

/// `GET /.well-known/oauth-authorization-server` (RFC 8414).
async fn authorization_server(State(state): State<AppState>) -> Json<Value> {
    let base = base_url(&state);
    Json(json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/oauth/authorize"),
        "token_endpoint": format!("{base}/oauth/token"),
        "registration_endpoint": format!("{base}/oauth/register"),
        "revocation_endpoint": format!("{base}/oauth/revoke"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "revocation_endpoint_auth_methods_supported": ["none"],
    }))
}

// ---------------------------------------------------------------------
// Enregistrement (RFC 7591)
// ---------------------------------------------------------------------

#[derive(Deserialize)]
struct Registration {
    client_name: Option<String>,
    #[serde(default)]
    redirect_uris: Vec<String>,
    token_endpoint_auth_method: Option<String>,
}

/// `POST /oauth/register` : ouvert à tous, c'est ce qu'attend Claude, mais
/// seulement pour les adresses de retour de Claude.
async fn register(State(state): State<AppState>, body: Bytes) -> Response {
    let Ok(registration) = serde_json::from_slice::<Registration>(&body) else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_client_metadata",
            "corps JSON attendu",
        );
    };
    if registration.redirect_uris.is_empty() || registration.redirect_uris.len() > MAX_REDIRECTS {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_redirect_uri",
            "une à dix adresses de retour attendues",
        );
    }
    if let Some(refused) = registration
        .redirect_uris
        .iter()
        .find(|uri| !redirect_allowed(uri))
    {
        tracing::warn!(uri = refused, "enregistrement OAuth refusé");
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_redirect_uri",
            "seules les adresses de retour de Claude sont acceptées",
        );
    }
    if registration
        .token_endpoint_auth_method
        .as_deref()
        .is_some_and(|m| m != "none")
    {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_client_metadata",
            "client public seulement (token_endpoint_auth_method: none)",
        );
    }
    let name: String = registration
        .client_name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("Claude")
        .chars()
        .filter(|c| !c.is_control())
        .take(NAME_MAX_CHARS)
        .collect();

    let now = time::now();
    let client_id = format!("client_{}", &crate::web::csrf::random_token()[..32]);
    let uris = serde_json::to_string(&registration.redirect_uris).unwrap_or_default();
    let stale = now - UNUSED_CLIENT_SECONDS;
    let stored = async {
        // Ménage : les clients jamais autorisés depuis 24 heures.
        sqlx::query!(
            "DELETE FROM oauth_clients WHERE created_at < ?
               AND client_id NOT IN (SELECT client_id FROM oauth_grants)",
            stale
        )
        .execute(&state.db)
        .await?;
        sqlx::query!(
            "INSERT INTO oauth_clients (client_id, name, redirect_uris, created_at) VALUES (?, ?, ?, ?)",
            client_id,
            name,
            uris,
            now
        )
        .execute(&state.db)
        .await
    }
    .await;
    if let Err(error) = stored {
        tracing::error!(error = format!("{error:#}"), "enregistrement OAuth");
        return oauth_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "server_error",
            "erreur interne",
        );
    }
    tracing::info!(client = name, "application enregistrée");
    (
        StatusCode::CREATED,
        Json(json!({
            "client_id": client_id,
            "client_id_issued_at": now,
            "client_name": name,
            "redirect_uris": registration.redirect_uris,
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        })),
    )
        .into_response()
}

// ---------------------------------------------------------------------
// Échange de jetons
// ---------------------------------------------------------------------

#[derive(Default)]
struct TokenForm {
    grant_type: Option<String>,
    code: Option<String>,
    redirect_uri: Option<String>,
    client_id: Option<String>,
    code_verifier: Option<String>,
    refresh_token: Option<String>,
    resource: Option<String>,
}

fn invalid_grant(description: &str) -> Response {
    oauth_error(StatusCode::BAD_REQUEST, "invalid_grant", description)
}

fn invalid_request(description: &str) -> Response {
    oauth_error(StatusCode::BAD_REQUEST, "invalid_request", description)
}

fn server_error(error: impl Into<anyhow::Error>) -> Response {
    tracing::error!(
        error = format!("{:#}", error.into()),
        "échange de jeton OAuth"
    );
    oauth_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "server_error",
        "erreur interne",
    )
}

/// `POST /oauth/token` : un code contre des jetons, ou un jeton de
/// renouvellement contre de nouveaux jetons. Corps en
/// `application/x-www-form-urlencoded`, comme le veut OAuth.
async fn token(State(state): State<AppState>, body: Bytes) -> Response {
    let form = read_form(&body);
    if let Some(asked) = form.resource.as_deref()
        && asked.trim_end_matches('/') != resource(&state)
    {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_target",
            "ressource inconnue : seul /mcp est protégé ici",
        );
    }
    // Ménage à chaque passage : codes, jetons d'accès et autorisations expirés.
    let now = time::now();
    let cleaned = async {
        sqlx::query!("DELETE FROM oauth_codes WHERE expires_at <= ?", now)
            .execute(&state.db)
            .await?;
        sqlx::query!(
            "DELETE FROM access_tokens WHERE grant_id IS NOT NULL AND expires_at <= ?",
            now
        )
        .execute(&state.db)
        .await?;
        sqlx::query!("DELETE FROM oauth_grants WHERE expires_at <= ?", now)
            .execute(&state.db)
            .await
    }
    .await;
    if let Err(error) = cleaned {
        return server_error(error);
    }

    let result = match form.grant_type.as_deref() {
        Some("authorization_code") => exchange_code(&state, &form, now).await,
        Some("refresh_token") => refresh(&state, &form, now).await,
        _ => {
            return oauth_error(
                StatusCode::BAD_REQUEST,
                "unsupported_grant_type",
                "authorization_code ou refresh_token",
            );
        }
    };
    match result {
        Ok(response) => response,
        Err(error) => server_error(error),
    }
}

/// Lit un formulaire OAuth (application/x-www-form-urlencoded) ; un champ
/// inconnu est ignoré.
fn read_form(body: &[u8]) -> TokenForm {
    let mut form = TokenForm::default();
    for (key, value) in form_urlencoded::parse(body) {
        let value = Some(value.into_owned());
        match key.as_ref() {
            "grant_type" => form.grant_type = value,
            "code" => form.code = value,
            "redirect_uri" => form.redirect_uri = value,
            "client_id" => form.client_id = value,
            "code_verifier" => form.code_verifier = value,
            "refresh_token" => form.refresh_token = value,
            "resource" => form.resource = value,
            _ => {}
        }
    }
    form
}

/// Jetons d'une autorisation : un jeton d'accès neuf (1 heure) et le jeton
/// de renouvellement donné.
async fn issue(
    state: &AppState,
    grant_id: i64,
    user_id: i64,
    client_name: &str,
    refresh_token: &str,
    now: i64,
) -> sqlx::Result<Response> {
    let access = tokens::generate();
    let access_hash = tokens::hash(&access);
    let expires_at = now + ACCESS_SECONDS;
    sqlx::query!(
        "INSERT INTO access_tokens (user_id, grant_id, name, token_hash, created_at, expires_at)
         VALUES (?, ?, ?, ?, ?, ?)",
        user_id,
        grant_id,
        client_name,
        access_hash,
        now,
        expires_at
    )
    .execute(&state.db)
    .await?;
    Ok(no_store(
        Json(json!({
            "access_token": access,
            "token_type": "Bearer",
            "expires_in": ACCESS_SECONDS,
            "refresh_token": refresh_token,
        }))
        .into_response(),
    ))
}

async fn exchange_code(state: &AppState, form: &TokenForm, now: i64) -> anyhow::Result<Response> {
    let (Some(code), Some(verifier), Some(client_id), Some(redirect_uri)) = (
        form.code.as_deref(),
        form.code_verifier.as_deref(),
        form.client_id.as_deref(),
        form.redirect_uri.as_deref(),
    ) else {
        return Ok(invalid_request(
            "code, code_verifier, client_id et redirect_uri sont obligatoires",
        ));
    };
    // À usage unique : le code est effacé avant toute vérification.
    let code_hash = tokens::hash(code);
    let Some(stored) = sqlx::query!(
        "DELETE FROM oauth_codes WHERE code_hash = ? AND expires_at > ?
         RETURNING client_id, user_id, redirect_uri, code_challenge",
        code_hash,
        now
    )
    .fetch_optional(&state.db)
    .await?
    else {
        return Ok(invalid_grant("code inconnu, déjà utilisé ou expiré"));
    };
    if stored.client_id != client_id || stored.redirect_uri != redirect_uri {
        return Ok(invalid_grant(
            "ce code n'a pas été donné à ce client ou à cette adresse de retour",
        ));
    }
    if !pkce_matches(verifier, &stored.code_challenge) {
        return Ok(invalid_grant(
            "code_verifier ne correspond pas au défi PKCE",
        ));
    }
    let Some(user) = sqlx::query!(
        "SELECT u.email, c.name AS client_name FROM users u, oauth_clients c WHERE u.id = ? AND c.client_id = ?",
        stored.user_id,
        client_id
    )
    .fetch_optional(&state.db)
    .await?
    else {
        return Ok(invalid_grant("compte ou client disparu"));
    };
    if !state.config.is_allowed(&user.email) {
        return Ok(invalid_grant("ce compte n'a plus accès"));
    }

    let refresh_token = tokens::generate();
    let refresh_hash = tokens::hash(&refresh_token);
    let expires_at = now + REFRESH_SECONDS;
    let grant_id = sqlx::query!(
        "INSERT INTO oauth_grants (user_id, client_id, refresh_hash, created_at, last_used_at, expires_at)
         VALUES (?, ?, ?, ?, ?, ?)",
        stored.user_id,
        client_id,
        refresh_hash,
        now,
        now,
        expires_at
    )
    .execute(&state.db)
    .await?
    .last_insert_rowid();
    tracing::info!(
        user = stored.user_id,
        client = user.client_name,
        "jetons OAuth donnés"
    );
    Ok(issue(
        state,
        grant_id,
        stored.user_id,
        &user.client_name,
        &refresh_token,
        now,
    )
    .await?)
}

async fn refresh(state: &AppState, form: &TokenForm, now: i64) -> anyhow::Result<Response> {
    let (Some(presented), Some(client_id)) =
        (form.refresh_token.as_deref(), form.client_id.as_deref())
    else {
        return Ok(invalid_request(
            "refresh_token et client_id sont obligatoires",
        ));
    };
    let presented_hash = tokens::hash(presented);

    // Un ancien jeton qui revient a été copié : toute l'autorisation tombe.
    let reused = sqlx::query_scalar!(
        r#"SELECT id AS "id!" FROM oauth_grants WHERE previous_refresh_hash = ?"#,
        presented_hash
    )
    .fetch_optional(&state.db)
    .await?;
    if let Some(grant_id) = reused {
        sqlx::query!("DELETE FROM oauth_grants WHERE id = ?", grant_id)
            .execute(&state.db)
            .await?;
        tracing::warn!(
            grant_id,
            "jeton de renouvellement réutilisé : autorisation révoquée"
        );
        return Ok(invalid_grant(
            "jeton de renouvellement déjà utilisé : l'autorisation est révoquée",
        ));
    }

    let Some(grant) = sqlx::query!(
        r#"SELECT g.id AS "id!", g.user_id, g.client_id, u.email, c.name AS client_name
           FROM oauth_grants g
           JOIN users u ON u.id = g.user_id
           JOIN oauth_clients c ON c.client_id = g.client_id
           WHERE g.refresh_hash = ? AND g.expires_at > ?"#,
        presented_hash,
        now
    )
    .fetch_optional(&state.db)
    .await?
    else {
        return Ok(invalid_grant("jeton de renouvellement inconnu ou expiré"));
    };
    if grant.client_id != client_id {
        return Ok(invalid_grant("ce jeton n'a pas été donné à ce client"));
    }
    if !state.config.is_allowed(&grant.email) {
        return Ok(invalid_grant("ce compte n'a plus accès"));
    }

    let refresh_token = tokens::generate();
    let refresh_hash = tokens::hash(&refresh_token);
    let expires_at = now + REFRESH_SECONDS;
    sqlx::query!(
        "UPDATE oauth_grants SET previous_refresh_hash = refresh_hash, refresh_hash = ?,
           last_used_at = ?, expires_at = ? WHERE id = ?",
        refresh_hash,
        now,
        expires_at,
        grant.id
    )
    .execute(&state.db)
    .await?;
    Ok(issue(
        state,
        grant.id,
        grant.user_id,
        &grant.client_name,
        &refresh_token,
        now,
    )
    .await?)
}

// ---------------------------------------------------------------------
// Révocation (RFC 7009)
// ---------------------------------------------------------------------

/// `POST /oauth/revoke` : un jeton d'accès ou de renouvellement d'une
/// autorisation ; toute l'autorisation tombe. Répond 200 même pour un jeton
/// inconnu, comme le veut la RFC.
async fn revoke(State(state): State<AppState>, body: Bytes) -> Response {
    let token = form_urlencoded::parse(&body)
        .find(|(key, _)| key == "token")
        .map(|(_, value)| value.into_owned());
    let Some(token) = token else {
        return invalid_request("token est obligatoire");
    };
    let hash = tokens::hash(&token);
    let revoked = sqlx::query!(
        "DELETE FROM oauth_grants WHERE refresh_hash = ?1
           OR id = (SELECT grant_id FROM access_tokens WHERE token_hash = ?1 AND grant_id IS NOT NULL)",
        hash
    )
    .execute(&state.db)
    .await;
    match revoked {
        Ok(done) => {
            if done.rows_affected() > 0 {
                tracing::info!("autorisation révoquée par le client");
            }
            no_store(StatusCode::OK.into_response())
        }
        Err(error) => server_error(error),
    }
}
