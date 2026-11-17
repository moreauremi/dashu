//! Google Agenda (SPEC, « Événements et Google Agenda ») : un compte dashu
//! relie son agenda Google, puis ses événements créés dans dashu y sont
//! copiés, modifiés et supprimés. Un seul sens, de dashu vers Google.
//!
//! Connexion (OAuth 2.0, flux « authorization code » avec PKCE) :
//!   1. `/settings/google-calendar/connect` : on garde `state` et le secret
//!      PKCE en session et on envoie le navigateur chez Google, qui demande la
//!      permission `calendar.events` ;
//!   2. Google revient sur `/auth/google/callback`, la même adresse que la
//!      connexion par Google (déjà déclarée dans la console) : son handler
//!      passe la main à [`finish_link`] quand le `state` est le nôtre ;
//!   3. on échange le code contre un jeton de rafraîchissement, gardé chiffré
//!      (ChaCha20-Poly1305, clé dérivée de SESSION_SECRET par HKDF).
//!
//! Envoi : après chaque changement ([`push_soon`]), et toutes les cinq minutes
//! pour ce qui n'est pas passé ([`spawn_retry`]). Un envoi à la fois
//! (`AppState::google_push`). Les adresses de Google viennent de la
//! configuration : les tests les font pointer vers un faux Google local.

use std::time::Duration;

use axum::extract::State;
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::aead::{CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower_sessions::Session;

use crate::AppState;
use crate::auth::session::{Ctx, set_flash};
use crate::config::GoogleConfig;
use crate::time;
use crate::web::{AppError, AppResult, see_other};

/// Permission demandée : créer, modifier et supprimer des événements. Plus
/// l'adresse e-mail, pour montrer à quel compte l'agenda est relié.
const SCOPE: &str = "openid email https://www.googleapis.com/auth/calendar.events";
const CALENDAR_SCOPE: &str = "https://www.googleapis.com/auth/calendar.events";
/// Ce qu'on garde en session entre le départ chez Google et le retour.
const FLOW_KEY: &str = "google_calendar";
/// Retour dans les réglages, sur la carte Google Agenda.
const BACK: &str = "/settings#google-agenda";
/// Nouvel essai de ce qui n'est pas passé.
const RETRY_EVERY: Duration = Duration::from_secs(5 * 60);

// ------------------------------------------------------------- Chiffrement

/// Clé de chiffrement du jeton, dérivée de SESSION_SECRET : une copie de la
/// base ou d'une sauvegarde ne suffit pas à écrire dans l'agenda.
fn key(secret: &str) -> LessSafeKey {
    struct Len32;
    impl ring::hkdf::KeyType for Len32 {
        fn len(&self) -> usize {
            32
        }
    }
    let salt = ring::hkdf::Salt::new(ring::hkdf::HKDF_SHA256, b"dashu google agenda");
    let prk = salt.extract(secret.as_bytes());
    let info: [&[u8]; 1] = [b"jeton de rafraichissement v1"];
    let okm = prk.expand(&info, Len32).expect("HKDF : 32 octets");
    let mut bytes = [0u8; 32];
    okm.fill(&mut bytes).expect("HKDF : remplissage");
    LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, &bytes).expect("clé ChaCha20"))
}

/// Chiffre avec la clé du serveur (SESSION_SECRET).
pub fn seal(state: &AppState, plain: &str) -> Vec<u8> {
    seal_with(&state.config.session_secret, plain)
}

/// Déchiffre avec la clé du serveur ; `None` si SESSION_SECRET a changé ou si
/// les octets ont été modifiés.
pub fn open(state: &AppState, sealed: &[u8]) -> Option<String> {
    open_with(&state.config.session_secret, sealed)
}

/// Chiffre : nonce aléatoire de 12 octets, puis le texte chiffré et son tag.
fn seal_with(secret: &str, plain: &str) -> Vec<u8> {
    let mut nonce = [0u8; 12];
    SystemRandom::new()
        .fill(&mut nonce)
        .expect("générateur aléatoire du système");
    let mut data = plain.as_bytes().to_vec();
    key(secret)
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            ring::aead::Aad::empty(),
            &mut data,
        )
        .expect("chiffrement");
    let mut out = nonce.to_vec();
    out.extend(data);
    out
}

