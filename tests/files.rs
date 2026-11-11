//! Fichiers (v2, étape 2) : envoi, liste, téléchargement, suppression,
//! limites, isolation entre comptes et entre espaces.

mod common;

use axum::http::StatusCode;
use common::{ALICE, BOB, MAX_UPLOAD, TestApp, between};

/// Identifiant du premier fichier de la liste.
fn first_file_id(page: &str) -> i64 {
    between(page, "href=\"/files/", "/download\"")
        .expect("un fichier dans la liste")
        .parse()
        .unwrap()
}

#[tokio::test]
async fn envoi_liste_telechargement_puis_suppression() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.get("/files").await;

    let content = b"%PDF-1.4 contenu de test".to_vec();
    let sent = alice.upload("Cours réseau.pdf", &content).await;
    assert_eq!(sent.status, StatusCode::CREATED, "{}", sent.body);

    let page = alice.get("/files").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("Cours réseau.pdf"));
    assert!(page.body.contains(&format!("PDF · {} o", content.len())));
    let id = first_file_id(&page.body);

    let file = alice.get(&format!("/files/{id}/download")).await;
    assert_eq!(file.status, StatusCode::OK);
    assert_eq!(file.bytes, content);
    assert_eq!(file.header("content-type"), "application/octet-stream");
    assert_eq!(
        file.header("content-disposition"),
        "attachment; filename=\"Cours r_seau.pdf\"; filename*=UTF-8''Cours%20r%C3%A9seau.pdf"
    );
    assert_eq!(app.stored_files().len(), 1);

    let deleted = alice.post(&format!("/files/{id}/delete"), &[]).await;
    assert_eq!(deleted.status, StatusCode::SEE_OTHER);
    assert!(app.stored_files().is_empty(), "le contenu quitte le disque");
    let page = alice.get("/files").await;
    assert!(!page.body.contains("Cours réseau.pdf"));
    assert_eq!(
        alice.get(&format!("/files/{id}/download")).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn un_fichier_html_n_est_jamais_affiche_comme_une_page() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.get("/files").await;
    let trap = b"<html><script>alert(document.cookie)</script></html>";
    alice.upload("piege.html", trap).await;
    alice
        .upload("dessin.svg", b"<svg onload=\"alert(1)\"/>")
        .await;

    let page = alice.get("/files").await;
    for name in ["piege.html", "dessin.svg"] {
        let id = id_of(&page.body, name);
        let file = alice.get(&format!("/files/{id}/download")).await;
        assert_eq!(file.status, StatusCode::OK);
        // Téléchargé, typé « octets bruts », et isolé si un navigateur
        // décidait malgré tout de l'afficher.
        assert!(
            file.header("content-disposition")
                .starts_with("attachment;")
        );
        assert_eq!(file.header("content-type"), "application/octet-stream");
        assert_eq!(
            file.header("content-security-policy"),
            "default-src 'none'; sandbox"
        );
        assert_eq!(file.header("x-content-type-options"), "nosniff");
    }
}

/// Identifiant du fichier nommé `name` dans la liste.
fn id_of(page: &str, name: &str) -> i64 {
    let at = page
        .find(&format!("<span>{name}</span>"))
        .expect("fichier listé");
    let before = &page[..at];
    let start = before.rfind("href=\"/files/").unwrap() + "href=\"/files/".len();
    let len = before[start..].find('/').unwrap();
    before[start..start + len].parse().unwrap()
}

#[tokio::test]
async fn fichier_trop_gros_refuse_sans_rien_laisser() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.get("/files").await;
    let too_big = vec![0u8; MAX_UPLOAD as usize + 1];

    // Taille annoncée : refus avant d'écrire quoi que ce soit.
    let refused = alice.upload("gros.bin", &too_big).await;
    assert_eq!(refused.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(refused.body.contains("64 Ko au plus"), "{}", refused.body);

    // Taille non annoncée : le comptage des octets reçus arrête l'envoi.
    let refused = alice.upload_unannounced("gros.bin", &too_big).await;
    assert_eq!(refused.status, StatusCode::PAYLOAD_TOO_LARGE);

    assert!(app.stored_files().is_empty(), "{:?}", app.stored_files());
    assert!(!alice.get("/files").await.body.contains("gros.bin"));

    // Pile à la limite : accepté.
    let at_limit = vec![0u8; MAX_UPLOAD as usize];
    let sent = alice.upload_unannounced("juste.bin", &at_limit).await;
    assert_eq!(sent.status, StatusCode::CREATED, "{}", sent.body);
}

#[tokio::test]
async fn un_compte_n_a_aucun_acces_aux_fichiers_d_un_autre() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.get("/files").await;
    alice.upload("secret.txt", b"journal intime").await;
    let id = first_file_id(&alice.get("/files").await.body);

    let mut bob = app.logged_in(BOB).await;
    let page = bob.get("/files").await;
    assert!(!page.body.contains("secret.txt"));
    assert_eq!(
        bob.get(&format!("/files/{id}/download")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        bob.post(&format!("/files/{id}/delete"), &[]).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(app.stored_files().len(), 1, "le fichier d'Alice est intact");
}

#[tokio::test]
async fn envoi_sans_jeton_csrf_refuse() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.get("/files").await;
    let refused = alice.upload_without_csrf("note.txt", b"bonjour").await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert!(app.stored_files().is_empty());
}

#[tokio::test]
async fn un_nom_piege_reste_un_simple_nom() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.get("/files").await;
    let sent = alice.upload("../../etc/passwd", b"root:x:0:0").await;
    assert_eq!(sent.status, StatusCode::CREATED);

    let page = alice.get("/files").await;
    assert!(page.body.contains("<span>passwd</span>"));
    assert!(!page.body.contains("etc/passwd"));

    // Sur le disque : sous le dossier des fichiers, avec un nom aléatoire.
    let stored = app.stored_files();
    assert_eq!(stored.len(), 1);
    let path = &stored[0];
    assert!(path.starts_with(&app.files_dir));
    let name = path.file_name().unwrap().to_str().unwrap();
    assert_eq!(name.len(), 64);
    assert!(name.chars().all(|c| c.is_ascii_hexdigit()));
}

#[tokio::test]
async fn un_fichier_appartient_a_l_espace_actif() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.post("/space", &[("slug", "ecole")]).await;
    alice.get("/files").await;
    alice.upload("td-vlan.pdf", b"%PDF td").await;
    assert!(alice.get("/files").await.body.contains("td-vlan.pdf"));

    alice.post("/space", &[("slug", "taff")]).await;
    assert!(!alice.get("/files").await.body.contains("td-vlan.pdf"));
}

#[tokio::test]
async fn sans_files_dir_les_fichiers_sont_desactives() {
    let app = TestApp::without_files().await;
    let mut alice = app.logged_in(ALICE).await;
    let page = alice.get("/files").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("pas activés"));
    assert!(!page.body.contains("data-upload-input"));
    let refused = alice.upload("note.txt", b"bonjour").await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn la_page_fichiers_est_dans_la_navigation() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let page = alice.get("/").await;
    assert!(page.body.contains("href=\"/files\" data-key=\"g f\""));
    let page = alice.get("/files").await;
    assert!(
        page.body
            .contains("href=\"/files\" data-key=\"g f\" aria-current=\"page\"")
    );
}
