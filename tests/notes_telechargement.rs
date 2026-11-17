//! Télécharger une note en Markdown ou en PDF (bouton « Télécharger » de
//! l'éditeur). Le service de conversion est remplacé par un faux, qui tourne
//! à côté de la requête : la vraie conversion (pandoc et Typst) est vérifiée
//! par la CI (`documents_dans_le_bac_a_sable`).

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::http::StatusCode;
use common::{ALICE, BOB, TestApp, between, note_id};
use dashu::tools::{Conversion, JobSpec, Media, Task, worker};

/// Une note d'Alice, avec son titre et son texte.
async fn note(app: &TestApp, title: &str, body: &str) -> i64 {
    let mut alice = app.logged_in(ALICE).await;
    let id = note_id(&alice.post("/notes", &[]).await);
    let saved = alice
        .post(
            &format!("/notes/{id}"),
            &[("title", title), ("body_md", body)],
        )
        .await;
    assert_eq!(saved.status, StatusCode::OK);
    id
}

/// Tâches prises par le faux service : leur `job.json` et leur entrée.
type Seen = std::sync::Arc<std::sync::Mutex<Vec<(JobSpec, String)>>>;

/// Faux service de conversion : prend les tâches prêtes du dossier de
/// travail, comme `dashu worker`, et les confie à `convert`.
fn fake_worker(
    work_dir: PathBuf,
    convert: fn(&JobSpec, &Path) -> Result<(), String>,
) -> (tokio::task::JoinHandle<()>, Seen) {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    let handle = tokio::spawn(async move {
        let jobs = work_dir.join("jobs");
        loop {
            for entry in std::fs::read_dir(&jobs).into_iter().flatten().flatten() {
                let dir = entry.path();
                if !dir.join("job.json").exists() || dir.join("started").exists() {
                    continue;
                }
                std::fs::write(dir.join("started"), "").unwrap();
                let spec: JobSpec =
                    serde_json::from_slice(&std::fs::read(dir.join("job.json")).unwrap()).unwrap();
                let input = std::fs::read_to_string(dir.join(&spec.input)).unwrap();
                log.lock().unwrap().push((spec, input));
                worker::process_job(&dir, &|spec, dir, _| convert(spec, dir));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });
    (handle, seen)
}

fn fake_pdf(spec: &JobSpec, dir: &Path) -> Result<(), String> {
    std::fs::write(dir.join(&spec.output), b"%PDF-1.7 faux").map_err(|e| e.to_string())
}

fn broken(_: &JobSpec, _: &Path) -> Result<(), String> {
    Err("La conversion a échoué : fichier illisible ou abîmé.".into())
}

/// Dossiers restés dans le dossier de travail.
fn leftovers(app: &TestApp) -> usize {
    std::fs::read_dir(app.work_dir.join("jobs"))
        .map(|entries| entries.count())
        .unwrap_or(0)
}

#[tokio::test]
async fn l_editeur_propose_markdown_et_pdf() {
    let app = TestApp::new().await;
    let id = note(&app, "Cours", "Texte").await;
    let mut alice = app.logged_in(ALICE).await;
    let page = alice.get(&format!("/notes/{id}")).await.body;
    let menu = between(&page, r#"id="pm-note-download""#, "</div>").unwrap();
    // Un lien ordinaire (hors htmx), repéré par app.js.
    assert!(menu.contains(&format!(
        r#"href="/notes/{id}/download.md" hx-boost="false" data-note-export>"#
    )));
    // « PDF… » ouvre ses options.
    assert!(menu.contains(r#"popovertarget="pm-note-pdf">"#));
    let options = between(&page, r#"id="pm-note-pdf""#, "</div>").unwrap();
    assert!(options.contains(&format!(
        r#"method="get" action="/notes/{id}/download.pdf" hx-boost="false" data-note-export="pdf">"#
    )));
    // Taille du texte : les tailles acceptées par le serveur, 100 % d'abord.
    for zoom in dashu::notes::export::ZOOMS {
        assert!(
            options.contains(&format!(r#"<option value="{zoom}""#)),
            "{zoom}"
        );
    }
    assert!(options.contains(r#"<option value="100" selected>100 %</option>"#));
    // Titres gardés avec leur texte : coché par défaut.
    assert!(options.contains(r#"<input type="checkbox" name="titres" value="1" checked>"#));
}

#[tokio::test]
async fn une_note_se_telecharge_en_markdown() {
    let app = TestApp::new().await;
    let id = note(
        &app,
        "Réseaux : VLAN/trunk",
        "Un **trunk** transporte plusieurs VLAN.\r\n\r\n- [ ] relire",
    )
    .await;
    let mut alice = app.logged_in(ALICE).await;

    let md = alice.get(&format!("/notes/{id}/download.md")).await;
    assert_eq!(md.status, StatusCode::OK);
    assert_eq!(md.header("content-type"), "text/markdown; charset=utf-8");
    // Le titre donne le nom du fichier (la barre oblique ne le coupe pas).
    let disposition = md.header("content-disposition");
    assert!(disposition.starts_with("attachment; "), "{disposition}");
    assert!(
        disposition.contains("filename*=UTF-8''R%C3%A9seaux%20%3A%20VLAN-trunk.md"),
        "{disposition}"
    );
    // Jamais affiché comme une page.
    assert_eq!(
        md.header("content-security-policy"),
        "default-src 'none'; sandbox"
    );
    assert_eq!(md.header("cache-control"), "private, no-store");
    // Le titre en tête, puis le texte tel qu'enregistré.
    assert_eq!(
        md.body,
        "# Réseaux : VLAN/trunk\n\nUn **trunk** transporte plusieurs VLAN.\n\n- [ ] relire\n"
    );

    // Aussi par l'adresse d'un espace.
    let prefixed = alice.get(&format!("/e/perso/notes/{id}/download.md")).await;
    assert_eq!(prefixed.status, StatusCode::OK);
    assert_eq!(prefixed.body, md.body);
}

#[tokio::test]
async fn une_note_d_un_autre_compte_ne_se_telecharge_pas() {
    let app = TestApp::new().await;
    let id = note(&app, "Secret", "Rien pour Bob").await;
    let mut bob = app.logged_in(BOB).await;
    for format in ["md", "pdf"] {
        let refused = bob.get(&format!("/notes/{id}/download.{format}")).await;
        assert_eq!(refused.status, StatusCode::NOT_FOUND, "{format}");
        assert!(!refused.body.contains("Rien pour Bob"));
    }
    // Ni sans être connecté.
    let anonymous = app.client().get(&format!("/notes/{id}/download.md")).await;
    assert!(!anonymous.body.contains("Rien pour Bob"));
    assert_ne!(anonymous.status, StatusCode::OK);
    assert_eq!(leftovers(&app), 0, "aucune tâche pour un refus");
}

#[tokio::test]
async fn le_pdf_passe_par_le_service_de_conversion() {
    let app = TestApp::new().await;
    let id = note(&app, "*Brouillon* d'été", "# Partie 1\n\nTexte.").await;
    let (worker, seen) = fake_worker(app.work_dir.clone(), fake_pdf);
    let mut alice = app.logged_in(ALICE).await;

    let pdf = alice
        .get(&format!("/notes/{id}/download.pdf?zoom=125&titres=1"))
        .await;
    assert_eq!(pdf.status, StatusCode::OK, "{}", pdf.body);
    assert_eq!(pdf.header("content-type"), "application/pdf");
    assert!(
        pdf.header("content-disposition")
            .contains("filename*=UTF-8''%2ABrouillon%2A%20d%27%C3%A9t%C3%A9.pdf"),
        "{}",
        pdf.header("content-disposition")
    );
    assert_eq!(
        pdf.header("content-security-policy"),
        "default-src 'none'; sandbox"
    );
    assert_eq!(pdf.bytes, b"%PDF-1.7 faux");

    // Une conversion de document en PDF, comme « Convertir › En PDF », avec
    // le titre (lu tel quel) et la date en en-tête.
    // Les options arrivent dans l'en-tête : texte à 125 %, titres gardés.
    let seen_first = seen.lock().unwrap().clone();
    assert_eq!(seen_first.len(), 1);
    let (spec, input) = &seen_first[0];
    assert_eq!(spec.task, Task::Convert);
    assert_eq!(spec.media, Media::Document);
    assert_eq!(spec.to, Some(Conversion::Pdf));
    assert!(
        input.starts_with("---\ntitle: '\\*Brouillon\\* d\\''été'\ndate: 'Modifiée le "),
        "{input}"
    );
    assert!(
        input.ends_with("\nzoom: '1.25'\ngarder-titres: true\n---\n\n# Partie 1\n\nTexte.\n"),
        "{input}"
    );

    // Sans option (case décochée, 100 %) : ni zoom ni titres gardés.
    let plain = alice
        .get(&format!("/notes/{id}/download.pdf?zoom=100"))
        .await;
    worker.abort();
    assert_eq!(plain.status, StatusCode::OK);
    let input = seen.lock().unwrap()[1].1.clone();
    assert!(
        !input.contains("zoom:") && !input.contains("garder-titres"),
        "{input}"
    );

    // Rien ne reste : ni dossier de travail, ni tâche affichée, ni fichier
    // rangé dans l'espace.
    assert_eq!(leftovers(&app), 0);
    let jobs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    let files: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM files")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!((jobs, files), (0, 0));
}

#[tokio::test]
async fn une_taille_de_texte_inconnue_est_refusee() {
    let app = TestApp::new().await;
    let id = note(&app, "Cours", "Texte").await;
    let mut alice = app.logged_in(ALICE).await;
    let refused = alice
        .get(&format!("/notes/{id}/download.pdf?zoom=105"))
        .await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    assert!(refused.body.contains("Taille du texte inconnue"));
    // Refusée avant toute tâche.
    assert_eq!(leftovers(&app), 0);
}

#[tokio::test]
async fn un_pdf_en_echec_le_dit() {
    let app = TestApp::new().await;
    let id = note(&app, "Cours", "Texte").await;
    let (worker, _) = fake_worker(app.work_dir.clone(), broken);
    let mut alice = app.logged_in(ALICE).await;

    let failed = alice.get(&format!("/notes/{id}/download.pdf")).await;
    worker.abort();
    assert_eq!(failed.status, StatusCode::SERVICE_UNAVAILABLE);
    // Une page d'erreur lisible, pas un fichier.
    assert!(failed.header("content-disposition").is_empty());
    assert!(failed.body.contains("Indisponible pour l"));
    assert!(
        failed
            .body
            .contains("a pas pu être préparé. La conversion a échoué"),
        "{}",
        failed.body
    );
    assert_eq!(leftovers(&app), 0);
}

#[tokio::test]
async fn sans_service_de_conversion_seulement_le_markdown() {
    let app = TestApp::without_files().await;
    let id = note(&app, "Cours", "Texte").await;
    let mut alice = app.logged_in(ALICE).await;

    let page = alice.get(&format!("/notes/{id}")).await.body;
    assert!(page.contains(&format!("/notes/{id}/download.md")));
    assert!(!page.contains("download.pdf"));
    assert_eq!(
        alice.get(&format!("/notes/{id}/download.pdf")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        alice.get(&format!("/notes/{id}/download.md")).await.status,
        StatusCode::OK
    );
}
