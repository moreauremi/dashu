//! Espaces partagés : invitations, membres, départ (SPEC, « Espaces
//! partagés »). Les droits du propriétaire sont vérifiés par le serveur.

mod common;

use axum::http::StatusCode;
use common::{ALICE, BOB, Client, TestApp};

/// Nombre de lignes renvoyées par une requête SQL à un seul entier.
async fn scalar(app: &TestApp, sql: &str) -> i64 {
    sqlx::query_scalar(sql)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

/// Alice invite Bob dans son espace `slug` ; renvoie la page Membres suivie.
async fn invite(app: &TestApp, alice: &mut Client, slug: &str, email: &str) -> String {
    let space = app.space_id(ALICE, slug).await;
    let response = alice
        .post(
            &format!("/settings/spaces/{space}/invitations"),
            &[("email", email)],
        )
        .await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(
        response.location(),
        format!("/settings/spaces/{space}/members")
    );
    alice.follow(response).await.body
}

async fn invitations(app: &TestApp) -> i64 {
    scalar(app, "SELECT count(*) FROM space_invitations").await
}

async fn invitation_of(app: &TestApp, email: &str) -> i64 {
    let user = app.user_id(email).await;
    sqlx::query_scalar("SELECT id FROM space_invitations WHERE user_id = ?")
        .bind(user)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn inviter_puis_accepter() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let mut bob = app.logged_in(BOB).await;
    // Bob a déjà un « ecole » : l'espace partagé prendra une adresse suffixée.
    let page = invite(&app, &mut alice, "ecole", BOB).await;
    assert!(page.contains("Invitation envoyée"));
    assert!(page.contains("Invitations en attente"));

    let settings = bob.get("/settings").await;
    assert!(settings.body.contains("Invitations"));
    assert!(settings.body.contains("Accepter"));
    assert!(settings.body.contains("Refuser"));

    let inv = invitation_of(&app, BOB).await;
    let accepted = bob
        .post(&format!("/settings/invitations/{inv}/accept"), &[])
        .await;
    assert_eq!(accepted.status, StatusCode::SEE_OTHER);
    assert_eq!(invitations(&app).await, 0);

    // Son slug est suffixé et il arrive en dernier dans son île.
    let (slug, position): (String, i64) =
        sqlx::query_as("SELECT slug, position FROM space_members WHERE user_id = ?")
            .bind(app.user_id(BOB).await)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(slug, "ecole-2");
    assert_eq!(position, 3);
    assert_eq!(bob.get("/e/ecole-2/").await.status, StatusCode::OK);

    let settings = bob.get("/settings").await;
    assert!(settings.body.contains("Partagé par"));
    assert!(!settings.body.contains("invité par"));
    // Alice voit qu'elle partage avec une personne.
    let settings = alice.get("/settings").await;
    assert!(settings.body.contains("Partagé avec 1 personne"));
    assert!(!settings.body.contains("1 personnes"));
}

#[tokio::test]
async fn refus_d_invitation_sans_rien_creer() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    // Un compte qui existe mais n'est plus dans la liste autorisée.
    app.add_user("dave@example.test").await;

    let cases = [
        ("carol@example.test", "Aucun compte"),
        ("dave@example.test", "Aucun compte"),
        ("", "Saisis"),
        (ALICE, "déjà le propriétaire"),
    ];
    for (email, message) in cases {
        let page = invite(&app, &mut alice, "ecole", email).await;
        assert!(page.contains(message), "{email} : {page}");
    }
    assert_eq!(invitations(&app).await, 0);

    // Perso reste privé.
    let perso = app.space_id(ALICE, "perso").await;
    let response = alice
        .post(
            &format!("/settings/spaces/{perso}/invitations"),
            &[("email", BOB)],
        )
        .await;
    let page = alice.follow(response).await;
    assert!(page.body.contains("reste privé"));
    assert_eq!(invitations(&app).await, 0);

    // Doublon.
    invite(&app, &mut alice, "ecole", BOB).await;
    let page = invite(&app, &mut alice, "ecole", BOB).await;
    assert!(page.contains("déjà une invitation"));
    assert_eq!(invitations(&app).await, 1);

    // Déjà membre.
    app.share_space(ALICE, "taff", BOB).await;
    let page = invite(&app, &mut alice, "taff", BOB).await;
    assert!(page.contains("déjà membre"));
    assert_eq!(invitations(&app).await, 1);

    // Espace archivé.
    sqlx::query("UPDATE spaces SET archived_at = 1 WHERE id = ?")
        .bind(app.space_id(ALICE, "ecole").await)
        .execute(&app.state.db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM space_invitations")
        .execute(&app.state.db)
        .await
        .unwrap();
    let page = invite(&app, &mut alice, "ecole", BOB).await;
    assert!(page.contains("archivé"));
    assert_eq!(invitations(&app).await, 0);
}

#[tokio::test]
async fn refuser_et_annuler_une_invitation() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let mut bob = app.logged_in(BOB).await;
    invite(&app, &mut alice, "ecole", BOB).await;
    let inv = invitation_of(&app, BOB).await;

    let declined = bob
        .post(&format!("/settings/invitations/{inv}/decline"), &[])
        .await;
    assert_eq!(declined.status, StatusCode::SEE_OTHER);
    assert_eq!(invitations(&app).await, 0);
    assert_eq!(scalar(&app, "SELECT count(*) FROM space_members").await, 0);
    // Refuser deux fois : plus rien à refuser.
    let again = bob
        .post(&format!("/settings/invitations/{inv}/decline"), &[])
        .await;
    assert_eq!(again.status, StatusCode::NOT_FOUND);

    // Le propriétaire réinvite, puis annule.
    invite(&app, &mut alice, "ecole", BOB).await;
    let inv = invitation_of(&app, BOB).await;
    let space = app.space_id(ALICE, "ecole").await;
    let cancelled = alice
        .post(
            &format!("/settings/spaces/{space}/invitations/{inv}/cancel"),
            &[],
        )
        .await;
    assert_eq!(cancelled.status, StatusCode::SEE_OTHER);
    assert_eq!(invitations(&app).await, 0);
}

