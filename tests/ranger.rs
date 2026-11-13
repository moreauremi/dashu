//! Ranger après coup : choisir l'espace d'une tâche ou d'un envoi, déplacer
//! une tâche ou un fichier vers un autre espace (SPEC, espaces et outils, B).

mod common;

use axum::http::StatusCode;
use common::{ALICE, BOB, TestApp};

async fn task_space(app: &TestApp, title: &str) -> i64 {
    sqlx::query_scalar("SELECT space_id FROM tasks WHERE title = ?")
        .bind(title)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

async fn task_id(app: &TestApp, title: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM tasks WHERE title = ?")
        .bind(title)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

async fn file_of(app: &TestApp, name: &str) -> (i64, i64) {
    sqlx::query_as("SELECT id, space_id FROM files WHERE name = ?")
        .bind(name)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn une_tache_ajoutee_dans_un_autre_espace() {
    let app = TestApp::new().await;
    // Les espaces d'un compte naissent à sa première connexion.
    app.logged_in(BOB).await;
    let mut alice = app.logged_in(ALICE).await;
    let (perso, ecole) = (
        app.space_id(ALICE, "perso").await,
        app.space_id(ALICE, "ecole").await,
    );

    // Le formulaire propose les espaces, l'espace actif choisi par défaut.
    let widget = alice.get_fragment("/widgets/tasks").await;
    assert!(widget.body.contains(&format!(
        "<option value=\"{perso}\" selected>Perso</option>"
    )));
    assert!(
        widget
            .body
            .contains(&format!("<option value=\"{ecole}\">École</option>"))
    );

    // Depuis Perso, une tâche pour École, sans changer d'espace.
    let added = alice
        .post(
            "/tasks",
            &[("title", "Rendre le TP"), ("space_id", &ecole.to_string())],
        )
        .await;
    assert_eq!(added.status, StatusCode::OK);
    assert_eq!(task_space(&app, "Rendre le TP").await, ecole);
    // Perso montre toutes les tâches : elle y reste visible, marquée École.
    let row = common::between(&added.body, "Rendre le TP</span>", "</li>").unwrap();
    assert!(row.contains("École</span>"));

    // Depuis École, une tâche pour Perso : elle ne se voit plus ici, la
    // phrase le dit.
    alice.post("/space", &[("slug", "ecole")]).await;
    let added = alice
        .post(
            "/tasks",
            &[
                ("title", "Acheter du pain"),
                ("space_id", &perso.to_string()),
            ],
        )
        .await;
    assert!(added.body.contains("Tâche ajoutée dans Perso"));
    assert!(!added.body.contains("Acheter du pain"));
    assert_eq!(task_space(&app, "Acheter du pain").await, perso);
    alice.post("/space", &[("slug", "perso")]).await;

    // Sans précision, l'espace actif.
    alice.post("/tasks", &[("title", "Courses")]).await;
    assert_eq!(task_space(&app, "Courses").await, perso);

    // L'espace d'un autre compte est refusé, et rien n'est créé.
    let bob_space = app.space_id(BOB, "perso").await;
    let refused = alice
        .post(
            "/tasks",
            &[("title", "Intrus"), ("space_id", &bob_space.to_string())],
        )
        .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM tasks WHERE title = 'Intrus'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn une_tache_se_deplace() {
    let app = TestApp::new().await;
    // Les espaces d'un compte naissent à sa première connexion.
    app.logged_in(BOB).await;
    let mut alice = app.logged_in(ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    alice.post("/tasks", &[("title", "Réviser")]).await;
    let id = task_id(&app, "Réviser").await;

    // Le menu de la ligne propose les autres espaces, pas le sien.
    let widget = alice.get_fragment("/widgets/tasks").await;
    let row = common::between(&widget.body, "Réviser", "</li>").unwrap();
    assert!(row.contains(&format!("hx-post=\"/tasks/{id}/move\"")));
    assert!(row.contains("École</option>"));
    assert!(!row.contains("Perso</option>"));

    let moved = alice
        .post(
            &format!("/tasks/{id}/move"),
            &[("space_id", &ecole.to_string())],
        )
        .await;
    assert_eq!(moved.status, StatusCode::OK);
    assert!(moved.body.contains("Tâche déplacée vers École"));
    assert_eq!(task_space(&app, "Réviser").await, ecole);

    // Vers l'espace d'un autre compte : refusé. La tâche d'un autre : introuvable.
    let bob_space = app.space_id(BOB, "perso").await;
    let refused = alice
        .post(
            &format!("/tasks/{id}/move"),
            &[("space_id", &bob_space.to_string())],
        )
        .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
    assert_eq!(task_space(&app, "Réviser").await, ecole);
    let mut bob = app.logged_in(BOB).await;
    let refused = bob
        .post(
            &format!("/tasks/{id}/move"),
            &[("space_id", &bob_space.to_string())],
        )
        .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
    assert_eq!(task_space(&app, "Réviser").await, ecole);
}

#[tokio::test]
async fn un_fichier_se_deplace_avec_sa_vignette() {
    let app = TestApp::new().await;
    // Les espaces d'un compte naissent à sa première connexion.
    app.logged_in(BOB).await;
    let mut alice = app.logged_in(ALICE).await;
    let (perso, ecole) = (
        app.space_id(ALICE, "perso").await,
        app.space_id(ALICE, "ecole").await,
    );
    alice.upload("cours.pdf", b"%PDF-1.4 cours").await;
    let (id, space) = file_of(&app, "cours.pdf").await;
    assert_eq!(space, perso);
    // Une vignette déjà faite, rangée à côté du fichier.
    let key: String = sqlx::query_scalar("SELECT storage_key FROM files WHERE id = ?")
        .bind(id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    let stored = dashu::files::stored_path(&app.files_dir, &key);
    std::fs::write(dashu::files::thumb_path(&stored), b"vignette").unwrap();
    sqlx::query("UPDATE files SET thumb = 1 WHERE id = ?")
        .bind(id)
        .execute(&app.state.db)
        .await
        .unwrap();

    // La liste et l'aperçu proposent le déplacement.
    let list = alice.get("/files").await;
    assert!(list.body.contains(&format!("action=\"/files/{id}/move\"")));
    let view = alice.get(&format!("/files/{id}")).await;
    assert!(view.body.contains("Déplacer vers École"));
    assert!(!view.body.contains("Déplacer vers Perso"));

    let moved = alice
        .post(
            &format!("/files/{id}/move"),
            &[
                ("space_id", &ecole.to_string()),
                ("next", &format!("/files/{id}")),
            ],
        )
        .await;
    assert_eq!(moved.location(), format!("/files/{id}"));
    // Le fichier est maintenant dans École : son aperçu s'ouvre à l'adresse
    // d'École.
    let opened = alice.follow(moved).await;
    assert_eq!(opened.location(), format!("/e/ecole/files/{id}"));
    let page = alice.follow(opened).await;
    assert!(page.body.contains("Fichier déplacé vers École"));
    assert_eq!(file_of(&app, "cours.pdf").await.1, ecole);
    let thumb = alice.get(&format!("/files/{id}/thumb")).await;
    assert_eq!(thumb.status, StatusCode::OK);
    assert_eq!(thumb.bytes, b"vignette");

    // Plus dans Perso (« Seulement Perso »), mais dans École. (L'aperçu
    // ouvert à l'adresse d'École a fait d'École le dernier espace utilisé.)
    alice.post("/space", &[("slug", "perso")]).await;
    assert!(!alice.get("/files?seul=1").await.body.contains("cours.pdf"));
    alice.post("/space", &[("slug", "ecole")]).await;
    assert!(alice.get("/files").await.body.contains("cours.pdf"));

    // Une adresse de retour hors du site est ignorée.
    let moved = alice
        .post(
            &format!("/files/{id}/move"),
            &[("space_id", &perso.to_string()), ("next", "//evil.example")],
        )
        .await;
    assert_eq!(moved.location(), "/");

    // Vers l'espace d'un autre compte, ou le fichier d'un autre : refusé.
    let bob_space = app.space_id(BOB, "perso").await;
    let refused = alice
        .post(
            &format!("/files/{id}/move"),
            &[("space_id", &bob_space.to_string())],
        )
        .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
    let mut bob = app.logged_in(BOB).await;
    let refused = bob
        .post(
            &format!("/files/{id}/move"),
            &[("space_id", &bob_space.to_string())],
        )
        .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
    assert_eq!(file_of(&app, "cours.pdf").await.1, perso);
}

#[tokio::test]
async fn un_envoi_dans_l_espace_choisi() {
    let app = TestApp::new().await;
    // Les espaces d'un compte naissent à sa première connexion.
    app.logged_in(BOB).await;
    let mut alice = app.logged_in(ALICE).await;
    let (perso, ecole) = (
        app.space_id(ALICE, "perso").await,
        app.space_id(ALICE, "ecole").await,
    );
    let page = alice.get("/files").await;
    assert!(page.body.contains("data-upload-space"));
    assert!(page.body.contains(&format!(
        "<option value=\"{perso}\" selected>Perso</option>"
    )));

    let content = b"notes de cours";
    let start = alice.start_upload_in("td.txt", content.len(), ecole).await;
    assert_eq!(start.status, StatusCode::CREATED);
    let done = alice.put_chunk(&start.body, 0, content, true).await;
    assert_eq!(done.status, StatusCode::CREATED, "{}", done.body);
    assert_eq!(file_of(&app, "td.txt").await.1, ecole);

    // Dans l'espace d'un autre compte : refusé dès l'ouverture.
    let bob_space = app.space_id(BOB, "perso").await;
    let refused = alice.start_upload_in("x.txt", 3, bob_space).await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn on_sait_toujours_dans_quel_espace_on_est() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.post("/space", &[("slug", "ecole")]).await;

    for (path, title) in [
        ("/notes", "<title>Notes · École · Hermes Dashboard</title>"),
        (
            "/files",
            "<title>Fichiers · École · Hermes Dashboard</title>",
        ),
        (
            "/calendar",
            "<title>Agenda · École · Hermes Dashboard</title>",
        ),
        ("/", "<title>Dashboard · École · Hermes Dashboard</title>"),
    ] {
        let page = alice.get(path).await;
        assert!(page.body.contains(title), "{path}");
    }
    // Au-dessus du titre des pages d'un espace, son nom.
    let notes = alice.get("/notes").await;
    assert!(notes.body.contains(
        "<p class=\"space-kicker\"><span class=\"dot\" aria-hidden=\"true\"></span><span class=\"sr-only\">Espace </span>École</p>"
    ));
    // Le sélecteur nomme chaque espace, même ceux qui ne montrent que leur icône.
    for name in ["Perso", "École", "Taff"] {
        assert!(notes.body.contains(&format!("title=\"{name}\"")), "{name}");
        assert!(
            notes
                .body
                .contains(&format!("<span class=\"isl-label\">{name}</span>")),
            "{name}"
        );
    }
    // Réglages n'appartient à aucun espace : pas de rappel.
    assert!(!alice.get("/settings").await.body.contains("space-kicker"));
}

/// Crée une note dans l'espace actif, avec ce titre.
async fn note(client: &mut common::Client, title: &str) {
    let created = client.post("/notes", &[]).await;
    let path = created.location().to_string();
    client
        .post(&path, &[("title", title), ("body_md", "contenu")])
        .await;
}

#[tokio::test]
async fn perso_montre_tous_les_espaces() {
    let app = TestApp::new().await;
    let mut bob = app.logged_in(BOB).await;
    note(&mut bob, "Note de Bob").await;
    let mut alice = app.logged_in(ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await;

    // Du contenu dans École, et un peu dans Perso.
    alice.post("/space", &[("slug", "ecole")]).await;
    note(&mut alice, "Cours de réseau").await;
    alice.post("/tasks", &[("title", "Rendre le TP")]).await;
    let start = alice.start_upload_in("td.pdf", 4, ecole).await;
    alice.put_chunk(&start.body, 0, b"%PDF", true).await;
    alice.post("/space", &[("slug", "perso")]).await;
    note(&mut alice, "Liste de courses").await;
    alice.post("/tasks", &[("title", "Courses")]).await;

    // Dans Perso : tout, et ce qui vient d'École porte son nom.
    let tag = "<span class=\"space-tag\" data-accent=\"blue\"><span class=\"dot\" aria-hidden=\"true\"></span>École</span>";
    let tasks = alice.get_fragment("/widgets/tasks").await;
    assert!(tasks.body.contains("Rendre le TP") && tasks.body.contains("Courses"));
    assert!(tasks.body.contains(tag));
    let notes = alice.get_fragment("/widgets/notes").await;
    assert!(notes.body.contains("Cours de réseau") && notes.body.contains(tag));
    let files = alice.get_fragment("/widgets/files").await;
    assert!(files.body.contains("td.pdf") && files.body.contains(tag));

    let list = alice.get("/notes").await;
    assert!(list.body.contains("Cours de réseau") && list.body.contains("Liste de courses"));
    assert!(list.body.contains(tag));
    assert!(list.body.contains("Seulement Perso"));
    assert!(!list.body.contains("Note de Bob"));
    // La recherche aussi couvre tous les espaces.
    assert!(alice.get("/notes?q=reseau").await.body.contains("Cours de"));

    // « Seulement Perso » : le contenu propre de Perso.
    let only = alice.get("/notes?seul=1").await;
    assert!(only.body.contains("Liste de courses"));
    assert!(!only.body.contains("Cours de réseau"));
    assert!(alice.get("/files").await.body.contains("td.pdf"));
    assert!(!alice.get("/files?seul=1").await.body.contains("td.pdf"));

    // Dans École : seulement École, sans pastille ni filtre.
    alice.post("/space", &[("slug", "ecole")]).await;
    let ecole_notes = alice.get("/notes").await;
    assert!(ecole_notes.body.contains("Cours de réseau"));
    assert!(!ecole_notes.body.contains("Liste de courses"));
    assert!(!ecole_notes.body.contains("space-tag"));
    assert!(!ecole_notes.body.contains("Seulement"));
    let ecole_tasks = alice.get_fragment("/widgets/tasks").await;
    assert!(!ecole_tasks.body.contains("Courses</span>"));
}
