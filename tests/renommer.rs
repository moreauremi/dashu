//! Renommer un fichier (SPEC v2, « Page Fichiers ») : la partie avant
//! l'extension change, l'extension et le type du fichier jamais.

mod common;

use axum::http::{StatusCode, header};
use common::{ALICE, BOB, TestApp};

async fn name_and_mime(app: &TestApp, id: i64) -> (String, String) {
    sqlx::query_as("SELECT name, mime FROM files WHERE id = ?")
        .bind(id)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn renommer_un_fichier_garde_son_type() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.upload("scan_0042.pdf", b"%PDF-1.4").await;
    let id: i64 = sqlx::query_scalar("SELECT id FROM files WHERE name = 'scan_0042.pdf'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();

    // L'aperçu et la liste proposent de renommer : le nom sans son extension,
    // l'extension à côté.
    let view = alice.get(&format!("/files/{id}")).await.body;
    assert!(view.contains(&format!("action=\"/files/{id}/rename\"")));
    assert!(view.contains("name=\"name\" value=\"scan_0042\""));
    assert!(view.contains("<span class=\"rename-ext\">.pdf</span>"));
    let list = alice.get("/files?dossier=espace").await.body;
    assert!(list.contains(&format!("popovertarget=\"pm-rename-file-{id}\"")));

    // Renommé : l'extension reste, le type aussi ; retour à l'aperçu.
    let renamed = alice
        .post(
            &format!("/files/{id}/rename"),
            &[("name", " Cours VLAN "), ("next", &format!("/files/{id}"))],
        )
        .await;
    assert_eq!(renamed.location(), format!("/files/{id}"));
    let page = alice.follow(renamed).await;
    assert!(page.body.contains("Fichier renommé en « Cours VLAN.pdf »"));
    assert_eq!(
        name_and_mime(&app, id).await,
        ("Cours VLAN.pdf".into(), "application/pdf".into())
    );
    // Une autre extension tapée ne change pas le type.
    alice
        .post(&format!("/files/{id}/rename"), &[("name", "vlan.html")])
        .await;
    assert_eq!(
        name_and_mime(&app, id).await,
        ("vlan.html.pdf".into(), "application/pdf".into())
    );

    // Il se trouve et se télécharge sous son nouveau nom.
    let found = alice.get("/files?dossier=espace&q=vlan").await.body;
    assert!(found.contains("vlan.html.pdf"));
    let file = alice.get(&format!("/files/{id}/download")).await;
    assert!(
        file.header(header::CONTENT_DISPOSITION.as_str())
            .contains("vlan.html.pdf")
    );

    // Un nom vide est refusé, rien ne change.
    let empty = alice
        .post(&format!("/files/{id}/rename"), &[("name", "   ")])
        .await;
    assert!(
        alice
            .follow(empty)
            .await
            .body
            .contains("Donne un nom au fichier.")
    );
    assert_eq!(name_and_mime(&app, id).await.0, "vlan.html.pdf");

    // Un autre compte ne renomme pas le fichier d'Alice.
    let mut bob = app.logged_in(BOB).await;
    assert_eq!(
        bob.post(&format!("/files/{id}/rename"), &[("name", "intrus")])
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(name_and_mime(&app, id).await.0, "vlan.html.pdf");
}
