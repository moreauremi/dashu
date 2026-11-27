//! API, lot « contenu » (SPEC, « API complète et corbeille ») : les notes au
//! complet, les fichiers, les liens de partage, les dossiers et la
//! corbeille. Chaque route appelle les mêmes fonctions que le site : mêmes
//! droits (`space_access`), mêmes limites, mêmes refus.
//!
//! Une suppression part dans la corbeille, avec le nom du jeton comme trace.
//! Supprimer définitivement et vider la corbeille n'ont pas de route : cela
//! ne se fait que sur le site.

use axum::body::Body;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::json;
use sqlx::SqlitePool;

use super::{ApiError, ApiResult, ApiUser, iso};
use crate::AppState;
use crate::files::{self, Received, share, tags};
use crate::folders;
use crate::notes;
use crate::trash::{self, Kind};

/// 200 fichiers au plus par liste.
const FILES_MAX: i64 = 200;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/notes/{id}/download.md", get(note_markdown))
        .route("/notes/{id}/download.pdf", get(note_pdf))
        .route("/files", get(list_files))
        .route("/files/uploads", post(start_upload))
        .route("/files/uploads/{id}", put(upload_chunk))
        .route(
            "/files/{id}",
            get(get_file).patch(update_file).delete(delete_file),
        )
        .route("/files/{id}/download", get(download_file))
        .route("/files/{id}/shares", get(list_links).post(create_link))
        .route(
            "/files/{id}/shares/{link}",
            axum::routing::delete(revoke_link),
        )
        .route("/folders", get(list_folders).post(create_folder))
        .route(
            "/folders/{id}",
            axum::routing::patch(update_folder).delete(delete_folder),
        )
        .route("/trash", get(list_trash))
        .route("/trash/{kind}/{id}/restore", post(restore))
}

/// Un champ qui peut être absent (ne rien changer) ou `null` (la racine) :
/// `Option<Option<T>>`, avec `#[serde(default, deserialize_with = "some")]`.
pub(crate) fn some<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// Un refus « à montrer » du site (nom vide, nom pris…) devient un 400.
fn refused<T>(result: Result<T, String>) -> ApiResult<T> {
    result.map_err(ApiError::bad_request)
}

// ---------------------------------------------------------------------
// Notes : téléchargement (le reste est dans api/mod.rs)
// ---------------------------------------------------------------------

/// `GET /api/v1/notes/{id}/download.md`
async fn note_markdown(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    Ok(notes::export::markdown_download(&state, user.id, id).await?)
}

/// `GET /api/v1/notes/{id}/download.pdf?zoom=125&titres=1`
async fn note_pdf(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    query: Result<Query<notes::export::PdfQuery>, QueryRejection>,
) -> ApiResult<Response> {
    let Query(query) = query?;
    Ok(notes::export::pdf_download(&state, user.id, id, query).await?)
}

// ---------------------------------------------------------------------
// Fichiers
// ---------------------------------------------------------------------

#[derive(Serialize)]
pub(crate) struct FileOut {
    id: i64,
    name: String,
    pub(crate) mime: String,
    size: i64,
    /// Espace du fichier ; vide pour un fichier de Temporaire.
    space: String,
    folder: Option<i64>,
    tags: Vec<String>,
    /// Fichier de Temporaire (onglet Outils), effacé à `expires_at`.
    temporary: bool,
    expires_at: Option<String>,
    created_at: String,
    updated_at: String,
}

struct FileRow {
    id: i64,
    name: String,
    pub(crate) mime: String,
    size: i64,
    space_id: i64,
    folder_id: Option<i64>,
    expires_at: Option<i64>,
    created_at: i64,
    updated_at: i64,
}

fn file_out(user: &ApiUser, row: FileRow, tags: Vec<String>) -> FileOut {
    let temporary = row.expires_at.is_some();
    FileOut {
        id: row.id,
        name: row.name,
        mime: row.mime,
        size: row.size,
        space: if temporary {
            String::new()
        } else {
            user.slug_of(row.space_id)
        },
        folder: row.folder_id,
        tags,
        temporary,
        expires_at: row.expires_at.map(iso),
        created_at: iso(row.created_at),
        updated_at: iso(row.updated_at),
    }
}