#[tokio::test]
async fn invitation_d_un_autre_compte() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    invite(&app, &mut alice, "ecole", BOB).await;
    let inv = invitation_of(&app, BOB).await;

    // Alice n'est pas l'invitée : ni accepter, ni refuser.
    let accepted = alice
        .post(&format!("/settings/invitations/{inv}/accept"), &[])
        .await;
    assert_eq!(accepted.status, StatusCode::NOT_FOUND);
    let declined = alice
        .post(&format!("/settings/invitations/{inv}/decline"), &[])
        .await;
    assert_eq!(declined.status, StatusCode::NOT_FOUND);
    assert_eq!(invitations(&app).await, 1);
    assert_eq!(scalar(&app, "SELECT count(*) FROM space_members").await, 0);
}

#[tokio::test]
async fn limite_de_huit_espaces() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let mut bob = app.logged_in(BOB).await;
    for name in ["Un", "Deux", "Trois", "Quatre", "Cinq"] {
        bob.post(
            "/settings/spaces",
            &[("name", name), ("color", "blue"), ("icon", "home")],
        )
        .await;
    }
    assert_eq!(
        scalar(
            &app,
            &format!(
                "SELECT count(*) FROM spaces WHERE user_id = {}",
                app.user_id(BOB).await
            )
        )
        .await,
        8
    );
    invite(&app, &mut alice, "ecole", BOB).await;
    let inv = invitation_of(&app, BOB).await;
    let response = bob
        .post(&format!("/settings/invitations/{inv}/accept"), &[])
        .await;
    let page = bob.follow(response).await;
    assert!(page.body.contains("8 espaces au plus"));
    assert_eq!(scalar(&app, "SELECT count(*) FROM space_members").await, 0);
    assert_eq!(invitations(&app).await, 1);
}

