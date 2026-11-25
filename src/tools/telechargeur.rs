//! Téléchargeur de liens (SPEC v2, « Outils : téléchargeur de liens ») : un
//! lien YouTube, Instagram ou TikTok devient une vidéo MP4 ou un son MP3.
//!
//! Réservé au compte `DOWNLOADER_EMAIL` : pour tout autre compte, l'outil
//! n'existe pas (ni formulaire, ni route). Il n'est ni dans l'API ni dans le
//! connecteur Claude.
//!
//! Même circuit que les conversions, dans un dossier à part,
//! `WORK_DIR/downloads/<id>/` :
//!   1. dashu vérifie le lien ([`check_link`]) et écrit `job.json` ([`queue`]) ;
//!   2. le service `dashu-download` ([`run`]) le prend, revérifie le lien,
//!      lance yt-dlp, et laisse `output.<ext>`, `title`, puis `done` ou `error` ;
//!   3. dashu range le résultat dans Temporaire ([`import`]), marqué « venu
//!      d'un lien » : il ne se partagera jamais par lien.
//!
//! Le service a Internet, mais pas le réseau de la maison : `IPAddressDeny`
//! (deploy/lxc/dashu-download.service). Dans un conteneur, systemd peut
//! ignorer ce filtre et lancer le service sans lui : le service le vérifie
//! donc lui-même ([`firewall_active`]) et ne télécharge rien sans lui.

use std::fs;
use std::io::ErrorKind;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path as FsPath, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use anyhow::Context as _;
use axum::extract::State;
use axum::response::Response;
use serde::{Deserialize, Serialize};

use super::worker::{ToolError, claim, ready_jobs, recover_with, run_command, write_atomic};
use crate::AppState;
use crate::auth::session::Ctx;
use crate::config::FilesConfig;
use crate::files;
use crate::time;
use crate::web::{self, AppError, AppResult, see_other};

/// Dossier des téléchargements, dans `WORK_DIR`.
pub const DIR: &str = "downloads";
/// Plus long téléchargement : au-delà, yt-dlp est arrêté.
const TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Téléchargements en attente ou en cours, au plus, par compte.
const MAX_PENDING: i64 = 3;
/// Un lien plus long est refusé sans être lu.
const URL_MAX: usize = 2000;
/// Copie du cookie de session, le temps d'un téléchargement (yt-dlp réécrit
/// le fichier qu'on lui donne).
const JAR: &str = "cookies.txt";

/// Fichier où le service dit s'il tourne, et avec son pare-feu. Il le
/// réécrit toutes les 30 secondes.
const STATUS: &str = "service";
const READY: &str = "actif";
const NO_FIREWALL: &str = "sans pare-feu";
/// Au-delà, le service est réputé arrêté.
const STATUS_MAX_AGE: Duration = Duration::from_secs(120);

const NO_FIREWALL_MESSAGE: &str = "Le téléchargeur est arrêté : son pare-feu n'est pas actif sur ce serveur (README, « Téléchargeur de liens »).";
const STOPPED_MESSAGE: &str =
    "Le service de téléchargement ne tourne pas sur le serveur (dashu-telechargeur etat).";

// ---------------------------------------------------------------------
// Les liens acceptés
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Site {
    YouTube,
    Instagram,
    TikTok,
}

impl Site {
    pub fn name(self) -> &'static str {
        match self {
            Self::YouTube => "YouTube",
            Self::Instagram => "Instagram",
            Self::TikTok => "TikTok",
        }
    }
}

/// Hôtes acceptés, exactement (pas de sous-domaine au hasard).
const HOSTS: [(&str, Site); 12] = [
    ("youtube.com", Site::YouTube),
    ("www.youtube.com", Site::YouTube),
    ("m.youtube.com", Site::YouTube),
    ("music.youtube.com", Site::YouTube),
    ("youtu.be", Site::YouTube),
    ("instagram.com", Site::Instagram),
    ("www.instagram.com", Site::Instagram),
    ("tiktok.com", Site::TikTok),
    ("www.tiktok.com", Site::TikTok),
    ("m.tiktok.com", Site::TikTok),
    ("vm.tiktok.com", Site::TikTok),
    ("vt.tiktok.com", Site::TikTok),
];

