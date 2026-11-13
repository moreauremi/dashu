//! Outils (v2) : compression d'un fichier en une copie plus légère, et
//! conversions du menu « Convertir » (photo en JPEG, PNG ou WebP, vidéo en
//! MP4, son en MP3, pages d'un PDF). L'original est toujours gardé. L'onglet
//! Outils reçoit en plus des fichiers pas encore rangés (voir [`page`]).
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
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::auth::session::{Ctx, Shell};
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
    #[serde(default)]
    pub task: Task,
    pub media: Media,
    /// Nom du fichier d'entrée dans le dossier de la tâche.
    pub input: String,
    /// Nom du fichier à produire.
    pub output: String,
    /// Vignette : grand aperçu à produire en plus (photo HEIC, PDF, vidéo).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// Conversion : la sortie choisie dans le menu « Convertir ».
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Conversion>,
    /// PDF : première et dernière page à garder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<(u32, u32)>,
}

/// Sorte de tâche.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Task {
    /// Copie plus légère du fichier (bouton « Compresser »).
    #[default]
    Compress,
    /// Vignette de la liste et grand aperçu, faits à l'arrivée du fichier.
    Thumbnail,
    /// Conversion choisie dans le menu « Convertir » (voir [`Conversion`]).
    Convert,
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
    /// Son : lu par ffmpeg (conversion en MP3 seulement).
    Audio,
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

    /// Façon de lire un fichier à convertir, ou `None`.
    pub fn for_input(mime: &str) -> Option<Self> {
        match mime {
            "image/gif" => Some(Self::Image),
            m if m.starts_with("audio/") => Some(Self::Audio),
            _ => Self::for_mime(mime),
        }
    }

    /// Vignette possible pour ce type MIME (le GIF compris : sa première image).
    pub fn for_thumbnail(mime: &str) -> Option<Self> {
        match mime {
            "image/gif" => Some(Self::Image),
            _ => Self::for_mime(mime),
        }
    }

    /// Un grand aperçu est fait en plus de la vignette : les navigateurs
    /// n'affichent pas le HEIC ni (dans dashu) le PDF, et une vidéo a besoin
    /// d'une image avant sa lecture.
    pub fn wants_preview(self) -> bool {
        matches!(self, Self::Heic | Self::Pdf | Self::Video)
    }

    pub fn output_ext(self) -> &'static str {
        match self {
            Self::Video => "mp4",
            Self::Image | Self::Heic => "jpg",
            Self::Pdf => "pdf",
            Self::Audio => "mp3",
        }
    }
}

/// Ce que propose le menu « Convertir » d'un fichier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Conversion {
    /// Copie plus légère (réglages par famille, voir [`Media`]).
    Compress,
    Jpg,
    Png,
    /// WebP sans perte : l'encodeur de la crate `image` ne fait que ça.
    Webp,
    /// Vidéo en MP4 (H.264 et AAC), dans sa définition d'origine.
    Mp4,
    /// Le son d'une vidéo ou d'un fichier audio, en MP3.
    Mp3,
    /// Une plage de pages d'un PDF, dans un nouveau PDF.
    Pages,
}

impl Conversion {
    pub const ALL: [Self; 7] = [
        Self::Compress,
        Self::Jpg,
        Self::Png,
        Self::Webp,
        Self::Mp4,
        Self::Mp3,
        Self::Pages,
    ];

    /// Valeur envoyée par le formulaire (et rangée dans `jobs.target`).
    pub fn key(self) -> &'static str {
        match self {
            Self::Compress => "compress",
            Self::Jpg => "jpg",
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Mp4 => "mp4",
            Self::Mp3 => "mp3",
            Self::Pages => "pages",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.key() == key)
    }

    /// Libellé du menu.
    pub fn label(self) -> &'static str {
        match self {
            Self::Compress => "Compresser",
            Self::Jpg => "En JPEG",
            Self::Png => "En PNG",
            Self::Webp => "En WebP",
            Self::Mp4 => "En MP4",
            Self::Mp3 => "Le son en MP3",
            Self::Pages => "Extraire des pages",
        }
    }

    /// Extension du fichier produit (la compression garde celle de sa famille).
    fn ext(self, media: Media) -> &'static str {
        match self {
            Self::Compress => media.output_ext(),
            Self::Jpg => "jpg",
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Mp4 => "mp4",
            Self::Mp3 => "mp3",
            Self::Pages => "pdf",
        }
    }
}

/// Conversions possibles pour un type MIME, dans l'ordre du menu. Seules
/// celles des outils installés : pandoc et Typst (Markdown vers PDF…)
/// viendront plus tard.
pub fn conversions_for(mime: &str) -> Vec<Conversion> {
    use Conversion::*;
    let mut out = Vec::new();
    if Media::for_mime(mime).is_some() {
        out.push(Compress);
    }
    match mime {
        "image/jpeg" => out.extend([Png, Webp]),
        "image/png" => out.extend([Jpg, Webp]),
        "image/webp" => out.extend([Jpg, Png]),
        "image/gif" | "image/heic" | "image/heif" => out.extend([Jpg, Png, Webp]),
        "video/mp4" => out.push(Mp3),
        m if m.starts_with("video/") => out.extend([Mp4, Mp3]),
        "audio/mpeg" => {}
        m if m.starts_with("audio/") => out.push(Mp3),
        "application/pdf" => out.push(Pages),
        _ => {}
    }
    out
}

/// Plage de pages tapée dans le menu : « 2-4 », « 2 – 4 » ou « 3 ». Deux
/// nombres de 1 à 9999 dans l'ordre, rien d'autre : la valeur ne part jamais
/// telle quelle vers Ghostscript.
pub fn parse_pages(text: &str) -> Option<(u32, u32)> {
    let page = |s: &str| {
        s.trim()
            .parse::<u32>()
            .ok()
            .filter(|n| (1..=9999).contains(n))
    };
    let text = text.trim();
    let (first, last) = match text.split_once(['-', '–']) {
        Some((a, b)) => (page(a)?, page(b)?),
        None => (page(text)?, page(text)?),
    };
    (first <= last).then_some((first, last))
}