#[tokio::test]
async fn un_membre_ne_gere_pas_l_espace() {
    let app = TestApp::new().await;
    app.logged_in(ALICE).await;
    app.logged_in(BOB).await;
    let slug = app.share_space(ALICE, "ecole", BOB).await;
    let space = app.space_id(ALICE, "ecole").await;
    let mut bob = app.logged_in(BOB).await;

    let form = [("name", "Pirate"), ("color", "blue"), ("icon", "home")];
    let update = bob.post(&format!("/settings/spaces/{space}"), &form).await;
    assert_eq!(update.status, StatusCode::FORBIDDEN);
    let archive = bob
        .post(&format!("/settings/spaces/{space}/archive"), &[])
        .await;
    assert_eq!(archive.status, StatusCode::FORBIDDEN);
    sqlx::query("UPDATE spaces SET archived_at = 1 WHERE id = ?")
        .bind(space)
        .execute(&app.state.db)
        .await
        .unwrap();
    for action in ["restore"] {
        let response = bob
            .post(&format!("/settings/spaces/{space}/{action}"), &[])
            .await;
        assert_eq!(response.status, StatusCode::FORBIDDEN, "{action}");
    }
    let page = bob.get(&format!("/settings/spaces/{space}/delete")).await;
    assert_eq!(page.status, StatusCode::FORBIDDEN);
    let delete = bob
        .post(
            &format!("/settings/spaces/{space}/delete"),
            &[("content", "delete")],
        )
        .await;
    assert_eq!(delete.status, StatusCode::FORBIDDEN);
    let invite = bob
        .post(
            &format!("/settings/spaces/{space}/invitations"),
            &[("email", ALICE)],
        )
        .await;
    assert_eq!(invite.status, StatusCode::FORBIDDEN);
    let alice_id = app.user_id(ALICE).await;
    let remove = bob
        .post(
            &format!("/settings/spaces/{space}/members/{alice_id}/remove"),
            &[],
        )
        .await;
    assert_eq!(remove.status, StatusCode::FORBIDDEN);

    let name: String = sqlx::query_scalar("SELECT name FROM spaces WHERE id = ?")
        .bind(space)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(name, "École");

    // Sa page Membres : lecture et « Quitter », sans formulaire d'invitation.
    let members = bob.get(&format!("/settings/spaces/{space}/members")).await;
    assert_eq!(members.status, StatusCode::OK);
    assert!(members.body.contains("Quitter l'espace"));
    assert!(!members.body.contains("/invitations\""));
    // Le propriétaire, lui, ne « quitte » pas.
    let mut alice = app.logged_in(ALICE).await;
    let leave = alice
        .post(&format!("/settings/spaces/{space}/leave"), &[])
        .await;
    assert_eq!(leave.status, StatusCode::FORBIDDEN);
    // Perso n'a pas de page Membres, et un espace inconnu non plus.
    let perso = app.space_id(ALICE, "perso").await;
    let page = alice
        .get(&format!("/settings/spaces/{perso}/members"))
        .await;
    assert_eq!(page.status, StatusCode::NOT_FOUND);
    let inaccessible = bob.get(&format!("/settings/spaces/{perso}/members")).await;
    assert_eq!(inaccessible.status, StatusCode::NOT_FOUND);
    assert_eq!(bob.get(&format!("/e/{slug}/")).await.status, StatusCode::OK);
}

/// Prépare ce que le départ de Bob doit défaire : un agenda dans l'espace,
/// un lien de partage et un espace « moment ». Renvoie (espace, lien).
async fn prepare_departure(app: &TestApp) -> (i64, i64) {
    // Les espaces d'un compte naissent à sa première connexion.
    app.logged_in(ALICE).await;
    app.logged_in(BOB).await;
    app.share_space(ALICE, "ecole", BOB).await;
    let space = app.space_id(ALICE, "ecole").await;
    let bob = app.user_id(BOB).await;
    sqlx::query(
        "INSERT INTO calendar_sources (user_id, space_id, name, ical_url, created_at)
         VALUES (?, ?, 'Cours', 'https://example.test/x.ics', 0)",
    )
    .bind(bob)
    .bind(space)
    .execute(&app.state.db)
    .await
    .unwrap();
    let file = sqlx::query(
        "INSERT INTO files (user_id, space_id, name, mime, size, sha256, storage_key, created_at, updated_at)
         VALUES (?, ?, 'cours.txt', 'text/plain', 1, 'x', 'cle-depart', 0, 0)",
    )
    .bind(bob)
    .bind(space)
    .execute(&app.state.db)
    .await
    .unwrap()
    .last_insert_rowid();
    let link = sqlx::query(
        "INSERT INTO share_links (user_id, file_id, token_hash, expires_at, created_at)
         VALUES (?, ?, 'hash-depart', 4102444800, 0)",
    )
    .bind(bob)
    .bind(file)
    .execute(&app.state.db)
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query("UPDATE users SET moment_space = ? WHERE id = ?")
        .bind(space)
        .bind(bob)
        .execute(&app.state.db)
        .await
        .unwrap();
    (space, link)
}

/// Bob n'a plus rien de l'espace, mais le contenu y est resté.
async fn assert_departed(app: &TestApp, space: i64, link: i64, bob: &mut Client) {
    assert_eq!(scalar(app, "SELECT count(*) FROM space_members").await, 0);
    assert_eq!(bob.get("/e/ecole-2/").await.status, StatusCode::NOT_FOUND);
    let perso = app.space_id(BOB, "perso").await;
    let calendar: i64 = sqlx::query_scalar("SELECT space_id FROM calendar_sources")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(calendar, perso, "agenda revenu dans Perso");
    let revoked: Option<i64> =
        sqlx::query_scalar("SELECT revoked_at FROM share_links WHERE id = ?")
            .bind(link)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert!(revoked.is_some(), "lien de partage révoqué");
    let moment: Option<i64> = sqlx::query_scalar("SELECT moment_space FROM users WHERE id = ?")
        .bind(app.user_id(BOB).await)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(moment, None);
    let files: i64 = sqlx::query_scalar("SELECT count(*) FROM files WHERE space_id = ?")
        .bind(space)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(files, 1, "ce qu'il a créé reste");
}

