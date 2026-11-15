//! Partage d'un fichier par lien temporaire (SPEC v2, « Partage par lien »).
//!
//! * Le lien contient 32 octets aléatoires ; seule leur empreinte SHA-256 est
//!   en base. Le lien entier ne s'affiche qu'une fois, à sa création.
//! * Valable 1 heure, 1 jour ou 7 jours ; révocable à tout moment depuis
//!   l'aperçu du fichier. Seul un fichier rangé se partage (pas un fichier
//!   de l'onglet Outils).
//! * La page publique `/s/<jeton>` ne montre que le nom et la taille, et
//!   télécharge le fichier (jamais affiché). Elle est limitée en nombre de
//!   requêtes, comme la connexion. Lien expiré, révoqué ou inconnu : la même
//!   page « lien invalide », sans dire lequel des trois.

use std::collections::HashSet;

use askama::Template;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tower_governor::GovernorError;

use crate::AppState;
use crate::auth::session::Ctx;
use crate::time;
use crate::web::{self, AppError, AppResult};

/// Durées proposées : (valeur du formulaire, secondes, libellé).
const DURATIONS: [(&str, i64, &str); 3] = [
    ("1h", 3600, "1 heure"),
    ("1j", 24 * 3600, "1 jour"),
    ("7j", 7 * 24 * 3600, "7 jours"),
];

/// Un lien encore valable, dans l'aperçu du fichier.
pub struct LinkRow {
    pub id: i64,
    /// « expire dans 23 h »
    pub expires: String,
    /// « créé il y a 5 min »
    pub created: String,
}

#[derive(Template)]
#[template(path = "files/_share.html")]
pub(crate) struct ShareFragment {
    pub id: i64,
    pub links: Vec<LinkRow>,
    /// Le lien qui vient d'être créé, en entier : affiché une seule fois.
    pub new_link: Option<String>,
    /// Sa durée, pour le message : « 1 jour ».
    pub new_for: &'static str,
    pub durations: [(&'static str, i64, &'static str); 3],
}

impl ShareFragment {
    pub(crate) async fn load(db: &SqlitePool, user: i64, id: i64) -> AppResult<Self> {
        let now = time::now();
        let links = sqlx::query!(
            r#"SELECT id AS "id!", expires_at, created_at FROM share_links
               WHERE user_id = ? AND file_id = ? AND revoked_at IS NULL AND expires_at > ?
               ORDER BY created_at DESC, id DESC"#,
            user,
            id,
            now
        )
        .fetch_all(db)
        .await?
        .into_iter()
        .map(|row| LinkRow {
            id: row.id,
            expires: format!(
                "expire dans {}",
                crate::tools::time_left(row.expires_at, now)
            ),
            created: format!("créé {}", time::ago(row.created_at, now)),
        })
        .collect();
        Ok(Self {
            id,
            links,
            new_link: None,
            new_for: "",
            durations: DURATIONS,
        })
    }
}

/// Empreinte d'un jeton, telle qu'elle est en base.
fn hash(token: &str) -> String {
    super::to_hex(&Sha256::digest(token.as_bytes()))
}

/// 404 si le fichier n'est pas au compte, ou pas encore rangé (onglet Outils).
async fn ensure_shareable(db: &SqlitePool, user: i64, id: i64) -> AppResult<()> {
    sqlx::query_scalar!(
        r#"SELECT id AS "id!" FROM files WHERE id = ? AND user_id = ? AND expires_at IS NULL"#,
        id,
        user
    )
    .fetch_optional(db)
    .await?
    .map(|_| ())
    .ok_or(AppError::NotFound)
}

#[derive(Deserialize)]
pub struct CreateForm {
    duree: String,
}

/// `POST /files/{id}/share` : crée un lien. Répond par le bloc « Partager »,
/// qui montre le lien entier cette fois-ci seulement.
pub async fn create(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<CreateForm>,
) -> AppResult<Response> {
    let user = ctx.user.id;
    ensure_shareable(&state.db, user, id).await?;
    let (_, seconds, label) = DURATIONS
        .iter()
        .find(|(key, _, _)| *key == form.duree)
        .copied()
        .ok_or_else(|| AppError::BadRequest("Durée inconnue.".into()))?;
    let token = web::csrf::random_token();
    let token_hash = hash(&token);
    let now = time::now();
    let expires_at = now + seconds;
    sqlx::query!(
        "INSERT INTO share_links (user_id, file_id, token_hash, expires_at, created_at) VALUES (?, ?, ?, ?, ?)",
        user,
        id,
        token_hash,
        expires_at,
        now
    )
    .execute(&state.db)
    .await?;
    let mut fragment = ShareFragment::load(&state.db, user, id).await?;
    fragment.new_link = Some(format!(
        "{}/s/{token}",
        state.config.base_url.trim_end_matches('/')
    ));
    fragment.new_for = label;
    Ok(web::html(fragment))
}