/// Un lien accepté, réécrit à partir de ses morceaux.
#[derive(Debug, PartialEq, Eq)]
pub struct Link {
    /// `https://<hôte><chemin>?<paramètres>`, sans identifiant, port ni ancre.
    pub url: String,
    pub site: Site,
    /// Version courte, affichée dans le suivi : `youtube.com/watch?v=…`.
    pub label: String,
}

/// Lit un lien collé dans le formulaire : `https`, sans identifiant ni port,
/// sur un des hôtes de [`HOSTS`] (l'hôte exact, pas une recherche de texte),
/// avec un chemin. Rend le lien réécrit, ou le refus à montrer.
pub fn check_link(input: &str) -> Result<Link, &'static str> {
    const REFUSED: &str = "Seuls les liens YouTube, Instagram et TikTok sont acceptés.";
    let input = input.trim();
    if input.is_empty() {
        return Err("Colle le lien d'une vidéo YouTube, Instagram ou TikTok.");
    }
    if input.len() > URL_MAX {
        return Err("Ce lien est trop long.");
    }
    let url =
        url::Url::parse(input).map_err(|_| "Ce n'est pas un lien : il commence par https://.")?;
    if url.scheme() != "https" {
        return Err("Le lien doit commencer par https://.");
    }
    if !url.username().is_empty() || url.password().is_some() || url.port().is_some() {
        return Err(REFUSED);
    }
    let host = match url.host() {
        Some(url::Host::Domain(domain)) => domain.trim_end_matches('.').to_ascii_lowercase(),
        _ => return Err(REFUSED),
    };
    let site = HOSTS
        .iter()
        .find(|(known, _)| *known == host)
        .map(|(_, site)| *site)
        .ok_or(REFUSED)?;
    let path = url.path();
    if path.is_empty() || path == "/" {
        return Err("Ce lien ne mène pas à une vidéo.");
    }
    let rest = match url.query() {
        Some(query) => format!("{path}?{query}"),
        None => path.to_string(),
    };
    let short = format!("{host}{rest}");
    let label = if short.chars().count() > 80 {
        format!("{}…", short.chars().take(79).collect::<String>())
    } else {
        short
    };
    Ok(Link {
        url: format!("https://{host}{rest}"),
        site,
        label,
    })
}

/// Ce que l'on veut du lien.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// Vidéo MP4, H.264 de 1080p au plus quand la plateforme le propose.
    Mp4,
    /// Le son seul, en MP3.
    Mp3,
}

impl Format {
    /// Valeur du formulaire, rangée dans `jobs.target`.
    pub fn key(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mp3 => "mp3",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "mp4" => Some(Self::Mp4),
            "mp3" => Some(Self::Mp3),
            _ => None,
        }
    }

    pub fn ext(self) -> &'static str {
        self.key()
    }

    /// Libellé dans le suivi des tâches.
    pub fn label(self) -> &'static str {
        match self {
            Self::Mp4 => "Vidéo depuis un lien",
            Self::Mp3 => "Son depuis un lien",
        }
    }
}

/// Ce que le service doit faire : le contenu de `job.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadSpec {
    pub url: String,
    pub to: Format,
    /// Taille maximale du résultat, en octets (celle des envois).
    pub max_bytes: u64,
}

/// Dossier d'un téléchargement.
pub fn job_dir(work_dir: &FsPath, id: i64) -> PathBuf {
    work_dir.join(DIR).join(id.to_string())
}

// ---------------------------------------------------------------------
// Côté dashu : demander un téléchargement
// ---------------------------------------------------------------------

