//! Isolation des données : un utilisateur ne voit ni ne modifie jamais les
//! données d'un autre, même en devinant leurs identifiants.

mod common;

use axum::http::StatusCode;
use common::{ALICE, BOB, TestApp, note_id};
use dashu::calendar::ical::ParsedEvent;
use dashu::calendar::sync;

/// Mot présent uniquement dans les données d'Alice.
const SECRET: &str = "ornithorynque";

#[tokio::test]
async fn les_notes_d_alice_sont_invisibles_et_intouchables_pour_bob() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let mut bob = app.logged_in(BOB).await;

    let created = alice.post("/notes", &[]).await;
    let id = note_id(&created);
    alice
        .post(
            &format!("/notes/{id}"),
            &[
                ("title", "Journal"),
                ("body_md", &format!("Mot de passe du wifi : {SECRET}")),
            ],
        )
        .await;
    alice
        .post(&format!("/notes/{id}/tags"), &[("name", "prive")])
        .await;
    let tag_id: i64 = sqlx::query_scalar("SELECT id FROM tags WHERE name = 'prive'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();

    // Lecture : la note n'existe pas pour Bob, où qu'il regarde.
    assert_eq!(
        bob.get(&format!("/notes/{id}")).await.status,
        StatusCode::NOT_FOUND
    );
    for path in [
        "/notes",
        "/notes?tag=prive",
        "/cmd/suggest?q=",
        "/cmd/catalogue",
        "/widgets/notes",
        "/",
    ] {
        let response = bob.get(path).await;
        assert_eq!(response.status, StatusCode::OK, "{path}");
        assert!(
            !response.body.contains(SECRET),
            "{path} laisse fuiter le contenu"
        );
        assert!(
            !response.body.contains("Journal"),
            "{path} laisse fuiter le titre"
        );
        assert!(
            !response.body.contains("#prive"),
            "{path} laisse fuiter le tag"
        );
    }
    // Recherche du mot exact, dans la liste et dans /open : aucun résultat.
    // (Le mot lui-même réapparaît dans le champ de recherche de Bob, c'est normal.)
    let list = bob.get(&format!("/notes?q={SECRET}")).await;
    assert!(list.body.contains("Aucune note ne correspond<"));
    assert!(!list.body.contains("note-row") && !list.body.contains("Journal"));
    let open = bob.get_fragment(&format!("/cmd/suggest?q={SECRET}")).await;
    assert_eq!(open.body, r#"{"items":[]}"#);
    let refused = bob.command(&format!("/open {SECRET}"), "/").await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    // Alice, elle, retrouve bien sa note avec la même recherche.
    assert!(
        alice
            .get(&format!("/notes?q={SECRET}"))
            .await
            .body
            .contains("note-row")
    );

    // Écriture : toutes les actions répondent 404 et ne changent rien.
    let bob_space = app.space_id(BOB, "perso").await.to_string();
    let attempts = [
        bob.post(
            &format!("/notes/{id}"),
            &[("title", "piraté"), ("body_md", "piraté")],
        )
        .await,
        bob.post(&format!("/notes/{id}/pin"), &[]).await,
        bob.post(&format!("/notes/{id}/move"), &[("space_id", &bob_space)])
            .await,
        bob.post(&format!("/notes/{id}/tags"), &[("name", "bob")])
            .await,
        bob.delete(&format!("/notes/{id}/tags/{tag_id}")).await,
        bob.post(&format!("/notes/{id}/delete"), &[]).await,
    ];
    for (index, response) in attempts.iter().enumerate() {
        assert_eq!(response.status, StatusCode::NOT_FOUND, "action {index}");
    }

    // La note d'Alice est intacte.
    let page = alice.get(&format!("/notes/{id}")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains(SECRET));
    assert!(page.body.contains("value=\"Journal\""));
    assert!(page.body.contains("#prive"));
    assert!(!page.body.contains("piraté"));
}

#[tokio::test]
async fn bob_ne_peut_pas_deplacer_sa_note_dans_un_espace_d_alice() {
    let app = TestApp::new().await;
    let _alice = app.logged_in(ALICE).await;
    let mut bob = app.logged_in(BOB).await;

    let created = bob.post("/notes", &[]).await;
    let id = note_id(&created);
    let alice_space = app.space_id(ALICE, "perso").await.to_string();

    let response = bob
        .post(&format!("/notes/{id}/move"), &[("space_id", &alice_space)])
        .await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);

    let owner_space: i64 = sqlx::query_scalar("SELECT space_id FROM notes WHERE id = ?")
        .bind(id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(owner_space, app.space_id(BOB, "perso").await);
}

#[tokio::test]
async fn les_taches_sont_cloisonnees() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let mut bob = app.logged_in(BOB).await;

    alice
        .post("/tasks", &[("title", &format!("Nourrir l'{SECRET}"))])
        .await;
    let task_id: i64 = sqlx::query_scalar("SELECT id FROM tasks")
        .fetch_one(&app.state.db)
        .await
        .unwrap();

    assert!(!bob.get("/widgets/tasks").await.body.contains(SECRET));
    assert_eq!(
        bob.post(&format!("/tasks/{task_id}/toggle"), &[])
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        bob.delete(&format!("/tasks/{task_id}")).await.status,
        StatusCode::NOT_FOUND
    );

    let done: bool = sqlx::query_scalar("SELECT done FROM tasks WHERE id = ?")
        .bind(task_id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert!(!done, "la tâche d'Alice n'a pas été cochée par Bob");
    assert!(alice.get("/widgets/tasks").await.body.contains(SECRET));
}

#[tokio::test]
async fn les_agendas_sont_cloisonnes() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let mut bob = app.logged_in(BOB).await;

    // Une source iCal d'Alice, avec un événement en cours.
    let now = dashu::time::now();
    let source_id = sqlx::query(
        "INSERT INTO calendar_sources (user_id, space_id, name, ical_url, created_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(app.user_id(ALICE).await)
    .bind(app.space_id(ALICE, "perso").await)
    .bind("AgendaAlice")
    .bind("https://campus.example.test/ical/alice.ics?jeton=SECRET-ALICE")
    .bind(now)
    .execute(&app.state.db)
    .await
    .unwrap()
    .last_insert_rowid();
    let event = ParsedEvent {
        uid: "1".into(),
        title: format!("Rendez-vous {SECRET}"),
        location: "Cabinet".into(),
        starts_at: now - 600,
        ends_at: now + 3600,
        all_day: false,
    };
    sync::store(&app.state.db, source_id, &[event], now)
        .await
        .unwrap();

    assert!(alice.get("/widgets/courses").await.body.contains(SECRET));
    assert!(alice.get("/calendar?view=day").await.body.contains(SECRET));

    for path in [
        "/widgets/courses",
        "/calendar",
        "/calendar?view=day",
        "/settings",
    ] {
        let response = bob.get(path).await;
        assert!(!response.body.contains(SECRET), "{path}");
        assert!(!response.body.contains("AgendaAlice"), "{path}");
    }
    let response = bob
        .post(&format!("/settings/calendars/{source_id}/delete"), &[])
        .await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);

    // L'URL du flux est un secret : même Alice ne la revoit jamais en entier.
    let settings = alice.get("/settings").await;
    assert!(settings.body.contains("https://campus.example.test/…"));
    assert!(!settings.body.contains("SECRET-ALICE"));
}
