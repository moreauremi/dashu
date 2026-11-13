//! Espaces et outils, suite (SPEC) : archiver un espace (E), règles
//! automatiques à l'arrivée d'un fichier (I), remplacer l'original par sa
//! copie (J), ouvrir l'app sur l'espace du moment (F).

mod common;

use std::path::Path;

use axum::http::StatusCode;
use common::{ALICE, BOB, TestApp};
use dashu::tools::{self, JobSpec, worker};

/// Crée une note dans l'espace actif, avec ce titre.
async fn note(client: &mut common::Client, title: &str) {
    let created = client.post("/notes", &[]).await;
    let path = created.location().to_string();
    client
        .post(&path, &[("title", title), ("body_md", "contenu")])
        .await;
}

async fn file_id(app: &TestApp, name: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM files WHERE name = ?")
        .bind(name)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

/// Conversions en file pour ce fichier : (sorte, cible).
async fn jobs_for(app: &TestApp, file: i64) -> Vec<(String, Option<String>)> {
    sqlx::query_as(
        "SELECT kind, target FROM jobs WHERE file_id = ? AND kind <> 'thumbnail' ORDER BY id",
    )
    .bind(file)
    .fetch_all(&app.state.db)
    .await
    .unwrap()
}

/// Envoi d'un fichier dans un espace donné.
async fn upload_in(client: &mut common::Client, name: &str, content: &[u8], space: i64) {
    let start = client.start_upload_in(name, content.len(), space).await;
    assert_eq!(start.status, StatusCode::CREATED, "{}", start.body);
    let done = client.put_chunk(&start.body, 0, content, true).await;
    assert_eq!(done.status, StatusCode::CREATED, "{}", done.body);
}

#[tokio::test]
async fn archiver_puis_restaurer_un_espace() {
    let app = TestApp::new().await;
    app.logged_in(BOB).await;
    let mut alice = app.logged_in(ALICE).await;
    let taff = app.space_id(ALICE, "taff").await;
    alice.post("/space", &[("slug", "taff")]).await;
    note(&mut alice, "Compte rendu de stage").await;

    let archived = alice
        .post(&format!("/settings/spaces/{taff}/archive"), &[])
        .await;
    let settings = alice.follow(archived).await;
    assert!(settings.body.contains("Taff archivé"));
    assert!(settings.body.contains("Archivés"));
    assert!(settings.body.contains("href=\"/e/taff/\""));

    // Plus dans le sélecteur ; l'espace actif revient au premier en service.
    let home = alice.get("/").await;
    assert!(!home.body.contains("title=\"Taff\""));
    assert!(home.body.contains("<title>Dashboard · Perso"));
    // Ni dans la vue d'ensemble de Perso…
    assert!(!alice.get("/notes").await.body.contains("Compte rendu"));
    // … mais cherchable par la palette, et ouvert par son adresse.
    assert!(
        alice
            .get_fragment("/search/palette?q=compte")
            .await
            .body
            .contains("rendu de stage")
    );
    let opened = alice.get("/e/taff/notes").await;
    assert_eq!(opened.status, StatusCode::OK);
    assert!(opened.body.contains("Espace archivé"));
    assert!(opened.body.contains("Compte rendu de stage"));

    // Perso ne s'archive pas ; l'espace d'un autre compte non plus.
    let perso = app.space_id(ALICE, "perso").await;
    let refused = alice
        .post(&format!("/settings/spaces/{perso}/archive"), &[])
        .await;
    assert!(
        alice
            .follow(refused)
            .await
            .body
            .contains("ne s'archive pas")
    );
    let bob_space = app.space_id(BOB, "ecole").await;
    assert_eq!(
        alice
            .post(&format!("/settings/spaces/{bob_space}/archive"), &[])
            .await
            .status,
        StatusCode::NOT_FOUND
    );

    // Restauré : de retour dans le sélecteur et dans Perso.
    let restored = alice
        .post(&format!("/settings/spaces/{taff}/restore"), &[])
        .await;
    assert!(
        alice
            .follow(restored)
            .await
            .body
            .contains("Taff est de retour")
    );
    assert!(alice.get("/").await.body.contains("title=\"Taff\""));
    assert!(alice.get("/notes").await.body.contains("Compte rendu"));
}

#[tokio::test]
async fn les_regles_d_un_espace_a_l_arrivee() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let (perso, ecole) = (
        app.space_id(ALICE, "perso").await,
        app.space_id(ALICE, "ecole").await,
    );
    // Règles d'École : vidéos en MP4, HEIC doublées d'un JPEG.
    alice
        .post(
            &format!("/settings/spaces/{ecole}"),
            &[
                ("name", "École"),
                ("color", "blue"),
                ("icon", "school"),
                ("rules", "1"),
                ("auto_mp4", "on"),
                ("auto_heic_jpeg", "on"),
            ],
        )
        .await;
    let settings = alice.get("/settings").await;
    assert!(settings.body.contains("name=\"auto_mp4\" checked"));

    upload_in(&mut alice, "clip.mov", b"video", ecole).await;
    upload_in(&mut alice, "photo.heic", b"heic", ecole).await;
    upload_in(&mut alice, "deja.mp4", b"video", ecole).await;
    upload_in(&mut alice, "perso.mov", b"video", perso).await;
    let convert = |target: &str| vec![("convert".to_string(), Some(target.to_string()))];
    assert_eq!(
        jobs_for(&app, file_id(&app, "clip.mov").await).await,
        convert("mp4")
    );
    assert_eq!(
        jobs_for(&app, file_id(&app, "photo.heic").await).await,
        convert("jpg")
    );
    // Déjà en MP4 : rien ; dans Perso, sans règle : rien.
    assert!(
        jobs_for(&app, file_id(&app, "deja.mp4").await)
            .await
            .is_empty()
    );
    assert!(
        jobs_for(&app, file_id(&app, "perso.mov").await)
            .await
            .is_empty()
    );

    // Décochées : plus rien à l'arrivée.
    alice
        .post(
            &format!("/settings/spaces/{ecole}"),
            &[
                ("name", "École"),
                ("color", "blue"),
                ("icon", "school"),
                ("rules", "1"),
            ],
        )
        .await;
    upload_in(&mut alice, "autre.mov", b"video", ecole).await;
    assert!(
        jobs_for(&app, file_id(&app, "autre.mov").await)
            .await
            .is_empty()
    );
}

