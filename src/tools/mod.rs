//! Outils (v2) : pour l'instant, la compression d'un fichier (vidéo, photo,
//! PDF) en une copie plus légère, l'original étant gardé.
//!
//! dashu ne lance aucune conversion lui-même : il est limité à 256 Mo et n'a
//! aucun droit. Le travail passe par un dossier partagé `WORK_DIR/jobs/<id>/` :
//!   1. dashu y copie le fichier (`input.<ext>`) et écrit `job.json` en
//!      dernier : le dossier n'est visible qu'une fois prêt ;
//!   2. le service `dashu-worker` ([`worker`]), sans réseau et sans accès à
//!      la base ni aux fichiers rangés, produit `output.<ext>`, écrit
//!      `progress` au fil de l'eau, puis `done` ou `error` ;
//!   3. dashu relève le résultat toutes les 2 secondes ([`spawn_collector`]),
//!      le range parmi les fichiers de l'espace et efface le dossier.

pub mod worker;

use std::path::{Path as FsPath, PathBuf};

use anyhow::Context as _;
use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::HeaderValue;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::auth::session::Ctx;
use crate::config::FilesConfig;
use crate::files::{self, human_size};
use crate::time;
use crate::web::{self, AppError, AppResult, see_other};

/// Une tâche sans résultat au bout de ce délai est déclarée en échec : le
/// service de conversion ne tourne sans doute pas.
const STALE_JOB_SECONDS: i64 = 24 * 3600;
/// Les tâches terminées restent affichées ce temps-là.
const RECENT_JOB_SECONDS: i64 = 10 * 60;

/// Ce que le service de conversion doit faire : le contenu de `job.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSpec {
    pub media: Media,
    /// Nom du fichier d'entrée dans le dossier de la tâche.
    pub input: String,
    /// Nom du fichier à produire.
    pub output: String,
}

/// Famille de fichier, qui décide de la façon de compresser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Media {
    /// MP4 H.264, 1080p au plus, qualité CRF 23, son AAC 128 kb/s.
    Video,
    /// JPEG, 2560 px au plus sur le grand côté, qualité 82.
    Image,
    /// Photo d'iPhone : décodée par libheif, puis comme `Image`.
    Heic,
    /// Ghostscript, réglage « ebook » (images à 150 dpi).
    Pdf,
}

impl Media {
    /// Compression possible pour ce type MIME, ou `None`.
    pub fn for_mime(mime: &str) -> Option<Self> {
        match mime {
            "application/pdf" => Some(Self::Pdf),
            "image/heic" | "image/heif" => Some(Self::Heic),
            // Pas le GIF : il perdrait son animation.
            "image/jpeg" | "image/png" | "image/webp" => Some(Self::Image),
            m if m.starts_with("video/") => Some(Self::Video),
            _ => None,
        }
    }

    pub fn output_ext(self) -> &'static str {
        match self {
            Self::Video => "mp4",
            Self::Image | Self::Heic => "jpg",
            Self::Pdf => "pdf",
        }
    }
}

/// Nom de la copie compressée : « IMG_1234.MOV » -> « IMG_1234 (compressé).mp4 ».
pub fn compressed_name(source: &str, media: Media) -> String {
    let stem = match source.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => source,
    };
    files::clean_name(&format!("{stem} (compressé).{}", media.output_ext()))
}

/// Dossier de travail d'une tâche.
pub fn job_dir(work_dir: &FsPath, id: i64) -> PathBuf {
    work_dir.join("jobs").join(id.to_string())
}

// ---------------------------------------------------------------------
// Lancer une compression
// ---------------------------------------------------------------------