/// Partie du nom avant l'extension.
fn stem(source: &str) -> &str {
    match source.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => source,
    }
}

/// Nom du fichier produit : « photo.heic » -> « photo.jpg », « cours.pdf »
/// -> « cours (pages 2-4).pdf », compression -> « … (compressé).… ».
pub fn result_name(source: &str, spec: &JobSpec) -> String {
    let stem = stem(source);
    match (spec.to, spec.pages) {
        (Some(Conversion::Pages), Some((a, b))) if a == b => {
            files::clean_name(&format!("{stem} (page {a}).pdf"))
        }
        (Some(Conversion::Pages), Some((a, b))) => {
            files::clean_name(&format!("{stem} (pages {a}-{b}).pdf"))
        }
        (Some(to), _) if to != Conversion::Compress => {
            files::clean_name(&format!("{stem}.{}", to.ext(spec.media)))
        }
        _ => compressed_name(source, spec.media),
    }
}

/// Nom de la copie compressée : « IMG_1234.MOV » -> « IMG_1234 (compressé).mp4 ».
pub fn compressed_name(source: &str, media: Media) -> String {
    let stem = stem(source);
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
    start(&ctx, &state, id, Conversion::Compress, None).await?;
    ctx.flash_ok("Compression lancée : la copie apparaîtra dans la liste")
        .await?;
    Ok(see_other("/files"))
}

#[derive(Deserialize)]
pub struct ConvertForm {
    pub(crate) to: String,
    /// PDF : « 2-4 », ou « 3 ».
    pub(crate) pages: Option<String>,
    /// Page où revenir : la liste, l'aperçu du fichier, l'onglet Outils.
    pub(crate) next: Option<String>,
}

const IMPOSSIBLE: &str = "Cette conversion n'est pas possible pour ce fichier.";

/// `POST /files/{id}/convert` : lance une conversion du menu « Convertir ».
/// Le résultat ira dans le même espace que l'original.
pub async fn convert(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<ConvertForm>,
) -> AppResult<Response> {
    let to =
        Conversion::from_key(&form.to).ok_or_else(|| AppError::BadRequest(IMPOSSIBLE.into()))?;
    let pages = match to {
        Conversion::Pages => Some(
            parse_pages(form.pages.as_deref().unwrap_or_default()).ok_or_else(|| {
                AppError::BadRequest("Pages à extraire : par exemple 2-4.".into())
            })?,
        ),
        _ => None,
    };
    start(&ctx, &state, id, to, pages).await?;
    ctx.flash_ok(match to {
        Conversion::Compress => "Compression lancée : la copie apparaîtra dans la liste",
        _ => "Conversion lancée : le résultat apparaîtra dans la liste",
    })
    .await?;
    Ok(see_other(match form.next.as_deref() {
        Some(next) => web::local_path(Some(next)),
        None => "/files",
    }))
}

/// Prépare la tâche d'une compression ou d'une conversion, pour un fichier de
/// l'utilisateur et une sortie que son type permet.
async fn start(
    ctx: &Ctx,
    state: &AppState,
    id: i64,
    to: Conversion,
    pages: Option<(u32, u32)>,
) -> AppResult<()> {
    queue(state, ctx.user.id, id, to, pages).await
}

/// Règles automatiques de l'espace d'un fichier qui vient d'arriver : vidéo
/// convertie en MP4, photo HEIC doublée d'un JPEG.
pub async fn apply_rules(state: &AppState, user_id: i64, file_id: i64) -> AppResult<()> {
    if state.config.work_dir.is_none() {
        return Ok(());
    }
    let Some(row) = sqlx::query!(
        r#"SELECT f.mime, s.auto_mp4 AS "auto_mp4!: bool", s.auto_heic_jpeg AS "auto_heic_jpeg!: bool"
           FROM files f JOIN spaces s ON s.id = f.space_id
           WHERE f.id = ? AND f.user_id = ? AND f.expires_at IS NULL"#,
        file_id,
        user_id
    )
    .fetch_optional(&state.db)
    .await?
    else {
        return Ok(());
    };
    let wanted = conversions_for(&row.mime);
    if row.auto_mp4 && wanted.contains(&Conversion::Mp4) {
        queue(state, user_id, file_id, Conversion::Mp4, None).await?;
    }
    if row.auto_heic_jpeg
        && matches!(row.mime.as_str(), "image/heic" | "image/heif")
        && wanted.contains(&Conversion::Jpg)
    {
        queue(state, user_id, file_id, Conversion::Jpg, None).await?;
    }
    Ok(())
}

