//! Outils (v2) : compression d'un fichier. dashu prépare la tâche, le
//! service de conversion la traite (ici avec de faux convertisseurs : les
//! vraies conversions de photos sont testées dans src/tools/worker.rs, celle
//! des vidéos par le test ignoré en fin de fichier), puis dashu range le
//! résultat.

mod common;

use std::path::{Path, PathBuf};

use axum::http::StatusCode;
use common::{ALICE, BOB, TestApp, between};
use dashu::tools::{self, JobSpec, Media, Task, worker};

/// Identifiant du fichier nommé `name` dans la liste.
fn id_of(page: &str, name: &str) -> i64 {
    let at = page
        .find(&format!("<span>{name}</span>"))
        .unwrap_or_else(|| panic!("{name} absent de la liste"));
    let before = &page[..at];
    let start = before.rfind("class=\"file-name\" href=\"/files/").unwrap()
        + "class=\"file-name\" href=\"/files/".len();
    before[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap()
}

/// Envoie un fichier, lance sa compression, et renvoie le dossier de la tâche.
async fn start_compression(
    app: &TestApp,
    client: &mut common::Client,
    name: &str,
    content: &[u8],
) -> PathBuf {
    client.get("/files?dossier=espace").await;
    let sent = client.upload(name, content).await;
    assert_eq!(sent.status, StatusCode::CREATED, "{}", sent.body);
    let id = id_of(&client.get("/files?dossier=espace").await.body, name);
    let launched = client.post(&format!("/files/{id}/compress"), &[]).await;
    assert_eq!(launched.status, StatusCode::SEE_OTHER, "{}", launched.body);
    let job: i64 = sqlx::query_scalar("SELECT MAX(id) FROM jobs WHERE kind = 'compress'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    tools::job_dir(&app.work_dir, job)
}

/// Faux convertisseur : un résultat de `size` octets, après une progression.
fn fake(size: usize) -> impl Fn(&JobSpec, &Path, &dyn Fn(u8)) -> Result<(), String> + Sync {
    move |spec, dir, progress| {
        progress(50);
        std::fs::write(dir.join(&spec.output), vec![1u8; size]).map_err(|e| e.to_string())
    }
}

async fn job_status(app: &TestApp) -> (String, Option<String>) {
    sqlx::query_as(
        "SELECT status, error FROM jobs WHERE kind = 'compress' ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap()
}

#[tokio::test]
async fn compresser_prepare_une_tache_pour_le_service() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let content = b"fausse video".repeat(100);
    let dir = start_compression(&app, &mut alice, "Vacances.MOV", &content).await;

    // Une copie du fichier, puis la description de la tâche.
    assert_eq!(std::fs::read(dir.join("input.mov")).unwrap(), content);
    let spec: JobSpec =
        serde_json::from_slice(&std::fs::read(dir.join("job.json")).unwrap()).unwrap();
    assert_eq!(
        spec,
        JobSpec {
            task: Task::Compress,
            media: Media::Video,
            input: "input.mov".into(),
            output: "output.mp4".into(),
            preview: None,
            to: None,
            pages: None,
        }
    );
    assert_eq!(job_status(&app).await.0, "queued");

    // La page le montre, et se redemandera tant que la tâche tourne.
    let page = alice.get("/files?dossier=espace").await;
    assert!(page.body.contains("En attente"));
    // Perso montre les tâches de tous les espaces : le suivi aussi.
    assert!(
        page.body
            .contains("hx-get=\"/files/jobs?active=1&amp;all=1\"")
    );
}

#[tokio::test]
async fn la_copie_compressee_rejoint_la_liste() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let dir = start_compression(&app, &mut alice, "Vacances.MOV", &[9u8; 4096]).await;

    // Le service prend la tâche et la traite.
    std::fs::write(dir.join("started"), "").unwrap();
    worker::process_job(&dir, &fake(1000));
    assert!(dir.join("done").exists());

    tools::collect(&app.state).await.unwrap();
    assert_eq!(job_status(&app).await.0, "done");
    assert!(!dir.exists(), "le dossier de travail est effacé");

    let page = alice.get("/files?dossier=espace").await;
    assert!(page.body.contains("<span>Vacances (compressé).mp4</span>"));
    assert!(
        page.body.contains("<span>Vacances.MOV</span>"),
        "l'original est gardé"
    );
    assert!(page.body.contains("4 Ko → 1000 o"), "{}", page.body);

    // Le suivi signale la fin à la page, qui recharge alors la liste.
    let fragment = alice.get_fragment("/files/jobs?active=1").await;
    assert_eq!(fragment.header("hx-trigger"), "files-changed");
    assert!(!fragment.body.contains("every 2s"));

    let id = id_of(&page.body, "Vacances (compressé).mp4");
    let file = alice.get(&format!("/files/{id}/download")).await;
    assert_eq!(file.bytes, vec![1u8; 1000]);
}

#[tokio::test]
async fn pas_de_gain_pas_de_copie() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let dir = start_compression(&app, &mut alice, "deja.pdf", &[9u8; 500]).await;
    std::fs::write(dir.join("started"), "").unwrap();
    worker::process_job(&dir, &fake(800));
    tools::collect(&app.state).await.unwrap();

    let (status, error) = job_status(&app).await;
    assert_eq!(status, "failed");
    assert!(error.unwrap().contains("Pas de gain"));
    let page = alice.get("/files?dossier=espace").await;
    assert!(!page.body.contains("(compressé)"));
    assert_eq!(app.stored_files().len(), 1, "seul l'original reste");
}

