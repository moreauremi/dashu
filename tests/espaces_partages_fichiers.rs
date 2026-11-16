//! Espaces partagés (SPEC, « Espaces partagés »), côté fichiers : Alice
//! partage son espace École avec Bob. Un membre voit, télécharge, renomme,
//! range et supprime les fichiers de l'espace, y envoie les siens et partage
//! ses dossiers ; Temporaire, les agendas restent à chacun ; un espace non
//! partagé répond 404.

mod common;

use axum::http::StatusCode;
use common::{ALICE, BOB, Client, TestApp};

/// Envoie un fichier dans l'espace `space` (en un morceau).
async fn upload_in(client: &mut Client, space: i64, name: &str, content: &[u8]) {
    let start = client.start_upload_in(name, content.len(), space).await;
    assert_eq!(start.status, StatusCode::CREATED, "{}", start.body);
    let sent = client.put_chunk(&start.body, 0, content, true).await;
    assert_eq!(sent.status, StatusCode::CREATED, "{}", sent.body);
}

async fn file_id(app: &TestApp, name: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM files WHERE name = ?")
        .bind(name)
        .fetch_one(&app.state.db)
        .await
        .unwrap_or_else(|_| panic!("fichier « {name} » absent"))
}

async fn folder_id(app: &TestApp, name: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM folders WHERE name = ?")
        .bind(name)
        .fetch_one(&app.state.db)
        .await
        .unwrap_or_else(|_| panic!("dossier « {name} » absent"))
}

/// Alice et Bob connectés, École d'Alice partagée avec Bob. Rend aussi
/// l'identifiant de l'espace partagé et son adresse chez Bob.
async fn setup() -> (TestApp, Client, Client, i64, String) {
    let app = TestApp::new().await;
    let alice = app.logged_in(ALICE).await;
    let bob = app.logged_in(BOB).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    let slug = app.share_space(ALICE, "ecole", BOB).await;
    (app, alice, bob, ecole, slug)
}

#[tokio::test]
async fn un_membre_voit_telecharge_et_previsualise() {
    let (app, mut alice, mut bob, ecole, slug) = setup().await;
    upload_in(&mut alice, ecole, "cours.pdf", b"%PDF-1.4 cours").await;
    let id = file_id(&app, "cours.pdf").await;

    let list = bob.get(&format!("/e/{slug}/files?dossier=espace")).await;
    assert_eq!(list.status, StatusCode::OK);
    assert!(list.body.contains("cours.pdf"));

    let download = bob.get(&format!("/files/{id}/download")).await;
    assert_eq!(download.status, StatusCode::OK);
    assert_eq!(download.bytes, b"%PDF-1.4 cours");

    // L'aperçu s'ouvre dans l'espace partagé, à l'adresse de Bob.
    let view = bob.get(&format!("/e/{slug}/files/{id}")).await;
    assert_eq!(view.status, StatusCode::OK, "{}", view.body);
    assert!(view.body.contains("cours.pdf"));
}