/// Le compte a-t-il le téléchargeur ? Il faut les fichiers, le dossier de
/// travail, et être le compte de `DOWNLOADER_EMAIL`.
pub fn allowed(state: &AppState, email: &str) -> bool {
    state.config.files.is_some()
        && state.config.work_dir.is_some()
        && state
            .config
            .downloader_email
            .as_deref()
            .is_some_and(|wanted| wanted.eq_ignore_ascii_case(email.trim()))
}

/// État du service, d'après ce qu'il écrit dans son dossier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    Ready,
    NoFirewall,
    Stopped,
}

pub async fn service_state(work_dir: &FsPath) -> ServiceState {
    let path = work_dir.join(DIR).join(STATUS);
    let fresh = tokio::fs::metadata(&path)
        .await
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age < STATUS_MAX_AGE);
    if !fresh {
        return ServiceState::Stopped;
    }
    match super::read_small(&path).await.as_deref() {
        Some(READY) => ServiceState::Ready,
        Some(NO_FIREWALL) => ServiceState::NoFirewall,
        _ => ServiceState::Stopped,
    }
}

/// Ce que l'onglet Outils dit au-dessus du formulaire quand le service ne
/// peut pas télécharger, ou rien.
pub async fn note(work_dir: &FsPath) -> Option<&'static str> {
    match service_state(work_dir).await {
        ServiceState::Ready => None,
        ServiceState::NoFirewall => Some(NO_FIREWALL_MESSAGE),
        ServiceState::Stopped => Some(STOPPED_MESSAGE),
    }
}

#[derive(Deserialize)]
pub struct DownloadForm {
    #[serde(default)]
    url: String,
    #[serde(default)]
    to: String,
}

/// `POST /tools/download` : lance un téléchargement. 404 pour tout compte
/// qui n'est pas `DOWNLOADER_EMAIL`.
pub async fn start(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<DownloadForm>,
) -> AppResult<Response> {
    if !allowed(&state, &ctx.user.email) {
        return Err(AppError::NotFound);
    }
    match queue(&state, ctx.user.id, ctx.active.id, &form.url, &form.to).await? {
        Ok(_) => {
            ctx.flash_ok("Téléchargement lancé : le fichier arrivera ici, dans Temporaire")
                .await?
        }
        Err(message) => ctx.flash_error(message).await?,
    }
    Ok(see_other("/tools"))
}

/// Met un téléchargement en file. Rend son numéro, ou le refus à montrer.
pub(crate) async fn queue(
    state: &AppState,
    user_id: i64,
    space_id: i64,
    raw_url: &str,
    raw_to: &str,
) -> AppResult<Result<i64, String>> {
    let (Some(files), Some(work_dir)) = (&state.config.files, &state.config.work_dir) else {
        return Err(AppError::NotFound);
    };
    let Some(to) = Format::from_key(raw_to) else {
        return Ok(Err("Choisis la vidéo (MP4) ou le son (MP3).".into()));
    };
    let link = match check_link(raw_url) {
        Ok(link) => link,
        Err(message) => return Ok(Err(message.into())),
    };
    match service_state(work_dir).await {
        ServiceState::Ready => {}
        ServiceState::NoFirewall => return Ok(Err(NO_FIREWALL_MESSAGE.into())),
        ServiceState::Stopped => return Ok(Err(STOPPED_MESSAGE.into())),
    }
    let pending = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!: i64" FROM jobs
           WHERE user_id = ? AND kind = 'download' AND status IN ('queued', 'running')"#,
        user_id
    )
    .fetch_one(&state.db)
    .await?;
    if pending >= MAX_PENDING {
        return Ok(Err(format!(
            "{MAX_PENDING} téléchargements sont déjà en attente : laisse-les finir."
        )));
    }

    let now = time::now();
    let target = to.key();
    let job_id = sqlx::query!(
        "INSERT INTO jobs (user_id, space_id, kind, target, source_name, source_size, created_at, updated_at)
         VALUES (?, ?, 'download', ?, ?, 0, ?, ?)",
        user_id,
        space_id,
        target,
        link.label,
        now,
        now
    )
    .execute(&state.db)
    .await?
    .last_insert_rowid();
    let spec = DownloadSpec {
        url: link.url,
        to,
        max_bytes: files.max_upload_bytes,
    };
    if let Err(error) = prepare(work_dir, job_id, &spec).await {
        let _ = tokio::fs::remove_dir_all(job_dir(work_dir, job_id)).await;
        sqlx::query!("DELETE FROM jobs WHERE id = ?", job_id)
            .execute(&state.db)
            .await?;
        return Err(error.into());
    }
    tracing::info!(
        user = user_id,
        job = job_id,
        site = link.site.name(),
        "téléchargement demandé"
    );
    Ok(Ok(job_id))
}

