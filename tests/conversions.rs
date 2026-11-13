//! Menu « Convertir », suivi des conversions dans la barre latérale et
//! onglet Outils (SPEC, espaces et outils, G et H). Le service de conversion
//! est remplacé par un faux : les vraies conversions sont testées dans
//! src/tools/worker.rs.

mod common;

use std::path::Path;

use axum::http::StatusCode;
use common::{ALICE, BOB, TestApp};
use dashu::tools::{self, Conversion, JobSpec, Media, Task, worker};

/// Identifiant du fichier `name`, et son espace.
async fn file_of(app: &TestApp, name: &str) -> (i64, i64) {
    sqlx::query_as("SELECT id, space_id FROM files WHERE name = ?")
        .bind(name)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

async fn last_job(app: &TestApp) -> i64 {
    sqlx::query_scalar("SELECT MAX(id) FROM jobs WHERE kind IN ('compress', 'convert')")
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

/// Faux service : un résultat de `size` octets.
fn fake(size: usize) -> impl Fn(&JobSpec, &Path, &dyn Fn(u8)) -> Result<(), String> + Sync {
    move |spec, dir, _| {
        std::fs::write(dir.join(&spec.output), vec![7u8; size]).map_err(|e| e.to_string())
    }
}

/// Le service prend la dernière tâche et la traite, puis dashu la relève.
async fn run_last_job(app: &TestApp, size: usize) {
    let dir = tools::job_dir(&app.work_dir, last_job(app).await);
    std::fs::write(dir.join("started"), "").unwrap();
    worker::process_job(&dir, &fake(size));
    tools::collect(&app.state).await.unwrap();
}

#[tokio::test]
async fn le_menu_ne_propose_que_le_possible() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.upload("IMG_1.HEIC", b"heic").await;
    alice.upload("cours.pdf", b"%PDF-1.4").await;
    let (photo, _) = file_of(&app, "IMG_1.HEIC").await;
    let (pdf, _) = file_of(&app, "cours.pdf").await;

    let list = alice.get("/files").await;
    let row = common::between(&list.body, "IMG_1.HEIC</span>", "</li>").unwrap();
    for label in ["Compresser", "En JPEG", "En PNG", "En WebP"] {
        assert!(row.contains(&format!(">{label}</option>")), "{label}");
    }
    assert!(!row.contains("MP3"));

    // L'aperçu d'un PDF propose l'extraction de pages, avec sa saisie.
    let view = alice.get(&format!("/files/{pdf}")).await;
    assert!(view.body.contains("<span>Convertir</span>"));
    assert!(view.body.contains("name=\"pages\""));
    assert!(view.body.contains(">Compresser</button>"));

    // Hors menu : refusé, sans tâche créée.
    for (id, to) in [(photo, "mp3"), (photo, "exe"), (pdf, "jpg")] {
        let refused = alice
            .post(&format!("/files/{id}/convert"), &[("to", to)])
            .await;
        assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{to}");
    }
    // Une plage de pages absente ou piégée : refusée aussi.
    for pages in ["", "4-2", "2-4 -sOutputFile=/etc/passwd"] {
        let refused = alice
            .post(
                &format!("/files/{pdf}/convert"),
                &[("to", "pages"), ("pages", pages)],
            )
            .await;
        assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{pages}");
    }
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'convert'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(jobs, 0);

    // Le fichier d'un autre compte : introuvable.
    let mut bob = app.logged_in(BOB).await;
    let refused = bob
        .post(&format!("/files/{photo}/convert"), &[("to", "jpg")])
        .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn une_conversion_va_jusqu_au_resultat() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.upload("IMG_1.HEIC", b"heic").await;
    let (photo, space) = file_of(&app, "IMG_1.HEIC").await;

    let launched = alice
        .post(
            &format!("/files/{photo}/convert"),
            &[("to", "jpg"), ("next", &format!("/files/{photo}"))],
        )
        .await;
    assert_eq!(launched.location(), format!("/files/{photo}"));
    let job = last_job(&app).await;
    let spec: JobSpec = serde_json::from_slice(
        &std::fs::read(tools::job_dir(&app.work_dir, job).join("job.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(spec.task, Task::Convert);
    assert_eq!(spec.media, Media::Heic);
    assert_eq!(spec.to, Some(Conversion::Jpg));
    assert_eq!(spec.output, "output.jpg");
    let (kind, target): (String, String) =
        sqlx::query_as("SELECT kind, target FROM jobs WHERE id = ?")
            .bind(job)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!((kind.as_str(), target.as_str()), ("convert", "jpg"));

    // En cours : la barre latérale le dit sur toutes les pages.
    for path in ["/", "/notes", "/calendar"] {
        let page = alice.get(path).await;
        assert!(page.body.contains("1 conversion en cours"), "{path}");
        assert!(
            page.body
                .contains("hx-get=\"/jobs/indicator?active=1\" hx-trigger=\"every 3s\""),
            "{path}"
        );
    }
    let still = alice.get_fragment("/jobs/indicator?active=1").await;
    assert!(still.body.contains("1 conversion en cours"));

    // Fini : le résultat rejoint l'espace, l'original reste.
    run_last_job(&app, 900).await;
    let (result, result_space) = file_of(&app, "IMG_1.jpg").await;
    assert_eq!(result_space, space);
    assert!(
        alice
            .get("/files")
            .await
            .body
            .contains("<span>IMG_1.HEIC</span>")
    );

    // L'indicateur annonce ce qui est prêt, avec un lien, et ne se redemande plus.
    let done = alice.get_fragment("/jobs/indicator?active=1").await;
    assert!(done.body.contains("Prêt : IMG_1.jpg"));
    assert!(done.body.contains(&format!("href=\"/files/{result}\"")));
    assert!(!done.body.contains("every 3s"));
    assert!(
        !alice
            .get("/notes")
            .await
            .body
            .contains("conversion en cours")
    );

    // La liste du suivi : la conversion faite, avec un lien vers le résultat.
    let panel = alice.get_fragment("/files/jobs?active=0").await;
    assert!(panel.body.contains("En JPEG"));
    assert!(panel.body.contains(&format!("href=\"/files/{result}\"")));
}

#[tokio::test]
async fn pages_d_un_pdf() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.upload("cours.pdf", b"%PDF-1.4").await;
    let (pdf, _) = file_of(&app, "cours.pdf").await;
    alice
        .post(
            &format!("/files/{pdf}/convert"),
            &[("to", "pages"), ("pages", "2 - 4")],
        )
        .await;
    let job = last_job(&app).await;
    let spec: JobSpec = serde_json::from_slice(
        &std::fs::read(tools::job_dir(&app.work_dir, job).join("job.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(spec.pages, Some((2, 4)));
    // Une copie plus grosse que l'original reste gardée : pas de « gain »
    // exigé d'une conversion.
    run_last_job(&app, 5000).await;
    file_of(&app, "cours (pages 2-4).pdf").await;
}

#[tokio::test]
async fn l_onglet_outils() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await;

    let page = alice.get("/tools").await;
    assert!(page.body.contains("data-dropzone data-tools"));
    assert!(page.body.contains("Aucun fichier en attente"));

    let sent = alice.upload_to_tools("scan.pdf", b"%PDF-1.4 scan").await;
    assert_eq!(sent.status, StatusCode::CREATED);
    let (scan, _) = file_of(&app, "scan.pdf").await;
    assert_eq!(sent.header("x-file-id"), scan.to_string());

    // Hors des listes et du dashboard tant qu'il n'est pas gardé.
    assert!(!alice.get("/files").await.body.contains("scan.pdf"));
    assert!(
        !alice
            .get_fragment("/widgets/files")
            .await
            .body
            .contains("scan.pdf")
    );
    let tools_page = alice.get("/tools").await;
    assert!(tools_page.body.contains("<span>scan.pdf</span>"));
    assert!(tools_page.body.contains("encore 24 h"));
    assert!(tools_page.body.contains("name=\"pages\""));
    // Son aperçu dit qu'il n'est pas encore rangé.
    let view = alice.get(&format!("/files/{scan}")).await;
    assert!(view.body.contains("pas encore rangé"));

    // Sa conversion reste dans l'onglet Outils, avec la même échéance.
    alice
        .post(
            &format!("/files/{scan}/convert"),
            &[("to", "compress"), ("next", "/tools")],
        )
        .await;
    let home = alice.get("/").await;
    let indicator = common::between(&home.body, "id=\"jobs-indicator\"", "</div>").unwrap();
    assert!(indicator.contains("href=\"/tools\""), "{indicator}");
    run_last_job(&app, 5).await;
    let compressed: Option<i64> =
        sqlx::query_scalar("SELECT expires_at FROM files WHERE name = 'scan (compressé).pdf'")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert!(compressed.is_some());

    // Le garder dans École : il rejoint la liste d'École et n'expire plus.
    let kept = alice
        .post(
            &format!("/files/{scan}/keep"),
            &[("space_id", &ecole.to_string())],
        )
        .await;
    assert_eq!(kept.location(), "/tools");
    assert!(
        alice
            .follow(kept)
            .await
            .body
            .contains("Fichier gardé dans École")
    );
    assert_eq!(file_of(&app, "scan.pdf").await.1, ecole);
    alice.post("/space", &[("slug", "ecole")]).await;
    assert!(
        alice
            .get("/files")
            .await
            .body
            .contains("<span>scan.pdf</span>")
    );
    // Un fichier déjà rangé ne se « garde » pas une seconde fois.
    let again = alice
        .post(
            &format!("/files/{scan}/keep"),
            &[("space_id", &ecole.to_string())],
        )
        .await;
    assert_eq!(again.status, StatusCode::NOT_FOUND);

    // Échu : effacé, sur le disque aussi.
    let before = app.stored_files().len();
    sqlx::query("UPDATE files SET expires_at = 1 WHERE name = 'scan (compressé).pdf'")
        .execute(&app.state.db)
        .await
        .unwrap();
    dashu::files::cleanup_stale(&app.state.db, app.state.config.files.as_ref().unwrap())
        .await
        .unwrap();
    let gone: i64 =
        sqlx::query_scalar("SELECT count(*) FROM files WHERE name = 'scan (compressé).pdf'")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(gone, 0);
    assert_eq!(app.stored_files().len(), before - 1);

    // Bob ne voit ni ne garde les fichiers d'Alice.
    alice.upload_to_tools("prive.txt", b"secret").await;
    let (prive, _) = file_of(&app, "prive.txt").await;
    let mut bob = app.logged_in(BOB).await;
    assert!(!bob.get("/tools").await.body.contains("prive.txt"));
    let bob_space = app.space_id(BOB, "perso").await;
    let refused = bob
        .post(
            &format!("/files/{prive}/keep"),
            &[("space_id", &bob_space.to_string())],
        )
        .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
}