#[tokio::test]
async fn un_membre_renomme_range_et_supprime() {
    let (app, mut alice, mut bob, ecole, slug) = setup().await;
    upload_in(&mut alice, ecole, "a-renommer.pdf", b"%PDF un").await;
    upload_in(&mut alice, ecole, "a-ranger.pdf", b"%PDF deux").await;
    upload_in(&mut alice, ecole, "a-supprimer.pdf", b"%PDF trois").await;
    let (rename, moved, gone) = (
        file_id(&app, "a-renommer.pdf").await,
        file_id(&app, "a-ranger.pdf").await,
        file_id(&app, "a-supprimer.pdf").await,
    );

    // Renommer : l'extension reste.
    let renamed = bob
        .post(&format!("/files/{rename}/rename"), &[("name", "Nouveau")])
        .await;
    assert_eq!(renamed.status, StatusCode::SEE_OTHER, "{}", renamed.body);
    let name: String = sqlx::query_scalar("SELECT name FROM files WHERE id = ?")
        .bind(rename)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(name, "Nouveau.pdf");

    // Ranger vers son propre espace : le fichier quitte l'espace partagé.
    let mine = app.space_id(BOB, "taff").await;
    let put = bob
        .post(
            &format!("/files/{moved}/move"),
            &[("space_id", &mine.to_string())],
        )
        .await;
    assert_eq!(put.status, StatusCode::SEE_OTHER, "{}", put.body);
    assert_eq!(
        alice.get(&format!("/files/{moved}/download")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        bob.get(&format!("/files/{moved}/download")).await.status,
        StatusCode::OK
    );

    // Supprimer.
    let deleted = bob.post(&format!("/files/{gone}/delete"), &[]).await;
    assert_eq!(deleted.status, StatusCode::SEE_OTHER, "{}", deleted.body);
    assert_eq!(
        alice.get(&format!("/files/{gone}/download")).await.status,
        StatusCode::NOT_FOUND
    );
    assert!(
        !bob.get(&format!("/e/{slug}/files?dossier=espace"))
            .await
            .body
            .contains("a-supprimer.pdf")
    );
}

#[tokio::test]
async fn un_membre_envoie_dans_l_espace_partage() {
    let (app, mut alice, mut bob, ecole, slug) = setup().await;
    let _ = slug;
    upload_in(&mut bob, ecole, "de-bob.pdf", b"%PDF bob").await;
    let id = file_id(&app, "de-bob.pdf").await;

    let list = alice.get("/e/ecole/files?dossier=espace").await;
    assert!(list.body.contains("de-bob.pdf"));
    let download = alice.get(&format!("/files/{id}/download")).await;
    assert_eq!(download.bytes, b"%PDF bob");
    // L'auteur reste Bob.
    let author: i64 = sqlx::query_scalar("SELECT user_id FROM files WHERE id = ?")
        .bind(id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(author, app.user_id(BOB).await);
}

#[tokio::test]
async fn un_espace_non_partage_repond_404() {
    let (app, mut alice, mut bob, _, _) = setup().await;
    let taff = app.space_id(ALICE, "taff").await;
    upload_in(&mut alice, taff, "secret.pdf", b"%PDF secret").await;
    let id = file_id(&app, "secret.pdf").await;

    for path in [
        format!("/files/{id}/download"),
        format!("/files/{id}"),
        format!("/files/{id}/raw"),
    ] {
        assert_eq!(bob.get(&path).await.status, StatusCode::NOT_FOUND, "{path}");
    }
    for path in [
        format!("/files/{id}/rename"),
        format!("/files/{id}/delete"),
        format!("/files/{id}/tags"),
        format!("/files/{id}/share"),
    ] {
        let form: &[(&str, &str)] = &[("name", "x"), ("duree", "1j")];
        assert_eq!(
            bob.post(&path, form).await.status,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    // Un envoi vers cet espace est refusé aussi.
    let start = bob.start_upload_in("intrus.pdf", 4, taff).await;
    assert_eq!(start.status, StatusCode::NOT_FOUND);
    // Et le fichier est toujours là.
    assert_eq!(
        alice.get(&format!("/files/{id}/download")).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn les_dossiers_sont_communs() {
    let (app, mut alice, mut bob, ecole, slug) = setup().await;

    // Un dossier créé par Alice : Bob le voit et y envoie un fichier.
    let created = alice
        .post(
            "/folders",
            &[("name", "Cours"), ("space_id", &ecole.to_string())],
        )
        .await;
    assert_eq!(created.status, StatusCode::SEE_OTHER, "{}", created.body);
    let cours = folder_id(&app, "Cours").await;
    let list = bob.get(&format!("/e/{slug}/files?dossier=espace")).await;
    assert!(list.body.contains("Cours"));
    let sent = bob.upload_in_folder("tp.pdf", b"%PDF tp", cours).await;
    assert_eq!(sent.status, StatusCode::CREATED, "{}", sent.body);
    let place: (i64, Option<i64>) =
        sqlx::query_as("SELECT space_id, folder_id FROM files WHERE name = 'tp.pdf'")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(place, (ecole, Some(cours)));

    // Un dossier créé par Bob, un sous-dossier, et Alice les voit.
    let created = bob
        .post(
            "/folders",
            &[("name", "Examens"), ("space_id", &ecole.to_string())],
        )
        .await;
    assert_eq!(created.status, StatusCode::SEE_OTHER, "{}", created.body);
    let examens = folder_id(&app, "Examens").await;
    let sub = bob
        .post(
            "/folders",
            &[("name", "Corriges"), ("parent_id", &examens.to_string())],
        )
        .await;
    assert_eq!(sub.status, StatusCode::SEE_OTHER, "{}", sub.body);
    let alice_list = alice.get("/e/ecole/files?dossier=espace").await;
    assert!(alice_list.body.contains("Examens"));
    let inside = alice
        .get(&format!("/e/ecole/files?dossier={examens}"))
        .await;
    assert!(inside.body.contains("Corriges"));

    // Bob renomme le dossier d'Alice, puis le supprime avec son contenu.
    let renamed = bob
        .post(&format!("/folders/{cours}/rename"), &[("name", "Cours 2")])
        .await;
    assert_eq!(renamed.status, StatusCode::SEE_OTHER);
    assert!(
        alice
            .get("/e/ecole/files?dossier=espace")
            .await
            .body
            .contains("Cours 2")
    );
    let deleted = bob
        .post(
            &format!("/folders/{cours}/delete"),
            &[("contenu", "supprimer")],
        )
        .await;
    assert_eq!(deleted.status, StatusCode::SEE_OTHER, "{}", deleted.body);
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM files WHERE name = 'tp.pdf'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn un_dossier_non_partage_est_refuse() {
    let (app, mut alice, mut bob, _, _) = setup().await;
    let taff = app.space_id(ALICE, "taff").await;
    alice
        .post(
            "/folders",
            &[("name", "Prive"), ("space_id", &taff.to_string())],
        )
        .await;
    let prive = folder_id(&app, "Prive").await;

    let sent = bob.upload_in_folder("intrus.pdf", b"%PDF", prive).await;
    assert_eq!(sent.status, StatusCode::NOT_FOUND);
    let sub = bob
        .post(
            "/folders",
            &[("name", "Piege"), ("parent_id", &prive.to_string())],
        )
        .await;
    assert_eq!(sub.status, StatusCode::NOT_FOUND);
    let in_space = bob
        .post(
            "/folders",
            &[("name", "Piege"), ("space_id", &taff.to_string())],
        )
        .await;
    assert_eq!(in_space.status, StatusCode::NOT_FOUND);
    for path in [
        format!("/folders/{prive}/rename"),
        format!("/folders/{prive}/delete"),
        format!("/folders/{prive}/move"),
    ] {
        let mine = app.space_id(BOB, "taff").await.to_string();
        let form: &[(&str, &str)] = &[("name", "x"), ("space_id", &mine)];
        assert_eq!(
            bob.post(&path, form).await.status,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM folders WHERE name = 'Piege'")
            .fetch_one(&app.state.db)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn le_temporaire_reste_a_son_auteur() {
    let (app, mut alice, mut bob, ecole, slug) = setup().await;
    let sent = bob
        .upload_to_tools("brouillon.pdf", b"%PDF brouillon")
        .await;
    assert_eq!(sent.status, StatusCode::CREATED, "{}", sent.body);
    let id = file_id(&app, "brouillon.pdf").await;

    // Alice ne le voit ni dans Temporaire, ni dans l'espace, ni par son numéro.
    assert!(
        !alice
            .get("/files?dossier=temporaire")
            .await
            .body
            .contains("brouillon.pdf")
    );
    assert!(
        !alice
            .get("/e/ecole/files?dossier=espace")
            .await
            .body
            .contains("brouillon.pdf")
    );
    assert_eq!(
        alice.get(&format!("/files/{id}/download")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        alice.get(&format!("/files/{id}")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        bob.get(&format!("/files/{id}/download")).await.status,
        StatusCode::OK
    );

    // Bob le garde dans l'espace partagé : Alice le voit alors.
    let kept = bob
        .post(
            &format!("/files/{id}/keep"),
            &[("space_id", &ecole.to_string())],
        )
        .await;
    assert_eq!(kept.status, StatusCode::SEE_OTHER, "{}", kept.body);
    assert!(
        alice
            .get("/e/ecole/files?dossier=espace")
            .await
            .body
            .contains("brouillon.pdf")
    );
    let _ = slug;
}

#[tokio::test]
async fn un_membre_voit_et_revoque_un_lien_d_alice() {
    let (app, mut alice, mut bob, ecole, slug) = setup().await;
    upload_in(&mut alice, ecole, "rapport.pdf", b"%PDF rapport").await;
    let id = file_id(&app, "rapport.pdf").await;
    let created = alice
        .post(&format!("/files/{id}/share"), &[("duree", "1j")])
        .await;
    assert_eq!(created.status, StatusCode::OK, "{}", created.body);
    let at = created.body.find("/s/").unwrap() + 3;
    let token: String = created.body[at..].chars().take(64).collect();
    let link: i64 = sqlx::query_scalar("SELECT id FROM share_links")
        .fetch_one(&app.state.db)
        .await
        .unwrap();

    // Bob voit le lien d'Alice dans l'aperçu, avec son bouton « Révoquer ».
    let view = bob.get(&format!("/e/{slug}/files/{id}")).await;
    assert_eq!(view.status, StatusCode::OK);
    assert!(
        view.body
            .contains(&format!("/files/{id}/share/{link}/revoke")),
        "le lien d'Alice n'est pas listé"
    );
    // Le fichier est repéré comme partagé dans la liste de Bob.
    let list = bob.get(&format!("/e/{slug}/files?dossier=espace")).await;
    assert!(list.body.contains("rapport.pdf"));

    let mut stranger = app.client();
    assert_eq!(
        stranger.get(&format!("/s/{token}")).await.status,
        StatusCode::OK
    );
    let revoked = bob
        .post(&format!("/files/{id}/share/{link}/revoke"), &[])
        .await;
    assert_eq!(revoked.status, StatusCode::OK, "{}", revoked.body);
    assert_eq!(
        stranger.get(&format!("/s/{token}")).await.status,
        StatusCode::NOT_FOUND
    );

    // Bob crée aussi un lien, qu'Alice voit ; un lien d'un fichier privé, non.
    let by_bob = bob
        .post(&format!("/files/{id}/share"), &[("duree", "7j")])
        .await;
    assert_eq!(by_bob.status, StatusCode::OK, "{}", by_bob.body);
    let alice_view = alice.get(&format!("/e/ecole/files/{id}")).await;
    assert!(alice_view.body.contains("/revoke"));
}

#[tokio::test]
async fn un_lien_ne_sert_plus_un_fichier_sorti_de_l_espace_partage() {
    // Bob partage un fichier de l'espace commun, puis Alice le range dans son
    // espace privé : le lien de Bob ne doit pas continuer à le servir.
    let (app, mut alice, mut bob, ecole, _slug) = setup().await;
    upload_in(&mut alice, ecole, "prive.pdf", b"%PDF prive").await;
    let id = file_id(&app, "prive.pdf").await;
    let created = bob
        .post(&format!("/files/{id}/share"), &[("duree", "1j")])
        .await;
    assert_eq!(created.status, StatusCode::OK, "{}", created.body);
    let at = created.body.find("/s/").unwrap() + 3;
    let token: String = created.body[at..].chars().take(64).collect();

    let mut stranger = app.client();
    assert_eq!(
        stranger.get(&format!("/s/{token}")).await.status,
        StatusCode::OK
    );

    let taff = app.space_id(ALICE, "taff").await;
    let moved = alice
        .post(
            &format!("/files/{id}/move"),
            &[("space_id", &taff.to_string())],
        )
        .await;
    assert_eq!(moved.status, StatusCode::SEE_OTHER, "{}", moved.body);
    assert_eq!(
        stranger.get(&format!("/s/{token}")).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn deux_tags_de_meme_nom_n_en_font_qu_un() {
    let (app, mut alice, mut bob, ecole, slug) = setup().await;
    upload_in(&mut alice, ecole, "cours.pdf", b"%PDF cours").await;
    let id = file_id(&app, "cours.pdf").await;

    let first = alice
        .post(&format!("/files/{id}/tags"), &[("name", "Maths")])
        .await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.body);
    let second = bob
        .post(&format!("/files/{id}/tags"), &[("name", "maths")])
        .await;
    assert_eq!(second.status, StatusCode::OK, "{}", second.body);
    assert_eq!(second.body.matches("#maths").count(), 1, "{}", second.body);
    let linked: i64 = sqlx::query_scalar("SELECT count(*) FROM file_tags WHERE file_id = ?")
        .bind(id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(linked, 1, "un seul tag est lié au fichier");

    // Une seule pastille de filtre, avec un fichier.
    let list = bob.get(&format!("/e/{slug}/files?dossier=espace")).await;
    assert_eq!(list.body.matches("tag=maths").count(), 1, "{}", list.body);

    // Bob retire le tag d'Alice : il disparaît pour les deux.
    let tag: i64 = sqlx::query_scalar("SELECT tag_id FROM file_tags WHERE file_id = ?")
        .bind(id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    let removed = bob.delete(&format!("/files/{id}/tags/{tag}")).await;
    assert_eq!(removed.status, StatusCode::OK);
    assert!(
        !alice
            .get(&format!("/e/ecole/files/{id}"))
            .await
            .body
            .contains("#maths")
    );
}

#[tokio::test]
async fn un_agenda_de_bob_reste_invisible_a_alice() {
    let (app, mut alice, mut bob, ecole, slug) = setup().await;
    let bob_id = app.user_id(BOB).await;
    let source: i64 = sqlx::query_scalar(
        "INSERT INTO calendar_sources (user_id, space_id, name, ical_url, created_at)
         VALUES (?, ?, 'Agenda de Bob', 'https://example.test/bob.ics', 0) RETURNING id",
    )
    .bind(bob_id)
    .bind(ecole)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    sqlx::query(
        "INSERT INTO calendar_events (source_id, uid, title, location, starts_at, ends_at)
         VALUES (?, 'x', 'RENDEZVOUS-SECRET-DE-BOB', 'B1', ?, ?)",
    )
    .bind(source)
    .bind(now - 60)
    .bind(now + 3 * 3600)
    .execute(&app.state.db)
    .await
    .unwrap();

    let bobs = bob.get(&format!("/e/{slug}/calendar")).await;
    assert_eq!(bobs.status, StatusCode::OK);
    assert!(bobs.body.contains("RENDEZVOUS-SECRET-DE-BOB"));
    let alices = alice.get("/e/ecole/calendar").await;
    assert_eq!(alices.status, StatusCode::OK);
    assert!(!alices.body.contains("RENDEZVOUS-SECRET-DE-BOB"));
    assert!(!alices.body.contains("Agenda de Bob"));
}