/// Dossier d'un téléchargement : `job.json`, ouvert au service, posé en
/// dernier (le service ne prend la tâche qu'une fois là).
async fn prepare(work_dir: &FsPath, job_id: i64, spec: &DownloadSpec) -> anyhow::Result<()> {
    let dir = job_dir(work_dir, job_id);
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("dossier de téléchargement {}", dir.display()))?;
    let tmp = dir.join(".job.json");
    tokio::fs::write(&tmp, serde_json::to_vec_pretty(spec)?).await?;
    super::share_with_worker(&dir, &[&tmp]).context("droits du dossier de téléchargement")?;
    tokio::fs::rename(&tmp, dir.join("job.json")).await?;
    Ok(())
}

// ---------------------------------------------------------------------
// Côté dashu : ranger le résultat
// ---------------------------------------------------------------------

/// Range le résultat d'un téléchargement réussi dans Temporaire (24 heures
/// pour le garder dans un espace), marqué « venu d'un lien ».
///
/// Le dossier appartient aussi au service, qui a Internet : on ne se fie ni
/// à ce qu'il a pu réécrire dans `job.json` (le format vient de la base), ni
/// à un lien symbolique posé à la place du résultat. Le résultat est d'abord
/// sorti de son dossier, hors de portée du service, puis vérifié.
pub(super) async fn import(
    state: &AppState,
    files: &FilesConfig,
    work_dir: &FsPath,
    dir: &FsPath,
    job: &super::Pending,
    target: Option<&str>,
) -> anyhow::Result<()> {
    let format = target
        .and_then(Format::from_key)
        .context("format de téléchargement inconnu")?;
    let taken = work_dir.join(format!(".telechargement-{}", job.id));
    tokio::fs::rename(dir.join(format!("output.{}", format.ext())), &taken)
        .await
        .context("résultat du téléchargement")?;
    let result = store(state, files, dir, &taken, job, format).await;
    let _ = tokio::fs::remove_file(&taken).await;
    result
}

/// Nom du fichier téléchargé : le titre de la vidéo, venu de la plateforme.
/// Une barre n'y est pas un dossier (« épisode 1/2 » garde son début), et le
/// titre est raccourci avant l'extension, qui reste.
pub fn result_name(title: &str, format: Format) -> String {
    let title: String = title
        .replace(['/', '\\'], "-")
        .chars()
        .filter(|c| !c.is_control())
        .take(150)
        .collect();
    let title = match title.trim() {
        "" | "." | ".." => "Téléchargement",
        title => title,
    };
    files::clean_name(&format!("{title}.{}", format.ext()))
}