/// Faux service : un résultat de `size` octets.
fn fake(size: usize) -> impl Fn(&JobSpec, &Path, &dyn Fn(u8)) -> Result<(), String> + Sync {
    move |spec, dir, _| {
        std::fs::write(dir.join(&spec.output), vec![7u8; size]).map_err(|e| e.to_string())
    }
}

#[tokio::test]
async fn remplacer_l_original_par_sa_copie() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.upload("Vacances.MOV", &[9u8; 4096]).await;
    let original = file_id(&app, "Vacances.MOV").await;
    alice
        .post(&format!("/files/{original}/compress"), &[])
        .await;
    let job: i64 = sqlx::query_scalar("SELECT MAX(id) FROM jobs WHERE kind = 'compress'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    let dir = tools::job_dir(&app.work_dir, job);
    std::fs::write(dir.join("started"), "").unwrap();
    worker::process_job(&dir, &fake(1000));
    tools::collect(&app.state).await.unwrap();
    let copy = file_id(&app, "Vacances (compressé).mp4").await;

    // Le suivi et l'aperçu de la copie proposent de remplacer l'original.
    let panel = alice.get_fragment("/files/jobs?active=0").await;
    assert!(
        panel
            .body
            .contains(&format!("action=\"/files/jobs/{job}/replace\""))
    );
    let view = alice.get(&format!("/files/{copy}")).await;
    assert!(view.body.contains("Remplacer l'original"));

    // Un autre compte ne le peut pas.
    let mut bob = app.logged_in(BOB).await;
    assert_eq!(
        bob.post(&format!("/files/jobs/{job}/replace"), &[])
            .await
            .status,
        StatusCode::NOT_FOUND
    );

    let stored_before = app.stored_files().len();
    let replaced = alice.post(&format!("/files/jobs/{job}/replace"), &[]).await;
    assert_eq!(replaced.location(), format!("/files/{copy}"));
    assert!(
        alice
            .follow(replaced)
            .await
            .body
            .contains("Original remplacé : il reste « Vacances.mp4 »")
    );
    let names: Vec<String> = sqlx::query_scalar("SELECT name FROM files ORDER BY id")
        .fetch_all(&app.state.db)
        .await
        .unwrap();
    assert_eq!(names, vec!["Vacances.mp4".to_string()]);
    assert_eq!(app.stored_files().len(), stored_before - 1);
    // Une seconde fois : la tâche est partie avec l'original.
    assert_eq!(
        alice
            .post(&format!("/files/jobs/{job}/replace"), &[])
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn ouvrir_sur_l_espace_du_moment() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let (ecole, taff) = (
        app.space_id(ALICE, "ecole").await,
        app.space_id(ALICE, "taff").await,
    );
    let user = app.user_id(ALICE).await;

    // Désactivé par défaut : l'app s'ouvre où on était.
    assert_eq!(alice.get("/").await.status, StatusCode::OK);

    alice
        .post("/settings/moment", &[("space", &taff.to_string())])
        .await;
    // Un jour sans cours : l'espace choisi.
    let opened = alice.get("/").await;
    assert_eq!(opened.location(), "/e/taff/");

    // Un cours d'École en ce moment : École.
    let now = dashu::time::now();
    let source: i64 = sqlx::query_scalar(
        "INSERT INTO calendar_sources (user_id, space_id, name, ical_url, created_at)
         VALUES (?, ?, 'Pronote', 'https://example.test/x.ics', ?) RETURNING id",
    )
    .bind(user)
    .bind(ecole)
    .bind(now)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO calendar_events (source_id, uid, title, starts_at, ends_at)
         VALUES (?, 'c1', 'Réseaux', ?, ?)",
    )
    .bind(source)
    .bind(now - 600)
    .bind(now + 3000)
    .execute(&app.state.db)
    .await
    .unwrap();
    assert_eq!(alice.get("/").await.location(), "/e/ecole/");

    // Seulement à l'ouverture : ni une navigation htmx, ni une adresse
    // qui nomme déjà un espace.
    assert_eq!(alice.get_boosted("/").await.status, StatusCode::OK);
    assert_eq!(alice.get("/e/taff/").await.status, StatusCode::OK);

    // L'espace d'un autre compte est refusé ; désactiver le réglage.
    let bob = {
        let mut bob = app.logged_in(BOB).await;
        let _ = bob.get("/").await;
        app.space_id(BOB, "perso").await
    };
    assert_eq!(
        alice
            .post("/settings/moment", &[("space", &bob.to_string())])
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    alice.post("/settings/moment", &[("space", "")]).await;
    assert_eq!(alice.get("/").await.status, StatusCode::OK);
}
