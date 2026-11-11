//! Fichiers (v2) : envoi, liste, téléchargement et suppression, par espace.
//!
//! * Un fichier envoyé est une donnée, jamais une page : il est servi en
//!   téléchargement, avec une CSP `sandbox` (voir [`download`]).
//! * Sur le disque, il ne porte jamais le nom choisi par l'utilisateur, mais
//!   une clé aléatoire : `<FILES_DIR>/<2 premiers caractères>/<clé>`. Un nom
//!   piégé (`../../etc/passwd`) ne peut donc désigner aucun autre fichier.
//! * L'envoi arrive en continu et va directement sur le disque : la mémoire
//!   de dashu ne dépend pas de la taille du fichier.
//! * Toutes les requêtes filtrent sur `user_id`, comme les notes.

use std::fmt::Write as _;
use std::path::{Path as FsPath, PathBuf};

use anyhow::Context as _;
use askama::Template;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
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
// Envoi
// ---------------------------------------------------------------------

#[derive(Deserialize)]
pub struct UploadQuery {
    name: String,
}

/// `POST /files?name=…` : le corps de la requête est le contenu du fichier,
/// tel quel (pas de formulaire multipart). C'est `static/js/files.js` qui
/// l'envoie, un fichier par requête, avec le jeton CSRF dans l'en-tête.
/// Le fichier va dans l'espace actif.
pub async fn upload(
    ctx: Ctx,
    State(state): State<AppState>,
    Query(query): Query<UploadQuery>,
    headers: HeaderMap,
    body: Body,
) -> AppResult<Response> {
    let files = state.config.files.as_ref().ok_or(AppError::NotFound)?;
    let max = files.max_upload_bytes;
    let too_large = || {
        AppError::TooLarge(format!(
            "Fichier trop volumineux : {} au plus.",
            human_size(i64::try_from(max).unwrap_or(i64::MAX))
        ))
    };

    // Taille annoncée par le navigateur : refus immédiat, sans rien écrire.
    let announced = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if announced.is_some_and(|len| len > max) {
        return Err(too_large());
    }

    let name = clean_name(&query.name);
    let incoming = files.dir.join(INCOMING);
    tokio::fs::create_dir_all(&incoming)
        .await
        .with_context(|| format!("dossier des envois {}", incoming.display()))?;
    let part = incoming.join(format!("{}.part", web::csrf::random_token()));

    // Un envoi refusé ou interrompu ne laisse rien sur le disque.
    let (size, sha256) = match receive(body, &part, max).await {
        Ok(received) => received,
        Err(error) => {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(match error {
                Receive::TooLarge => too_large(),
                Receive::Interrupted => AppError::BadRequest("Envoi interrompu.".into()),
                Receive::Disk(error) => {
                    AppError::Internal(anyhow::Error::new(error).context("écriture d'un envoi"))
                }
            });
        }
    };

    let key = web::csrf::random_token();
    let dest = stored_path(&files.dir, &key);
    let placed = async {
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::rename(&part, &dest).await
    }
    .await;
    if let Err(error) = placed {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(anyhow::Error::new(error)
            .context("mise en place d'un fichier reçu")
            .into());
    }

    let mime = mime_for(&name);
    let size = i64::try_from(size).unwrap_or(i64::MAX);
    let now = time::now();
    let inserted = sqlx::query!(
        "INSERT INTO files (user_id, space_id, name, mime, size, sha256, storage_key, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        ctx.user.id,
        ctx.active.id,
        name,
        mime,
        size,
        sha256,
        key,
        now,
        now
    )
    .execute(&state.db)
    .await;
    if let Err(error) = inserted {
        let _ = tokio::fs::remove_file(&dest).await;
        return Err(error.into());
    }
    Ok((StatusCode::CREATED, "Fichier reçu").into_response())
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

/// Écrit le corps de la requête dans `path`, morceau par morceau, en
/// comptant les octets. Renvoie la taille et l'empreinte SHA-256.
async fn receive(mut body: Body, path: &FsPath, max: u64) -> Result<(u64, String), Receive> {
    let mut out = tokio::fs::File::create(path).await?;
    let mut hasher = Sha256::new();
    let mut size: u64 = 0;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| Receive::Interrupted)?;
        let Ok(data) = frame.into_data() else {
            continue;
        };
        size += data.len() as u64;
        if size > max {
            return Err(Receive::TooLarge);
        }
        hasher.update(&data);
        out.write_all(&data).await?;
    }
    out.flush().await?;
    out.sync_all().await?;
    Ok((size, to_hex(&hasher.finalize())))
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

/// Au démarrage : crée le dossier des envois et efface les envois restés en
/// plan (coupure pendant un envoi). Un dossier inutilisable n'empêche pas
/// dashu de démarrer : seuls les fichiers sont touchés, et l'erreur est
/// signalée dans les logs.
pub fn prepare_storage(config: &FilesConfig) {
    let incoming = config.dir.join(INCOMING);
    if let Err(error) = std::fs::create_dir_all(&incoming) {
        tracing::warn!(%error, dir = %config.dir.display(), "dossier des fichiers inutilisable");
        return;
    }
    if let Ok(entries) = std::fs::read_dir(&incoming) {
        for entry in entries.flatten() {
            let _ = std::fs::remove_file(entry.path());
        }
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