async fn store(
    state: &AppState,
    files: &FilesConfig,
    dir: &FsPath,
    taken: &FsPath,
    job: &super::Pending,
    format: Format,
) -> anyhow::Result<()> {
    let meta = tokio::fs::symlink_metadata(taken).await?;
    if !meta.file_type().is_file() {
        anyhow::bail!("le résultat du téléchargement n'est pas un fichier");
    }
    if meta.len() > files.max_upload_bytes {
        super::fail(
            state,
            job.id,
            "Trop gros : plus grand que la taille maximale des fichiers.",
        )
        .await?;
        return Ok(());
    }
    let size = i64::try_from(meta.len())?;
    let title = super::read_small(&dir.join("title")).await;
    let name = result_name(title.as_deref().unwrap_or_default(), format);
    let mime = files::mime_for(&name);

    let sha256 = files::hash_file(taken.to_path_buf()).await?;
    let key = web::csrf::random_token();
    let dest = files::stored_path(&files.dir, &key);
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    super::move_file(taken, &dest)
        .await
        .context("rangement d'un téléchargement")?;

    let now = time::now();
    let expires_at = now + super::TOOLS_KEEP_SECONDS;
    let saved: anyhow::Result<i64> = async {
        let mut tx = state.db.begin().await?;
        let file_id = sqlx::query!(
            "INSERT INTO files (user_id, space_id, name, mime, size, sha256, storage_key, expires_at, from_link, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
            job.user_id,
            job.space_id,
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
            if let Err(error) = super::enqueue_thumbnail(state, file_id).await {
                tracing::warn!(error = format!("{error:#}"), "vignette d'un téléchargement");
            }
            Ok(())
        }
        Err(error) => {
            let _ = tokio::fs::remove_file(&dest).await;
            Err(error)
        }
    }
}

// ---------------------------------------------------------------------
// Le service dashu-download
// ---------------------------------------------------------------------

/// Adresses que le service ne doit jamais joindre : la machine elle-même, le
/// réseau de la maison, Tailscale. Avec le pare-feu, la connexion est
/// refusée sur-le-champ (EPERM) ; sans lui, elle est refusée par la cible,
/// aboutit, ou n'aboutit pas à temps.
const PROBES: [&str; 3] = ["127.0.0.1:9", "192.168.0.1:9", "100.100.100.100:53"];

/// Vrai si le pare-feu du service est en place.
pub fn firewall_active() -> bool {
    PROBES.iter().all(|probe| {
        let Ok(address) = probe.parse::<SocketAddr>() else {
            return false;
        };
        matches!(
            TcpStream::connect_timeout(&address, Duration::from_secs(1)),
            Err(error) if error.kind() == ErrorKind::PermissionDenied
        )
    })
}

/// Vérifie le pare-feu et l'écrit pour dashu.
fn check_and_report(dir: &FsPath) -> bool {
    let active = firewall_active();
    write_atomic(dir, STATUS, if active { READY } else { NO_FIREWALL });
    active
}

/// Boucle du service : un téléchargement à la fois. Ne rend jamais la main,
/// sauf erreur au démarrage.
pub fn run(work_dir: &FsPath, cookies: Option<PathBuf>) -> anyhow::Result<()> {
    let dir = work_dir.join(DIR);
    fs::create_dir_all(&dir).with_context(|| format!("dossier {}", dir.display()))?;
    recover_with(
        &dir,
        "Téléchargement interrompu deux fois : vidéo trop lourde pour le serveur ?",
    );
    if check_and_report(&dir) {
        tracing::info!(dir = %dir.display(), "téléchargeur démarré, pare-feu actif");
    } else {
        tracing::error!(
            "pare-feu absent : le téléchargeur joindrait le réseau local ; aucun téléchargement tant qu'il manque (IPAddressDeny ignoré par ce conteneur ?)"
        );
    }
    let mut ticks: u32 = 0;
    loop {
        for job in ready_jobs(&dir) {
            if !claim(&job) {
                continue;
            }
            // Revérifié avant chaque téléchargement : le filtre ne doit pas
            // avoir disparu depuis le démarrage.
            if check_and_report(&dir) {
                process(&job, cookies.as_deref());
            } else {
                write_atomic(&job, "error", NO_FIREWALL_MESSAGE);
            }
        }
        ticks = ticks.wrapping_add(1);
        if ticks.is_multiple_of(15) {
            check_and_report(&dir);
        }
        thread::sleep(Duration::from_secs(2));
    }
}