#[tokio::test]
async fn un_echec_du_service_est_affiche() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let dir = start_compression(&app, &mut alice, "abimee.mp4", &[9u8; 500]).await;
    std::fs::write(dir.join("started"), "").unwrap();
    worker::process_job(&dir, &|_: &JobSpec, _: &Path, _: &dyn Fn(u8)| {
        Err("Vidéo illisible.".to_string())
    });
    tools::collect(&app.state).await.unwrap();

    let (status, error) = job_status(&app).await;
    assert_eq!(status, "failed");
    assert_eq!(error.as_deref(), Some("Vidéo illisible."));
    let fragment = alice.get_fragment("/files/jobs").await;
    assert!(fragment.body.contains("Vidéo illisible."));
}

#[tokio::test]
async fn la_progression_remonte_jusqu_a_la_page() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let dir = start_compression(&app, &mut alice, "long.mp4", &[9u8; 500]).await;
    std::fs::write(dir.join("started"), "").unwrap();
    std::fs::write(dir.join("progress"), "47").unwrap();
    tools::collect(&app.state).await.unwrap();

    assert_eq!(job_status(&app).await.0, "running");
    let fragment = alice.get_fragment("/files/jobs?active=1").await;
    assert!(fragment.body.contains("47 %"));
    assert!(fragment.body.contains("value=\"47\""));
    assert_eq!(fragment.header("hx-trigger"), "");
}

#[tokio::test]
async fn seuls_videos_photos_et_pdf_se_compressent() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.get("/files?dossier=espace").await;
    alice.upload("archive.zip", b"PK").await;
    alice.upload("cours.pdf", b"%PDF").await;
    let page = alice.get("/files?dossier=espace").await;
    // Le menu « Convertir » de la ligne, avec la compression.
    assert!(page.body.contains("aria-label=\"Convertir « cours.pdf »\""));
    assert!(
        page.body
            .contains("<input type=\"hidden\" name=\"to\" value=\"compress\">")
    );
    assert!(
        !page
            .body
            .contains("aria-label=\"Convertir « archive.zip »\"")
    );

    let id = id_of(&page.body, "archive.zip");
    let refused = alice.post(&format!("/files/{id}/compress"), &[]).await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn on_ne_compresse_pas_le_fichier_d_un_autre() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.get("/files?dossier=espace").await;
    alice.upload("perso.mp4", b"video").await;
    let id = id_of(&alice.get("/files?dossier=espace").await.body, "perso.mp4");

    let mut bob = app.logged_in(BOB).await;
    bob.get("/files?dossier=espace").await;
    let refused = bob.post(&format!("/files/{id}/compress"), &[]).await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
    let jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE kind = 'compress'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(jobs, 0);
    // Et le suivi de Bob ne montre rien d'Alice.
    assert!(
        between(
            &bob.get("/files?dossier=espace").await.body,
            "id=\"file-jobs\"",
            "</section>"
        )
        .is_some_and(|section| !section.contains("perso.mp4"))
    );
}

#[tokio::test]
async fn sans_dossier_de_travail_pas_de_compression() {
    let app = TestApp::without_files().await;
    let mut alice = app.logged_in(ALICE).await;
    let page = alice.get("/files?dossier=espace").await;
    assert!(!page.body.contains("Compresser"));
    assert_eq!(
        alice.post("/files/1/compress", &[]).await.status,
        StatusCode::NOT_FOUND
    );
}

/// Vraie vidéo, vrai ffmpeg : lancé à la main (`cargo test -- --ignored`),
/// sur une machine où ffmpeg est installé.
#[tokio::test]
#[ignore]
async fn une_vraie_video_est_compressee_par_ffmpeg() {
    let app = TestApp::new().await;
    let dir = app.work_dir.join("jobs").join("1");
    std::fs::create_dir_all(&dir).unwrap();
    let made = std::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=3840x2160:rate=30:duration=3",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=3",
            "-c:v",
            "libx264",
            "-crf",
            "5",
            "-c:a",
            "aac",
        ])
        .arg(dir.join("input.mov"))
        .status()
        .expect("ffmpeg installé");
    assert!(made.success());
    let spec = JobSpec {
        task: Task::Compress,
        media: Media::Video,
        input: "input.mov".into(),
        output: "output.mp4".into(),
        preview: None,
        to: None,
        pages: None,
    };
    std::fs::write(dir.join("job.json"), serde_json::to_vec(&spec).unwrap()).unwrap();

    let tools = worker::Tools::detect();
    let seen = std::sync::Mutex::new(Vec::new());
    worker::process_job(&dir, &|spec, dir, _progress| {
        tools.convert(spec, dir, &|p| seen.lock().unwrap().push(p))
    });
    assert!(
        dir.join("done").exists(),
        "{:?}",
        std::fs::read_to_string(dir.join("error"))
    );
    let input = std::fs::metadata(dir.join("input.mov")).unwrap().len();
    let output = std::fs::metadata(dir.join("output.mp4")).unwrap().len();
    assert!(output < input, "{output} >= {input}");
    assert!(!seen.lock().unwrap().is_empty(), "progression signalée");
    let probe = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0",
        ])
        .arg(dir.join("output.mp4"))
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&probe.stdout).trim(), "1920,1080");
}
