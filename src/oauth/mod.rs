//! OAuth 2.1 pour le connecteur Claude (SPEC, « Connecteur Claude (MCP) »,
//! étape 3).
//!
//! claude.ai, l'app iPhone et Claude Code ne demandent pas de jeton collé :
//! ils s'autorisent depuis une page de dashu, comme on autorise une
//! application à se connecter à son compte Google. Le déroulé, tel que MCP le
//! demande :
//!
//! 1. Claude appelle `/mcp` sans jeton : 401, avec l'adresse du document
//!    `/.well-known/oauth-protected-resource` (RFC 9728), qui désigne dashu
//!    comme serveur d'autorisation.
//! 2. Claude lit `/.well-known/oauth-authorization-server` (RFC 8414) : les
//!    adresses des étapes suivantes, PKCE obligatoire.
//! 3. Enregistrement (`POST /oauth/register`, RFC 7591) : un client public,
//!    sans secret, dont les adresses de retour sont celles de Claude.
//! 4. Autorisation (`GET /oauth/authorize`) : la page « Autoriser Claude »,
//!    après la connexion habituelle si besoin. Autoriser renvoie le
//!    navigateur chez Claude avec un code à usage unique (60 secondes).
//! 5. Échange (`POST /oauth/token`) : le code et le secret PKCE contre un
//!    jeton d'accès (1 heure, `/mcp` seulement) et un jeton de
//!    renouvellement (90 jours, changé à chaque usage).
//!
//! PKCE (Proof Key for Code Exchange) : Claude invente un secret, n'envoie
//! que son empreinte à l'autorisation, puis le secret lui-même à l'échange.
//! Un code intercepté ne sert donc à rien.

