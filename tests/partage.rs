//! Partage d'un fichier par lien temporaire (SPEC v2, « Partage par lien ») :
//! création depuis l'aperçu, page publique sans connexion, téléchargement
//! qui n'exécute rien, révocation, expiration, isolement entre comptes.

mod common;

use axum::http::{StatusCode, header};
use common::{ALICE, BOB, Client, TestApp};
use sha2::{Digest, Sha256};

async fn file_id(app: &TestApp, name: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM files WHERE name = ?")
        .bind(name)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

/// Crée un lien et rend (jeton, identifiant du lien).
async fn share(app: &TestApp, alice: &mut Client, id: i64, duree: &str) -> (String, i64) {
    let created = alice
        .post(&format!("/files/{id}/share"), &[("duree", duree)])
        .await;
    assert_eq!(created.status, StatusCode::OK, "{}", created.body);
    let at = created.body.find("/s/").expect("lien affiché") + 3;
    let token: String = created.body[at..].chars().take(64).collect();
    assert!(token.bytes().all(|b| b.is_ascii_hexdigit()) && token.len() == 64);
    let link: i64 = sqlx::query_scalar("SELECT max(id) FROM share_links")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    (token, link)
}

const INVALID: &str = "Ce lien n'est pas valable";

#[tokio::test]
async fn un_lien_marche_sans_connexion_puis_se_revoque() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.upload("compte-rendu.pdf", b"%PDF-1.4 contenu").await;
    let id = file_id(&app, "compte-rendu.pdf").await;

    // L'aperçu propose de partager, pour 1 heure, 1 jour ou 7 jours.
    let view = alice.get(&format!("/files/{id}")).await.body;
    assert!(view.contains("popovertarget=\"pm-share\""));
    for duree in ["1h", "1j", "7j"] {
        assert!(
            view.contains(&format!("name=\"duree\" value=\"{duree}\"")),
            "{duree}"
        );
    }
    let created = alice
        .post(&format!("/files/{id}/share"), &[("duree", "1j")])
        .await;
    assert!(created.body.contains("Lien créé, valable 1 jour"));
    assert!(created.body.contains("value=\"http://localhost:3000/s/"));
    assert!(created.body.contains("expire dans"));
    let at = created.body.find("/s/").unwrap() + 3;
    let token: String = created.body[at..].chars().take(64).collect();

    // Seule l'empreinte est en base, jamais le lien.
    let stored: String = sqlx::query_scalar("SELECT token_hash FROM share_links")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_ne!(stored, token);
    let expected: String = Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(stored, expected);
    // Revenu sur l'aperçu, le lien entier n'est plus affiché.
    let view = alice.get(&format!("/files/{id}")).await.body;
    assert!(!view.contains(&token) && view.contains("Révoquer"));
    // La liste le marque comme partagé.
    let list = alice.get("/files?dossier=espace").await.body;
    let row = common::between(&list, "compte-rendu.pdf</span>", "</li>").unwrap();
    assert!(row.contains("file-shared"));

    // Sans connexion : le nom, la taille, le téléchargement.
    let mut stranger = app.client();
    let page = stranger.get(&format!("/s/{token}")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("compte-rendu.pdf") && page.body.contains("16 o"));
    assert!(page.body.contains(&format!("href=\"/s/{token}/fichier\"")));
    assert_eq!(page.header("x-robots-tag"), "noindex, nofollow");
    let file = stranger.get(&format!("/s/{token}/fichier")).await;
    assert_eq!(file.status, StatusCode::OK);
    assert_eq!(file.bytes, b"%PDF-1.4 contenu");
    assert_eq!(
        file.header(header::CONTENT_TYPE.as_str()),
        "application/octet-stream"
    );
    assert!(
        file.header(header::CONTENT_DISPOSITION.as_str())
            .starts_with("attachment")
    );
    assert!(
        file.header(header::CONTENT_SECURITY_POLICY.as_str())
            .contains("sandbox")
    );
    assert_eq!(file.header("x-content-type-options"), "nosniff");

    // Révoqué : plus rien, tout de suite.
    let link: i64 = sqlx::query_scalar("SELECT id FROM share_links")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    let revoked = alice
        .post(&format!("/files/{id}/share/{link}/revoke"), &[])
        .await;
    assert_eq!(revoked.status, StatusCode::OK);
    assert!(!revoked.body.contains("Révoquer"));
    let page = stranger.get(&format!("/s/{token}")).await;
    assert_eq!(page.status, StatusCode::NOT_FOUND);
    assert!(page.body.contains(INVALID));
    assert!(!page.body.contains("compte-rendu"));
    assert_eq!(
        stranger.get(&format!("/s/{token}/fichier")).await.status,
        StatusCode::NOT_FOUND
    );
    // Une durée inconnue est refusée.
    assert_eq!(
        alice
            .post(&format!("/files/{id}/share"), &[("duree", "1an")])
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn expire_revoque_ou_inconnu_la_meme_page() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.upload("a.pdf", b"%PDF a").await;
    alice.upload("b.pdf", b"%PDF b").await;
    let (a, b) = (file_id(&app, "a.pdf").await, file_id(&app, "b.pdf").await);
    let (expired, link) = share(&app, &mut alice, a, "1h").await;
    sqlx::query("UPDATE share_links SET expires_at = 1 WHERE id = ?")
        .bind(link)
        .execute(&app.state.db)
        .await
        .unwrap();
    let (revoked, link) = share(&app, &mut alice, b, "7j").await;
    alice
        .post(&format!("/files/{b}/share/{link}/revoke"), &[])
        .await;
    let unknown = "ab".repeat(32);

    let mut stranger = app.client();
    let mut bodies = Vec::new();
    for token in [
        expired.as_str(),
        revoked.as_str(),
        unknown.as_str(),
        "pas-un-jeton",
    ] {
        let page = stranger.get(&format!("/s/{token}")).await;
        assert_eq!(page.status, StatusCode::NOT_FOUND, "{token}");
        assert!(page.body.contains(INVALID), "{token}");
        assert_eq!(
            stranger.get(&format!("/s/{token}/fichier")).await.status,
            StatusCode::NOT_FOUND
        );
        bodies.push(page.body);
    }
    // Rien ne dit lequel des trois.
    assert!(bodies.windows(2).all(|w| w[0] == w[1]));

    // Un fichier supprimé emporte ses liens.
    let (gone, _) = share(&app, &mut alice, a, "1j").await;
    alice.post(&format!("/files/{a}/delete"), &[]).await;
    assert_eq!(
        stranger.get(&format!("/s/{gone}")).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn seul_le_proprietaire_partage_et_revoque() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.upload("prive.pdf", b"%PDF").await;
    let id = file_id(&app, "prive.pdf").await;
    let (token, link) = share(&app, &mut alice, id, "1j").await;

    let mut bob = app.logged_in(BOB).await;
    assert_eq!(
        bob.post(&format!("/files/{id}/share"), &[("duree", "7j")])
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        bob.post(&format!("/files/{id}/share/{link}/revoke"), &[])
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    // Le lien d'Alice marche toujours.
    assert_eq!(bob.get(&format!("/s/{token}")).await.status, StatusCode::OK);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM share_links")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(count, 1);

    // Un fichier de l'onglet Outils, pas encore rangé, ne se partage pas.
    alice.upload_to_tools("scan.pdf", b"%PDF scan").await;
    let scan = file_id(&app, "scan.pdf").await;
    assert!(
        !alice
            .get(&format!("/files/{scan}"))
            .await
            .body
            .contains("pm-share")
    );
    assert_eq!(
        alice
            .post(&format!("/files/{scan}/share"), &[("duree", "1h")])
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn la_page_publique_est_limitee_en_requetes() {
    let app = TestApp::new().await;
    let mut stranger = app.client();
    let path = format!("/s/{}", "cd".repeat(32));
    for n in 0..20 {
        assert_eq!(
            stranger.get(&path).await.status,
            StatusCode::NOT_FOUND,
            "{n}"
        );
    }
    let limited = stranger.get(&path).await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.body.contains("Trop de demandes sur ce lien"));
    // Une autre adresse n'est pas touchée, et la connexion non plus.
    assert_eq!(app.client().get(&path).await.status, StatusCode::NOT_FOUND);
    assert_eq!(stranger.get("/login").await.status, StatusCode::OK);
}
