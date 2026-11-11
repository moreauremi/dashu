//! Fichiers (v2) : envoi, liste, téléchargement et suppression, par espace.
//!
//! * Un fichier envoyé est une donnée, jamais une page : il est servi en
//!   téléchargement, avec une CSP `sandbox` (voir [`download`]).
//! * Sur le disque, il ne porte jamais le nom choisi par l'utilisateur, mais
//!   une clé aléatoire : `<FILES_DIR>/<2 premiers caractères>/<clé>`. Un nom
//!   piégé (`../../etc/passwd`) ne peut donc désigner aucun autre fichier.
//! * L'envoi arrive par morceaux de 50 Mo au plus (limite de Cloudflare),
//!   chacun écrit sur le disque au fil de l'eau : la mémoire de dashu ne
//!   dépend pas de la taille du fichier.
//! * Toutes les requêtes filtrent sur `user_id`, comme les notes.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use askama::Template;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tokio::io::AsyncWriteExt;

use crate::AppState;
use crate::auth::session::{Ctx, Shell};
use crate::config::FilesConfig;
use crate::time;
use crate::web::{self, AppError, AppResult, see_other};

/// Dossier des envois en cours, à l'intérieur de `FILES_DIR` : même disque,
/// donc un simple renommage met le fichier à sa place une fois reçu.
const INCOMING: &str = ".envois";
const NAME_MAX_CHARS: usize = 200;
/// CSP d'un fichier servi : rien ne s'exécute, même si le navigateur
/// décidait d'afficher le fichier au lieu de le télécharger.
const FILE_CSP: &str = "default-src 'none'; sandbox";

// ---------------------------------------------------------------------
// Liste
// ---------------------------------------------------------------------

pub struct FileItem {
    pub id: i64,
    pub name: String,
    pub size: String,
    /// Libellé court du type : « PDF », « Image »…
    pub kind: &'static str,
    pub updated: String,
}

#[derive(Template)]
#[template(path = "files/list.html")]
struct ListPage {
    shell: Shell,
    /// Faux si `FILES_DIR` n'est pas configuré.
    enabled: bool,
    /// Taille maximale d'un fichier, lisible (« 2 Go ») et en octets (pour
    /// refuser tout de suite, dans le navigateur, un fichier trop gros).
    max_size: String,
    max_bytes: u64,
    items: Vec<FileItem>,
}

/// `GET /files` : les fichiers de l'espace actif, les plus récents d'abord.
pub async fn list(ctx: Ctx, State(state): State<AppState>) -> AppResult<Response> {
    let now = time::now();
    let items = sqlx::query!(
        r#"SELECT id AS "id!", name, mime, size, updated_at
           FROM files
           WHERE user_id = ? AND space_id = ?
           ORDER BY updated_at DESC, id DESC"#,
        ctx.user.id,
        ctx.active.id
    )
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|row| FileItem {
        id: row.id,
        size: human_size(row.size),
        kind: kind_label(&row.mime),
        name: row.name,
        updated: time::ago(row.updated_at, now),
    })
    .collect();

    let files = state.config.files.as_ref();
    let page = ListPage {
        shell: ctx.shell("files").await?,
        enabled: files.is_some(),
        max_size: files
            .map(|f| human_size(i64::try_from(f.max_upload_bytes).unwrap_or(i64::MAX)))
            .unwrap_or_default(),
        max_bytes: files.map(|f| f.max_upload_bytes).unwrap_or_default(),
        items,
    };
    Ok(web::html(page))
}

// ---------------------------------------------------------------------
// Envoi, par morceaux
// ---------------------------------------------------------------------
//
// Un fichier part en morceaux de 50 Mo au plus (static/js/files.js) :
// Cloudflare refuse toute requête de plus de 100 Mo, et un morceau raté se
// renvoie seul. Le déroulé :
//   1. `POST /files/uploads?name=…&size=…` ouvre un envoi et renvoie son
//      identifiant ; la taille totale est vérifiée tout de suite ;
//   2. `PUT /files/uploads/{id}?offset=…` ajoute un morceau, dans l'ordre ;
//   3. au dernier morceau, le fichier rejoint la liste (réponse 201).
// Un envoi en cours survit à un redémarrage de dashu ; abandonné, il est
// effacé au bout de 24 heures (`cleanup_stale`).