/// Un fichier accessible : rangé dans un espace du compte, ou dans son
/// Temporaire. 404 sinon.
pub(crate) async fn fetch_file(db: &SqlitePool, user: &ApiUser, id: i64) -> ApiResult<FileOut> {
    let row = sqlx::query_as!(
        FileRow,
        r#"SELECT id AS "id!", name, mime, size, space_id, folder_id, expires_at, created_at, updated_at
           FROM files WHERE id = ?2
             AND ((expires_at IS NULL AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?1))
                  OR (expires_at IS NOT NULL AND user_id = ?1))"#,
        user.id,
        id
    )
    .fetch_optional(db)
    .await?
    .ok_or_else(|| ApiError::not_found("fichier"))?;
    let tags = tags::tags_of(db, user.id, id)
        .await?
        .into_iter()
        .map(|t| t.name)
        .collect();
    Ok(file_out(user, row, tags))
}

#[derive(Deserialize, Default)]
pub(crate) struct FileListQuery {
    pub(crate) space: Option<String>,
    /// Seulement ce dossier (pas ses sous-dossiers).
    pub(crate) folder: Option<i64>,
    pub(crate) tag: Option<String>,
    /// Recherche sur le nom (accents ignorés, début de mot).
    pub(crate) q: Option<String>,
    /// `true` : les fichiers de Temporaire au lieu des fichiers rangés.
    #[serde(default)]
    pub(crate) temporary: bool,
    pub(crate) limit: Option<i64>,
}

/// `GET /api/v1/files?space=ecole&folder=12&tag=bts&q=vlan&temporary=false&limit=50`
async fn list_files(
    user: ApiUser,
    State(state): State<AppState>,
    query: Result<Query<FileListQuery>, QueryRejection>,
) -> ApiResult<Json<Vec<FileOut>>> {
    let Query(query) = query?;
    Ok(Json(files_list(&state, &user, query).await?))
}

pub(crate) async fn files_list(
    state: &AppState,
    user: &ApiUser,
    query: FileListQuery,
) -> ApiResult<Vec<FileOut>> {
    let space = user.space_filter(query.space.as_deref())?;
    let tag = query
        .tag
        .as_deref()
        .map(notes::normalize_tag)
        .filter(|t| !t.is_empty());
    let fts = query.q.as_deref().and_then(notes::fts_query);
    let limit = query.limit.unwrap_or(50).clamp(1, FILES_MAX);
    let rows = sqlx::query_as!(
        FileRow,
        r#"SELECT f.id AS "id!", f.name, f.mime, f.size, f.space_id, f.folder_id, f.expires_at,
                  f.created_at, f.updated_at
           FROM files f
           WHERE ((?2 = 0 AND f.expires_at IS NULL
                   AND f.space_id IN (SELECT space_id FROM space_access WHERE user_id = ?1))
                  OR (?2 = 1 AND f.expires_at IS NOT NULL AND f.user_id = ?1))
             AND (?3 IS NULL OR f.space_id = ?3)
             AND (?4 IS NULL OR f.folder_id = ?4)
             AND (?5 IS NULL OR EXISTS (
                   SELECT 1 FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
                   WHERE ft.file_id = f.id AND t.name = ?5))
             AND (?6 IS NULL OR f.id IN (SELECT rowid FROM files_fts WHERE files_fts MATCH ?6))
           ORDER BY f.updated_at DESC, f.id DESC
           LIMIT ?7"#,
        user.id,
        query.temporary,
        space,
        query.folder,
        tag,
        fts,
        limit
    )
    .fetch_all(&state.db)
    .await?;
    let mut tags = tags::by_file(&state.db, user.id).await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let file_tags = tags.remove(&row.id).unwrap_or_default();
            file_out(user, row, file_tags)
        })
        .collect())
}