fn open_with(secret: &str, sealed: &[u8]) -> Option<String> {
    if sealed.len() < 12 + 16 {
        return None;
    }
    let (nonce, rest) = sealed.split_at(12);
    let nonce = Nonce::try_assume_unique_for_key(nonce).ok()?;
    let mut data = rest.to_vec();
    let plain = key(secret)
        .open_in_place(nonce, ring::aead::Aad::empty(), &mut data)
        .ok()?;
    String::from_utf8(plain.to_vec()).ok()
}

// ------------------------------------------------------------- Connexion

#[derive(Serialize, Deserialize)]
struct Flow {
    state: String,
    verifier: String,
    user_id: i64,
}

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .expect("générateur aléatoire du système");
    URL_SAFE_NO_PAD.encode(bytes)
}

fn redirect_uri(state: &AppState) -> String {
    format!(
        "{}/auth/google/callback",
        state.config.base_url.trim_end_matches('/')
    )
}

fn google(state: &AppState) -> AppResult<&GoogleConfig> {
    state.config.google.as_ref().ok_or(AppError::NotFound)
}

/// `GET /settings/google-calendar/connect` : part chez Google.
pub async fn connect(ctx: Ctx, State(state): State<AppState>) -> AppResult<Response> {
    let google = google(&state)?;
    let verifier = random_token();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let flow = Flow {
        state: random_token(),
        verifier,
        user_id: ctx.user.id,
    };
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", &google.client_id)
        .append_pair("redirect_uri", &redirect_uri(&state))
        .append_pair("response_type", "code")
        .append_pair("scope", SCOPE)
        // Un jeton de rafraîchissement, redonné à chaque connexion.
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent")
        .append_pair("include_granted_scopes", "true")
        .append_pair("login_hint", &ctx.user.email)
        .append_pair("state", &flow.state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .finish();
    ctx.session.insert(FLOW_KEY, flow).await?;
    Ok(see_other(&format!("{}?{query}", google.endpoints.auth)))
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    #[serde(default)]
    scope: String,
}

#[derive(Deserialize)]
struct UserInfo {
    email: String,
}

/// Retour de Google sur `/auth/google/callback`. `None` si ce retour n'est
/// pas celui d'une connexion de Google Agenda (c'est alors une connexion par
/// Google, que le handler appelant traite).
pub async fn finish_link(
    state: &AppState,
    session: &Session,
    code: Option<&str>,
    returned_state: Option<&str>,
) -> AppResult<Option<Response>> {
    let Some(flow) = session.get::<Flow>(FLOW_KEY).await? else {
        return Ok(None);
    };
    if returned_state != Some(flow.state.as_str()) {
        return Ok(None);
    }
    session.remove::<Flow>(FLOW_KEY).await?;
    // Le compte qui a demandé la connexion doit être celui qui revient.
    if session.get::<i64>(crate::auth::session::USER_ID).await? != Some(flow.user_id) {
        return Ok(Some(see_other("/login")));
    }
    let Some(code) = code else {
        set_flash(session, "error", "Connexion à Google Agenda annulée").await?;
        return Ok(Some(see_other(BACK)));
    };
    let message = match link(state, flow.user_id, code, &flow.verifier).await {
        Ok(email) => {
            set_flash(
                session,
                "ok",
                format!("Google Agenda est relié au compte {email}"),
            )
            .await?;
            push_soon(state, flow.user_id);
            return Ok(Some(see_other(BACK)));
        }
        Err(message) => message,
    };
    set_flash(session, "error", message).await?;
    Ok(Some(see_other(BACK)))
}

/// Échange le code, vérifie la permission, garde le jeton chiffré.
async fn link(state: &AppState, user: i64, code: &str, verifier: &str) -> Result<String, String> {
    let google = google(state).map_err(|_| "Google n'est pas configuré".to_string())?;
    let response = state
        .http
        .post(&google.endpoints.token)
        .form(&[
            ("code", code),
            ("client_id", &google.client_id),
            ("client_secret", &google.client_secret),
            ("redirect_uri", &redirect_uri(state)),
            ("grant_type", "authorization_code"),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .map_err(|e| format!("Google ne répond pas : {e}"))?;
    if !response.status().is_success() {
        tracing::warn!(status = %response.status(), "échange du code Google Agenda refusé");
        return Err("Google a refusé la connexion : réessaie".into());
    }
    let tokens: TokenResponse = response
        .json()
        .await
        .map_err(|_| "Réponse de Google illisible".to_string())?;
    // L'écran de Google laisse décocher la permission : sans elle, rien à faire.
    if !tokens.scope.split(' ').any(|s| s == CALENDAR_SCOPE) {
        return Err(
            "La permission de gérer les événements n'a pas été accordée : reconnecte Google Agenda en la cochant".into(),
        );
    }
    let Some(refresh) = tokens.refresh_token else {
        return Err("Google n'a pas donné d'accès durable : réessaie".into());
    };
    let email = state
        .http
        .get(&google.endpoints.userinfo)
        .bearer_auth(&tokens.access_token)
        .send()
        .await
        .ok()
        .filter(|r| r.status().is_success())
        .ok_or("Google ne donne pas l'adresse du compte")?
        .json::<UserInfo>()
        .await
        .map_err(|_| "Réponse de Google illisible".to_string())?
        .email;

    let sealed = seal(state, &refresh);
    let now = time::now();
    let saved = async {
        let mut tx = state.db.begin().await?;
        sqlx::query!(
            "INSERT INTO google_accounts (user_id, email, refresh_token, connected_at, last_error)
             VALUES (?, ?, ?, ?, NULL)
             ON CONFLICT (user_id) DO UPDATE SET email = excluded.email,
               refresh_token = excluded.refresh_token, connected_at = excluded.connected_at,
               last_error = NULL",
            user,
            email,
            sealed,
            now
        )
        .execute(&mut *tx)
        .await?;
        // Tout ce qui doit être dans Google y repart (un autre compte Google
        // peut avoir été choisi : les copies absentes seront recréées).
        sqlx::query!(
            "UPDATE events SET google_dirty = google_dirty + 1 WHERE user_id = ? AND google = 1",
            user
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok::<_, sqlx::Error>(())
    };
    saved.await.map_err(|e| {
        tracing::error!(error = %e, "enregistrement du compte Google Agenda");
        "Erreur interne : réessaie".to_string()
    })?;
    Ok(email)
}

/// `POST /settings/google-calendar/disconnect` : retire la permission chez
/// Google (au mieux) et oublie le jeton. Les copies déjà dans Google restent.
pub async fn disconnect(ctx: Ctx, State(state): State<AppState>) -> AppResult<Response> {
    let google = google(&state)?;
    let account = sqlx::query_scalar!(
        "SELECT refresh_token FROM google_accounts WHERE user_id = ?",
        ctx.user.id
    )
    .fetch_optional(&state.db)
    .await?;
    if let Some(token) = account.as_deref().and_then(|t| open(&state, t))
        && let Err(error) = state
            .http
            .post(&google.endpoints.revoke)
            .form(&[("token", token.as_str())])
            .send()
            .await
    {
        tracing::warn!(%error, "retrait de la permission Google Agenda");
    }
    sqlx::query!("DELETE FROM google_accounts WHERE user_id = ?", ctx.user.id)
        .execute(&state.db)
        .await?;
    sqlx::query!(
        "DELETE FROM google_deletions WHERE user_id = ?",
        ctx.user.id
    )
    .execute(&state.db)
    .await?;
    ctx.flash_ok("Google Agenda est déconnecté. Les événements déjà copiés y restent.")
        .await?;
    Ok(see_other(BACK))
}

/// Le compte Google relié, pour les réglages et le formulaire d'événement.
pub struct Linked {
    pub email: String,
    pub last_error: Option<String>,
}

pub async fn linked(state: &AppState, user: i64) -> AppResult<Option<Linked>> {
    if state.config.google.is_none() {
        return Ok(None);
    }
    Ok(sqlx::query!(
        "SELECT email, last_error FROM google_accounts WHERE user_id = ?",
        user
    )
    .fetch_optional(&state.db)
    .await?
    .map(|row| Linked {
        email: row.email,
        last_error: row.last_error,
    }))
}

// ------------------------------------------------------------- Envoi

/// Lance l'envoi des changements d'un compte, sans attendre.
pub fn push_soon(state: &AppState, user: i64) {
    let state = state.clone();
    tokio::spawn(async move { push_user(&state, user).await });
}

/// Reprend toutes les cinq minutes ce qui n'est pas passé.
pub fn spawn_retry(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(RETRY_EVERY);
        loop {
            tick.tick().await;
            let users = sqlx::query_scalar!(
                r#"SELECT user_id AS "user_id!: i64" FROM google_accounts a
                   WHERE EXISTS (SELECT 1 FROM events e WHERE e.user_id = a.user_id AND e.google_dirty > 0)
                      OR EXISTS (SELECT 1 FROM google_deletions d WHERE d.user_id = a.user_id)"#
            )
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();
            for user in users {
                push_user(&state, user).await;
            }
        }
    });
}

/// Ce qui empêche d'envoyer quoi que ce soit (jeton retiré, Google absent).
async fn account_error(state: &AppState, user: i64, message: &str) {
    let _ = sqlx::query!(
        "UPDATE google_accounts SET last_error = ? WHERE user_id = ?",
        message,
        user
    )
    .execute(&state.db)
    .await;
}

/// Envoie à Google tous les changements en attente d'un compte : les
/// suppressions, puis les créations et modifications. Une erreur reste
/// notée sur l'événement, qui repartira au passage suivant.
pub async fn push_user(state: &AppState, user: i64) {
    let _one_at_a_time = state.google_push.lock().await;
    let Some(google) = state.config.google.as_ref() else {
        return;
    };
    let Ok(Some(sealed)) = sqlx::query_scalar!(
        "SELECT refresh_token FROM google_accounts WHERE user_id = ?",
        user
    )
    .fetch_optional(&state.db)
    .await
    else {
        return;
    };
    let Some(refresh) = open(state, &sealed) else {
        account_error(
            state,
            user,
            "Le jeton de Google Agenda ne se lit plus (SESSION_SECRET a changé) : reconnecte-le",
        )
        .await;
        return;
    };
    let access = match access_token(state, google, &refresh).await {
        Ok(token) => token,
        Err(message) => {
            account_error(state, user, &message).await;
            return;
        }
    };
    let api = Api {
        state,
        google,
        access: &access,
    };

    // Suppressions en attente.
    let deletions = sqlx::query_scalar!(
        "SELECT google_event_id FROM google_deletions WHERE user_id = ?",
        user
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    for id in deletions {
        if api.delete(&id).await.is_ok() {
            let _ = sqlx::query!(
                "DELETE FROM google_deletions WHERE user_id = ? AND google_event_id = ?",
                user,
                id
            )
            .execute(&state.db)
            .await;
        }
    }

    // Créations et modifications.
    let events = sqlx::query!(
        r#"SELECT id AS "id!: i64", title, location, starts_at, ends_at,
                  all_day AS "all_day!: bool", google AS "google!: bool", google_event_id,
                  google_dirty
           FROM events WHERE user_id = ? AND google_dirty > 0"#,
        user
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    for event in events {
        let body = event_body(
            event.id,
            &event.title,
            &event.location,
            event.starts_at,
            event.ends_at,
            event.all_day,
        );
        let result = match (event.google, event.google_event_id.as_deref()) {
            // Plus à envoyer (case décochée) : on retire la copie.
            (false, Some(id)) => api.delete(id).await.map(|()| None),
            (false, None) => Ok(None),
            (true, Some(id)) => match api.update(id, &body).await {
                Ok(()) => Ok(Some(id.to_string())),
                // Supprimée dans Google, ou autre compte Google : on recrée.
                Err(Failure::Gone) => api.insert(&body).await.map(Some),
                Err(e) => Err(e),
            },
            (true, None) => api.insert(&body).await.map(Some),
        };
        match result {
            Ok(google_id) => {
                // On ne retire que les changements envoyés : une modification
                // arrivée pendant l'envoi repartira au passage suivant.
                let saved = sqlx::query!(
                    "UPDATE events SET google_event_id = ?, google_error = NULL,
                       google_dirty = max(0, google_dirty - ?)
                     WHERE id = ?",
                    google_id,
                    event.google_dirty,
                    event.id
                )
                .execute(&state.db)
                .await
                .map(|r| r.rows_affected())
                .unwrap_or(1);
                // Supprimé dans dashu pendant que Google créait la copie :
                // elle part à la suppression.
                if saved == 0
                    && let Some(id) = google_id
                {
                    let _ = sqlx::query!(
                        "INSERT OR IGNORE INTO google_deletions (user_id, google_event_id) VALUES (?, ?)",
                        user,
                        id
                    )
                    .execute(&state.db)
                    .await;
                }
            }
            Err(failure) => {
                let message = failure.message();
                let _ = sqlx::query!(
                    "UPDATE events SET google_error = ? WHERE id = ?",
                    message,
                    event.id
                )
                .execute(&state.db)
                .await;
            }
        }
    }
    let _ = sqlx::query!(
        "UPDATE google_accounts SET last_error = NULL WHERE user_id = ?",
        user
    )
    .execute(&state.db)
    .await;
}

/// Jeton d'accès frais (une heure) à partir du jeton de rafraîchissement.
async fn access_token(
    state: &AppState,
    google: &GoogleConfig,
    refresh: &str,
) -> Result<String, String> {
    let response = state
        .http
        .post(&google.endpoints.token)
        .form(&[
            ("client_id", google.client_id.as_str()),
            ("client_secret", google.client_secret.as_str()),
            ("refresh_token", refresh),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .map_err(|e| format!("Google ne répond pas : {e}"))?;
    if response.status().is_client_error() {
        return Err(
            "L'accès à Google Agenda a été retiré ou a expiré : reconnecte-le dans Réglages".into(),
        );
    }
    if !response.status().is_success() {
        return Err(format!("Google répond {}", response.status()));
    }
    response
        .json::<TokenResponse>()
        .await
        .map(|t| t.access_token)
        .map_err(|_| "Réponse de Google illisible".into())
}

/// Corps d'un événement pour l'API Google Agenda. Les heures partent avec le
/// décalage de Paris et le fuseau, comme les afficherait Google.
pub fn event_body(
    id: i64,
    title: &str,
    location: &str,
    starts_at: i64,
    ends_at: i64,
    all_day: bool,
) -> Value {
    let when = |ts: i64| {
        if all_day {
            json!({ "date": time::paris_date(ts).format("%Y-%m-%d").to_string() })
        } else {
            json!({ "dateTime": time::paris(ts).to_rfc3339(), "timeZone": "Europe/Paris" })
        }
    };
    let mut body = json!({
        "summary": title,
        "start": when(starts_at),
        "end": when(ends_at),
        "extendedProperties": { "private": { "dashu": id.to_string() } },
    });
    if !location.is_empty() {
        body["location"] = json!(location);
    }
    body
}

/// Échec d'un appel à l'API Google Agenda.
enum Failure {
    /// L'événement n'existe pas (ou plus) chez Google.
    Gone,
    Other(String),
}

impl Failure {
    fn message(&self) -> String {
        match self {
            Self::Gone => "Introuvable dans Google Agenda".into(),
            Self::Other(message) => message.clone(),
        }
    }
}

struct Api<'a> {
    state: &'a AppState,
    google: &'a GoogleConfig,
    access: &'a str,
}

impl Api<'_> {
    fn url(&self, id: Option<&str>) -> String {
        let base = format!(
            "{}/calendars/primary/events",
            self.google.endpoints.calendar
        );
        match id {
            Some(id) => format!("{base}/{}", urlencode(id)),
            None => base,
        }
    }

    async fn check(
        response: reqwest::Result<reqwest::Response>,
    ) -> Result<reqwest::Response, Failure> {
        let response =
            response.map_err(|e| Failure::Other(format!("Google ne répond pas : {e}")))?;
        match response.status().as_u16() {
            200..=299 => Ok(response),
            404 | 410 => Err(Failure::Gone),
            status => Err(Failure::Other(format!("Google répond {status}"))),
        }
    }

    async fn insert(&self, body: &Value) -> Result<String, Failure> {
        let response = Self::check(
            self.state
                .http
                .post(self.url(None))
                .bearer_auth(self.access)
                .json(body)
                .send()
                .await,
        )
        .await?;
        let created: Value = response
            .json()
            .await
            .map_err(|_| Failure::Other("Réponse de Google illisible".into()))?;
        created["id"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| Failure::Other("Google n'a pas donné d'identifiant".into()))
    }

    async fn update(&self, id: &str, body: &Value) -> Result<(), Failure> {
        Self::check(
            self.state
                .http
                .put(self.url(Some(id)))
                .bearer_auth(self.access)
                .json(body)
                .send()
                .await,
        )
        .await
        .map(|_| ())
    }

    /// Supprimée chez Google, ou déjà absente : c'est pareil.
    async fn delete(&self, id: &str) -> Result<(), Failure> {
        match Self::check(
            self.state
                .http
                .delete(self.url(Some(id)))
                .bearer_auth(self.access)
                .send()
                .await,
        )
        .await
        {
            Ok(_) | Err(Failure::Gone) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

fn urlencode(text: &str) -> String {
    form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    #[test]
    fn le_jeton_chiffre_ne_se_lit_qu_avec_la_bonne_cle() {
        let secret = "0123456789abcdef".repeat(4);
        let sealed = seal_with(&secret, "1//jeton-de-rafraichissement");
        assert!(
            !sealed.windows(5).any(|w| w == b"jeton"),
            "le jeton n'apparaît pas en clair"
        );
        assert_eq!(
            open_with(&secret, &sealed).as_deref(),
            Some("1//jeton-de-rafraichissement")
        );
        // Deux chiffrements du même jeton diffèrent (nonce aléatoire).
        assert_ne!(sealed, seal_with(&secret, "1//jeton-de-rafraichissement"));
        // Autre SESSION_SECRET, octet modifié, trop court : illisible.
        assert_eq!(open_with(&"f".repeat(64), &sealed), None);
        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert_eq!(open_with(&secret, &tampered), None);
        assert_eq!(open_with(&secret, &sealed[..20]), None);
    }

    #[test]
    fn corps_d_un_evenement_pour_google() {
        let date = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
        let start = time::local_to_ts(date.and_hms_opt(14, 0, 0).unwrap(), time::PARIS);
        let body = event_body(7, "Réunion", "B204", start, start + 5400, false);
        assert_eq!(body["summary"], "Réunion");
        assert_eq!(body["location"], "B204");
        assert_eq!(body["start"]["dateTime"], "2026-11-18T14:00:00+01:00");
        assert_eq!(body["end"]["dateTime"], "2026-11-18T15:30:00+01:00");
        assert_eq!(body["start"]["timeZone"], "Europe/Paris");
        assert_eq!(body["extendedProperties"]["private"]["dashu"], "7");

        // Journée entière : des dates, la fin exclue (le lendemain) ; sans lieu.
        let day = time::day_start(date);
        let body = event_body(
            8,
            "Férié",
            "",
            day,
            time::day_start(date.succ_opt().unwrap()),
            true,
        );
        assert_eq!(body["start"], json!({ "date": "2026-11-18" }));
        assert_eq!(body["end"], json!({ "date": "2026-11-19" }));
        assert!(body.get("location").is_none());
    }
}
