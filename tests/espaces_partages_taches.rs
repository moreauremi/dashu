//! Espaces partagés : tâches, tableau de bord, barre de commandes et API.

mod common;

use axum::http::{Method, StatusCode};
use common::{ALICE, BOB, TestApp};

const T: Option<&str> = Some(common::API_TOKEN);

async fn add_task(app: &TestApp, email: &str, space: i64, title: &str) -> i64 {
    let user = app.user_id(email).await;
    sqlx::query_scalar(
        "INSERT INTO tasks (user_id, space_id, title, due_date, created_at)
         VALUES (?, ?, ?, '2000-01-01', 0) RETURNING id",
    )
    .bind(user)
    .bind(space)
    .bind(title)
    .fetch_one(&app.state.db)
    .await
    .unwrap()
}

async fn add_note(app: &TestApp, email: &str, space: i64, title: &str, body: &str) -> i64 {
    let user = app.user_id(email).await;
    sqlx::query_scalar(
        "INSERT INTO notes (user_id, space_id, title, body_md, created_at, updated_at)
         VALUES (?, ?, ?, ?, 0, 0) RETURNING id",
    )
    .bind(user)
    .bind(space)
    .bind(title)
    .bind(body)
    .fetch_one(&app.state.db)
    .await
    .unwrap()
}

