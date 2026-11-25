//! Téléchargeur de liens (SPEC v2, « Outils : téléchargeur de liens »). dashu
//! vérifie le lien et prépare la tâche ; le service `dashu-download` est joué
//! ici à la main (yt-dlp ne tourne pas dans les tests : ses arguments sont
//! vérifiés dans src/tools/telechargeur.rs) ; dashu range le résultat dans
//! Temporaire, marqué « venu d'un lien ».

mod common;

use std::path::{Path, PathBuf};

use axum::http::{Method, StatusCode};
use common::{ALICE, API_TOKEN, BOB, MAX_UPLOAD, TestApp};
use dashu::tools::{self, JobSpec, telechargeur, worker};
use serde_json::{Value, json};

const VIDEO: &str = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";

/// Le service dit qu'il tourne, pare-feu compris.
fn service_ready(app: &TestApp) {
    let dir = app.work_dir.join(telechargeur::DIR);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("service"), "actif").unwrap();
}

async fn downloads(app: &TestApp) -> Vec<(i64, String, Option<String>)> {
    sqlx::query_as("SELECT id, status, error FROM jobs WHERE kind = 'download' ORDER BY id")
        .fetch_all(&app.state.db)
        .await
        .unwrap()
}

/// Le service a fini : `output.<ext>`, le titre, puis `done`.
fn finish(dir: &Path, ext: &str, title: &str, content: &[u8]) {
    std::fs::write(dir.join("started"), "").unwrap();
    std::fs::write(dir.join(format!("output.{ext}")), content).unwrap();
    std::fs::write(dir.join("title"), format!("{title}\n")).unwrap();
    std::fs::write(dir.join("done"), "").unwrap();
}

async fn launch(client: &mut common::Client, url: &str, to: &str) -> String {
    let sent = client
        .post("/tools/download", &[("url", url), ("to", to)])
        .await;
    assert_eq!(sent.status, StatusCode::SEE_OTHER, "{}", sent.body);
    assert_eq!(sent.location(), "/tools");
    client.get("/tools").await.body
}

fn job_dir(app: &TestApp, id: i64) -> PathBuf {
    telechargeur::job_dir(&app.work_dir, id)
}

#[tokio::test]
async fn reserve_au_compte_de_downloader_email() {
    let app = TestApp::with_downloader().await;
    service_ready(&app);

    let mut alice = app.logged_in(ALICE).await;
    let page = alice.get("/tools").await.body;
    assert!(page.contains("action=\"/tools/download\""));
    assert!(page.contains("Depuis un lien"));

    // Pour tout autre compte, l'outil n'existe pas.
    let mut bob = app.logged_in(BOB).await;
    assert!(!bob.get("/tools").await.body.contains("/tools/download"));
    let refused = bob
        .post("/tools/download", &[("url", VIDEO), ("to", "mp4")])
        .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);

    // Sans DOWNLOADER_EMAIL, pour personne.
    let plain = TestApp::new().await;
    let mut alice = plain.logged_in(ALICE).await;
    assert!(!alice.get("/tools").await.body.contains("/tools/download"));
    let refused = alice
        .post("/tools/download", &[("url", VIDEO), ("to", "mp4")])
        .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);

    // Ni dans l'API, ni dans le connecteur.
    let (status, _) = app
        .api(
            Method::POST,
            "/api/v1/tools/download",
            Some(json!({ "url": VIDEO, "to": "mp4" })),
            Some(API_TOKEN),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(downloads(&app).await.is_empty());
}