/// Taille maximale d'un morceau, côté serveur. Le navigateur envoie des
/// morceaux de 50 Mo, sous la limite de Cloudflare (100 Mo par requête).
pub const CHUNK_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Un envoi sans nouvelles depuis plus longtemps est abandonné.
const STALE_UPLOAD_SECONDS: i64 = 24 * 3600;
/// En-tête qui donne au navigateur le nombre d'octets déjà reçus.
pub const RECEIVED_HEADER: &str = "x-upload-received";

/// Envois dont un morceau est en train d'arriver : un seul à la fois par
/// envoi, sinon deux morceaux pourraient s'écrire au même endroit.
pub type Busy = Arc<Mutex<HashSet<String>>>;

struct BusyGuard {
    set: Busy,
    id: String,
}

impl BusyGuard {
    fn take(set: &Busy, id: &str) -> Option<Self> {
        let mut busy = set.lock().unwrap_or_else(|e| e.into_inner());
        busy.insert(id.to_string()).then(|| Self {
            set: set.clone(),
            id: id.to_string(),
        })
    }
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.set
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}

fn too_large(max: u64) -> AppError {
    AppError::TooLarge(format!(
        "Fichier trop volumineux : {} au plus.",
        human_size(i64::try_from(max).unwrap_or(i64::MAX))
    ))
}

#[derive(Deserialize)]
pub struct StartQuery {
    name: String,
    size: u64,
}

/// `POST /files/uploads?name=…&size=…` : ouvre un envoi dans l'espace actif
/// et renvoie son identifiant.
pub async fn start_upload(
    ctx: Ctx,
    State(state): State<AppState>,
    Query(query): Query<StartQuery>,
) -> AppResult<Response> {
    let files = state.config.files.as_ref().ok_or(AppError::NotFound)?;
    if query.size > files.max_upload_bytes {
        return Err(too_large(files.max_upload_bytes));
    }
    let size = i64::try_from(query.size).map_err(|_| too_large(files.max_upload_bytes))?;
    let name = clean_name(&query.name);

    let id = web::csrf::random_token();
    let part = part_path(&files.dir, &id);
    if let Some(parent) = part.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("dossier des envois {}", parent.display()))?;
    }
    tokio::fs::File::create(&part)
        .await
        .with_context(|| format!("création de {}", part.display()))?;
    let now = time::now();
    sqlx::query!(
        "INSERT INTO uploads (id, user_id, space_id, name, size, received, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, 0, ?, ?)",
        id,
        ctx.user.id,
        ctx.active.id,
        name,
        size,
        now,
        now
    )
    .execute(&state.db)
    .await?;
    Ok((StatusCode::CREATED, id).into_response())
}

#[derive(Deserialize)]
pub struct ChunkQuery {
    offset: u64,
}

