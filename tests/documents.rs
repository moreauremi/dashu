//! Conversion des documents (SPEC v2, « Outils : conversions », Markdown et
//! texte vers PDF, HTML ou DOCX). Le service de conversion est remplacé par
//! un faux : les arguments de pandoc sont testés dans src/tools/worker.rs.

mod common;

use std::path::Path;

use axum::http::StatusCode;
use common::{ALICE, TestApp};
use dashu::tools::{self, Conversion, JobSpec, Media, Task, worker};

async fn file_of(app: &TestApp, name: &str) -> (i64, i64, Option<i64>) {
    sqlx::query_as("SELECT id, space_id, folder_id FROM files WHERE name = ?")
        .bind(name)
        .fetch_one(&app.state.db)
        .await
        .unwrap_or_else(|_| panic!("« {name} » absent"))
}

async fn last_job(app: &TestApp) -> i64 {
    sqlx::query_scalar("SELECT MAX(id) FROM jobs WHERE kind = 'convert'")
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn un_markdown_devient_pdf_html_ou_docx() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    alice
        .post(
            "/folders",
            &[("name", "Réseaux"), ("space_id", &ecole.to_string())],
        )
        .await;
    let reseaux: i64 = sqlx::query_scalar("SELECT id FROM folders WHERE name = 'Réseaux'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    alice
        .upload_in_folder(
            "fiche-vlan.md",
            b"# VLAN\n\nUn trunk transporte plusieurs VLAN.\n",
            reseaux,
        )
        .await;
    alice.upload("todo.txt", b"- relire le cours\n").await;
    let (md, space, folder) = file_of(&app, "fiche-vlan.md").await;

    // Le menu propose les trois sorties, pas de compression.
    alice.post("/space", &[("slug", "ecole")]).await;
    let list = alice.get(&format!("/files?dossier={reseaux}")).await.body;
    let row = common::between(&list, "fiche-vlan.md</span>", "</li>").unwrap();
    for label in ["En PDF", "En page web (HTML)", "En Word (DOCX)"] {
        assert!(row.contains(&format!(">{label}</button>")), "{label}");
    }
    assert!(!row.contains("Compresser"));
    let view = alice
        .get(&format!(
            "/e/perso/files/{}",
            file_of(&app, "todo.txt").await.0
        ))
        .await
        .body;
    assert!(view.contains(">En PDF</button>"));

    // Une sortie qui n'est pas pour un document est refusée.
    let refused = alice
        .post(&format!("/files/{md}/convert"), &[("to", "mp3")])
        .await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);

    for (to, conversion, name) in [
        ("pdf", Conversion::Pdf, "fiche-vlan.pdf"),
        ("html", Conversion::Html, "fiche-vlan.html"),
        ("docx", Conversion::Docx, "fiche-vlan.docx"),
    ] {
        let launched = alice
            .post(&format!("/files/{md}/convert"), &[("to", to)])
            .await;
        assert_eq!(launched.status, StatusCode::SEE_OTHER, "{to}");
        let job = last_job(&app).await;
        let dir = tools::job_dir(&app.work_dir, job);
        let spec: JobSpec =
            serde_json::from_slice(&std::fs::read(dir.join("job.json")).unwrap()).unwrap();
        assert_eq!(spec.task, Task::Convert);
        assert_eq!(spec.media, Media::Document);
        assert_eq!(spec.to, Some(conversion));
        assert_eq!(spec.output, format!("output.{to}"));
        // Le faux service écrit un résultat, dashu le relève.
        std::fs::write(dir.join("started"), "").unwrap();
        let fake = |spec: &JobSpec, dir: &Path, _: &dyn Fn(u8)| {
            std::fs::write(dir.join(&spec.output), b"resultat").map_err(|e| e.to_string())
        };
        worker::process_job(&dir, &fake);
        tools::collect(&app.state).await.unwrap();
        // Le résultat arrive à côté de l'original, même espace, même dossier.
        let (_, result_space, result_folder) = file_of(&app, name).await;
        assert_eq!((result_space, result_folder), (space, folder), "{name}");
    }
}