/// `POST /files/{id}/compress` : lance la compression d'un fichier. Le
/// résultat ira dans le même espace que l'original.
pub async fn compress(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let (Some(files), Some(work_dir)) = (&state.config.files, &state.config.work_dir) else {
        return Err(AppError::NotFound);
    };
    let file = sqlx::query!(
        "SELECT space_id, name, mime, size, storage_key FROM files WHERE id = ? AND user_id = ?",
        id,
        ctx.user.id
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;
    let Some(media) = Media::for_mime(&file.mime) else {
        return Err(AppError::BadRequest(
            "Ce type de fichier ne se compresse pas.".into(),
        ));
    };

    let now = time::now();
    let job_id = sqlx::query!(
        "INSERT INTO jobs (user_id, space_id, kind, source_name, source_size, created_at, updated_at)
         VALUES (?, ?, 'compress', ?, ?, ?, ?)",
        ctx.user.id,
        file.space_id,
        file.name,
        file.size,
        now,
        now
    )
    .execute(&state.db)
    .await?
    .last_insert_rowid();

    let source = files::stored_path(&files.dir, &file.storage_key);
    if let Err(error) = prepare_job(work_dir, job_id, &source, &file.name, media).await {
        let _ = tokio::fs::remove_dir_all(job_dir(work_dir, job_id)).await;
        sqlx::query!("DELETE FROM jobs WHERE id = ?", job_id)
            .execute(&state.db)
            .await?;
        return Err(error.into());
    }
    ctx.flash_ok("Compression lancée : la copie apparaîtra dans la liste")
        .await?;
    Ok(see_other("/files"))
}

/// Prépare le dossier d'une tâche : copie du fichier, puis `job.json`.
async fn prepare_job(
    work_dir: &FsPath,
    job_id: i64,
    source: &FsPath,
    name: &str,
    media: Media,
) -> anyhow::Result<()> {
    let dir = job_dir(work_dir, job_id);
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("dossier de travail {}", dir.display()))?;
    share_with_worker(&dir)?;

    // Une copie, pas un lien : le service de conversion n'a aucun droit sur
    // les fichiers rangés.
    let input = format!("input.{}", safe_ext(name));
    tokio::fs::copy(source, dir.join(&input))
        .await
        .context("copie du fichier à compresser")?;

    let spec = JobSpec {
        media,
        input,
        output: format!("output.{}", media.output_ext()),
    };
    let tmp = dir.join(".job.json");
    tokio::fs::write(&tmp, serde_json::to_vec_pretty(&spec)?).await?;
    tokio::fs::rename(&tmp, dir.join("job.json")).await?;
    Ok(())
}

/// Droits `2770` : le groupe du dossier (`dashu-jobs` en production) peut y
/// écrire, et ce qui s'y crée hérite de ce groupe.
fn share_with_worker(dir: &FsPath) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o2770))?;
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Extension du fichier d'entrée, réduite à des lettres et chiffres : elle
/// aide les outils à reconnaître le format, sans jamais former un chemin.
fn safe_ext(name: &str) -> String {
    let ext: String = name
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(8)
        .collect::<String>()
        .to_ascii_lowercase();
    if ext.is_empty() { "bin".into() } else { ext }
}

// ---------------------------------------------------------------------
// Relever les résultats
// ---------------------------------------------------------------------

/// Relève les tâches en cours toutes les 2 secondes.
pub fn spawn_collector(state: AppState) {
    if state.config.work_dir.is_none() {
        return;
    }
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(2));
        loop {
            tick.tick().await;
            if let Err(error) = collect(&state).await {
                tracing::warn!(error = format!("{error:#}"), "relevé des conversions");
            }
        }
    });
}

/// Un passage du relevé : progression, résultats, échecs.
pub async fn collect(state: &AppState) -> anyhow::Result<()> {
    let (Some(files), Some(work_dir)) = (&state.config.files, &state.config.work_dir) else {
        return Ok(());
    };
    let jobs = sqlx::query!(
        r#"SELECT id AS "id!", user_id, space_id, source_name, source_size, status, progress, created_at
           FROM jobs WHERE status IN ('queued', 'running')"#
    )
    .fetch_all(&state.db)
    .await?;

    for job in jobs {
        let dir = job_dir(work_dir, job.id);
        let pending = Pending {
            id: job.id,
            user_id: job.user_id,
            space_id: job.space_id,
            source_name: job.source_name,
            source_size: job.source_size,
        };
        if let Some(message) = read_small(&dir.join("error")).await {
            fail(state, job.id, &message).await?;
            let _ = tokio::fs::remove_dir_all(&dir).await;
        } else if dir.join("done").exists() {
            if let Err(error) = import(state, files, &dir, &pending).await {
                tracing::warn!(
                    error = format!("{error:#}"),
                    job = job.id,
                    "import d'une conversion"
                );
                fail(state, job.id, "Le résultat n'a pas pu être rangé.").await?;
            }
            let _ = tokio::fs::remove_dir_all(&dir).await;
        } else if !dir.exists() {
            fail(state, job.id, "Dossier de travail introuvable.").await?;
        } else if time::now() - job.created_at > STALE_JOB_SECONDS {
            fail(state, job.id, "Le service de conversion n'a pas répondu.").await?;
            let _ = tokio::fs::remove_dir_all(&dir).await;
        } else if dir.join("started").exists() {
            let progress = read_small(&dir.join("progress"))
                .await
                .and_then(|p| p.parse::<i64>().ok())
                .unwrap_or(0)
                .clamp(0, 99);
            if job.status != "running" || progress != job.progress {
                let now = time::now();
                sqlx::query!(
                    "UPDATE jobs SET status = 'running', progress = ?, updated_at = ? WHERE id = ?",
                    progress,
                    now,
                    job.id
                )
                .execute(&state.db)
                .await?;
            }
        }
    }
    Ok(())
}