/// Met en file la conversion `to` d'un fichier de `user_id`.
async fn queue(
    state: &AppState,
    user_id: i64,
    id: i64,
    to: Conversion,
    pages: Option<(u32, u32)>,
) -> AppResult<()> {
    let (Some(files), Some(work_dir)) = (&state.config.files, &state.config.work_dir) else {
        return Err(AppError::NotFound);
    };
    let file = sqlx::query!(
        "SELECT space_id, name, mime, size, storage_key FROM files WHERE id = ? AND user_id = ?",
        id,
        user_id
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;
    let refused = || {
        AppError::BadRequest(match to {
            Conversion::Compress => "Ce type de fichier ne se compresse pas.".into(),
            _ => IMPOSSIBLE.into(),
        })
    };
    if !conversions_for(&file.mime).contains(&to) {
        return Err(refused());
    }
    let media = match to {
        Conversion::Compress => Media::for_mime(&file.mime),
        _ => Media::for_input(&file.mime),
    }
    .ok_or_else(refused)?;
    let (kind, task) = match to {
        Conversion::Compress => ("compress", Task::Compress),
        _ => ("convert", Task::Convert),
    };
    let target = to.key();

    let now = time::now();
    let job_id = sqlx::query!(
        "INSERT INTO jobs (user_id, space_id, kind, target, source_name, source_size, file_id, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        user_id,
        file.space_id,
        kind,
        target,
        file.name,
        file.size,
        id,
        now,
        now
    )
    .execute(&state.db)
    .await?
    .last_insert_rowid();

    let source = files::stored_path(&files.dir, &file.storage_key);
    let spec = JobSpec {
        task,
        media,
        input: format!("input.{}", safe_ext(&file.name)),
        output: format!("output.{}", to.ext(media)),
        preview: None,
        to: (to != Conversion::Compress).then_some(to),
        pages,
    };
    if let Err(error) = prepare_job(work_dir, job_id, &source, &spec).await {
        let _ = tokio::fs::remove_dir_all(job_dir(work_dir, job_id)).await;
        sqlx::query!("DELETE FROM jobs WHERE id = ?", job_id)
            .execute(&state.db)
            .await?;
        return Err(error.into());
    }
    Ok(())
}

/// Prépare le dossier d'une tâche : copie du fichier, puis `job.json`.
async fn prepare_job(
    work_dir: &FsPath,
    job_id: i64,
    source: &FsPath,
    spec: &JobSpec,
) -> anyhow::Result<()> {
    let dir = job_dir(work_dir, job_id);
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("dossier de travail {}", dir.display()))?;

    // Une copie, pas un lien : le service de conversion n'a aucun droit sur
    // les fichiers rangés.
    let input = dir.join(&spec.input);
    tokio::fs::copy(source, &input)
        .await
        .context("copie du fichier à traiter")?;

    let tmp = dir.join(".job.json");
    tokio::fs::write(&tmp, serde_json::to_vec_pretty(spec)?).await?;
    share_with_worker(&dir, &[&input, &tmp]).context("droits du dossier de travail")?;
    // En dernier : le service ne prend la tâche qu'une fois `job.json` là.
    tokio::fs::rename(&tmp, dir.join("job.json")).await?;
    Ok(())
}

/// Demande la vignette d'un fichier (à son arrivée). Sans outils, ou pour un
/// type sans vignette, le fichier est marqué « pas de vignette » (-1) et la
/// liste affiche une icône.
pub async fn enqueue_thumbnail(state: &AppState, file_id: i64) -> anyhow::Result<()> {
    let (Some(files), Some(work_dir)) = (&state.config.files, &state.config.work_dir) else {
        return Ok(());
    };
    let Some(file) = sqlx::query!(
        "SELECT user_id, space_id, name, mime, size, storage_key FROM files WHERE id = ?",
        file_id
    )
    .fetch_optional(&state.db)
    .await?
    else {
        return Ok(());
    };
    let Some(media) = Media::for_thumbnail(&file.mime) else {
        sqlx::query!("UPDATE files SET thumb = -1 WHERE id = ?", file_id)
            .execute(&state.db)
            .await?;
        return Ok(());
    };

    let now = time::now();
    let job_id = sqlx::query!(
        "INSERT INTO jobs (user_id, space_id, kind, source_name, source_size, file_id, created_at, updated_at)
         VALUES (?, ?, 'thumbnail', ?, ?, ?, ?, ?)",
        file.user_id,
        file.space_id,
        file.name,
        file.size,
        file_id,
        now,
        now
    )
    .execute(&state.db)
    .await?
    .last_insert_rowid();
    let spec = JobSpec {
        task: Task::Thumbnail,
        media,
        input: format!("input.{}", safe_ext(&file.name)),
        output: "thumb.jpg".into(),
        preview: media.wants_preview().then(|| "preview.jpg".into()),
        to: None,
        pages: None,
    };
    let source = files::stored_path(&files.dir, &file.storage_key);
    if let Err(error) = prepare_job(work_dir, job_id, &source, &spec).await {
        let _ = tokio::fs::remove_dir_all(job_dir(work_dir, job_id)).await;
        sqlx::query!("DELETE FROM jobs WHERE id = ?", job_id)
            .execute(&state.db)
            .await?;
        return Err(error);
    }
    Ok(())
}

/// Fichiers arrivés avant les vignettes, ou pendant que les outils étaient
/// éteints : on leur en demande une (au démarrage, 200 à la fois au plus).
async fn backfill_thumbnails(state: &AppState) -> anyhow::Result<()> {
    let ids = sqlx::query_scalar!(
        r#"SELECT f.id AS "id!" FROM files f
           WHERE f.thumb = 0
             AND NOT EXISTS (SELECT 1 FROM jobs j WHERE j.file_id = f.id AND j.kind = 'thumbnail')
           LIMIT 200"#
    )
    .fetch_all(&state.db)
    .await?;
    for id in ids {
        enqueue_thumbnail(state, id).await?;
    }
    Ok(())
}

/// Efface les dossiers de travail qu'aucune tâche en cours ne référence plus
/// (fichier supprimé pendant sa vignette, par exemple), au bout d'une heure.
async fn remove_orphan_dirs(state: &AppState, work_dir: &FsPath) -> anyhow::Result<()> {
    let live: std::collections::HashSet<i64> = sqlx::query_scalar!(
        r#"SELECT id AS "id!" FROM jobs WHERE status IN ('queued', 'running')"#
    )
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .collect();
    let Ok(mut entries) = tokio::fs::read_dir(work_dir.join("jobs")).await else {
        return Ok(());
    };
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    while let Some(entry) = entries.next_entry().await? {
        let Some(id) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<i64>().ok())
        else {
            continue;
        };
        let aged = entry
            .metadata()
            .await
            .and_then(|m| m.modified())
            .is_ok_and(|modified| modified < old);
        if !live.contains(&id) && aged {
            let _ = tokio::fs::remove_dir_all(entry.path()).await;
        }
    }
    Ok(())
}