/// `GET /api/v1/files/{id}` : les informations d'un fichier.
async fn get_file(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<FileOut>> {
    Ok(Json(fetch_file(&state.db, &user, id).await?))
}

/// `GET /api/v1/files/{id}/download` : le contenu, en pièce jointe, comme
/// sur le site (jamais affiché ni exécuté).
async fn download_file(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    let config = state.config.files.as_ref().ok_or_else(files_disabled)?;
    let (name, size, key) = stored(&state.db, &user, id).await?;
    Ok(files::attachment_response(config, &name, size, &key).await?)
}

fn files_disabled() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "fichiers désactivés sur ce serveur (FILES_DIR)",
    )
}

/// Nom, taille et clé de stockage d'un fichier accessible.
pub(crate) async fn stored(
    db: &SqlitePool,
    user: &ApiUser,
    id: i64,
) -> ApiResult<(String, i64, String)> {
    let row = sqlx::query!(
        "SELECT name, size, storage_key FROM files WHERE id = ?2
           AND ((expires_at IS NULL AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?1))
                OR (expires_at IS NOT NULL AND user_id = ?1))",
        user.id,
        id
    )
    .fetch_optional(db)
    .await?
    .ok_or_else(|| ApiError::not_found("fichier"))?;
    Ok((row.name, row.size, row.storage_key))
}

#[derive(Deserialize)]
pub(crate) struct NewUpload {
    pub(crate) name: String,
    /// Taille totale, en octets.
    pub(crate) size: u64,
    /// Espace d'arrivée (ou celui du dossier). Pour Temporaire : Perso par défaut.
    pub(crate) space: Option<String>,
    pub(crate) folder: Option<i64>,
    /// `true` : dans Temporaire (onglet Outils), effacé au bout de 24 heures.
    #[serde(default)]
    pub(crate) temporary: bool,
}

/// L'espace et le dossier d'arrivée d'un envoi.
pub(crate) async fn upload_place(
    db: &SqlitePool,
    user: &ApiUser,
    upload: &NewUpload,
) -> ApiResult<(i64, Option<i64>)> {
    if upload.temporary {
        let space = match upload.space.as_deref() {
            Some(slug) => user.space(slug)?.id,
            None => user
                .spaces
                .first()
                .map(|s| s.id)
                .ok_or_else(|| ApiError::bad_request("aucun espace"))?,
        };
        return Ok((space, None));
    }
    match (upload.folder, upload.space.as_deref()) {
        (Some(folder), space) => {
            let found = folders::find(db, user.id, folder)
                .await?
                .ok_or_else(|| ApiError::not_found("dossier"))?;
            if let Some(slug) = space
                && user.space(slug)?.id != found.space_id
            {
                return Err(ApiError::bad_request(
                    "ce dossier n'est pas dans cet espace",
                ));
            }
            Ok((found.space_id, Some(found.id)))
        }
        (None, Some(slug)) => Ok((user.space(slug)?.id, None)),
        (None, None) => Err(ApiError::bad_request(
            "space ou folder est obligatoire (ou temporary: true)",
        )),
    }
}

/// `POST /api/v1/files/uploads` `{"name", "size", "space"?, "folder"?, "temporary"?}` :
/// ouvre un envoi, comme le site. Le contenu part ensuite en morceaux de
/// 50 Mo au plus par `PUT /api/v1/files/uploads/{upload}?offset=…`.
async fn start_upload(
    user: ApiUser,
    State(state): State<AppState>,
    payload: Result<Json<NewUpload>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let Json(upload) = payload?;
    state.config.files.as_ref().ok_or_else(files_disabled)?;
    let place = upload_place(&state.db, &user, &upload).await?;
    let id = files::open_upload(
        &state,
        user.id,
        place,
        &upload.name,
        upload.size,
        upload.temporary,
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "upload": id, "chunk_max": files::CHUNK_MAX_BYTES })),
    ))
}