struct Pending {
    id: i64,
    user_id: i64,
    space_id: i64,
    source_name: String,
    source_size: i64,
}

/// Range le résultat d'une tâche réussie parmi les fichiers de l'espace.
async fn import(
    state: &AppState,
    files: &FilesConfig,
    dir: &FsPath,
    job: &Pending,
) -> anyhow::Result<()> {
    let spec: JobSpec = serde_json::from_slice(&tokio::fs::read(dir.join("job.json")).await?)?;
    let output = dir.join(&spec.output);
    let size = tokio::fs::metadata(&output).await?.len() as i64;
    if size >= job.source_size {
        fail(
            state,
            job.id,
            "Pas de gain : ce fichier est déjà bien compressé.",
        )
        .await?;
        return Ok(());
    }

    let sha256 = files::hash_file(output.clone()).await?;
    let key = web::csrf::random_token();
    let dest = files::stored_path(&files.dir, &key);
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::rename(&output, &dest).await?;

    let name = compressed_name(&job.source_name, spec.media);
    let mime = files::mime_for(&name);
    let now = time::now();
    let saved: anyhow::Result<()> = async {
        let mut tx = state.db.begin().await?;
        let file_id = sqlx::query!(
            "INSERT INTO files (user_id, space_id, name, mime, size, sha256, storage_key, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            job.user_id,
            job.space_id,
            name,
            mime,
            size,
            sha256,
            key,
            now,
            now
        )
        .execute(&mut *tx)
        .await?
        .last_insert_rowid();
        sqlx::query!(
            "UPDATE jobs SET status = 'done', progress = 100, result_file_id = ?, updated_at = ? WHERE id = ?",
            file_id,
            now,
            job.id
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
    .await;
    if saved.is_err() {
        let _ = tokio::fs::remove_file(&dest).await;
    }
    saved
}

async fn fail(state: &AppState, id: i64, message: &str) -> anyhow::Result<()> {
    let now = time::now();
    sqlx::query!(
        "UPDATE jobs SET status = 'failed', error = ?, updated_at = ? WHERE id = ?",
        message,
        now,
        id
    )
    .execute(&state.db)
    .await?;
    Ok(())
}

/// Contenu d'un petit fichier texte (500 caractères au plus), ou `None`.
async fn read_small(path: &FsPath) -> Option<String> {
    let text = tokio::fs::read_to_string(path).await.ok()?;
    Some(text.trim().chars().take(500).collect())
}

// ---------------------------------------------------------------------
// Suivi dans la page Fichiers
// ---------------------------------------------------------------------

pub struct JobItem {
    pub name: String,
    /// `queued`, `running`, `done` ou `failed`.
    pub state: String,
    /// Texte affiché à droite : « 47 % », « 120 Mo → 18 Mo »…
    pub status: String,
    /// Barre de progression : `Some(n)` pour une valeur, `None` sans barre.
    pub progress: Option<i64>,
    /// Barre sans valeur (en attente, ou photo et PDF en cours).
    pub busy: bool,
}

/// Tâches affichées au-dessus de la liste : celles en cours, et celles
/// terminées depuis moins de 10 minutes.
#[derive(Default)]
pub struct JobsPanel {
    pub items: Vec<JobItem>,
    /// Nombre de tâches en cours : tant qu'il y en a, la page se rafraîchit.
    pub active: usize,
}

pub async fn panel(state: &AppState, user_id: i64, space_id: i64) -> AppResult<JobsPanel> {
    if state.config.work_dir.is_none() {
        return Ok(JobsPanel::default());
    }
    let since = time::now() - RECENT_JOB_SECONDS;
    let rows = sqlx::query!(
        r#"SELECT j.source_name, j.source_size, j.status, j.progress, j.error,
                  f.size AS "result_size?: i64"
           FROM jobs j
           LEFT JOIN files f ON f.id = j.result_file_id
           WHERE j.user_id = ? AND j.space_id = ?
             AND (j.status IN ('queued', 'running') OR j.updated_at > ?)
           ORDER BY j.id DESC
           LIMIT 20"#,
        user_id,
        space_id,
        since
    )
    .fetch_all(&state.db)
    .await?;

    let mut panel = JobsPanel::default();
    for row in rows {
        let (status, progress, busy) = match row.status.as_str() {
            "queued" => ("En attente".to_string(), None, true),
            "running" if row.progress > 0 => {
                (format!("{} %", row.progress), Some(row.progress), false)
            }
            "running" => ("En cours".to_string(), None, true),
            "done" => (
                format!(
                    "{} → {}",
                    human_size(row.source_size),
                    human_size(row.result_size.unwrap_or(0))
                ),
                None,
                false,
            ),
            _ => (
                row.error
                    .unwrap_or_else(|| "Échec de la compression.".into()),
                None,
                false,
            ),
        };
        if matches!(row.status.as_str(), "queued" | "running") {
            panel.active += 1;
        }
        panel.items.push(JobItem {
            name: row.source_name,
            state: row.status,
            status,
            progress,
            busy,
        });
    }
    Ok(panel)
}

#[derive(Template)]
#[template(path = "files/_jobs.html")]
struct JobsFragment {
    jobs: JobsPanel,
}

#[derive(Deserialize)]
pub struct JobsQuery {
    /// Tâches en cours au passage précédent.
    #[serde(default)]
    active: usize,
}

/// `GET /files/jobs?active=…` : le suivi, redemandé toutes les 2 secondes
/// par la page tant qu'une tâche tourne. Quand une tâche s'est terminée
/// depuis, l'en-tête `HX-Trigger` fait recharger la liste des fichiers.
pub async fn jobs_fragment(
    ctx: Ctx,
    State(state): State<AppState>,
    Query(query): Query<JobsQuery>,
) -> AppResult<Response> {
    let jobs = panel(&state, ctx.user.id, ctx.active.id).await?;
    let finished = jobs.active < query.active;
    let mut response = web::html(JobsFragment { jobs });
    if finished {
        response
            .headers_mut()
            .insert("hx-trigger", HeaderValue::from_static("files-changed"));
    }
    Ok(response.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn familles_de_fichiers() {
        assert_eq!(Media::for_mime("video/quicktime"), Some(Media::Video));
        assert_eq!(Media::for_mime("image/heic"), Some(Media::Heic));
        assert_eq!(Media::for_mime("image/png"), Some(Media::Image));
        assert_eq!(Media::for_mime("application/pdf"), Some(Media::Pdf));
        assert_eq!(Media::for_mime("image/gif"), None);
        assert_eq!(Media::for_mime("application/zip"), None);
    }

    #[test]
    fn noms_des_copies() {
        assert_eq!(
            compressed_name("IMG_1234.MOV", Media::Video),
            "IMG_1234 (compressé).mp4"
        );
        assert_eq!(
            compressed_name("photo.heic", Media::Heic),
            "photo (compressé).jpg"
        );
        assert_eq!(
            compressed_name("sans-extension", Media::Pdf),
            "sans-extension (compressé).pdf"
        );
        assert_eq!(
            compressed_name(".mov", Media::Video),
            ".mov (compressé).mp4"
        );
    }

    #[test]
    fn extensions_sures() {
        assert_eq!(safe_ext("Vacances.MOV"), "mov");
        assert_eq!(safe_ext("a.b/../../x"), "x");
        assert_eq!(safe_ext("rien"), "bin");
    }
}