/// `PUT /files/uploads/{id}?offset=…` : ajoute un morceau. `offset` doit être
/// le nombre d'octets déjà reçus ; sinon la réponse 409 donne le bon dans
/// l'en-tête `x-upload-received`, et le navigateur reprend de là.
pub async fn upload_chunk(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ChunkQuery>,
    body: Body,
) -> AppResult<Response> {
    let files = state.config.files.as_ref().ok_or(AppError::NotFound)?;
    if !is_token(&id) {
        return Err(AppError::NotFound);
    }
    let Some(_busy) = BusyGuard::take(&state.uploads_busy, &id) else {
        return Err(AppError::BadRequest(
            "Un morceau de ce fichier est déjà en cours d'envoi.".into(),
        ));
    };
    let upload = sqlx::query!(
        "SELECT space_id, name, size, received FROM uploads WHERE id = ? AND user_id = ?",
        id,
        ctx.user.id
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;
    let (size, received) = (upload.size as u64, upload.received as u64);
    if query.offset != received {
        return Ok(out_of_order(received));
    }

    let part = part_path(&files.dir, &id);
    let Ok(mut out) = tokio::fs::OpenOptions::new().append(true).open(&part).await else {
        // Fichier partiel disparu : l'envoi ne peut pas reprendre.
        sqlx::query!("DELETE FROM uploads WHERE id = ?", id)
            .execute(&state.db)
            .await?;
        return Err(AppError::BadRequest("Envoi perdu : recommence-le.".into()));
    };
    // Coupure entre l'écriture d'un morceau et l'enregistrement en base : on
    // revient à ce que la base connaît.
    if out.metadata().await?.len() != received {
        out.set_len(received).await?;
    }

    let room = (size - received).min(CHUNK_MAX_BYTES);
    let written = match append(body, &mut out, room).await {
        Ok(written) => written,
        Err(error) => {
            let _ = out.set_len(received).await;
            return Err(match error {
                Receive::TooLarge if room < CHUNK_MAX_BYTES => {
                    AppError::TooLarge("Le fichier est plus gros qu'annoncé.".into())
                }
                Receive::TooLarge => AppError::TooLarge("Morceau trop gros.".into()),
                Receive::Interrupted => AppError::BadRequest("Envoi interrompu.".into()),
                Receive::Disk(error) => {
                    AppError::Internal(anyhow::Error::new(error).context("écriture d'un envoi"))
                }
            });
        }
    };
    out.sync_data().await?;
    drop(out);

    let received = received + written;
    if received < size {
        let (received_db, now) = (received as i64, time::now());
        sqlx::query!(
            "UPDATE uploads SET received = ?, updated_at = ? WHERE id = ?",
            received_db,
            now,
            id
        )
        .execute(&state.db)
        .await?;
        return Ok((
            StatusCode::OK,
            [(RECEIVED_HEADER, received.to_string())],
            "Morceau reçu",
        )
            .into_response());
    }

    finish(
        &state,
        files,
        &id,
        upload.space_id,
        &upload.name,
        ctx.user.id,
    )
    .await?;
    Ok((StatusCode::CREATED, "Fichier reçu").into_response())
}

/// Dernier morceau reçu : le fichier partiel prend sa place définitive et
/// rejoint la liste.
async fn finish(
    state: &AppState,
    files: &FilesConfig,
    id: &str,
    space_id: i64,
    name: &str,
    user_id: i64,
) -> AppResult<()> {
    let part = part_path(&files.dir, id);
    let sha256 = hash_file(part.clone())
        .await
        .context("empreinte d'un fichier reçu")?;
    let size = tokio::fs::metadata(&part).await?.len() as i64;

    let key = web::csrf::random_token();
    let dest = stored_path(&files.dir, &key);
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::rename(&part, &dest)
        .await
        .context("mise en place d'un fichier reçu")?;

    let mime = mime_for(name);
    let now = time::now();
    let saved = async {
        let mut tx = state.db.begin().await?;
        sqlx::query!(
            "INSERT INTO files (user_id, space_id, name, mime, size, sha256, storage_key, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            user_id,
            space_id,
            name,
            mime,
            size,
            sha256,
            key,
            now,
            now
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!("DELETE FROM uploads WHERE id = ?", id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    }
    .await;
    if let Err(error) = saved {
        let _ = tokio::fs::remove_file(&dest).await;
        return Err(error.into());
    }
    Ok(())
}

/// Réponse 409 : le morceau n'est pas celui attendu. Le corps est du HTML
/// (comme un fragment htmx) pour que l'en-tête arrive tel quel au navigateur.
fn out_of_order(received: u64) -> Response {
    (
        StatusCode::CONFLICT,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
            (
                header::HeaderName::from_static(RECEIVED_HEADER),
                received.to_string(),
            ),
        ],
        r#"<p class="frag-error" role="alert">Morceau inattendu : reprise de l'envoi.</p>"#,
    )
        .into_response()
}

enum Receive {
    TooLarge,
    Interrupted,
    Disk(std::io::Error),
}

impl From<std::io::Error> for Receive {
    fn from(error: std::io::Error) -> Self {
        Self::Disk(error)
    }
}

/// Ajoute le corps de la requête à `out`, au fil de l'eau, `room` octets au
/// plus. Renvoie le nombre d'octets écrits.
async fn append(mut body: Body, out: &mut tokio::fs::File, room: u64) -> Result<u64, Receive> {
    let mut written: u64 = 0;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| Receive::Interrupted)?;
        let Ok(data) = frame.into_data() else {
            continue;
        };
        written += data.len() as u64;
        if written > room {
            return Err(Receive::TooLarge);
        }
        out.write_all(&data).await?;
    }
    out.flush().await?;
    Ok(written)
}

/// Empreinte SHA-256 d'un fichier, lu par blocs de 1 Mo hors du fil
/// asynchrone (un fichier de 2 Go prend quelques secondes).
async fn hash_file(path: PathBuf) -> std::io::Result<String> {
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let mut file = std::fs::File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 1 << 20];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        Ok(to_hex(&hasher.finalize()))
    })
    .await
    .map_err(std::io::Error::other)?
}

