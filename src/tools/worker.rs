//! Service de conversion : `dashu worker`.
//!
//! Lancé par systemd (`deploy/lxc/dashu-worker.service`) sous son propre
//! utilisateur, sans réseau, sans accès à la base ni aux fichiers rangés : il
//! ne voit que `WORK_DIR`. Il n'a donc pas besoin de la configuration de
//! dashu (ni de ses secrets). Toutes les 2 secondes, il cherche les dossiers
//! `jobs/<id>/` prêts (un `job.json`, pas encore de `started`), en prend deux
//! au plus à la fois, et pour chacun :
//!   * écrit `started`, puis `progress` (0 à 100) au fil de l'eau ;
//!   * produit le fichier `output` décrit par `job.json` ;
//!   * finit par `done`, ou par `error` : un message lisible, le détail
//!     technique partant dans les logs.
//!
//! Les fichiers traités viennent d'utilisateurs : on les confie à des outils
//! éprouvés (ffmpeg, Ghostscript en mode `-dSAFER`, libheif) ou à la crate
//! `image`, écrite en Rust, avec des limites de taille, de mémoire et de temps.

use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits, Rgb, RgbImage};

use super::{Conversion, JobSpec, Media, Task};

/// Conversions simultanées au plus : le serveur est un vieux portable.
const PARALLEL: usize = 2;
/// Côté le plus long d'une vidéo compressée : 1920 px, soit du 1080p.
const VIDEO_MAX_SIDE: u32 = 1920;
/// Côté le plus long d'une photo compressée.
const IMAGE_MAX_SIDE: u32 = 2560;
const JPEG_QUALITY: u8 = 82;
/// Vignette de la liste : 256 px au plus (affichée en 40 px, nette sur un
/// écran de téléphone).
const THUMB_SIDE: u32 = 256;
/// Grand aperçu (photo HEIC, page de PDF, image d'une vidéo).
const PREVIEW_SIDE: u32 = 1600;

/// Signature d'une conversion : produit `spec.output` dans `dir`, en
/// signalant sa progression. L'erreur est un message pour l'utilisateur.
pub type Convert<'a> = &'a (dyn Fn(&JobSpec, &Path, &dyn Fn(u8)) -> Result<(), String> + Sync);

/// Boucle du service. Ne rend jamais la main, sauf erreur au démarrage.
pub fn run(work_dir: &Path) -> anyhow::Result<()> {
    let jobs = work_dir.join("jobs");
    fs::create_dir_all(&jobs)?;
    recover(&jobs);
    let tools = Arc::new(Tools::detect());
    tracing::info!(
        dir = %jobs.display(),
        tonemap_hdr = tools.zscale,
        "service de conversion démarré"
    );

    let running = Arc::new(AtomicUsize::new(0));
    // Une vidéo 4K compressée occupe plus d'un gigaoctet : jamais deux à la
    // fois, sinon la limite de mémoire du service serait dépassée.
    let video = Arc::new(AtomicBool::new(false));
    loop {
        for dir in ready_jobs(&jobs) {
            if running.load(Ordering::SeqCst) >= PARALLEL {
                break;
            }
            let heavy = is_video_compression(&dir);
            if heavy && video.load(Ordering::SeqCst) {
                continue;
            }
            if !claim(&dir) {
                continue;
            }
            running.fetch_add(1, Ordering::SeqCst);
            if heavy {
                video.store(true, Ordering::SeqCst);
            }
            let (running, video, tools) = (running.clone(), video.clone(), tools.clone());
            thread::spawn(move || {
                process_job(&dir, &|spec, dir, progress| {
                    tools.convert(spec, dir, progress)
                });
                if heavy {
                    video.store(false, Ordering::SeqCst);
                }
                running.fetch_sub(1, Ordering::SeqCst);
            });
        }
        thread::sleep(Duration::from_secs(2));
    }
}

fn is_video_compression(dir: &Path) -> bool {
    fs::read(dir.join("job.json"))
        .ok()
        .and_then(|raw| serde_json::from_slice::<JobSpec>(&raw).ok())
        .is_some_and(|spec| spec.task == Task::Compress && spec.media == Media::Video)
}