/// `POST /files/{id}/share/{link}/revoke` : le lien ne marche plus, tout de suite.
pub async fn revoke(
    ctx: Ctx,
    State(state): State<AppState>,
    Path((id, link)): Path<(i64, i64)>,
) -> AppResult<Response> {
    let user = ctx.user.id;
    let now = time::now();
    let revoked = sqlx::query!(
        "UPDATE share_links SET revoked_at = ? WHERE id = ? AND file_id = ? AND user_id = ? AND revoked_at IS NULL",
        now,
        link,
        id,
        user
    )
    .execute(&state.db)
    .await?
    .rows_affected();
    if revoked == 0 {
        return Err(AppError::NotFound);
    }
    Ok(web::html(ShareFragment::load(&state.db, user, id).await?))
}

/// Les fichiers d'un compte qui ont un lien valable (repère dans la liste).
pub(crate) async fn shared_files(db: &SqlitePool, user: i64) -> AppResult<HashSet<i64>> {
    let now = time::now();
    Ok(sqlx::query_scalar!(
        r#"SELECT DISTINCT file_id AS "file_id!" FROM share_links
           WHERE user_id = ? AND revoked_at IS NULL AND expires_at > ?"#,
        user,
        now
    )
    .fetch_all(db)
    .await?
    .into_iter()
    .collect())
}

// ---------------------------------------------------------------------
// Page publique
// ---------------------------------------------------------------------

/// Un fichier atteint par un lien valable.
struct Shared {
    name: String,
    size: i64,
    storage_key: String,
    expires_at: i64,
}

/// Le fichier d'un jeton, si le lien est valable : bon format, connu, ni
/// expiré ni révoqué, fichier toujours là et rangé.
async fn lookup(db: &SqlitePool, token: &str) -> AppResult<Option<Shared>> {
    if !super::is_token(token) {
        return Ok(None);
    }
    let token_hash = hash(token);
    let now = time::now();
    Ok(sqlx::query!(
        r#"SELECT f.name, f.size, f.storage_key, l.expires_at
           FROM share_links l JOIN files f ON f.id = l.file_id AND f.user_id = l.user_id
           WHERE l.token_hash = ? AND l.revoked_at IS NULL AND l.expires_at > ? AND f.expires_at IS NULL"#,
        token_hash,
        now
    )
    .fetch_optional(db)
    .await?
    .map(|row| Shared {
        name: row.name,
        size: row.size,
        storage_key: row.storage_key,
        expires_at: row.expires_at,
    }))
}

#[derive(Template)]
#[template(path = "share/page.html")]
struct PublicPage {
    /// `None` : lien invalide (expiré, révoqué ou inconnu, sans distinction).
    file: Option<PublicFile>,
    token: String,
}

struct PublicFile {
    name: String,
    size: String,
    /// « jusqu'au 16 novembre à 14:05 »
    until: String,
}

/// `GET /s/{token}` : la page du lien, sans connexion.
pub async fn page(State(state): State<AppState>, Path(token): Path<String>) -> AppResult<Response> {
    let shared = match state.config.files {
        Some(_) => lookup(&state.db, &token).await?,
        None => None,
    };
    let valid = shared.is_some();
    let page = PublicPage {
        file: shared.map(|s| PublicFile {
            size: super::human_size(s.size),
            until: format!(
                "jusqu'au {} à {}",
                time::day_month(time::paris_date(s.expires_at)),
                time::hm(s.expires_at)
            ),
            name: s.name,
        }),
        token: if valid { token } else { String::new() },
    };
    let mut response = web::html(page);
    if !valid {
        *response.status_mut() = StatusCode::NOT_FOUND;
    }
    // La page d'un lien n'a rien à faire dans un moteur de recherche.
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    Ok(response)
}

/// `GET /s/{token}/fichier` : le téléchargement, comme celui de l'app.
pub async fn download(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> AppResult<Response> {
    let files = state.config.files.as_ref().ok_or(AppError::NotFound)?;
    let shared = lookup(&state.db, &token).await?.ok_or(AppError::NotFound)?;
    let mut response =
        super::attachment_response(files, &shared.name, shared.size, &shared.storage_key).await?;
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    Ok(response)
}

/// Réponse 429 d'un lien trop sollicité : le middleware des pages d'erreur
/// la met en forme.
pub fn too_many_requests(error: GovernorError) -> Response {
    let message = match error {
        GovernorError::TooManyRequests { wait_time, .. } => {
            format!("Trop de demandes sur ce lien. Réessaie dans {wait_time} secondes.")
        }
        _ => "Trop de demandes sur ce lien. Réessaie dans une minute.".to_string(),
    };
    let status = StatusCode::TOO_MANY_REQUESTS;
    let mut response = (status, message.clone()).into_response();
    response
        .extensions_mut()
        .insert(web::error::ErrorInfo { status, message });
    response
}