use axum::Json;
use axum::Router;
use axum::extract::{Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tower_sessions::Session;

use crate::AppState;
use crate::auth::session::USER_ID;
use crate::auth::tokens;
use crate::time;
use crate::web::{self, AppError, AppResult, csrf};

pub mod endpoints;

pub use endpoints::router;

/// Clé de la session : l'autorisation en attente (pendant la connexion, puis
/// sur la page de consentement). Jamais dans l'adresse.
pub const PENDING: &str = "oauth_pending";
/// Une autorisation en attente vaut 10 minutes.
const PENDING_SECONDS: i64 = 600;
const CODE_SECONDS: i64 = 60;
pub(crate) const ACCESS_SECONDS: i64 = 3600;
pub(crate) const REFRESH_SECONDS: i64 = 90 * 24 * 3600;
/// Un client enregistré mais jamais autorisé est effacé au bout de 24 heures.
pub(crate) const UNUSED_CLIENT_SECONDS: i64 = 24 * 3600;

/// Adresses de retour de claude.ai (et donc de l'app iPhone et de Claude
/// Desktop, qui passent par le même compte). Claude Code, lui, revient sur
/// une adresse locale (`http://localhost:<port>/…`).
const CLAUDE_CALLBACKS: [&str; 2] = [
    "https://claude.ai/api/mcp/auth_callback",
    "https://claude.com/api/mcp/auth_callback",
];

pub(crate) fn base_url(state: &AppState) -> String {
    state.config.base_url.trim_end_matches('/').to_string()
}

/// L'adresse protégée : `/mcp` sous `BASE_URL`.
pub(crate) fn resource(state: &AppState) -> String {
    format!("{}/mcp", base_url(state))
}

/// Adresse de retour admise : celles de claude.ai, ou une adresse locale de
/// Claude Code (`http://localhost` ou `http://127.0.0.1`, n'importe quel
/// port). Sans cette règle, n'importe quel site pourrait s'enregistrer,
/// montrer la page « Autoriser » sous un faux nom et repartir avec un accès.
pub fn redirect_allowed(uri: &str) -> bool {
    if CLAUDE_CALLBACKS.contains(&uri) {
        return true;
    }
    let Ok(url) = url::Url::parse(uri) else {
        return false;
    };
    url.scheme() == "http"
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && matches!(url.host_str(), Some("localhost" | "127.0.0.1"))
}

/// Origine d'une adresse de retour (`https://claude.ai`,
/// `http://localhost:33418`), pour la CSP de la page de consentement.
fn origin_of(uri: &str) -> Option<String> {
    let url = url::Url::parse(uri).ok()?;
    Some(url.origin().ascii_serialization())
}

/// Ce que montre la page « Autoriser » de l'adresse de retour.
fn redirect_label(uri: &str) -> String {
    match url::Url::parse(uri)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
    {
        Some(host) if host == "localhost" || host == "127.0.0.1" => {
            "cet ordinateur (Claude Code)".to_string()
        }
        Some(host) => host,
        None => uri.to_string(),
    }
}

/// `BASE64URL(SHA-256(verifier))`, comparé au défi enregistré.
pub(crate) fn pkce_matches(verifier: &str, challenge: &str) -> bool {
    let valid = (43..=128).contains(&verifier.len())
        && verifier
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'));
    if !valid {
        return false;
    }
    let computed =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier));
    computed.len() == challenge.len()
        && computed
            .bytes()
            .zip(challenge.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

/// Un défi PKCE bien formé : 43 caractères en base64url (empreinte SHA-256).
fn challenge_valid(challenge: &str) -> bool {
    challenge.len() == 43
        && challenge
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

// ---------------------------------------------------------------------
// Autorisation : la page « Autoriser Claude »
// ---------------------------------------------------------------------

/// Une autorisation en attente, gardée dans la session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pending {
    client_id: String,
    client_name: String,
    redirect_uri: String,
    code_challenge: String,
    state: Option<String>,
    created_at: i64,
}

impl Pending {
    pub fn client_name(&self) -> &str {
        &self.client_name
    }
}

#[derive(Deserialize)]
pub struct AuthorizeQuery {
    response_type: Option<String>,
    client_id: Option<String>,
    redirect_uri: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    state: Option<String>,
    resource: Option<String>,
}

/// Renvoie le navigateur chez le client, avec ces paramètres en plus.
fn back_to_client(redirect_uri: &str, params: &[(&str, &str)]) -> Response {
    let mut url = match url::Url::parse(redirect_uri) {
        Ok(url) => url,
        Err(_) => {
            return AppError::BadRequest("Adresse de retour invalide.".into()).into_response();
        }
    };
    {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in params {
            pairs.append_pair(key, value);
        }
    }
    web::see_other(url.as_str())
}

/// Erreur rendue au client par son adresse de retour (RFC 6749, 4.1.2.1).
fn error_to_client(
    redirect_uri: &str,
    state: Option<&str>,
    error: &str,
    description: &str,
) -> Response {
    let mut params = vec![("error", error), ("error_description", description)];
    if let Some(state) = state {
        params.push(("state", state));
    }
    back_to_client(redirect_uri, &params)
}

struct ClientRow {
    name: String,
    redirect_uris: String,
}

async fn load_client(state: &AppState, client_id: &str) -> AppResult<Option<ClientRow>> {
    Ok(sqlx::query_as!(
        ClientRow,
        "SELECT name, redirect_uris FROM oauth_clients WHERE client_id = ?",
        client_id
    )
    .fetch_optional(&state.db)
    .await?)
}

#[derive(askama::Template)]
#[template(path = "oauth/authorize.html")]
struct ConsentPage {
    csrf: String,
    client_name: String,
    redirect: String,
    email: String,
}

/// Compte connecté dans cette session, s'il est toujours autorisé.
async fn signed_in(state: &AppState, session: &Session) -> AppResult<Option<(i64, String)>> {
    let Some(id) = session.get::<i64>(USER_ID).await? else {
        return Ok(None);
    };
    let email = sqlx::query_scalar!("SELECT email FROM users WHERE id = ?", id)
        .fetch_optional(&state.db)
        .await?;
    Ok(email
        .filter(|e| state.config.is_allowed(e))
        .map(|email| (id, email)))
}

/// `GET /oauth/authorize` : vérifie la demande, la garde dans la session,
/// fait passer par la connexion si besoin, puis montre la page de
/// consentement. Sans paramètres (retour de la connexion), reprend
/// l'autorisation en attente.
pub async fn authorize(
    State(state): State<AppState>,
    session: Session,
    Query(query): Query<AuthorizeQuery>,
) -> AppResult<Response> {
    let now = time::now();
    let pending = match query.client_id.as_deref() {
        Some(client_id) => {
            // Client et adresse de retour d'abord : tant qu'ils ne sont pas
            // sûrs, aucune redirection (on ne renvoie pas un navigateur vers
            // une adresse inconnue).
            let client = load_client(&state, client_id).await?.ok_or_else(|| {
                AppError::BadRequest("Application inconnue : recommence depuis Claude.".into())
            })?;
            let registered: Vec<String> =
                serde_json::from_str(&client.redirect_uris).unwrap_or_default();
            let redirect_uri = query.redirect_uri.clone().unwrap_or_default();
            if !registered.contains(&redirect_uri) {
                return Err(AppError::BadRequest(
                    "Adresse de retour inconnue pour cette application.".into(),
                ));
            }
            let client_state = query.state.as_deref();
            if query.response_type.as_deref() != Some("code") {
                return Ok(error_to_client(
                    &redirect_uri,
                    client_state,
                    "unsupported_response_type",
                    "response_type=code attendu",
                ));
            }
            let challenge = query.code_challenge.clone().unwrap_or_default();
            if query.code_challenge_method.as_deref() != Some("S256")
                || !challenge_valid(&challenge)
            {
                return Ok(error_to_client(
                    &redirect_uri,
                    client_state,
                    "invalid_request",
                    "PKCE obligatoire, en S256",
                ));
            }
            if let Some(resource) = query.resource.as_deref()
                && resource.trim_end_matches('/') != crate::oauth::resource(&state)
            {
                return Ok(error_to_client(
                    &redirect_uri,
                    client_state,
                    "invalid_target",
                    "ressource inconnue : seul /mcp est protégé ici",
                ));
            }
            let pending = Pending {
                client_id: client_id.to_string(),
                client_name: client.name,
                redirect_uri,
                code_challenge: challenge,
                state: query.state.clone(),
                created_at: now,
            };
            session.insert(PENDING, &pending).await?;
            pending
        }
        None => session
            .get::<Pending>(PENDING)
            .await?
            .filter(|p| now - p.created_at < PENDING_SECONDS)
            .ok_or_else(|| {
                AppError::BadRequest(
                    "Aucune autorisation en cours : recommence depuis Claude.".into(),
                )
            })?,
    };

    let Some((_, email)) = signed_in(&state, &session).await? else {
        // La connexion reviendra ici (voir `crate::auth::after_sign_in`).
        return Ok(web::see_other("/login"));
    };

    let page = ConsentPage {
        csrf: csrf::token(&session).await?,
        redirect: redirect_label(&pending.redirect_uri),
        client_name: pending.client_name.clone(),
        email,
    };
    let mut response = web::html(page);
    // Le formulaire renvoie le navigateur chez Claude : la CSP doit laisser
    // partir le formulaire vers cette origine-là, et seulement elle.
    if let Some(origin) = origin_of(&pending.redirect_uri)
        && let Ok(value) = HeaderValue::from_str(&web::headers::csp_with_form_action(&origin))
    {
        response
            .headers_mut()
            .insert(header::CONTENT_SECURITY_POLICY, value);
    }
    Ok(response)
}

#[derive(Deserialize)]
pub struct DecisionForm {
    decision: String,
}

/// `POST /oauth/authorize` : Autoriser ou Refuser. La demande vient de la
/// session, jamais du formulaire : elle ne peut pas être modifiée en route.
pub async fn decide(
    State(state): State<AppState>,
    session: Session,
    axum::Form(form): axum::Form<DecisionForm>,
) -> AppResult<Response> {
    let now = time::now();
    let pending = session
        .remove::<Pending>(PENDING)
        .await?
        .filter(|p| now - p.created_at < PENDING_SECONDS)
        .ok_or_else(|| {
            AppError::BadRequest("Cette autorisation a expiré : recommence depuis Claude.".into())
        })?;
    let Some((user_id, _)) = signed_in(&state, &session).await? else {
        return Ok(web::see_other("/login"));
    };
    let client_state = pending.state.as_deref();
    if form.decision != "allow" {
        tracing::info!(
            user = user_id,
            client = pending.client_name,
            "autorisation refusée"
        );
        return Ok(error_to_client(
            &pending.redirect_uri,
            client_state,
            "access_denied",
            "accès refusé",
        ));
    }

    let code = tokens::generate();
    let code_hash = tokens::hash(&code);
    let expires_at = now + CODE_SECONDS;
    sqlx::query!(
        "INSERT INTO oauth_codes (code_hash, client_id, user_id, redirect_uri, code_challenge, expires_at)
         VALUES (?, ?, ?, ?, ?, ?)",
        code_hash,
        pending.client_id,
        user_id,
        pending.redirect_uri,
        pending.code_challenge,
        expires_at
    )
    .execute(&state.db)
    .await?;
    tracing::info!(
        user = user_id,
        client = pending.client_name,
        "autorisation donnée"
    );

    let mut params = vec![("code", code.as_str())];
    if let Some(client_state) = client_state {
        params.push(("state", client_state));
    }
    Ok(back_to_client(&pending.redirect_uri, &params))
}

// ---------------------------------------------------------------------
// Réponses JSON des points d'entrée sans session
// ---------------------------------------------------------------------

/// Erreur OAuth en JSON (RFC 6749, 5.2).
pub(crate) fn oauth_error(status: StatusCode, error: &str, description: &str) -> Response {
    no_store(
        (
            status,
            Json(json!({ "error": error, "error_description": description })),
        )
            .into_response(),
    )
}

/// Une réponse qui porte un jeton ne se garde dans aucun cache.
pub(crate) fn no_store(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

/// La page d'autorisation : avec la session et la protection CSRF du site.
pub(crate) fn routes_with_session() -> Router<AppState> {
    Router::new().route("/oauth/authorize", get(authorize).post(decide))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adresses_de_retour() {
        assert!(redirect_allowed("https://claude.ai/api/mcp/auth_callback"));
        assert!(redirect_allowed("https://claude.com/api/mcp/auth_callback"));
        assert!(redirect_allowed("http://localhost:33418/callback"));
        assert!(redirect_allowed("http://127.0.0.1:5000/"));
        assert!(redirect_allowed("http://localhost/callback"));
        assert!(!redirect_allowed("https://claude.ai/autre"));
        assert!(!redirect_allowed(
            "https://claude.ai.evil.example/api/mcp/auth_callback"
        ));
        assert!(!redirect_allowed(
            "https://evil.example/api/mcp/auth_callback"
        ));
        assert!(!redirect_allowed("https://localhost/callback"));
        assert!(!redirect_allowed("http://localhost.evil.example/callback"));
        assert!(!redirect_allowed("http://user@localhost/callback"));
        assert!(!redirect_allowed("http://localhost/callback#x"));
        assert!(!redirect_allowed("http://192.168.1.10/callback"));
        assert!(!redirect_allowed("javascript:alert(1)"));
    }

    #[test]
    fn pkce_s256() {
        // Exemple de la RFC 7636, annexe B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert!(challenge_valid(challenge));
        assert!(pkce_matches(verifier, challenge));
        assert!(!pkce_matches(
            "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXl",
            challenge
        ));
        assert!(!pkce_matches("court", challenge));
        assert!(!challenge_valid("abc"));
    }

    #[test]
    fn origine_et_libelle_de_l_adresse_de_retour() {
        assert_eq!(
            origin_of("https://claude.ai/api/mcp/auth_callback").unwrap(),
            "https://claude.ai"
        );
        assert_eq!(
            origin_of("http://localhost:33418/callback").unwrap(),
            "http://localhost:33418"
        );
        assert_eq!(
            redirect_label("https://claude.ai/api/mcp/auth_callback"),
            "claude.ai"
        );
        assert_eq!(
            redirect_label("http://127.0.0.1:5000/cb"),
            "cet ordinateur (Claude Code)"
        );
    }
}