/// Efface les envois abandonnés (24 heures sans nouvelles) et les fichiers
/// partiels qu'aucun envoi ne référence plus. Ceux de moins d'une heure sont
/// laissés : un envoi peut être en train de s'ouvrir.
pub async fn cleanup_stale(db: &SqlitePool, config: &FilesConfig) -> anyhow::Result<()> {
    let limit = time::now() - STALE_UPLOAD_SECONDS;
    sqlx::query!("DELETE FROM uploads WHERE updated_at < ?", limit)
        .execute(db)
        .await?;
    let live: HashSet<String> = sqlx::query_scalar!(r#"SELECT id AS "id!" FROM uploads"#)
        .fetch_all(db)
        .await?
        .into_iter()
        .collect();

    let Ok(mut entries) = tokio::fs::read_dir(config.dir.join(INCOMING)).await else {
        return Ok(());
    };
    let recent = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let young = entry
            .metadata()
            .await
            .and_then(|m| m.modified())
            .is_ok_and(|modified| modified > recent);
        if !live.contains(id) && !young {
            let _ = tokio::fs::remove_file(&path).await;
        }
    }
    Ok(())
}

/// Nettoyage des envois abandonnés : au démarrage, puis toutes les heures.
pub fn spawn_cleanup(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
        loop {
            tick.tick().await;
            if let Some(files) = &state.config.files
                && let Err(error) = cleanup_stale(&state.db, files).await
            {
                tracing::warn!(error = format!("{error:#}"), "nettoyage des envois");
            }
        }
    });
}

/// Fichier partiel d'un envoi : `<dossier>/.envois/<id>.part`.
fn part_path(dir: &FsPath, id: &str) -> PathBuf {
    dir.join(INCOMING).join(format!("{id}.part"))
}

/// Vrai pour un identifiant tel que dashu les fabrique : 64 caractères
/// hexadécimaux. Rien d'autre ne peut donc atteindre un chemin.
fn is_token(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

// ---------------------------------------------------------------------
// Téléchargement et suppression
// ---------------------------------------------------------------------

/// `GET /files/{id}/download`
pub async fn download(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let files = state.config.files.as_ref().ok_or(AppError::NotFound)?;
    let row = sqlx::query!(
        "SELECT name, size, storage_key FROM files WHERE id = ? AND user_id = ?",
        id,
        ctx.user.id
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;

    let path = stored_path(&files.dir, &row.storage_key);
    let file = tokio::fs::File::open(&path)
        .await
        .with_context(|| format!("fichier #{id} absent du disque ({})", path.display()))?;

    let mut response = Body::from_stream(tokio_util::io::ReaderStream::new(file)).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(row.size));
    headers.insert(header::CONTENT_DISPOSITION, attachment(&row.name));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(FILE_CSP),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    Ok(response)
}

/// `POST /files/{id}/delete`
pub async fn delete(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let key = sqlx::query_scalar!(
        r#"DELETE FROM files WHERE id = ? AND user_id = ? RETURNING storage_key AS "storage_key!""#,
        id,
        ctx.user.id
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;

    if let Some(files) = &state.config.files {
        let path = stored_path(&files.dir, &key);
        if let Err(error) = tokio::fs::remove_file(&path).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%error, path = %path.display(), "suppression d'un fichier du disque");
        }
    }
    ctx.flash_ok("Fichier supprimé").await?;
    Ok(see_other("/files"))
}

// ---------------------------------------------------------------------
// Stockage
// ---------------------------------------------------------------------

/// Au démarrage : crée le dossier des envois. Un dossier inutilisable
/// n'empêche pas dashu de démarrer : seuls les fichiers sont touchés, et
/// l'erreur est signalée dans les logs. Les envois restés en plan sont
/// effacés plus tard, par `cleanup_stale` : un envoi en cours peut reprendre
/// après un redémarrage (une mise à jour, par exemple).
pub fn prepare_storage(config: &FilesConfig) {
    if let Err(error) = std::fs::create_dir_all(config.dir.join(INCOMING)) {
        tracing::warn!(%error, dir = %config.dir.display(), "dossier des fichiers inutilisable");
    }
}

/// Emplacement du contenu d'un fichier : `<dossier>/<ab>/<abcdef…>`.
pub fn stored_path(dir: &FsPath, key: &str) -> PathBuf {
    dir.join(&key[..2]).join(key)
}

// ---------------------------------------------------------------------
// Noms, types, tailles
// ---------------------------------------------------------------------

/// Nom affiché du fichier. Le navigateur n'envoie que le nom, mais on ne s'y
/// fie pas : on garde ce qui suit le dernier séparateur, sans caractère de
/// contrôle, et 200 caractères au plus.
pub fn clean_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control())
        .take(NAME_MAX_CHARS)
        .collect();
    let cleaned = cleaned.trim();
    match cleaned {
        "" | "." | ".." => "fichier".into(),
        _ => cleaned.into(),
    }
}