/// Après un arrêt brutal (coupure, mise à jour, mémoire dépassée), les
/// tâches commencées et jamais finies sont remises en attente : elles
/// repartiront de zéro. Interrompue deux fois, une tâche est abandonnée :
/// sinon un fichier qui fait tomber le service le ferait tomber en boucle.
pub fn recover(jobs: &Path) {
    for dir in job_dirs(jobs) {
        let finished = dir.join("done").exists() || dir.join("error").exists();
        if !dir.join("started").exists() || finished {
            continue;
        }
        if dir.join("attempts").exists() {
            write_atomic(
                &dir,
                "error",
                "Conversion interrompue deux fois : fichier trop lourd pour le serveur ?",
            );
            tracing::warn!(job = %dir.display(), "tâche interrompue deux fois, abandonnée");
            continue;
        }
        write_atomic(&dir, "attempts", "1");
        for name in ["started", "progress"] {
            let _ = fs::remove_file(dir.join(name));
        }
        tracing::info!(job = %dir.display(), "tâche interrompue, remise en attente");
    }
}

fn job_dirs(jobs: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<(u64, PathBuf)> = fs::read_dir(jobs)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let id = entry.file_name().to_str()?.parse::<u64>().ok()?;
            Some((id, entry.path()))
        })
        .collect();
    dirs.sort();
    dirs.into_iter().map(|(_, path)| path).collect()
}

/// Dossiers prêts et pas encore pris, les plus anciens d'abord.
fn ready_jobs(jobs: &Path) -> Vec<PathBuf> {
    job_dirs(jobs)
        .into_iter()
        .filter(|dir| dir.join("job.json").exists() && !dir.join("started").exists())
        .collect()
}

/// Prend une tâche : `started` créé de façon exclusive.
fn claim(dir: &Path) -> bool {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join("started"))
        .is_ok()
}

/// Traite une tâche déjà prise, et finit toujours par `done` ou `error`.
pub fn process_job(dir: &Path, convert: Convert<'_>) {
    let result = (|| {
        let raw = fs::read(dir.join("job.json")).map_err(|_| "Tâche illisible.".to_string())?;
        let spec: JobSpec =
            serde_json::from_slice(&raw).map_err(|_| "Tâche illisible.".to_string())?;
        let progress = |percent: u8| write_atomic(dir, "progress", &percent.min(99).to_string());
        convert(&spec, dir, &progress)?;
        let output = dir.join(&spec.output);
        match fs::metadata(&output) {
            Ok(meta) if meta.len() > 0 => Ok(()),
            _ => Err("Le résultat est vide.".to_string()),
        }
    })();
    match result {
        Ok(()) => write_atomic(dir, "done", ""),
        Err(message) => {
            tracing::warn!(job = %dir.display(), %message, "conversion en échec");
            write_atomic(dir, "error", &message);
        }
    }
}

/// Écrit un petit fichier d'état d'un coup (écriture puis renommage) : dashu
/// ne lit jamais un fichier à moitié écrit.
fn write_atomic(dir: &Path, name: &str, content: &str) {
    let tmp = dir.join(format!(".{name}.tmp"));
    if fs::write(&tmp, content).is_ok() {
        let _ = fs::rename(&tmp, dir.join(name));
    }
}

// ---------------------------------------------------------------------
// Les conversions
// ---------------------------------------------------------------------

/// Outils présents sur la machine.
pub struct Tools {
    /// Filtre ffmpeg `zscale` (bibliothèque zimg) : nécessaire pour ramener
    /// une vidéo HDR d'iPhone en couleurs normales.
    pub zscale: bool,
}

