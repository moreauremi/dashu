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
    /// Le lien ne marche plus : son auteur n'a plus accès à l'espace du
    /// fichier (fichier rangé ailleurs, membre parti). La page publique le
    /// refuse ; on le dit au lieu de le montrer comme valable.
    pub broken: bool,
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
            r#"SELECT l.id AS "id!", l.expires_at, l.created_at,
                      f.space_id NOT IN (SELECT space_id FROM space_access WHERE user_id = l.user_id) AS "broken!: bool"
               FROM share_links l
               JOIN files f ON f.id = l.file_id
               WHERE l.file_id = ?2 AND l.revoked_at IS NULL AND l.expires_at > ?3
                 AND f.space_id IN (SELECT space_id FROM space_access WHERE user_id = ?1)
               ORDER BY l.created_at DESC, l.id DESC"#,
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
            broken: row.broken,
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
        r#"SELECT id AS "id!" FROM files WHERE id = ? AND expires_at IS NULL
           AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?)"#,
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
    let created = create_link(&state, user, id, &form.duree).await?;
    let mut fragment = ShareFragment::load(&state.db, user, id).await?;
    fragment.new_link = Some(created.url);
    fragment.new_for = created.label;
    Ok(web::html(fragment))
}

/// Un lien tout juste créé : son adresse entière, montrée une seule fois.
pub(crate) struct NewLink {
    pub id: i64,
    pub url: String,
    pub label: &'static str,
    pub expires_at: i64,
}

/// Refus de partager un fichier venu du téléchargeur de liens.
pub(crate) const FROM_LINK: &str =
    "Un fichier venu du téléchargeur de liens ne se partage pas par lien.";

/// Crée un lien vers un fichier rangé et accessible (404 sinon), pour une
/// durée de `DURATIONS` (`1h`, `1j`, `7j`). Aussi pour l'API.
pub(crate) async fn create_link(
    state: &AppState,
    user: i64,
    id: i64,
    duration: &str,
) -> AppResult<NewLink> {
    ensure_shareable(&state.db, user, id).await?;
    // Un fichier venu du téléchargeur de liens ne se partage jamais par lien
    // (SPEC v2, « Outils : téléchargeur de liens »), ni ses conversions.
    let from_link = sqlx::query_scalar!(
        r#"SELECT from_link AS "from_link!: bool" FROM files WHERE id = ?"#,
        id
    )
    .fetch_one(&state.db)
    .await?;
    if from_link {
        return Err(AppError::BadRequest(FROM_LINK.into()));
    }
    let (_, seconds, label) = DURATIONS
        .iter()
        .find(|(key, _, _)| *key == duration)
        .copied()
        .ok_or_else(|| AppError::BadRequest("Durée inconnue : 1h, 1j ou 7j.".into()))?;
    let token = web::csrf::random_token();
    let token_hash = hash(&token);
    let now = time::now();
    let expires_at = now + seconds;
    let link = sqlx::query!(
        "INSERT INTO share_links (user_id, file_id, token_hash, expires_at, created_at) VALUES (?, ?, ?, ?, ?)",
        user,
        id,
        token_hash,
        expires_at,
        now
    )
    .execute(&state.db)
    .await?
    .last_insert_rowid();
    Ok(NewLink {
        id: link,
        url: format!("{}/s/{token}", state.config.base_url.trim_end_matches('/')),
        label,
        expires_at,
    })
}

/// Révoque un lien d'un fichier accessible (404 sinon). Aussi pour l'API.
pub(crate) async fn revoke_link(db: &SqlitePool, user: i64, id: i64, link: i64) -> AppResult<()> {
    let now = time::now();
    let revoked = sqlx::query!(
        "UPDATE share_links SET revoked_at = ?1 WHERE id = ?2 AND file_id = ?3 AND revoked_at IS NULL
           AND file_id IN (SELECT id FROM files WHERE expires_at IS NULL
                           AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?4))",
        now,
        link,
        id,
        user
    )
    .execute(db)
    .await?
    .rows_affected();
    if revoked == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// `POST /files/{id}/share/{link}/revoke` : le lien ne marche plus, tout de suite.
pub async fn revoke(
    ctx: Ctx,
    State(state): State<AppState>,
    Path((id, link)): Path<(i64, i64)>,
) -> AppResult<Response> {
    let user = ctx.user.id;
    revoke_link(&state.db, user, id, link).await?;
    Ok(web::html(ShareFragment::load(&state.db, user, id).await?))
}

/// Liens encore valables d'un fichier que leur auteur ne peut plus servir
/// (il n'a pas accès à l'espace du fichier) : un déplacement le dit.
pub(crate) async fn broken_links(db: &SqlitePool, file: i64) -> AppResult<i64> {
    let now = time::now();
    Ok(sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!: i64" FROM share_links l JOIN files f ON f.id = l.file_id
           WHERE l.file_id = ?1 AND l.revoked_at IS NULL AND l.expires_at > ?2
             AND f.space_id NOT IN (SELECT space_id FROM space_access WHERE user_id = l.user_id)"#,
        file,
        now
    )
    .fetch_one(db)
    .await?)
}

/// Les fichiers accessibles au compte (les siens et ceux de ses espaces) qui ont un lien valable (repère dans la liste).
pub(crate) async fn shared_files(db: &SqlitePool, user: i64) -> AppResult<HashSet<i64>> {
    let now = time::now();
    Ok(sqlx::query_scalar!(
        r#"SELECT DISTINCT l.file_id AS "file_id!" FROM share_links l
           JOIN files f ON f.id = l.file_id
           WHERE f.space_id IN (SELECT space_id FROM space_access WHERE user_id = ?1)
             AND l.revoked_at IS NULL AND l.expires_at > ?2
             AND f.space_id IN (SELECT space_id FROM space_access WHERE user_id = l.user_id)"#,
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
    // Le créateur du lien doit encore avoir accès à l'espace du fichier : un
    // fichier sorti d'un espace partagé vers l'espace privé d'un autre membre,
    // ou un membre parti, ne laisse pas de lien ouvert derrière lui.
    Ok(sqlx::query!(
        r#"SELECT f.name, f.size, f.storage_key, l.expires_at
           FROM share_links l JOIN files f ON f.id = l.file_id
           WHERE l.token_hash = ? AND l.revoked_at IS NULL AND l.expires_at > ? AND f.expires_at IS NULL
             AND f.space_id IN (SELECT space_id FROM space_access WHERE user_id = l.user_id)"#,
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