#[tokio::test]
async fn quitter_un_espace() {
    let app = TestApp::new().await;
    let (space, link) = prepare_departure(&app).await;
    let mut bob = app.logged_in(BOB).await;
    assert_eq!(bob.get("/e/ecole-2/").await.status, StatusCode::OK);

    let response = bob
        .post(&format!("/settings/spaces/{space}/leave"), &[])
        .await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_departed(&app, space, link, &mut bob).await;
}

#[tokio::test]
async fn retirer_un_membre() {
    let app = TestApp::new().await;
    let (space, link) = prepare_departure(&app).await;
    let mut alice = app.logged_in(ALICE).await;
    let mut bob = app.logged_in(BOB).await;
    let bob_id = app.user_id(BOB).await;

    let members = alice
        .get(&format!("/settings/spaces/{space}/members"))
        .await;
    assert!(members.body.contains("Retirer"));
    let response = alice
        .post(
            &format!("/settings/spaces/{space}/members/{bob_id}/remove"),
            &[],
        )
        .await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_departed(&app, space, link, &mut bob).await;

    // Retirer quelqu'un qui n'est pas membre : rien à retirer.
    let again = alice
        .post(
            &format!("/settings/spaces/{space}/members/{bob_id}/remove"),
            &[],
        )
        .await;
    assert_eq!(again.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn supprimer_un_espace_partage_en_gardant_le_contenu() {
    let app = TestApp::new().await;
    let (space, link) = prepare_departure(&app).await;
    let mut alice = app.logged_in(ALICE).await;

    // La page de confirmation compte le contenu de Bob aussi.
    let page = alice.get(&format!("/settings/spaces/{space}/delete")).await;
    assert!(page.body.contains("1 fichier"));

    let taff = app.space_id(ALICE, "taff").await;
    let target = taff.to_string();
    let response = alice
        .post(
            &format!("/settings/spaces/{space}/delete"),
            &[("content", "move"), ("target", &target)],
        )
        .await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(scalar(&app, "SELECT count(*) FROM space_members").await, 0);
    let moved: i64 = sqlx::query_scalar("SELECT count(*) FROM files WHERE space_id = ?")
        .bind(taff)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(moved, 1, "le fichier de Bob suit");
    let revoked: Option<i64> =
        sqlx::query_scalar("SELECT revoked_at FROM share_links WHERE id = ?")
            .bind(link)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert!(revoked.is_some());
    let perso = app.space_id(BOB, "perso").await;
    let calendar: i64 = sqlx::query_scalar("SELECT space_id FROM calendar_sources")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(calendar, perso);
    let mut bob = app.logged_in(BOB).await;
    assert_eq!(bob.get("/e/ecole-2/").await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn supprimer_un_espace_partage_avec_son_contenu() {
    let app = TestApp::new().await;
    let (space, _) = prepare_departure(&app).await;
    let mut alice = app.logged_in(ALICE).await;
    let response = alice
        .post(
            &format!("/settings/spaces/{space}/delete"),
            &[("content", "delete")],
        )
        .await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(scalar(&app, "SELECT count(*) FROM files").await, 0);
    assert_eq!(scalar(&app, "SELECT count(*) FROM space_members").await, 0);
    // L'agenda de Bob a quitté l'espace avant sa suppression.
    assert_eq!(
        scalar(&app, "SELECT count(*) FROM calendar_sources").await,
        1
    );
    let mut bob = app.logged_in(BOB).await;
    assert_eq!(bob.get("/e/ecole-2/").await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn le_transfert_n_accepte_que_les_espaces_du_proprietaire() {
    let app = TestApp::new().await;
    app.logged_in(ALICE).await;
    app.logged_in(BOB).await;
    // Alice partage « ecole » ; Bob partage « taff » avec Alice.
    app.share_space(ALICE, "ecole", BOB).await;
    app.share_space(BOB, "taff", ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    let bob_taff = app.space_id(BOB, "taff").await;
    let mut alice = app.logged_in(ALICE).await;

    let page = alice.get(&format!("/settings/spaces/{ecole}/delete")).await;
    assert!(!page.body.contains(&format!("value=\"{bob_taff}\"")));
    let target = bob_taff.to_string();
    let response = alice
        .post(
            &format!("/settings/spaces/{ecole}/delete"),
            &[("content", "move"), ("target", &target)],
        )
        .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    // Rien n'a bougé : Bob est toujours membre.
    assert_eq!(
        scalar(
            &app,
            &format!("SELECT count(*) FROM space_members WHERE space_id = {ecole}")
        )
        .await,
        1
    );
}