impl Tools {
    pub fn detect() -> Self {
        let zscale = Command::new("ffmpeg")
            .args(["-hide_banner", "-filters"])
            .output()
            .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).contains(" zscale "));
        Self { zscale }
    }

    pub fn convert(&self, spec: &JobSpec, dir: &Path, progress: &dyn Fn(u8)) -> Result<(), String> {
        match spec.task {
            Task::Compress => self.compress(spec, dir, progress),
            Task::Thumbnail => self.thumbnail(spec, dir),
            Task::Convert => self.transform(spec, dir, progress),
        }
    }

    /// Conversion du menu « Convertir » : la sortie `spec.to`, dans sa
    /// définition d'origine (seule la compression réduit).
    fn transform(&self, spec: &JobSpec, dir: &Path, progress: &dyn Fn(u8)) -> Result<(), String> {
        let input = dir.join(&spec.input);
        let output = dir.join(&spec.output);
        let impossible = || "Conversion impossible pour ce fichier.".to_string();
        match (spec.to.ok_or_else(impossible)?, spec.media) {
            (to @ (Conversion::Jpg | Conversion::Png | Conversion::Webp), media) => {
                let image = match media {
                    Media::Image => load_image(&input)?,
                    Media::Heic => {
                        let decoded = dir.join("decoded.png");
                        let mut cmd = Command::new("heif-dec");
                        cmd.arg(&input).arg(&decoded);
                        run_tool(cmd, Duration::from_secs(120), |_| {})
                            .map_err(|e| e.replace("La conversion", "La lecture de la photo"))?;
                        let image = load_image(&decoded);
                        let _ = fs::remove_file(&decoded);
                        image?
                    }
                    _ => return Err(impossible()),
                };
                save_as(&image, &output, to)
            }
            (Conversion::Mp4, Media::Video) => {
                let probe = probe(&input)?;
                let filters = full_size_filters(probe.hdr && self.zscale);
                let mut cmd = Command::new("ffmpeg");
                cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"])
                    .arg(&input)
                    .args(video_args(&filters))
                    .arg(&output);
                run_tool(cmd, Duration::from_secs(3600), |line| {
                    if let Some(percent) = ffmpeg_progress(line, probe.duration) {
                        progress(percent);
                    }
                })
            }
            (Conversion::Mp3, Media::Video | Media::Audio) => {
                let probe = probe(&input)?;
                let mut cmd = Command::new("ffmpeg");
                cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"])
                    .arg(&input)
                    .args(mp3_args())
                    .arg(&output);
                run_tool(cmd, Duration::from_secs(1800), |line| {
                    if let Some(percent) = ffmpeg_progress(line, probe.duration) {
                        progress(percent);
                    }
                })
                .map_err(|e| {
                    e.replace(
                        "fichier illisible ou abîmé",
                        "fichier sans son, illisible ou abîmé",
                    )
                })
            }
            (Conversion::Pages, Media::Pdf) => {
                let (first, last) = spec.pages.ok_or_else(impossible)?;
                let mut cmd = Command::new("gs");
                cmd.args(pages_args(&output, first, last)).arg(&input);
                run_tool(cmd, Duration::from_secs(600), |_| {})
            }
            _ => Err(impossible()),
        }
    }

    fn compress(&self, spec: &JobSpec, dir: &Path, progress: &dyn Fn(u8)) -> Result<(), String> {
        let input = dir.join(&spec.input);
        let output = dir.join(&spec.output);
        match spec.media {
            Media::Video => self.video(&input, &output, progress),
            Media::Image => compress_image(&input, &output),
            Media::Audio => Err("Conversion impossible pour ce fichier.".into()),
            Media::Heic => {
                // libheif applique la rotation enregistrée par l'iPhone.
                let decoded = dir.join("decoded.png");
                let mut cmd = Command::new("heif-dec");
                cmd.arg(&input).arg(&decoded);
                run_tool(cmd, Duration::from_secs(120), |_| {})
                    .map_err(|e| e.replace("La conversion", "La lecture de la photo"))?;
                let result = compress_image(&decoded, &output);
                let _ = fs::remove_file(&decoded);
                result
            }
            Media::Pdf => {
                let mut cmd = Command::new("gs");
                cmd.args(pdf_args(&output)).arg(&input);
                run_tool(cmd, Duration::from_secs(600), |_| {})
            }
        }
    }

    /// Vignette (`spec.output`) et, s'il est demandé, grand aperçu
    /// (`spec.preview`) : la photo, la première page du PDF, ou une image de
    /// la vidéo prise au début.
    fn thumbnail(&self, spec: &JobSpec, dir: &Path) -> Result<(), String> {
        let input = dir.join(&spec.input);
        let image = match spec.media {
            Media::Image => load_image(&input)?,
            Media::Audio => return Err("Pas de vignette pour un son.".into()),
            Media::Heic => {
                let decoded = dir.join("decoded.png");
                let mut cmd = Command::new("heif-dec");
                cmd.arg(&input).arg(&decoded);
                run_tool(cmd, Duration::from_secs(120), |_| {})?;
                let image = load_image(&decoded);
                let _ = fs::remove_file(&decoded);
                image?
            }
            Media::Pdf => {
                let page = dir.join("page.png");
                let mut cmd = Command::new("gs");
                cmd.args(first_page_args(&page)).arg(&input);
                run_tool(cmd, Duration::from_secs(120), |_| {})?;
                let image = load_image(&page);
                let _ = fs::remove_file(&page);
                image?
            }
            Media::Video => {
                let probe = probe(&input)?;
                let frame = dir.join("frame.jpg");
                let at = (probe.duration / 3.0).min(1.0);
                let mut cmd = Command::new("ffmpeg");
                cmd.args([
                    "-nostdin",
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-y",
                    "-ss",
                ])
                .arg(format!("{at:.2}"))
                .arg("-i")
                .arg(&input)
                .args(frame_args(probe.hdr && self.zscale))
                .arg(&frame);
                run_tool(cmd, Duration::from_secs(120), |_| {})?;
                let image = load_image(&frame);
                let _ = fs::remove_file(&frame);
                image?
            }
        };
        if let Some(preview) = &spec.preview {
            save_jpeg(
                &fit(image.clone(), PREVIEW_SIDE),
                &dir.join(preview),
                JPEG_QUALITY,
            )?;
        }
        save_jpeg(
            &image.thumbnail(THUMB_SIDE, THUMB_SIDE),
            &dir.join(&spec.output),
            75,
        )
    }

    fn video(&self, input: &Path, output: &Path, progress: &dyn Fn(u8)) -> Result<(), String> {
        let probe = probe(input)?;
        let filters = video_filters(probe.hdr && self.zscale);
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-nostdin", "-hide_banner", "-loglevel", "error", "-y", "-i"])
            .arg(input)
            .args(video_args(&filters))
            .arg(output);
        let duration = probe.duration;
        run_tool(cmd, Duration::from_secs(3600), |line| {
            if let Some(percent) = ffmpeg_progress(line, duration) {
                progress(percent);
            }
        })
    }
}

