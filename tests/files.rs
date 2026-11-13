//! Fichiers (v2, étape 2) : envoi, liste, téléchargement, suppression,
//! limites, isolation entre comptes et entre espaces.

mod common;

use axum::http::StatusCode;
use common::{ALICE, BOB, MAX_UPLOAD, TestApp};

/// Identifiant du premier fichier de la liste.
fn first_file_id(page: &str) -> i64 {
    let at = page
        .find("class=\"file-name\" href=\"/files/")
        .expect("un fichier dans la liste");
    digits_after(&page[at..], "href=\"/files/")
}

/// Nombre qui suit `marker` dans `text`.
fn digits_after(text: &str, marker: &str) -> i64 {
    let start = text.find(marker).unwrap() + marker.len();
    text[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
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
    let start = before.rfind("class=\"file-name\" href=\"/files/").unwrap();
    digits_after(&before[start..], "href=\"/files/")
}

#[tokio::test]
async fn fichier_trop_gros_refuse_sans_rien_laisser() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.get("/files").await;
    let too_big = vec![0u8; MAX_UPLOAD as usize + 1];

    // Taille totale au-delà de la limite : refus dès l'ouverture de l'envoi.
    let refused = alice.upload("gros.bin", &too_big).await;
    assert_eq!(refused.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(refused.body.contains("64 Ko au plus"), "{}", refused.body);
    assert!(app.stored_files().is_empty(), "{:?}", app.stored_files());

    // Taille annoncée mensongère, morceau sans Content-Length : le comptage
    // des octets reçus arrête l'envoi, et le fichier partiel revient à vide.
    let id = alice.start_upload("menteur.bin", 10).await.body;
    let refused = alice.put_chunk(&id, 0, &[0u8; 11], false).await;
    assert_eq!(refused.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(
        refused.body.contains("plus gros qu&#39;annoncé"),
        "{}",
        refused.body
    );
    assert!(
        app.stored_files()
            .iter()
            .all(|path| path.metadata().unwrap().len() == 0)
    );
    let page = alice.get("/files").await.body;
    assert!(!page.contains("gros.bin") && !page.contains("menteur.bin"));

    // Pile à la limite, en trois morceaux : accepté.
    let at_limit = vec![7u8; MAX_UPLOAD as usize];
    let sent = alice.upload_in_chunks("juste.bin", &at_limit, 25_000).await;
    assert_eq!(sent.status, StatusCode::CREATED, "{}", sent.body);
}

#[tokio::test]
async fn un_gros_fichier_arrive_en_plusieurs_morceaux() {
    use sha2::{Digest, Sha256};

    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.get("/files").await;
    let content: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();

    let sent = alice.upload_in_chunks("video.mp4", &content, 16_384).await;
    assert_eq!(sent.status, StatusCode::CREATED, "{}", sent.body);

    let page = alice.get("/files").await;
    assert!(page.body.contains("Vidéo · 49 Ko"), "{}", page.body);
    let id = first_file_id(&page.body);
    let file = alice.get(&format!("/files/{id}/download")).await;
    assert_eq!(file.bytes, content, "le fichier recollé est identique");

    let stored: String = sqlx::query_scalar("SELECT sha256 FROM files WHERE id = ?")
        .bind(id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    let expected: String = Sha256::digest(&content)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(stored, expected);
    // Plus d'envoi en cours, plus de fichier partiel.
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM uploads")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(pending, 0);
    assert_eq!(app.stored_files().len(), 1);
}

#[tokio::test]
async fn un_morceau_hors_ordre_donne_le_point_de_reprise() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.get("/files").await;
    let id = alice.start_upload("cours.pdf", 10).await.body;

    let early = alice.put_chunk(&id, 5, b"fghij", true).await;
    assert_eq!(early.status, StatusCode::CONFLICT);
    assert_eq!(early.header("x-upload-received"), "0");

    let first = alice.put_chunk(&id, 0, b"abcde", true).await;
    assert_eq!(first.status, StatusCode::OK);
    assert_eq!(first.header("x-upload-received"), "5");

    // Le même morceau renvoyé (réponse perdue, par exemple) : on reprend à 5.
    let again = alice.put_chunk(&id, 0, b"abcde", true).await;
    assert_eq!(again.status, StatusCode::CONFLICT);
    assert_eq!(again.header("x-upload-received"), "5");

    let last = alice.put_chunk(&id, 5, b"fghij", true).await;
    assert_eq!(last.status, StatusCode::CREATED);
    let id = first_file_id(&alice.get("/files").await.body);
    assert_eq!(
        alice.get(&format!("/files/{id}/download")).await.bytes,
        b"abcdefghij"
    );
}

#[tokio::test]
async fn un_fichier_vide_est_accepte() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.get("/files").await;
    let sent = alice.upload("vide.txt", b"").await;
    assert_eq!(sent.status, StatusCode::CREATED, "{}", sent.body);
    assert!(alice.get("/files").await.body.contains("Texte · 0 o"));
}

#[tokio::test]
async fn les_envois_abandonnes_sont_effaces() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.get("/files").await;
    let id = alice.start_upload("abandon.bin", 100).await.body;
    alice.put_chunk(&id, 0, &[1u8; 40], true).await;
    let part = app.files_dir.join(".envois").join(format!("{id}.part"));
    assert!(part.exists());

    // Un jour plus tard, sans nouvelles.
    sqlx::query("UPDATE uploads SET updated_at = updated_at - 2 * 86400")
        .execute(&app.state.db)
        .await
        .unwrap();
    let two_hours_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
    std::fs::File::options()
        .write(true)
        .open(&part)
        .unwrap()
        .set_modified(two_hours_ago)
        .unwrap();

    let files = app.state.config.files.as_ref().unwrap();
    dashu::files::cleanup_stale(&app.state.db, files)
        .await
        .unwrap();
    assert!(!part.exists());
    let refused = alice.put_chunk(&id, 40, &[1u8; 60], true).await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
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

    // Un envoi en cours d'Alice est tout aussi inaccessible à Bob.
    let upload = alice.start_upload("brouillon.txt", 5).await.body;
    bob.get("/files").await;
    let refused = bob.put_chunk(&upload, 0, b"pirat", true).await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn envoi_sans_jeton_csrf_refuse() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.get("/files").await;
    let refused = alice.start_upload_without_csrf("note.txt", 7).await;
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
    // La barre de commandes le dit aussi.
    let tree = alice.catalogue("/files").await;
    assert_eq!(
        common::command_node(&tree, &["add", "file"])["off"],
        "Les fichiers ne sont pas activés sur ce serveur."
    );
    let refused = alice.upload("note.txt", b"bonjour").await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn la_page_fichiers_est_dans_la_navigation() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let page = alice.get("/").await;
    assert!(
        page.body
            .contains("href=\"/e/perso/files\" data-key=\"g f\"")
    );
    let page = alice.get("/files").await;
    assert!(
        page.body
            .contains("href=\"/e/perso/files\" data-key=\"g f\" aria-current=\"page\"")
    );
}
