//! Connexion Google : OpenID Connect, flux « authorization code » avec PKCE.
//!
//! Déroulé :
//!   1. `/auth/google` : on génère `state`, `nonce` et le secret PKCE, on les
//!      garde en session, et on envoie le navigateur chez Google.
//!   2. Google renvoie le navigateur sur `/auth/google/callback?code=…&state=…`.
//!   3. On vérifie que `state` est celui de la session (la réponse correspond
//!      bien à une demande partie de ce navigateur), on échange le code contre
//!      un jeton d'identité en prouvant qu'on détient le secret PKCE, puis on
//!      vérifie la signature du jeton et son `nonce`.
//!   4. L'email vérifié par Google désigne le compte dashu.

use std::sync::Arc;

use anyhow::{Context, anyhow, bail};
use axum::extract::{Query, State};
use axum::response::Response;
use openidconnect::core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata};
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, Scope, TokenResponse, reqwest,
};
use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;
use tower_sessions::Session;

use super::session::{USER_ID, set_flash, sign_in};
use crate::AppState;
use crate::config::GoogleConfig;
use crate::web::{AppError, AppResult, see_other};

const ISSUER: &str = "https://accounts.google.com";
const FLOW_KEY: &str = "oidc";

pub struct GoogleOidc {
    client_id: ClientId,
    client_secret: ClientSecret,
    redirect_url: RedirectUrl,
    http: reqwest::Client,
    /// Configuration publique de Google (adresses, clés de signature),
    /// téléchargée à la première connexion puis gardée en mémoire.
    metadata: OnceCell<CoreProviderMetadata>,
}

/// Ce qu'on garde en session entre le départ chez Google et le retour.
#[derive(Serialize, Deserialize)]
struct Flow {
    state: String,
    nonce: String,
    pkce_verifier: String,
}

/// Identité renvoyée par Google, une fois le jeton vérifié.
pub struct GoogleIdentity {
    /// Identifiant stable du compte Google (ne change pas si l'email change).
    pub sub: String,
    pub email: String,
}

impl GoogleOidc {
    pub fn new(config: &GoogleConfig, base_url: &str) -> anyhow::Result<Arc<Self>> {
        let http = reqwest::ClientBuilder::new()
            // Suivre les redirections exposerait à des attaques SSRF.
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(10))
            .build()?;
        Ok(Arc::new(Self {
            client_id: ClientId::new(config.client_id.clone()),
            client_secret: ClientSecret::new(config.client_secret.clone()),
            redirect_url: RedirectUrl::new(format!("{base_url}/auth/google/callback"))
                .context("BASE_URL invalide")?,
            http,
            metadata: OnceCell::new(),
        }))
    }

    async fn metadata(&self) -> anyhow::Result<&CoreProviderMetadata> {
        self.metadata
            .get_or_try_init(|| async {
                CoreProviderMetadata::discover_async(
                    IssuerUrl::new(ISSUER.to_string())?,
                    &self.http,
                )
                .await
                .map_err(|e| anyhow!("découverte OpenID Connect : {e}"))
            })
            .await
    }

    /// Étape 1 : URL de Google vers laquelle envoyer le navigateur.
    async fn start(&self) -> anyhow::Result<(String, Flow)> {
        let client = CoreClient::from_provider_metadata(
            self.metadata().await?.clone(),
            self.client_id.clone(),
            Some(self.client_secret.clone()),
        )
        .set_redirect_uri(self.redirect_url.clone());

        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let (url, state, nonce) = client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            .add_scope(Scope::new("email".to_string()))
            .set_pkce_challenge(challenge)
            .url();

        let flow = Flow {
            state: state.secret().clone(),
            nonce: nonce.secret().clone(),
            pkce_verifier: verifier.secret().clone(),
        };
        Ok((url.to_string(), flow))
    }

    /// Étape 3 : échange le code contre un jeton d'identité et le vérifie.
    async fn finish(&self, flow: Flow, code: String) -> anyhow::Result<GoogleIdentity> {
        let client = CoreClient::from_provider_metadata(
            self.metadata().await?.clone(),
            self.client_id.clone(),
            Some(self.client_secret.clone()),
        )
        .set_redirect_uri(self.redirect_url.clone());

        let tokens = client
            .exchange_code(AuthorizationCode::new(code))
            .map_err(|e| anyhow!("configuration OpenID Connect : {e}"))?
            .set_pkce_verifier(PkceCodeVerifier::new(flow.pkce_verifier))
            .request_async(&self.http)
            .await
            .map_err(|e| anyhow!("échange du code : {e}"))?;

        let id_token = tokens
            .id_token()
            .context("Google n'a pas renvoyé de jeton d'identité")?;
        // Vérifie la signature (clés publiques de Google), l'émetteur, le
        // destinataire (notre client ID), l'expiration et le nonce.
        let claims = id_token
            .claims(&client.id_token_verifier(), &Nonce::new(flow.nonce))
            .map_err(|e| anyhow!("jeton d'identité invalide : {e}"))?;

        let email = claims
            .email()
            .context("Google n'a pas renvoyé d'email")?
            .as_str()
            .to_lowercase();
        if claims.email_verified() != Some(true) {
            bail!("email Google non vérifié");
        }
        Ok(GoogleIdentity {
            sub: claims.subject().as_str().to_string(),
            email,
        })
    }
}