#[derive(Deserialize)]
struct ChunkQuery {
    offset: u64,
}

/// `PUT /api/v1/files/uploads/{upload}?offset=…` : un morceau, dans l'ordre.
/// 200 `{"received"}` tant qu'il en manque ; 201 avec le fichier au dernier ;
/// 409 `{"received"}` si `offset` n'est pas le nombre d'octets déjà reçus.
async fn upload_chunk(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<String>,
    query: Result<Query<ChunkQuery>, QueryRejection>,
    body: Body,
) -> ApiResult<Response> {
    let Query(query) = query?;
    Ok(
        match files::receive(&state, user.id, &id, query.offset, body).await? {
            Received::OutOfOrder(received) => (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "ce n'est pas le morceau attendu : reprends à received",
                    "received": received,
                })),
            )
                .into_response(),
            Received::Partial(received) => Json(json!({ "received": received })).into_response(),
            Received::Done(file) => (
                StatusCode::CREATED,
                Json(fetch_file(&state.db, &user, file).await?),
            )
                .into_response(),
        },
    )
}

#[derive(Deserialize, Default)]
pub(crate) struct FileChanges {
    /// Nouveau nom ; l'extension ne change jamais.
    pub(crate) name: Option<String>,
    pub(crate) space: Option<String>,
    /// Dossier de l'espace ; `null` : sa racine.
    #[serde(default, deserialize_with = "some")]
    pub(crate) folder: Option<Option<i64>>,
    /// `true` : garde un fichier de Temporaire dans `space` (et `folder`).
    #[serde(default)]
    pub(crate) keep: bool,
    #[serde(default)]
    pub(crate) add_tags: Vec<String>,
    #[serde(default)]
    pub(crate) remove_tags: Vec<String>,
}

/// `PATCH /api/v1/files/{id}` `{"name"?, "space"?, "folder"?, "keep"?, "add_tags"?, "remove_tags"?}`
async fn update_file(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    payload: Result<Json<FileChanges>, JsonRejection>,
) -> ApiResult<Json<FileOut>> {
    let Json(changes) = payload?;
    Ok(Json(file_update(&state, &user, id, changes).await?))
}

pub(crate) async fn file_update(
    state: &AppState,
    user: &ApiUser,
    id: i64,
    changes: FileChanges,
) -> ApiResult<FileOut> {
    let db = &state.db;
    let current = fetch_file(db, user, id).await?;
    if changes.add_tags.len() + changes.remove_tags.len() > super::MAX_TAGS_PER_REQUEST {
        return Err(ApiError::bad_request(format!(
            "{} tags au plus par requête",
            super::MAX_TAGS_PER_REQUEST
        )));
    }
    // L'endroit visé est vérifié avant toute écriture.
    let target = match changes.space.as_deref() {
        Some(slug) => Some(user.space(slug)?.id),
        None => None,
    };
    if changes.keep {
        if !current.temporary {
            return Err(ApiError::bad_request("ce fichier est déjà rangé"));
        }
        let space = target.ok_or_else(|| ApiError::bad_request("keep demande space"))?;
        files::keep_in(state, user.id, id, space, changes.folder.flatten()).await?;
    } else if target.is_some() || changes.folder.is_some() {
        if current.temporary {
            return Err(ApiError::bad_request(
                "un fichier de Temporaire se range avec keep: true",
            ));
        }
        let space = match target {
            Some(space) => space,
            None => user.space(&current.space)?.id,
        };
        // Un autre espace sans dossier précisé : sa racine.
        let folder = match changes.folder {
            Some(folder) => folder,
            None if target.is_some() => None,
            None => current.folder,
        };
        files::move_into(db, user.id, id, space, folder).await?;
    }
    if let Some(name) = changes.name.as_deref()
        && files::rename_to(db, user.id, id, name).await?.is_none()
    {
        return Err(ApiError::bad_request("name ne peut pas être vide"));
    }
    for tag in &changes.add_tags {
        tags::attach(db, user.id, id, tag).await?;
    }
    for tag in &changes.remove_tags {
        tags::detach_named(db, user.id, id, tag).await?;
    }
    fetch_file(db, user, id).await
}