/// Arguments ffmpeg d'une vidéo compressée, entre l'entrée et la sortie.
pub fn video_args(filters: &str) -> Vec<String> {
    [
        "-map",
        "0:v:0",
        "-map",
        "0:a:0?",
        "-vf",
        filters,
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-crf",
        "23",
        "-c:a",
        "aac",
        "-b:a",
        "128k",
        "-movflags",
        "+faststart",
        "-progress",
        "pipe:1",
        "-nostats",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Filtres vidéo : 1080p au plus (dans les deux sens, pour les vidéos
/// verticales), et pour une vidéo HDR, retour aux couleurs standard (SDR).
pub fn video_filters(tonemap: bool) -> String {
    let scale = format!(
        "scale=w='min({VIDEO_MAX_SIDE},iw)':h='min({VIDEO_MAX_SIDE},ih)':force_original_aspect_ratio=decrease:force_divisible_by=2"
    );
    if tonemap {
        format!(
            "zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv,{scale},format=yuv420p"
        )
    } else {
        format!("{scale},format=yuv420p")
    }
}

/// Filtres d'une vidéo convertie en MP4 sans être réduite : dimensions
/// paires (exigées par H.264), et pour une vidéo HDR, retour au SDR.
pub fn full_size_filters(tonemap: bool) -> String {
    let even = "scale=w=trunc(iw/2)*2:h=trunc(ih/2)*2";
    if tonemap {
        format!(
            "zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv,{even},format=yuv420p"
        )
    } else {
        format!("{even},format=yuv420p")
    }
}

/// Arguments ffmpeg du son en MP3 (qualité variable, autour de 190 kb/s).
pub fn mp3_args() -> Vec<String> {
    [
        "-map",
        "0:a:0",
        "-vn",
        "-c:a",
        "libmp3lame",
        "-q:a",
        "2",
        "-progress",
        "pipe:1",
        "-nostats",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Arguments Ghostscript : les pages `first` à `last` dans un nouveau PDF.
/// Deux nombres déjà vérifiés (voir `parse_pages`), jamais du texte tapé.
pub fn pages_args(output: &Path, first: u32, last: u32) -> Vec<String> {
    vec![
        "-sDEVICE=pdfwrite".into(),
        format!("-dFirstPage={first}"),
        format!("-dLastPage={last}"),
        "-dSAFER".into(),
        "-dNOPAUSE".into(),
        "-dBATCH".into(),
        "-dQUIET".into(),
        format!("-sOutputFile={}", output.display()),
    ]
}

/// Arguments ffmpeg pour extraire une image d'une vidéo (1280 px au plus).
pub fn frame_args(tonemap: bool) -> Vec<String> {
    let scale = "scale=w='min(1280,iw)':h='min(1280,ih)':force_original_aspect_ratio=decrease";
    let filters = if tonemap {
        format!(
            "zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv,{scale},format=yuvj420p"
        )
    } else {
        scale.to_string()
    };
    ["-frames:v", "1", "-vf", &filters, "-q:v", "3"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Arguments Ghostscript : la première page d'un PDF en PNG (110 dpi).
pub fn first_page_args(output: &Path) -> Vec<String> {
    vec![
        "-sDEVICE=png16m".into(),
        "-r110".into(),
        "-dFirstPage=1".into(),
        "-dLastPage=1".into(),
        "-dTextAlphaBits=4".into(),
        "-dGraphicsAlphaBits=4".into(),
        "-dSAFER".into(),
        "-dNOPAUSE".into(),
        "-dBATCH".into(),
        "-dQUIET".into(),
        format!("-sOutputFile={}", output.display()),
    ]
}

/// Arguments Ghostscript : réglage « ebook », images à 150 dpi.
pub fn pdf_args(output: &Path) -> Vec<String> {
    vec![
        "-sDEVICE=pdfwrite".into(),
        "-dCompatibilityLevel=1.5".into(),
        "-dPDFSETTINGS=/ebook".into(),
        "-dSAFER".into(),
        "-dNOPAUSE".into(),
        "-dBATCH".into(),
        "-dQUIET".into(),
        format!("-sOutputFile={}", output.display()),
    ]
}

/// Progression lue dans une ligne de `-progress pipe:1` de ffmpeg.
pub fn ffmpeg_progress(line: &str, duration: f64) -> Option<u8> {
    let micros: f64 = line.strip_prefix("out_time_us=")?.trim().parse().ok()?;
    (duration > 0.0).then(|| ((micros / 1e6 / duration) * 100.0).clamp(0.0, 99.0) as u8)
}

struct Probe {
    /// Durée en secondes (0 si inconnue).
    duration: f64,
    /// Vidéo HDR (HLG ou PQ), comme celles des iPhone récents.
    hdr: bool,
}

fn probe(input: &Path) -> Result<Probe, String> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=color_transfer:format=duration",
            "-of",
            "default=noprint_wrappers=1",
        ])
        .arg(input)
        .stdin(Stdio::null())
        .output()
        .map_err(|_| "Outil de conversion absent du serveur (ffprobe).".to_string())?;
    if !out.status.success() {
        return Err("Vidéo illisible.".into());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut probe = Probe {
        duration: 0.0,
        hdr: false,
    };
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("duration=") {
            probe.duration = value.trim().parse().unwrap_or(0.0);
        } else if let Some(value) = line.strip_prefix("color_transfer=") {
            probe.hdr = matches!(value.trim(), "arib-std-b67" | "smpte2084");
        }
    }
    Ok(probe)
}

/// Lance un outil avec une limite de temps. `on_line` reçoit chaque ligne de
/// sa sortie standard (la progression de ffmpeg) ; la sortie d'erreur part
/// dans les logs en cas d'échec.
fn run_tool(
    mut cmd: Command,
    timeout: Duration,
    mut on_line: impl FnMut(&str),
) -> Result<(), String> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|_| format!("Outil de conversion absent du serveur ({program})."))?;

    let (tx, rx) = mpsc::channel::<String>();
    if let Some(stdout) = child.stdout.take() {
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
    }
    let stderr = child.stderr.take().map(|mut stderr| {
        thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            // Seule la fin compte, et on ne garde pas des mégaoctets.
            let tail: VecDeque<&str> = text.lines().rev().take(8).collect();
            tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
        })
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(line) => on_line(&line),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => thread::sleep(Duration::from_millis(200)),
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Trop long : la conversion a été arrêtée.".into());
            }
            Ok(None) => {}
            Err(_) => return Err("La conversion a échoué.".into()),
        }
    };
    while let Ok(line) = rx.try_recv() {
        on_line(&line);
    }
    if status.success() {
        return Ok(());
    }
    let detail = stderr.and_then(|h| h.join().ok()).unwrap_or_default();
    tracing::warn!(%program, %status, %detail, "outil de conversion en échec");
    Err("La conversion a échoué : fichier illisible ou abîmé.".into())
}