#[tokio::test]
async fn un_lien_refuse_ne_cree_rien() {
    let app = TestApp::with_downloader().await;
    let mut alice = app.logged_in(ALICE).await;

    // Service arrêté : la page le dit, et rien ne part.
    let page = alice.get("/tools").await.body;
    assert!(page.contains("ne tourne pas sur le serveur"), "{page}");
    let page = launch(&mut alice, VIDEO, "mp4").await;
    assert!(page.contains("ne tourne pas sur le serveur"));

    // Service sans pare-feu : pareil.
    let dir = app.work_dir.join(telechargeur::DIR);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("service"), "sans pare-feu").unwrap();
    let page = launch(&mut alice, VIDEO, "mp4").await;
    assert!(page.contains("son pare-feu n"), "{page}");

    service_ready(&app);
    for (url, to) in [
        ("https://www.dailymotion.com/video/abc", "mp4"),
        ("https://192.168.1.10/video.mp4", "mp4"),
        ("https://localhost/watch?v=abc", "mp3"),
        ("https://youtube.com.example.com/watch?v=abc", "mp4"),
        (
            "https://example.com/?next=https://www.youtube.com/watch",
            "mp4",
        ),
        ("http://www.youtube.com/watch?v=abc", "mp4"),
        ("https://www.youtube.com/", "mp4"),
        ("pas un lien", "mp4"),
        (VIDEO, "avi"),
    ] {
        let page = launch(&mut alice, url, to).await;
        assert!(page.contains("toast toast-error"), "{url} {to}");
    }
    assert!(downloads(&app).await.is_empty());
    let entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name() != "service")
        .collect();
    assert!(entries.is_empty(), "aucun dossier de téléchargement");
}