/// Traite un téléchargement déjà pris, et finit toujours par `done` ou `error`.
fn process(dir: &FsPath, cookies: Option<&FsPath>) {
    let result = download(dir, cookies);
    let _ = fs::remove_file(dir.join(JAR));
    match result {
        Ok(()) => write_atomic(dir, "done", ""),
        Err(message) => {
            tracing::warn!(job = %dir.display(), %message, "téléchargement en échec");
            write_atomic(dir, "error", &message);
        }
    }
}

fn download(dir: &FsPath, cookies: Option<&FsPath>) -> Result<(), String> {
    let raw = fs::read(dir.join("job.json")).map_err(|_| "Tâche illisible.".to_string())?;
    let spec: DownloadSpec =
        serde_json::from_slice(&raw).map_err(|_| "Tâche illisible.".to_string())?;
    // Le service ne se fie qu'à ce qu'il a relu lui-même.
    let link = check_link(&spec.url).map_err(str::to_string)?;
    let jar = cookies.filter(|path| path.is_file()).and_then(|path| {
        let copy = dir.join(JAR);
        fs::copy(path, &copy).ok().map(|_| copy)
    });

    let mut cmd = Command::new("yt-dlp");
    cmd.args(ytdlp_args(&spec, &link.url, dir, jar.as_deref()))
        .current_dir(dir);
    let mut best = 0u8;
    run_command(cmd, TIMEOUT, |line| {
        if let Some(percent) = progress(line)
            && percent > best
        {
            best = percent;
            write_atomic(dir, "progress", &percent.to_string());
        }
    })
    .map_err(|error| {
        match error {
            ToolError::Missing(_) => {
                "yt-dlp n'est pas installé sur le serveur (dashu-telechargeur activer)."
            }
            ToolError::Timeout => {
                "Trop long : le téléchargement a été arrêté au bout de 30 minutes."
            }
            ToolError::Lost => FAILED,
            ToolError::Failed(detail) => explain(&detail),
        }
        .to_string()
    })?;
    match fs::metadata(dir.join(format!("output.{}", spec.to.ext()))) {
        Ok(meta) if meta.len() > 0 => Ok(()),
        _ => Err(NOTHING.into()),
    }
}

const FAILED: &str =
    "Le téléchargement a échoué : la plateforme a refusé, ou yt-dlp n'est plus à jour.";
const NOTHING: &str = "Rien n'a été téléchargé : direct en cours, fichier trop gros, ou lien vers autre chose qu'une vidéo.";

/// Arguments de yt-dlp. Aucune configuration lue ailleurs, une seule vidéo,
/// pas de direct, la taille maximale des envois, un nom de sortie fixe ; le
/// lien vient en dernier, après `--`.
pub fn ytdlp_args(
    spec: &DownloadSpec,
    url: &str,
    dir: &FsPath,
    jar: Option<&FsPath>,
) -> Vec<String> {
    let mut args: Vec<String> = [
        "--ignore-config",
        "--no-playlist",
        "--playlist-items",
        "1",
        "--match-filter",
        "!is_live",
        "--quiet",
        "--progress",
        "--newline",
        "--progress-template",
        "download:%(progress._percent_str)s",
        "--socket-timeout",
        "30",
        "--no-mtime",
    ]
    .map(String::from)
    .to_vec();
    args.extend([
        "--max-filesize".to_string(),
        spec.max_bytes.to_string(),
        "--paths".to_string(),
        dir.display().to_string(),
        "--output".to_string(),
        "output.%(ext)s".to_string(),
        "--print-to-file".to_string(),
        "%(title)s".to_string(),
        dir.join("title").display().to_string(),
    ]);
    if let Some(jar) = jar {
        args.extend(["--cookies".to_string(), jar.display().to_string()]);
    }
    let format: &[&str] = match spec.to {
        Format::Mp4 => &[
            "--format",
            "bv*+ba/b",
            "--format-sort",
            "res:1080,vcodec:h264,acodec:m4a",
            "--merge-output-format",
            "mp4",
            "--remux-video",
            "mp4",
        ],
        Format::Mp3 => &[
            "--format",
            "ba/b",
            "--extract-audio",
            "--audio-format",
            "mp3",
            "--audio-quality",
            "2",
        ],
    };
    args.extend(format.iter().map(|arg| arg.to_string()));
    args.push("--".into());
    args.push(url.into());
    args
}