/// Photo : orientation EXIF appliquée, 2560 px au plus sur le grand côté,
/// transparence posée sur du blanc, JPEG qualité 82.
pub fn compress_image(input: &Path, output: &Path) -> Result<(), String> {
    save_jpeg(
        &fit(load_image(input)?, IMAGE_MAX_SIDE),
        output,
        JPEG_QUALITY,
    )
}

/// Lit une image, avec l'orientation enregistrée par l'appareil appliquée.
fn load_image(path: &Path) -> Result<DynamicImage, String> {
    let unreadable = |_| "Image illisible.".to_string();
    let mut reader = ImageReader::open(path)
        .map_err(unreadable)?
        .with_guessed_format()
        .map_err(unreadable)?;
    // Contre les « bombes » : une petite image qui se décompresse en géant.
    let mut limits = Limits::default();
    limits.max_image_width = Some(20_000);
    limits.max_image_height = Some(20_000);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);

    let mut decoder = reader
        .into_decoder()
        .map_err(|_| "Image illisible.".to_string())?;
    let orientation = decoder.orientation().ok();
    let mut image = DynamicImage::from_decoder(decoder)
        .map_err(|_| "Image illisible ou trop grande.".to_string())?;
    if let Some(orientation) = orientation {
        image.apply_orientation(orientation);
    }
    Ok(image)
}