/// `GET /auth/google`
pub async fn start(State(state): State<AppState>, session: Session) -> AppResult<Response> {
    let Some(google) = &state.google else {
        return Err(AppError::NotFound);
    };
    match google.start().await {
        Ok((url, flow)) => {
            session.insert(FLOW_KEY, flow).await?;
            Ok(see_other(&url))
        }
        Err(error) => {
            tracing::warn!(error = format!("{error:#}"), "connexion Google impossible");
            Ok(see_other("/login?e=google"))
        }
    }
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

/// `GET /auth/google/callback`
pub async fn callback(
    State(state): State<AppState>,
    session: Session,
    Query(query): Query<CallbackQuery>,
) -> AppResult<Response> {
    let Some(google) = &state.google else {
        return Err(AppError::NotFound);
    };
    // Le flux est à usage unique : on le retire de la session quoi qu'il arrive.
    let flow = session.remove::<Flow>(FLOW_KEY).await?;
    let (Some(flow), Some(code), Some(returned_state)) = (flow, query.code, query.state) else {
        return Ok(see_other("/login?e=google"));
    };
    if returned_state != flow.state {
        tracing::warn!("retour Google avec un paramètre state inattendu");
        return Ok(see_other("/login?e=google"));
    }

    let identity = match google.finish(flow, code).await {
        Ok(identity) => identity,
        Err(error) => {
            tracing::warn!(error = format!("{error:#}"), "échec de la connexion Google");
            return Ok(see_other("/login?e=google"));
        }
    };

    // Inscription fermée : même refus qu'un mauvais mot de passe.
    if !state.config.is_allowed(&identity.email) {
        tracing::warn!("connexion Google refusée : email hors de ALLOWED_EMAILS");
        return Ok(see_other("/login?e=refuse"));
    }

    // Liaison demandée depuis les réglages par un utilisateur déjà connecté :
    // le compte Google doit porter la même adresse que le compte dashu. On le
    // vérifie avant toute écriture, pour ne pas créer un second compte.
    if let Some(current) = session.get::<i64>(USER_ID).await? {
        let email = sqlx::query_scalar!("SELECT email FROM users WHERE id = ?", current)
            .fetch_optional(&state.db)
            .await?;
        if email.is_some_and(|email| email.eq_ignore_ascii_case(&identity.email)) {
            sqlx::query!(
                "UPDATE users SET google_sub = ? WHERE id = ?",
                identity.sub,
                current
            )
            .execute(&state.db)
            .await?;
            set_flash(&session, "ok", "Compte Google lié").await?;
        } else {
            set_flash(
                &session,
                "error",
                "Ce compte Google n'a pas la même adresse email que ton compte dashu.",
            )
            .await?;
        }
        return Ok(see_other("/settings"));
    }

    let user_id = find_or_create_user(&state, &identity).await?;
    sign_in(&state, &session, user_id).await?;
    Ok(see_other("/"))
}

/// Retrouve le compte dashu d'une identité Google, ou le crée.
///
/// L'ordre compte : d'abord par identifiant Google (compte déjà lié), puis
/// par email vérifié (premier passage par Google d'un compte créé avec un mot
/// de passe : on le lie), sinon création.
async fn find_or_create_user(state: &AppState, identity: &GoogleIdentity) -> AppResult<i64> {
    if let Some(id) = sqlx::query_scalar!(
        r#"SELECT id AS "id!" FROM users WHERE google_sub = ?"#,
        identity.sub
    )
    .fetch_optional(&state.db)
    .await?
    {
        return Ok(id);
    }

    if let Some(id) = sqlx::query_scalar!(
        r#"SELECT id AS "id!" FROM users WHERE email = ?"#,
        identity.email
    )
    .fetch_optional(&state.db)
    .await?
    {
        sqlx::query!(
            "UPDATE users SET google_sub = ? WHERE id = ?",
            identity.sub,
            id
        )
        .execute(&state.db)
        .await?;
        return Ok(id);
    }

    let now = crate::time::now();
    let id = sqlx::query!(
        "INSERT INTO users (email, google_sub, created_at) VALUES (?, ?, ?)",
        identity.email,
        identity.sub,
        now
    )
    .execute(&state.db)
    .await?
    .last_insert_rowid();
    Ok(id)
}