#[tokio::test]
async fn un_lien_arrive_dans_temporaire_et_ne_se_partage_pas() {
    let app = TestApp::with_downloader().await;
    service_ready(&app);
    let mut alice = app.logged_in(ALICE).await;

    let page = launch(&mut alice, &format!(" {VIDEO}&t=42#debut "), "mp4").await;
    assert!(page.contains("Téléchargement lancé"));
    assert!(page.contains("Vidéo depuis un lien"), "suivi dans la page");
    let jobs = downloads(&app).await;
    assert_eq!(jobs.len(), 1);
    let (id, status, _) = &jobs[0];
    assert_eq!(status, "queued");

    // Ce que lira le service : le lien réécrit, le format, la taille maximale.
    let dir = job_dir(&app, *id);
    let spec: Value =
        serde_json::from_slice(&std::fs::read(dir.join("job.json")).unwrap()).unwrap();
    assert_eq!(
        spec,
        json!({ "url": format!("{VIDEO}&t=42"), "to": "mp4", "max_bytes": MAX_UPLOAD })
    );

    // Le service a fini : dashu range le résultat dans Temporaire.
    finish(&dir, "mp4", "Ma vidéo : épisode 1/2", &[7u8; 2048]);
    tools::collect(&app.state).await.unwrap();
    assert_eq!(downloads(&app).await[0].1, "done");
    assert!(!dir.exists(), "le dossier de travail est effacé");
    let (file, name, from_link, expires): (i64, String, bool, Option<i64>) =
        sqlx::query_as("SELECT id, name, from_link, expires_at FROM files WHERE from_link = 1")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert!(from_link);
    assert!(expires.is_some(), "dans Temporaire");
    assert!(
        name.starts_with("Ma vidéo") && name.ends_with(".mp4"),
        "{name}"
    );
    assert!(!name.contains('/'));
    let page = alice.get("/tools").await.body;
    assert!(page.contains(&format!("href=\"/files/{file}\"")));

    // Gardé dans un espace, il ne se partage toujours pas : ni bouton, ni
    // route du site, ni API.
    let ecole = app.space_id(ALICE, "ecole").await.to_string();
    let kept = alice
        .post(&format!("/files/{file}/keep"), &[("space_id", &ecole)])
        .await;
    assert_eq!(kept.status, StatusCode::SEE_OTHER, "{}", kept.body);
    let view = alice.get(&format!("/files/{file}")).await.body;
    assert!(!view.contains("popovertarget=\"pm-share\""));
    let shared = alice
        .post(&format!("/files/{file}/share"), &[("duree", "1j")])
        .await;
    assert_eq!(shared.status, StatusCode::BAD_REQUEST);
    let (status, body) = app
        .api(
            Method::POST,
            &format!("/api/v1/files/{file}/shares"),
            Some(json!({ "duration": "1j" })),
            Some(API_TOKEN),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("téléchargeur"));
    let links: i64 = sqlx::query_scalar("SELECT count(*) FROM share_links")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(links, 0);

    // Sa conversion en MP3 hérite de la marque.
    let converted = alice
        .post(&format!("/files/{file}/convert"), &[("to", "mp3")])
        .await;
    assert_eq!(
        converted.status,
        StatusCode::SEE_OTHER,
        "{}",
        converted.body
    );
    let job: i64 = sqlx::query_scalar("SELECT max(id) FROM jobs WHERE kind = 'convert'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    let dir = tools::job_dir(&app.work_dir, job);
    std::fs::write(dir.join("started"), "").unwrap();
    worker::process_job(&dir, &|spec: &JobSpec, dir: &Path, _: &dyn Fn(u8)| {
        std::fs::write(dir.join(&spec.output), [3u8; 512]).map_err(|e| e.to_string())
    });
    tools::collect(&app.state).await.unwrap();
    let marked: i64 = sqlx::query_scalar("SELECT count(*) FROM files WHERE from_link = 1")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(marked, 2, "la conversion est marquée aussi");
}

#[tokio::test]
async fn un_echec_s_affiche_et_trois_en_attente_au_plus() {
    let app = TestApp::with_downloader().await;
    service_ready(&app);
    let mut alice = app.logged_in(ALICE).await;

    launch(&mut alice, VIDEO, "mp3").await;
    launch(&mut alice, "https://vm.tiktok.com/ZMabc/", "mp4").await;
    launch(&mut alice, "https://www.instagram.com/reel/C1abc/", "mp4").await;
    let page = launch(&mut alice, "https://youtu.be/abc", "mp4").await;
    assert!(page.contains("déjà en attente"), "{page}");
    assert_eq!(downloads(&app).await.len(), 3);

    // Le service a échoué : son message s'affiche.
    let (first, _, _) = downloads(&app).await[0].clone();
    let dir = job_dir(&app, first);
    std::fs::write(dir.join("started"), "").unwrap();
    std::fs::write(
        dir.join("error"),
        "Vidéo privée : elle ne se télécharge pas.",
    )
    .unwrap();
    tools::collect(&app.state).await.unwrap();
    let (_, status, error) = downloads(&app).await[0].clone();
    assert_eq!(status, "failed");
    assert_eq!(
        error.as_deref(),
        Some("Vidéo privée : elle ne se télécharge pas.")
    );
    let page = alice.get("/tools").await.body;
    assert!(page.contains("Vidéo privée"));
    assert!(page.contains("Son depuis un lien"));

    // Une place s'est libérée.
    let page = launch(&mut alice, "https://youtu.be/abc", "mp4").await;
    assert!(page.contains("Téléchargement lancé"));
}

/// Le dossier appartient aussi au service, qui a Internet : un lien
/// symbolique posé à la place du résultat n'est jamais rangé.
#[cfg(unix)]
#[tokio::test]
async fn un_lien_symbolique_n_est_pas_range() {
    let app = TestApp::with_downloader().await;
    service_ready(&app);
    let mut alice = app.logged_in(ALICE).await;
    launch(&mut alice, VIDEO, "mp4").await;
    let (id, _, _) = downloads(&app).await[0].clone();
    let dir = job_dir(&app, id);
    let secret = app.work_dir.join("secret.txt");
    std::fs::write(&secret, "données de dashu").unwrap();
    std::os::unix::fs::symlink(&secret, dir.join("output.mp4")).unwrap();
    std::fs::write(dir.join("started"), "").unwrap();
    std::fs::write(dir.join("done"), "").unwrap();

    tools::collect(&app.state).await.unwrap();
    assert_eq!(downloads(&app).await[0].1, "failed");
    let files: i64 = sqlx::query_scalar("SELECT count(*) FROM files")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(files, 0);
    assert!(secret.exists());
}