/// Réduit une image pour qu'elle tienne dans un carré de `side` pixels.
fn fit(image: DynamicImage, side: u32) -> DynamicImage {
    if image.width() > side || image.height() > side {
        image.resize(side, side, FilterType::Lanczos3)
    } else {
        image
    }
}

/// Image convertie, dans sa définition d'origine. Le JPEG perd la
/// transparence (posée sur du blanc) ; PNG et WebP (sans perte) la gardent.
pub fn save_as(image: &DynamicImage, output: &Path, to: Conversion) -> Result<(), String> {
    let format = match to {
        Conversion::Jpg => return save_jpeg(image, output, 90),
        Conversion::Png => ImageFormat::Png,
        Conversion::Webp => ImageFormat::WebP,
        _ => return Err("Conversion impossible pour ce fichier.".into()),
    };
    // Le WebP sans perte ne connaît que le RGB et le RGBA en 8 bits.
    let image = match image.color().has_alpha() {
        true => DynamicImage::ImageRgba8(image.to_rgba8()),
        false => DynamicImage::ImageRgb8(image.to_rgb8()),
    };
    let file =
        fs::File::create(output).map_err(|_| "Écriture du résultat impossible.".to_string())?;
    image
        .write_to(&mut BufWriter::new(file), format)
        .map_err(|_| "Écriture du résultat impossible.".to_string())
}

fn save_jpeg(image: &DynamicImage, output: &Path, quality: u8) -> Result<(), String> {
    let rgb = flatten_on_white(image);
    let file =
        fs::File::create(output).map_err(|_| "Écriture du résultat impossible.".to_string())?;
    let mut writer = BufWriter::new(file);
    JpegEncoder::new_with_quality(&mut writer, quality)
        .encode_image(&rgb)
        .map_err(|_| "Écriture du résultat impossible.".to_string())
}