/// Ouvre le dossier d'une tâche au service de conversion (groupe
/// `dashu-jobs` en production) : `770` pour le dossier, où il écrit, `640`
/// pour les fichiers déposés par dashu, qu'il lit.
///
/// Le groupe vient du dossier `jobs/`, en `2770` (bit setgid) : ce qui s'y
/// crée en hérite. Le dossier de la tâche hérite aussi du bit, mais le `chmod`
/// ci-dessous l'efface : c'est pourquoi les fichiers sont créés avant. Ne
/// jamais demander `2770` ici : dashu tourne avec `RestrictSUIDSGID=yes`
/// (`deploy/lxc/hardening.conf`), qui refuse (EPERM) tout `chmod` posant le
/// bit setgid. Le service de conversion, lui, a `dashu-jobs` pour groupe
/// principal : ce qu'il crée prend ce groupe sans le bit.
fn share_with_worker(dir: &FsPath, files: &[&FsPath]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for file in files {
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o640))?;
        }
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o770))?;
    }
    #[cfg(not(unix))]
    let _ = (dir, files);
    Ok(())
}

/// Range un fichier produit par le service de conversion.
///
/// En production, le dossier de travail et celui des fichiers sont deux
/// points de montage distincts (systemd en crée un par `ReadWritePaths=`) :
/// `rename` y échoue (EXDEV), même sur le même disque. On copie alors, puis
/// on efface l'original.
async fn move_file(from: &FsPath, to: &FsPath) -> std::io::Result<()> {
    let Err(error) = tokio::fs::rename(from, to).await else {
        return Ok(());
    };
    if error.kind() == std::io::ErrorKind::NotFound {
        return Err(error);
    }
    if let Err(error) = tokio::fs::copy(from, to).await {
        let _ = tokio::fs::remove_file(to).await;
        return Err(error);
    }
    let _ = tokio::fs::remove_file(from).await;
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
        if let Err(error) = backfill_thumbnails(&state).await {
            tracing::warn!(error = format!("{error:#}"), "rattrapage des vignettes");
        }
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(2));
        let mut ticks: u32 = 0;
        loop {
            tick.tick().await;
            if let Err(error) = collect(&state).await {
                tracing::warn!(error = format!("{error:#}"), "relevé des conversions");
            }
            // Toutes les heures : ménage des dossiers orphelins.
            ticks = ticks.wrapping_add(1);
            if ticks % 1800 == 1
                && let Some(work_dir) = &state.config.work_dir
                && let Err(error) = remove_orphan_dirs(&state, work_dir).await
            {
                tracing::warn!(error = format!("{error:#}"), "ménage du dossier de travail");
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
        r#"SELECT id AS "id!", user_id, space_id, kind, file_id, source_name, source_size, status, progress, created_at
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
            file_id: job.file_id,
        };
        let thumbnail = job.kind == "thumbnail";
        if let Some(message) = read_small(&dir.join("error")).await {
            fail(state, job.id, &message).await?;
            if thumbnail && let Some(file_id) = job.file_id {
                sqlx::query!("UPDATE files SET thumb = -1 WHERE id = ?", file_id)
                    .execute(&state.db)
                    .await?;
            }
            let _ = tokio::fs::remove_dir_all(&dir).await;
        } else if thumbnail && dir.join("done").exists() {
            if let Some(file_id) = job.file_id
                && let Err(error) = import_thumbnail(state, files, &dir, job.id, file_id).await
            {
                tracing::warn!(
                    error = format!("{error:#}"),
                    job = job.id,
                    "import d'une vignette"
                );
                fail(state, job.id, "La vignette n'a pas pu être rangée.").await?;
            }
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
    /// Fichier de départ (compression, conversion), s'il existe encore.
    file_id: Option<i64>,
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
    if spec.task == Task::Compress && size >= job.source_size {
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
    move_file(&output, &dest)
        .await
        .context("rangement de la copie compressée")?;

    let name = result_name(&job.source_name, &spec);
    let mime = files::mime_for(&name);
    let now = time::now();
    // Le résultat d'un fichier de l'onglet Outils reste dans l'onglet Outils,
    // avec la même échéance ; celui d'un fichier rangé va dans son dossier
    // (s'il est toujours dans l'espace de la conversion).
    let source = match job.file_id {
        Some(source) => {
            sqlx::query!(
                "SELECT expires_at, space_id, folder_id FROM files WHERE id = ?",
                source
            )
            .fetch_optional(&state.db)
            .await?
        }
        None => None,
    };
    let expires_at = source.as_ref().and_then(|s| s.expires_at);
    let folder_id = source
        .as_ref()
        .filter(|s| s.space_id == job.space_id)
        .and_then(|s| s.folder_id);
    let saved: anyhow::Result<i64> = async {
        let mut tx = state.db.begin().await?;
        let file_id = sqlx::query!(
            "INSERT INTO files (user_id, space_id, folder_id, name, mime, size, sha256, storage_key, expires_at, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            job.user_id,
            job.space_id,
            folder_id,
            name,
            mime,
            size,
            sha256,
            key,
            expires_at,
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
        Ok(file_id)
    }
    .await;
    match saved {
        Ok(file_id) => {
            if let Err(error) = enqueue_thumbnail(state, file_id).await {
                tracing::warn!(
                    error = format!("{error:#}"),
                    "vignette d'une copie compressée"
                );
            }
            Ok(())
        }
        Err(error) => {
            let _ = tokio::fs::remove_file(&dest).await;
            Err(error)
        }
    }
}

/// Range la vignette (et le grand aperçu) d'un fichier, à côté de lui.
async fn import_thumbnail(
    state: &AppState,
    files: &FilesConfig,
    dir: &FsPath,
    job_id: i64,
    file_id: i64,
) -> anyhow::Result<()> {
    let Some(key) = sqlx::query_scalar!("SELECT storage_key FROM files WHERE id = ?", file_id)
        .fetch_optional(&state.db)
        .await?
    else {
        return Ok(());
    };
    let stored = files::stored_path(&files.dir, &key);
    move_file(&dir.join("thumb.jpg"), &files::thumb_path(&stored))
        .await
        .context("rangement de la vignette")?;
    let preview = move_file(&dir.join("preview.jpg"), &files::preview_path(&stored))
        .await
        .is_ok();
    let preview = i64::from(preview);
    let now = time::now();
    sqlx::query!(
        "UPDATE files SET thumb = 1, preview = ? WHERE id = ?",
        preview,
        file_id
    )
    .execute(&state.db)
    .await?;
    sqlx::query!(
        "UPDATE jobs SET status = 'done', progress = 100, updated_at = ? WHERE id = ?",
        now,
        job_id
    )
    .execute(&state.db)
    .await?;
    Ok(())
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
    /// « Compression », « En JPEG »…
    pub what: &'static str,
    /// `queued`, `running`, `done` ou `failed`.
    pub state: String,
    /// Texte affiché à droite : « 47 % », « 120 Mo → 18 Mo »…
    pub status: String,
    /// Barre de progression : `Some(n)` pour une valeur, `None` sans barre.
    pub progress: Option<i64>,
    /// Barre sans valeur (en attente, ou photo et PDF en cours).
    pub busy: bool,
    /// Résultat rangé : lien vers son aperçu.
    pub result_id: Option<i64>,
    pub job_id: i64,
    /// Terminée, résultat et original encore là : « Remplacer l'original ».
    pub can_replace: bool,
}

/// Tâches affichées au-dessus de la liste : celles en cours, et celles
/// terminées depuis moins de 10 minutes.
#[derive(Default)]
pub struct JobsPanel {
    pub items: Vec<JobItem>,
    /// Nombre de tâches en cours : tant qu'il y en a, la page se rafraîchit.
    pub active: usize,
    /// Tous les espaces (onglet Outils) plutôt que l'espace actif.
    pub all: bool,
}

/// Libellé d'une tâche d'après sa sorte et sa cible.
fn job_label(kind: &str, target: Option<&str>) -> &'static str {
    match (kind, target.and_then(Conversion::from_key)) {
        ("convert", Some(to)) => to.label(),
        _ => "Compression",
    }
}

/// Tâches de l'espace `space`, ou de tous les espaces si `None`.
pub async fn panel(state: &AppState, user_id: i64, space: Option<i64>) -> AppResult<JobsPanel> {
    if state.config.work_dir.is_none() {
        return Ok(JobsPanel::default());
    }
    let since = time::now() - RECENT_JOB_SECONDS;
    let rows = sqlx::query!(
        r#"SELECT j.id AS "id!", j.kind, j.target, j.source_name, j.source_size, j.status, j.progress, j.error,
                  j.result_file_id, j.file_id, f.size AS "result_size?: i64"
           FROM jobs j
           LEFT JOIN files f ON f.id = j.result_file_id
           WHERE j.user_id = ?1 AND (?2 IS NULL AND j.space_id NOT IN (SELECT id FROM spaces WHERE archived_at IS NOT NULL) OR j.space_id = ?2)
             AND j.kind IN ('compress', 'convert')
             AND (j.status IN ('queued', 'running') OR j.updated_at > ?3)
           ORDER BY j.id DESC
           LIMIT 20"#,
        user_id,
        space,
        since
    )
    .fetch_all(&state.db)
    .await?;

    let mut panel = JobsPanel {
        all: space.is_none(),
        ..JobsPanel::default()
    };
    for row in rows {
        let (status, progress, busy) = match row.status.as_str() {
            "queued" => ("En attente".to_string(), None, true),
            "running" if row.progress > 0 => {
                (format!("{} %", row.progress), Some(row.progress), false)
            }
            "running" => ("En cours".to_string(), None, true),
            "done" if row.kind == "convert" => (
                format!("Prêt · {}", human_size(row.result_size.unwrap_or(0))),
                None,
                false,
            ),
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
                    .unwrap_or_else(|| "Échec de la conversion.".into()),
                None,
                false,
            ),
        };
        if matches!(row.status.as_str(), "queued" | "running") {
            panel.active += 1;
        }
        // `file_id` s'efface avec l'original (ON DELETE CASCADE sur la tâche).
        let can_replace =
            row.status == "done" && row.result_file_id.is_some() && row.file_id.is_some();
        panel.items.push(JobItem {
            name: row.source_name,
            what: job_label(&row.kind, row.target.as_deref()),
            state: row.status,
            status,
            progress,
            busy,
            can_replace,
            result_id: row.result_file_id,
            job_id: row.id,
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
    /// `1` : tous les espaces (onglet Outils).
    #[serde(default)]
    all: u8,
}

/// `GET /files/jobs?active=…` : le suivi, redemandé toutes les 2 secondes
/// par la page tant qu'une tâche tourne. Quand une tâche s'est terminée
/// depuis, l'en-tête `HX-Trigger` fait recharger la liste des fichiers.
pub async fn jobs_fragment(
    ctx: Ctx,
    State(state): State<AppState>,
    Query(query): Query<JobsQuery>,
) -> AppResult<Response> {
    let space = (query.all == 0).then_some(ctx.active.id);
    let jobs = panel(&state, ctx.user.id, space).await?;
    let finished = jobs.active < query.active;
    let mut response = web::html(JobsFragment { jobs });
    if finished {
        response
            .headers_mut()
            .insert("hx-trigger", HeaderValue::from_static("files-changed"));
    }
    Ok(response.into_response())
}

#[derive(Deserialize)]
pub struct ReplaceForm {
    /// Page où revenir ; l'aperçu du résultat sans précision.
    next: Option<String>,
}

/// `POST /files/jobs/{id}/replace` : après une compression ou une
/// conversion, l'original est supprimé et la copie prend son nom (avec sa
/// propre extension : « IMG_1.HEIC » -> « IMG_1.jpg »).
pub async fn replace_original(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    axum::Form(form): axum::Form<ReplaceForm>,
) -> AppResult<Response> {
    let files = state.config.files.as_ref().ok_or(AppError::NotFound)?;
    let row = sqlx::query!(
        r#"SELECT src.id AS "source!: i64", src.name AS source_name, src.storage_key AS source_key,
                  src.space_id AS source_space, src.folder_id AS source_folder,
                  src.expires_at AS source_expires,
                  res.id AS "result!: i64", res.name AS result_name
           FROM jobs j
           JOIN files src ON src.id = j.file_id AND src.user_id = j.user_id
           JOIN files res ON res.id = j.result_file_id AND res.user_id = j.user_id
           WHERE j.id = ? AND j.user_id = ? AND j.status = 'done'"#,
        id,
        ctx.user.id
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;

    let ext = row
        .result_name
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .unwrap_or_default();
    let name = files::clean_name(&format!("{}.{ext}", stem(&row.source_name)));
    let mime = files::mime_for(&name);
    let now = time::now();
    let mut tx = state.db.begin().await?;
    // La tâche part avec l'original (ON DELETE CASCADE) : le bouton disparaît.
    sqlx::query!(
        "DELETE FROM files WHERE id = ? AND user_id = ?",
        row.source,
        ctx.user.id
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE files SET name = ?, mime = ?, space_id = ?, folder_id = ?, expires_at = ?, updated_at = ?
         WHERE id = ? AND user_id = ?",
        name,
        mime,
        row.source_space,
        row.source_folder,
        row.source_expires,
        now,
        row.result,
        ctx.user.id
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    files::remove_stored(files, &row.source_key).await;

    ctx.flash_ok(format!("Original remplacé : il reste « {name} »"))
        .await?;
    // Retour à la page d'où vient le clic (la liste des fichiers, l'onglet
    // Outils…), que htmx nomme dans HX-Current-URL ; sinon l'aperçu de la copie.
    let back = form
        .next
        .as_deref()
        .map(|next| web::local_path(Some(next)).to_string())
        .or_else(|| current_path(&headers))
        .unwrap_or_else(|| format!("/files/{}", row.result));
    Ok(see_other(&back))
}

/// Chemin de la page d'où vient une requête htmx (`HX-Current-URL`, une
/// adresse complète), s'il reste dans le site.
fn current_path(headers: &HeaderMap) -> Option<String> {
    let url = headers.get("hx-current-url")?.to_str().ok()?;
    let path = match url.split_once("://") {
        Some((_, rest)) => &rest[rest.find('/')?..],
        None => url,
    };
    let path = path.split('#').next().unwrap_or(path);
    // Même règle que web::local_path : pas d'autre domaine, pas de « \ ».
    (path.starts_with('/') && !path.starts_with("//") && !path.contains('\\'))
        .then(|| path.to_string())
}

/// Conversions en cours d'un utilisateur, tous espaces : leur nombre, et si
/// l'une d'elles part d'un fichier de l'onglet Outils (le lien y mène alors).
pub async fn active_jobs(db: &sqlx::SqlitePool, user_id: i64) -> AppResult<(i64, bool)> {
    let row = sqlx::query!(
        r#"SELECT count(*) AS "n!: i64",
                  coalesce(max(f.expires_at IS NOT NULL), 0) AS "tools!: bool"
           FROM jobs j
           LEFT JOIN files f ON f.id = j.file_id
           WHERE j.user_id = ? AND j.kind IN ('compress', 'convert')
             AND j.status IN ('queued', 'running')"#,
        user_id
    )
    .fetch_one(db)
    .await?;
    Ok((row.n, row.tools))
}

/// Fin d'une conversion, annoncée dans la barre latérale.
pub struct Finished {
    pub ok: bool,
    pub text: String,
    pub href: String,
}

#[derive(Template)]
#[template(path = "_jobs_indicator.html")]
struct Indicator {
    active: i64,
    tools: bool,
    done: Option<Finished>,
}

/// `GET /jobs/indicator?active=…` : l'indicateur de la barre latérale,
/// redemandé toutes les 3 secondes tant qu'une conversion tourne. Quand la
/// dernière se termine, il dit ce qui est prêt, avec un lien, et se tait.
pub async fn indicator(
    ctx: Ctx,
    State(state): State<AppState>,
    Query(query): Query<JobsQuery>,
) -> AppResult<Response> {
    if state.config.work_dir.is_none() {
        return Err(AppError::NotFound);
    }
    let (active, tools) = active_jobs(&state.db, ctx.user.id).await?;
    let mut done = None;
    if active == 0 && query.active > 0 {
        let last = sqlx::query!(
            r#"SELECT j.status, j.source_name, j.result_file_id, f.name AS "result_name?: String",
                      src.expires_at AS "source_expires?: i64"
               FROM jobs j
               LEFT JOIN files f ON f.id = j.result_file_id
               LEFT JOIN files src ON src.id = j.file_id
               WHERE j.user_id = ? AND j.kind IN ('compress', 'convert')
                 AND j.status IN ('done', 'failed')
               ORDER BY j.updated_at DESC, j.id DESC
               LIMIT 1"#,
            ctx.user.id
        )
        .fetch_optional(&state.db)
        .await?;
        done = last.map(|last| match (last.result_file_id, last.result_name) {
            (Some(id), Some(name)) if last.status == "done" => Finished {
                ok: true,
                text: format!("Prêt : {name}"),
                href: format!("/files/{id}"),
            },
            _ => Finished {
                ok: false,
                text: format!("Échec : {}", last.source_name),
                href: if last.source_expires.is_some() {
                    "/tools".into()
                } else {
                    "/files".into()
                },
            },
        });
    }
    Ok(web::html(Indicator {
        active,
        tools,
        done,
    }))
}

// ---------------------------------------------------------------------
// Onglet Outils
// ---------------------------------------------------------------------

/// Un fichier déposé dans l'onglet Outils est gardé ce temps-là.
pub const TOOLS_KEEP_SECONDS: i64 = 24 * 3600;

pub struct ToolItem {
    pub id: i64,
    pub name: String,
    /// « Photo · 2,4 Mo · encore 23 h »
    pub meta: String,
    pub conversions: Vec<(&'static str, &'static str)>,
    /// PDF : champ « pages » pour l'extraction.
    pub pdf: bool,
    pub thumb: bool,
    pub icon: &'static str,
}

#[derive(Template)]
#[template(path = "tools/page.html")]
struct ToolsPage {
    shell: Shell,
    /// Faux si les fichiers ou le service de conversion ne sont pas activés.
    enabled: bool,
    max_size: String,
    max_bytes: u64,
    items: Vec<ToolItem>,
    jobs: JobsPanel,
    spaces: Vec<crate::spaces::Space>,
}

/// Temps restant avant une échéance : « 23 h », « 40 min » (arrondi au-dessus).
pub fn time_left(expires_at: i64, now: i64) -> String {
    let left = (expires_at - now).max(0);
    if left >= 3600 {
        format!("{} h", (left + 3599) / 3600)
    } else {
        format!("{} min", ((left + 59) / 60).max(1))
    }
}

/// `GET /tools` : déposer un fichier pas encore rangé, le convertir, puis le
/// garder dans un espace ou le télécharger. Ce qui n'est pas gardé disparaît
/// au bout de 24 heures.
pub async fn page(ctx: Ctx, State(state): State<AppState>) -> AppResult<Response> {
    let files = state.config.files.as_ref();
    let enabled = files.is_some() && state.config.work_dir.is_some();
    let now = time::now();
    let items = if enabled {
        sqlx::query!(
            r#"SELECT id AS "id!", name, mime, size, thumb, expires_at AS "expires_at!: i64"
               FROM files
               WHERE user_id = ? AND expires_at IS NOT NULL
               ORDER BY created_at DESC, id DESC"#,
            ctx.user.id
        )
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .map(|row| {
            let conversions = conversions_for(&row.mime);
            ToolItem {
                id: row.id,
                meta: format!(
                    "{} · {} · encore {}",
                    files::kind_label(&row.mime),
                    human_size(row.size),
                    time_left(row.expires_at, now)
                ),
                pdf: conversions.contains(&Conversion::Pages),
                conversions: conversions.iter().map(|c| (c.key(), c.label())).collect(),
                thumb: row.thumb == 1,
                icon: files::icon_for(&row.mime),
                name: row.name,
            }
        })
        .collect()
    } else {
        Vec::new()
    };
    let page = ToolsPage {
        shell: ctx.shell("tools").await?,
        enabled,
        max_size: files
            .map(|f| human_size(i64::try_from(f.max_upload_bytes).unwrap_or(i64::MAX)))
            .unwrap_or_default(),
        max_bytes: files.map(|f| f.max_upload_bytes).unwrap_or_default(),
        items,
        jobs: panel(&state, ctx.user.id, None).await?,
        spaces: ctx.spaces.clone(),
    };
    Ok(web::html(page))
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
    fn menu_convertir_selon_le_type() {
        use Conversion::*;
        assert_eq!(
            conversions_for("image/heic"),
            vec![Compress, Jpg, Png, Webp]
        );
        assert_eq!(conversions_for("image/png"), vec![Compress, Jpg, Webp]);
        assert_eq!(conversions_for("image/gif"), vec![Jpg, Png, Webp]);
        assert_eq!(conversions_for("video/quicktime"), vec![Compress, Mp4, Mp3]);
        assert_eq!(conversions_for("video/mp4"), vec![Compress, Mp3]);
        assert_eq!(conversions_for("audio/x-m4a"), vec![Mp3]);
        assert_eq!(conversions_for("audio/mpeg"), Vec::<Conversion>::new());
        assert_eq!(conversions_for("application/pdf"), vec![Compress, Pages]);
        assert!(conversions_for("application/zip").is_empty());
        for c in Conversion::ALL {
            assert_eq!(Conversion::from_key(c.key()), Some(c));
        }
        assert_eq!(Conversion::from_key("exe"), None);
    }

    #[test]
    fn page_d_origine() {
        let with = |url: &str| {
            let mut headers = HeaderMap::new();
            headers.insert("hx-current-url", HeaderValue::from_str(url).unwrap());
            current_path(&headers)
        };
        assert_eq!(
            with("https://app.dashu.dev/e/perso/files?seul=1"),
            Some("/e/perso/files?seul=1".into())
        );
        assert_eq!(with("http://localhost:3001/tools#x"), Some("/tools".into()));
        assert_eq!(with("https://evil.example"), None);
        assert_eq!(with("https://app.dashu.dev//evil.example/"), None);
        assert_eq!(current_path(&HeaderMap::new()), None);
    }

    #[test]
    fn plages_de_pages() {
        assert_eq!(parse_pages("2-4"), Some((2, 4)));
        assert_eq!(parse_pages(" 2 – 4 "), Some((2, 4)));
        assert_eq!(parse_pages("3"), Some((3, 3)));
        assert_eq!(parse_pages("4-2"), None);
        assert_eq!(parse_pages("0-2"), None);
        assert_eq!(parse_pages("1-99999"), None);
        assert_eq!(parse_pages("2-4 -sOutputFile=/etc/passwd"), None);
        assert_eq!(parse_pages(""), None);
    }

    #[test]
    fn noms_des_resultats() {
        let spec = |media, to, pages| JobSpec {
            task: Task::Convert,
            media,
            input: String::new(),
            output: String::new(),
            preview: None,
            to,
            pages,
        };
        let heic = spec(Media::Heic, Some(Conversion::Jpg), None);
        assert_eq!(result_name("IMG_1.HEIC", &heic), "IMG_1.jpg");
        let pages = spec(Media::Pdf, Some(Conversion::Pages), Some((2, 4)));
        assert_eq!(result_name("cours.pdf", &pages), "cours (pages 2-4).pdf");
        let page = spec(Media::Pdf, Some(Conversion::Pages), Some((3, 3)));
        assert_eq!(result_name("cours.pdf", &page), "cours (page 3).pdf");
        let mp3 = spec(Media::Video, Some(Conversion::Mp3), None);
        assert_eq!(result_name("clip.mov", &mp3), "clip.mp3");
        let mut compress = spec(Media::Video, None, None);
        compress.task = Task::Compress;
        assert_eq!(result_name("clip.mov", &compress), "clip (compressé).mp4");
    }

    #[test]
    fn extensions_sures() {
        assert_eq!(safe_ext("Vacances.MOV"), "mov");
        assert_eq!(safe_ext("a.b/../../x"), "x");
        assert_eq!(safe_ext("rien"), "bin");
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dashu-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Le dossier d'une tâche s'ouvre au groupe sans jamais demander le bit
    /// setgid : sur le serveur, `RestrictSUIDSGID=yes` refuserait le `chmod`
    /// et toute compression échouerait dès le clic (bug du 13 novembre 2026).
    #[cfg(unix)]
    #[tokio::test]
    async fn dossier_de_tache_sans_bit_setgid() {
        use std::os::unix::fs::PermissionsExt;
        let work = scratch("droits");
        // Comme sur le serveur : jobs/ en 2770.
        std::fs::create_dir_all(work.join("jobs")).unwrap();
        std::fs::set_permissions(work.join("jobs"), std::fs::Permissions::from_mode(0o2770))
            .unwrap();
        let source = work.join("source.mov");
        std::fs::write(&source, b"video").unwrap();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600)).unwrap();
        let spec = JobSpec {
            task: Task::Compress,
            media: Media::Video,
            input: "input.mov".into(),
            output: "output.mp4".into(),
            preview: None,
            to: None,
            pages: None,
        };

        prepare_job(&work, 3, &source, &spec).await.unwrap();
        let dir = job_dir(&work, 3);
        let mode = |path: &FsPath| std::fs::metadata(path).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode(&dir), 0o770);
        assert_eq!(mode(&dir.join("input.mov")), 0o640);
        assert_eq!(mode(&dir.join("job.json")), 0o640);
        assert!(!dir.join(".job.json").exists());
        let _ = std::fs::remove_dir_all(&work);
    }

    #[tokio::test]
    async fn rangement_d_un_resultat() {
        let dir = scratch("rangement");
        let (from, to) = (dir.join("output.mp4"), dir.join("range"));
        std::fs::write(&from, b"resultat").unwrap();
        move_file(&from, &to).await.unwrap();
        assert!(!from.exists());
        assert_eq!(std::fs::read(&to).unwrap(), b"resultat");

        // Pas de grand aperçu produit : erreur, et rien d'écrasé à l'arrivée.
        let missing = move_file(&dir.join("preview.jpg"), &to).await.unwrap_err();
        assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
        assert_eq!(std::fs::read(&to).unwrap(), b"resultat");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Entre deux systèmes de fichiers (ici /dev/shm, en mémoire, et le
    /// dossier temporaire), `rename` échoue comme entre deux points de montage
    /// du serveur : le fichier est copié puis effacé.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn rangement_entre_deux_montages() {
        let shm = FsPath::new("/dev/shm");
        if !shm.is_dir() {
            return;
        }
        let from = shm.join(format!("dashu-output-{}", std::process::id()));
        std::fs::write(&from, b"resultat").unwrap();
        let dir = scratch("montages");
        let to = dir.join("range");
        move_file(&from, &to).await.unwrap();
        assert!(!from.exists());
        assert_eq!(std::fs::read(&to).unwrap(), b"resultat");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Rejoue le bac à sable du service dashu sur le serveur
    /// (`deploy/lxc/hardening.conf`) : la CI lance ce test par `systemd-run`,
    /// avec `RestrictSUIDSGID=yes` et `/srv/dashu/tmp`, `/srv/dashu/fichiers`
    /// en `ReadWritePaths=` (voir `.github/workflows/ci.yml`). Ignoré ailleurs.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "bac à sable systemd : lancé par la CI"]
    async fn bac_a_sable_du_serveur() {
        use std::os::unix::fs::PermissionsExt;
        const EPERM: i32 = 1;
        const EXDEV: i32 = 18;
        let work = FsPath::new("/srv/dashu/tmp");
        let stored = FsPath::new("/srv/dashu/fichiers");

        // Le bac à sable est bien là, sinon ce test ne prouverait rien : le
        // bit setgid est refusé, et les deux dossiers sont deux montages.
        let probe = work.join("jobs").join("sonde");
        std::fs::create_dir_all(&probe).unwrap();
        let refused =
            std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o2770)).unwrap_err();
        assert_eq!(refused.raw_os_error(), Some(EPERM), "{refused}");
        std::fs::write(probe.join("f"), b"x").unwrap();
        let crossed = std::fs::rename(probe.join("f"), stored.join("f")).unwrap_err();
        assert_eq!(crossed.raw_os_error(), Some(EXDEV), "{crossed}");
        std::fs::remove_dir_all(&probe).unwrap();

        // Une compression comme au clic sur « Compresser », puis le rangement
        // de son résultat parmi les fichiers.
        let source = stored.join("source.mov");
        std::fs::write(&source, b"video").unwrap();
        let spec = JobSpec {
            task: Task::Compress,
            media: Media::Video,
            input: "input.mov".into(),
            output: "output.mp4".into(),
            preview: None,
            to: None,
            pages: None,
        };
        prepare_job(work, 1, &source, &spec).await.unwrap();
        let dir = job_dir(work, 1);
        assert!(dir.join("job.json").exists());
        assert_eq!(std::fs::read(dir.join("input.mov")).unwrap(), b"video");

        std::fs::write(dir.join("output.mp4"), b"plus leger").unwrap();
        let dest = stored.join("range");
        move_file(&dir.join("output.mp4"), &dest).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"plus leger");
        assert!(!dir.join("output.mp4").exists());
    }
}