/// `DELETE /api/v1/files/{id}` : dans la corbeille (un fichier de
/// Temporaire, lui, est effacé, comme sur le site).
async fn delete_file(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    file_delete(&state, &user, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn file_delete(state: &AppState, user: &ApiUser, id: i64) -> ApiResult<()> {
    let temporary = sqlx::query_scalar!(
        r#"DELETE FROM files WHERE id = ? AND expires_at IS NOT NULL AND user_id = ?
           RETURNING storage_key AS "storage_key!""#,
        id,
        user.id
    )
    .fetch_optional(&state.db)
    .await?;
    match temporary {
        Some(key) => {
            if let Some(config) = &state.config.files {
                files::remove_stored(config, &key).await;
            }
        }
        None => trash::file(&state.db, user.who(), id).await?,
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Liens de partage
// ---------------------------------------------------------------------

#[derive(Serialize)]
pub(crate) struct LinkOut {
    id: i64,
    /// L'adresse entière, seulement à la création.
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    expires_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_at: Option<String>,
}

/// `GET /api/v1/files/{id}/shares` : les liens encore valables.
async fn list_links(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<LinkOut>>> {
    Ok(Json(links_list(&state.db, &user, id).await?))
}

pub(crate) async fn links_list(
    db: &SqlitePool,
    user: &ApiUser,
    id: i64,
) -> ApiResult<Vec<LinkOut>> {
    fetch_file(db, user, id).await?;
    let now = crate::time::now();
    Ok(sqlx::query!(
        r#"SELECT l.id AS "id!", l.expires_at, l.created_at FROM share_links l
           WHERE l.file_id = ? AND l.revoked_at IS NULL AND l.expires_at > ?
           ORDER BY l.created_at DESC, l.id DESC"#,
        id,
        now
    )
    .fetch_all(db)
    .await?
    .into_iter()
    .map(|l| LinkOut {
        id: l.id,
        url: None,
        expires_at: iso(l.expires_at),
        created_at: Some(iso(l.created_at)),
    })
    .collect())
}

#[derive(Deserialize)]
pub(crate) struct NewLink {
    /// `1h`, `1j` ou `7j`.
    pub(crate) duration: String,
}

/// `POST /api/v1/files/{id}/shares` `{"duration": "1j"}` : un lien public,
/// dont l'adresse entière n'est donnée qu'ici.
async fn create_link(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    payload: Result<Json<NewLink>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<LinkOut>)> {
    let Json(link) = payload?;
    Ok((
        StatusCode::CREATED,
        Json(link_create(&state, &user, id, &link.duration).await?),
    ))
}

pub(crate) async fn link_create(
    state: &AppState,
    user: &ApiUser,
    id: i64,
    duration: &str,
) -> ApiResult<LinkOut> {
    let created = share::create_link(state, user.id, id, duration).await?;
    tracing::info!(
        user = user.id,
        via = user.via,
        file = id,
        "lien de partage créé"
    );
    Ok(LinkOut {
        id: created.id,
        url: Some(created.url),
        expires_at: iso(created.expires_at),
        created_at: None,
    })
}

/// `DELETE /api/v1/files/{id}/shares/{link}`
async fn revoke_link(
    user: ApiUser,
    State(state): State<AppState>,
    Path((id, link)): Path<(i64, i64)>,
) -> ApiResult<StatusCode> {
    share::revoke_link(&state.db, user.id, id, link).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------
// Dossiers
// ---------------------------------------------------------------------

#[derive(Serialize)]
pub(crate) struct FolderOut {
    id: i64,
    name: String,
    space: String,
    /// Dossier qui le contient ; `null` à la racine de l'espace.
    parent: Option<i64>,
}

#[derive(Deserialize, Default)]
pub(crate) struct FolderListQuery {
    pub(crate) space: Option<String>,
}

/// `GET /api/v1/folders?space=ecole` : l'arborescence (chaque dossier avec
/// son parent ; cinq niveaux de dossiers au plus sous l'espace).
async fn list_folders(
    user: ApiUser,
    State(state): State<AppState>,
    query: Result<Query<FolderListQuery>, QueryRejection>,
) -> ApiResult<Json<Vec<FolderOut>>> {
    let Query(query) = query?;
    Ok(Json(folders_list(&state.db, &user, query).await?))
}

pub(crate) async fn folders_list(
    db: &SqlitePool,
    user: &ApiUser,
    query: FolderListQuery,
) -> ApiResult<Vec<FolderOut>> {
    let space = user.space_filter(query.space.as_deref())?;
    Ok(folders::all(db, user.id)
        .await?
        .into_iter()
        .filter(|f| space.is_none_or(|s| f.space_id == s))
        .map(|f| FolderOut {
            id: f.id,
            space: user.slug_of(f.space_id),
            name: f.name,
            parent: f.parent_id,
        })
        .collect())
}

async fn fetch_folder(db: &SqlitePool, user: &ApiUser, id: i64) -> ApiResult<FolderOut> {
    let folder = folders::find(db, user.id, id)
        .await?
        .ok_or_else(|| ApiError::not_found("dossier"))?;
    Ok(FolderOut {
        id: folder.id,
        space: user.slug_of(folder.space_id),
        name: folder.name,
        parent: folder.parent_id,
    })
}

#[derive(Deserialize)]
pub(crate) struct NewFolder {
    pub(crate) name: String,
    /// Espace où le créer, sans `parent`.
    pub(crate) space: Option<String>,
    /// Dossier où le créer (il donne l'espace) ; refusé au niveau le plus
    /// profond (cinq niveaux de dossiers au plus sous l'espace).
    pub(crate) parent: Option<i64>,
}

/// `POST /api/v1/folders` `{"name", "space"?, "parent"?}`
async fn create_folder(
    user: ApiUser,
    State(state): State<AppState>,
    payload: Result<Json<NewFolder>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<FolderOut>)> {
    let Json(folder) = payload?;
    Ok((
        StatusCode::CREATED,
        Json(folder_create(&state.db, &user, folder).await?),
    ))
}

pub(crate) async fn folder_create(
    db: &SqlitePool,
    user: &ApiUser,
    folder: NewFolder,
) -> ApiResult<FolderOut> {
    let space = match (folder.parent, folder.space.as_deref()) {
        (Some(parent), _) => {
            folders::find(db, user.id, parent)
                .await?
                .ok_or_else(|| ApiError::not_found("dossier parent"))?
                .space_id
        }
        (None, Some(slug)) => user.space(slug)?.id,
        (None, None) => return Err(ApiError::bad_request("space ou parent est obligatoire")),
    };
    let (id, _) =
        refused(folders::create_in(db, user.id, space, folder.parent, &folder.name).await?)?;
    fetch_folder(db, user, id).await
}

#[derive(Deserialize, Default)]
pub(crate) struct FolderChanges {
    pub(crate) name: Option<String>,
    /// Espace où l'emmener, avec son contenu.
    pub(crate) space: Option<String>,
    /// Dossier où le mettre ; `null` : la racine de l'espace. Le dossier et
    /// ses sous-dossiers doivent y tenir (cinq niveaux de dossiers au plus).
    #[serde(default, deserialize_with = "some")]
    pub(crate) parent: Option<Option<i64>>,
}

/// `PATCH /api/v1/folders/{id}` `{"name"?, "space"?, "parent"?}` : renomme,
/// ou déplace avec tout son contenu.
async fn update_folder(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    payload: Result<Json<FolderChanges>, JsonRejection>,
) -> ApiResult<Json<FolderOut>> {
    let Json(changes) = payload?;
    Ok(Json(folder_update(&state.db, &user, id, changes).await?))
}

pub(crate) async fn folder_update(
    db: &SqlitePool,
    user: &ApiUser,
    id: i64,
    changes: FolderChanges,
) -> ApiResult<FolderOut> {
    let folder = folders::find(db, user.id, id)
        .await?
        .ok_or_else(|| ApiError::not_found("dossier"))?;
    let target = match changes.space.as_deref() {
        Some(slug) => Some(user.space(slug)?.clone()),
        None => None,
    };
    if target.is_some() || changes.parent.is_some() {
        let space = match target {
            Some(space) => space,
            None => user
                .spaces
                .iter()
                .find(|s| s.id == folder.space_id)
                .cloned()
                .ok_or_else(|| ApiError::not_found("dossier"))?,
        };
        let parent = changes.parent.flatten();
        if let folders::Moved::Refused(message) =
            folders::move_to(db, user.id, id, &space, parent).await?
        {
            return Err(ApiError::bad_request(message));
        }
    }
    if let Some(name) = changes.name.as_deref() {
        let folder = folders::find(db, user.id, id)
            .await?
            .ok_or_else(|| ApiError::not_found("dossier"))?;
        refused(folders::rename_to(db, user.id, &folder, name).await?)?;
    }
    fetch_folder(db, user, id).await
}

/// `DELETE /api/v1/folders/{id}` : dans la corbeille, avec tout son contenu.
async fn delete_folder(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    folder_delete(&state.db, &user, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn folder_delete(db: &SqlitePool, user: &ApiUser, id: i64) -> ApiResult<()> {
    let folder = folders::find(db, user.id, id)
        .await?
        .ok_or_else(|| ApiError::not_found("dossier"))?;
    trash::folder(db, user.who(), &folder).await?;
    Ok(())
}

// ---------------------------------------------------------------------
// Corbeille
// ---------------------------------------------------------------------

#[derive(Serialize)]
pub(crate) struct TrashOut {
    /// `note`, `fichier`, `dossier` ou `espace`.
    kind: &'static str,
    id: i64,
    name: String,
    /// Espace d'origine.
    origin: String,
    /// Qui l'a supprimé.
    deleted_by: String,
    /// Par où : « le site », ou le nom du jeton.
    via: String,
    deleted_at: String,
    /// Ce qu'un dossier a emporté.
    #[serde(skip_serializing_if = "String::is_empty")]
    contents: String,
}

/// `GET /api/v1/trash` : ce que le compte voit dans la corbeille.
async fn list_trash(
    user: ApiUser,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<TrashOut>>> {
    Ok(Json(trash_list(&state.db, &user).await?))
}

pub(crate) async fn trash_list(db: &SqlitePool, user: &ApiUser) -> ApiResult<Vec<TrashOut>> {
    Ok(trash::page::entries(db, user.id)
        .await?
        .into_iter()
        .map(|e| TrashOut {
            kind: e.kind.key(),
            id: e.id,
            name: e.name,
            origin: e.origin,
            deleted_by: e.who,
            via: e.via,
            deleted_at: iso(e.deleted_at),
            contents: e.contents,
        })
        .collect())
}

/// `POST /api/v1/trash/{kind}/{id}/restore` : remet l'élément à sa place.
async fn restore(
    user: ApiUser,
    State(state): State<AppState>,
    Path((kind, id)): Path<(String, i64)>,
) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(trash_restore(&state.db, &user, &kind, id).await?))
}

pub(crate) async fn trash_restore(
    db: &SqlitePool,
    user: &ApiUser,
    kind: &str,
    id: i64,
) -> ApiResult<serde_json::Value> {
    let kind = Kind::from_key(kind)
        .ok_or_else(|| ApiError::bad_request("kind : note, fichier, dossier ou espace"))?;
    let name = trash::restore(db, user.id, kind, id).await?.name;
    tracing::info!(
        user = user.id,
        via = user.via,
        kind = kind.key(),
        id,
        "restauré"
    );
    Ok(json!({ "restored": name, "kind": kind.key(), "id": id }))
}