/// Progression lue dans une ligne de yt-dlp (« 42.3% »), de 0 à 99.
pub fn progress(line: &str) -> Option<u8> {
    let end = line.find('%')?;
    let head = &line[..end];
    let start = head
        .rfind(|c: char| !(c.is_ascii_digit() || c == '.'))
        .map_or(0, |i| i + 1);
    let value: f64 = head[start..].parse().ok()?;
    Some(value.clamp(0.0, 99.0) as u8)
}

/// Message lisible tiré de la fin de la sortie d'erreur de yt-dlp.
pub fn explain(detail: &str) -> &'static str {
    let detail = detail.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|word| detail.contains(word));
    if has(&["private video", "video is private"]) {
        "Vidéo privée : elle ne se télécharge pas."
    } else if has(&["confirm your age", "age-restricted", "age restricted"]) {
        "Vidéo réservée aux adultes : il faut le cookie de session d'un compte (README, « Téléchargeur de liens »)."
    } else if has(&["javascript", "js runtime", "deno"]) {
        "yt-dlp n'a pas son moteur JavaScript (Deno) : relance « dashu-telechargeur activer » sur le serveur."
    } else if has(&[
        "sign in",
        "login",
        "log in",
        "cookies",
        "rate-limit",
        "rate limit",
        "429",
    ]) {
        "La plateforme demande d'être connecté : il faut le cookie de session d'un compte (README, « Téléchargeur de liens »)."
    } else if has(&["requested format"]) {
        "Aucun format téléchargeable pour cette vidéo."
    } else if has(&["unsupported url"]) {
        "Ce lien ne mène pas à une vidéo."
    } else if has(&[
        "unavailable",
        "not available",
        "removed",
        "does not exist",
        "404",
    ]) {
        "Vidéo introuvable : supprimée, ou indisponible dans ce pays."
    } else {
        FAILED
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn liens_acceptes() {
        let link = check_link(" https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=10#ancre ").unwrap();
        assert_eq!(link.url, "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=10");
        assert_eq!(link.site, Site::YouTube);
        assert_eq!(link.label, "www.youtube.com/watch?v=dQw4w9WgXcQ&t=10");
        assert_eq!(
            check_link("https://YOUTU.BE/abc").unwrap().url,
            "https://youtu.be/abc"
        );
        assert_eq!(
            check_link("https://www.instagram.com/reel/C1abc/")
                .unwrap()
                .site,
            Site::Instagram
        );
        assert_eq!(
            check_link("https://vm.tiktok.com/ZMabc/").unwrap().site,
            Site::TikTok
        );
        assert_eq!(
            check_link("https://youtube.com./shorts/abc").unwrap().url,
            "https://youtube.com/shorts/abc"
        );
        let long = format!("https://www.youtube.com/watch?v={}", "a".repeat(200));
        assert!(check_link(&long).unwrap().label.ends_with('…'));
    }

    #[test]
    fn liens_refuses() {
        for refused in [
            "",
            "pas un lien",
            "http://www.youtube.com/watch?v=abc",
            "ftp://youtube.com/abc",
            "https://www.youtube.com",
            "https://www.youtube.com/",
            "https://youtube.com.example.com/watch?v=abc",
            "https://example.com/?u=https://youtube.com/watch",
            "https://example.com/youtube.com/watch",
            "https://youtube.com@example.com/watch",
            "https://user:pass@www.youtube.com/watch?v=abc",
            "https://www.youtube.com:8443/watch?v=abc",
            "https://192.168.1.10/watch",
            "https://[::1]/watch",
            "https://localhost/watch",
            "https://gaming.youtube.com/watch?v=abc",
            "https://xn--yutube-wqf.com/watch?v=abc",
            "https://dailymotion.com/video/abc",
        ] {
            assert!(check_link(refused).is_err(), "{refused}");
        }
        assert!(check_link(&format!("https://youtu.be/{}", "a".repeat(2001))).is_err());
    }

    #[test]
    fn arguments_de_yt_dlp() {
        let spec = DownloadSpec {
            url: "https://youtu.be/abc".into(),
            to: Format::Mp3,
            max_bytes: 1024,
        };
        let dir = FsPath::new("/srv/dashu/tmp/downloads/7");
        let args = ytdlp_args(&spec, "https://youtu.be/abc", dir, None);
        for wanted in ["--ignore-config", "--no-playlist", "--extract-audio"] {
            assert!(args.iter().any(|a| a == wanted), "{wanted}");
        }
        assert!(args.windows(2).any(|w| w == ["--max-filesize", "1024"]));
        assert!(args.windows(2).any(|w| w == ["--audio-format", "mp3"]));
        assert!(args.windows(2).any(|w| w == ["--output", "output.%(ext)s"]));
        // Le lien en dernier, après « -- » : jamais pris pour une option.
        assert_eq!(&args[args.len() - 2..], ["--", "https://youtu.be/abc"]);
        assert!(!args.iter().any(|a| a == "--cookies"));

        let video = DownloadSpec {
            to: Format::Mp4,
            ..spec
        };
        let jar = dir.join(JAR);
        let args = ytdlp_args(&video, "https://youtu.be/abc", dir, Some(&jar));
        assert!(
            args.windows(2)
                .any(|w| w == ["--merge-output-format", "mp4"])
        );
        assert!(args.iter().any(|a| a == "--cookies"));
        assert!(!args.iter().any(|a| a == "--extract-audio"));
    }

    #[test]
    fn progression() {
        assert_eq!(progress("download: 42.3%"), Some(42));
        assert_eq!(progress("download:100.0%"), Some(99));
        assert_eq!(progress("  7%"), Some(7));
        assert_eq!(progress("download:  N/A%"), None);
        assert_eq!(progress("rien"), None);
    }

    #[test]
    fn messages_d_erreur() {
        assert!(explain("ERROR: [youtube] abc: Private video. Sign in").contains("privée"));
        assert!(explain("ERROR: Sign in to confirm your age").contains("adultes"));
        assert!(explain("ERROR: Sign in to confirm you're not a bot").contains("connecté"));
        assert!(explain("ERROR: [youtube] abc: Video unavailable").contains("introuvable"));
        assert!(explain("WARNING: No supported JavaScript runtime").contains("Deno"));
        assert!(explain("ERROR: Unsupported URL: https://x").contains("vidéo"));
        assert_eq!(explain("ERROR: quelque chose d'autre"), FAILED);
    }

    #[test]
    fn noms_des_resultats() {
        assert_eq!(
            result_name("Ma vidéo : épisode 1/2", Format::Mp4),
            "Ma vidéo : épisode 1-2.mp4"
        );
        assert_eq!(result_name("  ", Format::Mp3), "Téléchargement.mp3");
        assert_eq!(result_name("..", Format::Mp3), "Téléchargement.mp3");
        assert_eq!(result_name("a\u{0}b\nc", Format::Mp3), "abc.mp3");
        let long = result_name(&"é".repeat(400), Format::Mp4);
        assert!(long.ends_with(".mp4"), "l'extension reste : {long}");
    }

    #[test]
    fn formats() {
        assert_eq!(Format::from_key("mp4"), Some(Format::Mp4));
        assert_eq!(Format::from_key("avi"), None);
        assert_eq!(Format::Mp3.ext(), "mp3");
    }
}