/// Type MIME d'après l'extension. Celui annoncé par le navigateur n'est pas
/// utilisé : il ne prouve rien.
pub fn mime_for(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "svg" => "image/svg+xml",
        "txt" | "log" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "zip" => "application/zip",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "wav" => "audio/wav",
        "ogg" | "opus" => "audio/ogg",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        _ => "application/octet-stream",
    }
}

/// Libellé court affiché dans la liste.
pub fn kind_label(mime: &str) -> &'static str {
    match mime {
        "application/pdf" => "PDF",
        "application/zip" => "Archive",
        m if m.starts_with("image/") => "Image",
        m if m.starts_with("video/") => "Vidéo",
        m if m.starts_with("audio/") => "Audio",
        m if m.starts_with("text/") => "Texte",
        m if m.contains("wordprocessingml")
            || m.contains("opendocument")
            || m == "application/msword" =>
        {
            "Document"
        }
        m if m.contains("spreadsheetml") => "Tableur",
        m if m.contains("presentationml") => "Présentation",
        _ => "Fichier",
    }
}

/// Taille lisible, en français : « 512 o », « 1,5 Mo », « 34 Ko ».
pub fn human_size(bytes: i64) -> String {
    const UNITS: [&str; 4] = ["Ko", "Mo", "Go", "To"];
    if bytes < 1024 {
        return format!("{} o", bytes.max(0));
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let text = if value < 10.0 {
        format!("{value:.1}").replace('.', ",")
    } else {
        format!("{value:.0}")
    };
    let text = text.strip_suffix(",0").unwrap_or(&text).to_string();
    format!("{text} {}", UNITS[unit])
}

/// En-tête `Content-Disposition` d'un téléchargement : un nom ASCII de
/// secours, puis le vrai nom encodé (RFC 6266), accents compris.
fn attachment(name: &str) -> HeaderValue {
    let fallback: String = name
        .chars()
        .map(|c| {
            if c == ' ' || (c.is_ascii_graphic() && c != '"' && c != '\\') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut encoded = String::with_capacity(name.len() * 3);
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&byte) {
            encoded.push(byte as char);
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    HeaderValue::from_str(&format!(
        "attachment; filename=\"{fallback}\"; filename*=UTF-8''{encoded}"
    ))
    .unwrap_or_else(|_| HeaderValue::from_static("attachment"))
}

fn to_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noms_nettoyes() {
        assert_eq!(clean_name("rapport.pdf"), "rapport.pdf");
        assert_eq!(clean_name("../../etc/passwd"), "passwd");
        assert_eq!(clean_name("C:\\Users\\moi\\photo.jpg"), "photo.jpg");
        assert_eq!(clean_name("a\u{0}b\nc.txt"), "abc.txt");
        assert_eq!(clean_name(".."), "fichier");
        assert_eq!(clean_name("   "), "fichier");
        assert_eq!(clean_name(&"é".repeat(300)).chars().count(), 200);
    }

    #[test]
    fn types_d_apres_l_extension() {
        assert_eq!(mime_for("Photo.JPG"), "image/jpeg");
        assert_eq!(mime_for("cours.pdf"), "application/pdf");
        assert_eq!(mime_for("page.html"), "text/html");
        assert_eq!(mime_for("sans-extension"), "application/octet-stream");
        assert_eq!(kind_label("image/png"), "Image");
        assert_eq!(kind_label(mime_for("cv.docx")), "Document");
    }

    #[test]
    fn tailles_en_francais() {
        assert_eq!(human_size(0), "0 o");
        assert_eq!(human_size(512), "512 o");
        assert_eq!(human_size(1024), "1 Ko");
        assert_eq!(human_size(1536), "1,5 Ko");
        assert_eq!(human_size(34 * 1024), "34 Ko");
        assert_eq!(human_size(2048 * 1024 * 1024), "2 Go");
    }

    #[test]
    fn nom_de_telechargement_encode() {
        let value = attachment("Résumé \"final\".pdf");
        assert_eq!(
            value.to_str().unwrap(),
            "attachment; filename=\"R_sum_ _final_.pdf\"; filename*=UTF-8''R%C3%A9sum%C3%A9%20%22final%22.pdf"
        );
    }

    #[test]
    fn chemin_de_stockage() {
        let key = "abcdef0123";
        assert_eq!(
            stored_path(FsPath::new("/srv/f"), key),
            PathBuf::from("/srv/f/ab/abcdef0123")
        );
    }
}