/// JPEG n'a pas de transparence : les zones transparentes deviennent blanches.
fn flatten_on_white(image: &DynamicImage) -> RgbImage {
    if !image.color().has_alpha() {
        return image.to_rgb8();
    }
    let rgba = image.to_rgba8();
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        let blend =
            |c: u8| ((u16::from(c) * u16::from(a) + 255 * (255 - u16::from(a))) / 255) as u8;
        Rgb([blend(r), blend(g), blend(b)])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progression_de_ffmpeg() {
        assert_eq!(ffmpeg_progress("out_time_us=5000000", 10.0), Some(50));
        assert_eq!(ffmpeg_progress("out_time_us=12000000", 10.0), Some(99));
        assert_eq!(ffmpeg_progress("out_time_us=N/A", 10.0), None);
        assert_eq!(ffmpeg_progress("frame=12", 10.0), None);
        assert_eq!(ffmpeg_progress("out_time_us=1000", 0.0), None);
    }

    #[test]
    fn filtres_video() {
        let sdr = video_filters(false);
        assert!(sdr.starts_with("scale=w='min(1920,iw)'"));
        assert!(sdr.ends_with("format=yuv420p"));
        assert!(!sdr.contains("tonemap"));
        let hdr = video_filters(true);
        assert!(hdr.starts_with("zscale=t=linear"));
        assert!(hdr.contains("tonemap=tonemap=hable"));
    }

    #[test]
    fn arguments_ghostscript() {
        let args = pdf_args(Path::new("/tmp/out.pdf"));
        assert!(args.contains(&"-dSAFER".to_string()));
        assert!(args.contains(&"-dPDFSETTINGS=/ebook".to_string()));
        assert_eq!(args.last().unwrap(), "-sOutputFile=/tmp/out.pdf");
    }

    #[test]
    fn photo_reduite_et_transparence_sur_blanc() {
        let dir = std::env::temp_dir().join(format!("dashu-photo-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let input = dir.join("grande.png");
        let output = dir.join("sortie.jpg");
        image::RgbaImage::from_pixel(4000, 1000, image::Rgba([10, 20, 30, 0]))
            .save(&input)
            .unwrap();

        compress_image(&input, &output).unwrap();
        let result = image::open(&output).unwrap();
        assert_eq!((result.width(), result.height()), (2560, 640));
        // Entièrement transparente : elle devient blanche.
        let pixel = result.to_rgb8().get_pixel(10, 10).0;
        assert!(pixel.iter().all(|&c| c > 250), "{pixel:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn photo_convertie_en_png_jpeg_et_webp() {
        let dir = std::env::temp_dir().join(format!("dashu-formats-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let input = dir.join("photo.png");
        image::RgbaImage::from_pixel(300, 200, image::Rgba([200, 80, 40, 255]))
            .save(&input)
            .unwrap();
        let image = load_image(&input).unwrap();
        for (to, format) in [
            (Conversion::Jpg, ImageFormat::Jpeg),
            (Conversion::Png, ImageFormat::Png),
            (Conversion::Webp, ImageFormat::WebP),
        ] {
            let output = dir.join(format!("sortie.{}", to.key()));
            save_as(&image, &output, to).unwrap();
            let reader = ImageReader::open(&output)
                .unwrap()
                .with_guessed_format()
                .unwrap();
            assert_eq!(reader.format(), Some(format), "{to:?}");
            // Aucune réduction : la définition d'origine.
            let back = reader.decode().unwrap();
            assert_eq!((back.width(), back.height()), (300, 200), "{to:?}");
        }
        assert!(save_as(&image, &dir.join("x.mp3"), Conversion::Mp3).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn arguments_des_conversions() {
        let pages = pages_args(Path::new("/tmp/out.pdf"), 2, 4);
        assert!(pages.contains(&"-dFirstPage=2".to_string()));
        assert!(pages.contains(&"-dLastPage=4".to_string()));
        assert!(pages.contains(&"-dSAFER".to_string()));
        assert!(mp3_args().contains(&"libmp3lame".to_string()));
        assert!(full_size_filters(false).starts_with("scale=w=trunc(iw/2)*2"));
        assert!(full_size_filters(true).contains("tonemap=tonemap=hable"));
    }

    #[test]
    fn image_illisible() {
        let dir = std::env::temp_dir().join(format!("dashu-illisible-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let input = dir.join("faux.jpg");
        fs::write(&input, b"pas une image").unwrap();
        let error = compress_image(&input, &dir.join("sortie.jpg")).unwrap_err();
        assert_eq!(error, "Image illisible.");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tache_reprise_apres_coupure() {
        let jobs = std::env::temp_dir().join(format!("dashu-reprise-{}", std::process::id()));
        let dir = jobs.join("7");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("job.json"), "{}").unwrap();
        fs::write(dir.join("started"), "").unwrap();
        fs::write(dir.join("progress"), "40").unwrap();
        assert!(ready_jobs(&jobs).is_empty());

        recover(&jobs);
        assert_eq!(ready_jobs(&jobs), vec![dir.clone()]);
        assert!(claim(&dir));
        assert!(!claim(&dir), "une tâche ne se prend qu'une fois");
        let _ = fs::remove_dir_all(&jobs);
    }
}