async fn task_count(app: &TestApp, title: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM tasks WHERE title = ?")
        .bind(title)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn les_membres_partagent_les_taches() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let mut bob = app.logged_in(BOB).await;
    let slug = app.share_space(ALICE, "ecole", BOB).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    let taff = app.space_id(ALICE, "taff").await;

    // Bob voit une tâche d'Alice et en ajoute une.
    add_task(&app, ALICE, ecole, "Rapport de stage").await;
    let widget = bob
        .with_header("x-espace", &slug)
        .get_fragment("/widgets/tasks")
        .await;
    assert!(widget.body.contains("Rapport de stage"));
    let added = bob
        .post(
            "/tasks",
            &[("title", "Réviser le TP"), ("space_id", &ecole.to_string())],
        )
        .await;
    assert_eq!(added.status, StatusCode::OK);
    assert_eq!(task_count(&app, "Réviser le TP").await, 1);

    // Alice voit celle de Bob.
    let widget = alice
        .with_header("x-espace", "ecole")
        .get_fragment("/widgets/tasks")
        .await;
    assert!(widget.body.contains("Réviser le TP"));

    // Bob coche, puis supprime les tâches de l'espace.
    let id: i64 = sqlx::query_scalar("SELECT id FROM tasks WHERE title = 'Rapport de stage'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(
        bob.post(&format!("/tasks/{id}/toggle"), &[]).await.status,
        StatusCode::OK
    );
    let done: bool = sqlx::query_scalar("SELECT done FROM tasks WHERE id = ?")
        .bind(id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert!(done);
    assert_eq!(
        bob.delete(&format!("/tasks/{id}")).await.status,
        StatusCode::OK
    );
    assert_eq!(task_count(&app, "Rapport de stage").await, 0);

    // Le Perso de Bob réunit les tâches de l'espace partagé.
    let widget = bob.get_fragment("/widgets/tasks").await;
    assert!(widget.body.contains("Réviser le TP"));

    // Une tâche de `taff` (non partagé) est refusée à Bob.
    let secret = add_task(&app, ALICE, taff, "Dossier client").await;
    assert_eq!(
        bob.post(&format!("/tasks/{secret}/toggle"), &[])
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        bob.delete(&format!("/tasks/{secret}")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        bob.post(
            "/tasks",
            &[("title", "Intrus"), ("space_id", &taff.to_string())]
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(task_count(&app, "Dossier client").await, 1);
    assert!(
        !bob.get_fragment("/widgets/tasks")
            .await
            .body
            .contains("Dossier client")
    );
    // Il ne peut pas non plus y déplacer une tâche.
    let mine = add_task(&app, BOB, ecole, "À déplacer").await;
    let moved = bob
        .post(
            &format!("/tasks/{mine}/move"),
            &[("space_id", &taff.to_string())],
        )
        .await;
    assert_eq!(moved.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn un_membre_modifie_le_tableau_de_bord_partage() {
    let app = TestApp::new().await;
    app.logged_in(ALICE).await;
    let mut bob = app.logged_in(BOB).await;
    let slug = app.share_space(ALICE, "ecole", BOB).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    let taff = app.space_id(ALICE, "taff").await;

    let saved = bob
        .with_header("x-espace", &slug)
        .post("/dashboard/layout", &[("layout", "notes:1x1")])
        .await;
    assert_eq!(saved.status, StatusCode::NO_CONTENT);
    let layout = |id: i64| {
        let db = app.state.db.clone();
        async move {
            sqlx::query_scalar::<_, Option<String>>("SELECT dashboard FROM spaces WHERE id = ?")
                .bind(id)
                .fetch_one(&db)
                .await
                .unwrap()
        }
    };
    assert_eq!(layout(ecole).await.as_deref(), Some("notes:1x1"));
    assert_eq!(layout(taff).await, None);
}

#[tokio::test]
async fn open_trouve_les_notes_des_espaces_partages() {
    let app = TestApp::new().await;
    app.logged_in(ALICE).await;
    let mut bob = app.logged_in(BOB).await;
    app.share_space(ALICE, "ecole", BOB).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    let taff = app.space_id(ALICE, "taff").await;
    let partagee = add_note(&app, ALICE, ecole, "Trunk partagé", "Les VLAN du groupe").await;
    add_note(&app, ALICE, taff, "Trunk secret", "Les VLAN du client").await;

    let found = bob.get_fragment("/cmd/suggest?q=trunk").await;
    assert!(found.body.contains("Trunk partagé"));
    assert!(!found.body.contains("Trunk secret"));
    let opened = common::hx_location(&bob.command("/open trunk", "/").await);
    assert_eq!(opened["path"], format!("/notes/{partagee}"));
    // Sans texte : seulement les notes accessibles.
    let recent = bob.get_fragment("/cmd/suggest?q=").await;
    assert!(recent.body.contains("Trunk partagé"));
    assert!(!recent.body.contains("Trunk secret"));
}

#[tokio::test]
async fn l_api_voit_les_espaces_partages() {
    let app = TestApp::new().await;
    // Bob partage son `ecole` avec Alice : elle l'a sous « ecole-2 ».
    app.logged_in(ALICE).await;
    app.logged_in(BOB).await;
    let slug = app.share_space(BOB, "ecole", ALICE).await;
    assert_eq!(slug, "ecole-2");
    let ecole = app.space_id(BOB, "ecole").await;
    add_task(&app, BOB, ecole, "Tâche de Bob").await;
    let note = add_note(&app, BOB, ecole, "Note de Bob", "texte").await;

    let (_, spaces) = app.api(Method::GET, "/api/v1/spaces", None, T).await;
    assert!(
        spaces
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["slug"] == "ecole-2")
    );

    let (_, tasks) = app
        .api(Method::GET, "/api/v1/tasks?space=ecole-2", None, T)
        .await;
    assert_eq!(tasks[0]["title"], "Tâche de Bob");
    assert_eq!(tasks[0]["space"], "ecole-2");

    let (_, notes) = app
        .api(Method::GET, "/api/v1/notes?space=ecole-2", None, T)
        .await;
    assert_eq!(notes[0]["title"], "Note de Bob");

    // Une modification de texte fait monter le numéro de version.
    let revision = || async {
        sqlx::query_scalar::<_, i64>("SELECT revision FROM notes WHERE id = ?")
            .bind(note)
            .fetch_one(&app.state.db)
            .await
            .unwrap()
    };
    assert_eq!(revision().await, 0);
    let (status, _) = app
        .api(
            Method::PATCH,
            &format!("/api/v1/notes/{note}"),
            Some(serde_json::json!({ "append": "suite" })),
            T,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(revision().await, 1);
}
