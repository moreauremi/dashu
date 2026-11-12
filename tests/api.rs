//! API JSON (`/api/v1`) : jeton, tâches, notes, agenda, isolation, erreurs.

mod common;

use axum::http::{Method, StatusCode};
use common::{ALICE, API_TOKEN, BOB, TestApp};
use serde_json::{Value, json};

const T: Option<&str> = Some(API_TOKEN);

#[tokio::test]
async fn jeton_obligatoire_et_verifie() {
    let app = TestApp::new().await;

    let (status, body) = app.api(Method::GET, "/api/v1/tasks", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body["error"].as_str().unwrap().contains("jeton"));

    let wrong = format!("{}x", &API_TOKEN[..API_TOKEN.len() - 1]);
    let (status, _) = app
        .api(Method::GET, "/api/v1/tasks", None, Some(&wrong))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, body) = app.api(Method::GET, "/api/v1/spaces", None, T).await;
    assert_eq!(status, StatusCode::OK);
    let slugs: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["slug"].as_str().unwrap())
        .collect();
    assert_eq!(slugs, ["perso", "ecole", "taff"]);
}

#[tokio::test]
async fn un_cookie_de_session_n_ouvre_pas_l_api() {
    let app = TestApp::new().await;
    let alice = app.logged_in(ALICE).await;
    let cookie = format!("dashu={}", alice.session_cookie().unwrap());
    let (status, _) = app
        .api_raw(
            Method::GET,
            "/api/v1/tasks",
            None,
            None,
            &[("cookie", &cookie)],
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_desactivee_sans_api_token() {
    let app = TestApp::without_api().await;
    let (status, body) = app.api(Method::GET, "/api/v1/tasks", None, T).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"].as_str().unwrap().contains("désactivée"));
}

#[tokio::test]
async fn cycle_de_vie_d_une_tache() {
    let app = TestApp::new().await;

    // Création, sans date : prévue aujourd'hui, comme depuis le dashboard.
    let (status, task) = app
        .api(
            Method::POST,
            "/api/v1/tasks",
            Some(json!({"title": "  Réviser le VLAN  ", "space": "École"})),
            T,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
    assert_eq!(task["title"], "Réviser le VLAN");
    assert_eq!(task["space"], "ecole");
    assert_eq!(task["done"], false);
    assert!(task["due_date"].as_str().unwrap().len() == 10);
    let id = task["id"].as_i64().unwrap();

    // Avec une date.
    let (status, later) = app
        .api(
            Method::POST,
            "/api/v1/tasks",
            Some(json!({"title": "Rendre le dossier", "space": "taff", "due_date": "2030-01-15"})),
            T,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(later["due_date"], "2030-01-15");

    // Liste filtrée par espace.
    let (_, ecole) = app
        .api(Method::GET, "/api/v1/tasks?space=ecole", None, T)
        .await;
    assert_eq!(ecole.as_array().unwrap().len(), 1);
    let (_, all) = app.api(Method::GET, "/api/v1/tasks", None, T).await;
    assert_eq!(all.as_array().unwrap().len(), 2);

    // Cochée : elle quitte les tâches ouvertes et apparaît dans done.
    let (status, done) = app
        .api(
            Method::PATCH,
            &format!("/api/v1/tasks/{id}"),
            Some(json!({"done": true})),
            T,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(done["done"], true);
    let done_at: Option<i64> = sqlx::query_scalar("SELECT done_at FROM tasks WHERE id = ?")
        .bind(id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert!(done_at.is_some(), "une tâche cochée garde l'heure de fin");
    let (_, open) = app.api(Method::GET, "/api/v1/tasks", None, T).await;
    assert_eq!(open.as_array().unwrap().len(), 1);
    let (_, finished) = app
        .api(Method::GET, "/api/v1/tasks?status=done", None, T)
        .await;
    assert_eq!(finished[0]["id"], id);

    // Décochée : l'heure de fin disparaît.
    app.api(
        Method::PATCH,
        &format!("/api/v1/tasks/{id}"),
        Some(json!({"done": false, "title": "Réviser le routage"})),
        T,
    )
    .await;
    let (_, again) = app
        .api(Method::GET, &format!("/api/v1/tasks/{id}"), None, T)
        .await;
    assert_eq!(again["done"], false);
    assert_eq!(again["title"], "Réviser le routage");

    // Suppression.
    let (status, body) = app
        .api(Method::DELETE, &format!("/api/v1/tasks/{id}"), None, T)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(body, Value::Null);
    let (status, _) = app
        .api(Method::GET, &format!("/api/v1/tasks/{id}"), None, T)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn la_tache_creee_par_l_api_s_affiche_dans_le_dashboard() {
    let app = TestApp::new().await;
    app.api(
        Method::POST,
        "/api/v1/tasks",
        Some(json!({"title": "Appeler le client", "space": "perso"})),
        T,
    )
    .await;
    let mut alice = app.logged_in(ALICE).await;
    let widget = alice.get_fragment("/widgets/tasks").await;
    assert!(widget.body.contains("Appeler le client"));
}

#[tokio::test]
async fn cycle_de_vie_d_une_note() {
    let app = TestApp::new().await;

    let (status, note) = app
        .api(
            Method::POST,
            "/api/v1/notes",
            Some(json!({
                "title": "Cours réseau",
                "body_md": "# VLAN\r\n\r\nLe **trunk** transporte plusieurs VLAN.",
                "space": "ecole",
                "tags": ["#Réseau", "bts"]
            })),
            T,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{note}");
    let id = note["id"].as_i64().unwrap();
    assert_eq!(note["tags"], json!(["bts", "réseau"]));
    assert_eq!(
        note["body_md"],
        "# VLAN\n\nLe **trunk** transporte plusieurs VLAN."
    );

    // Recherche plein texte, accents ignorés, sans balises dans l'extrait.
    let (_, found) = app
        .api(Method::GET, "/api/v1/notes?q=reseau", None, T)
        .await;
    assert_eq!(found[0]["id"], id);
    let (_, found) = app.api(Method::GET, "/api/v1/notes?q=trunk", None, T).await;
    let excerpt = found[0]["excerpt"].as_str().unwrap();
    assert!(excerpt.contains("trunk") && !excerpt.contains('<'));
    let (_, none) = app
        .api(Method::GET, "/api/v1/notes?q=trunk&space=perso", None, T)
        .await;
    assert!(none.as_array().unwrap().is_empty());
    let (_, by_tag) = app.api(Method::GET, "/api/v1/notes?tag=bts", None, T).await;
    assert_eq!(by_tag[0]["id"], id);

    // Compléter : le texte s'ajoute après une ligne vide.
    let (status, note) = app
        .api(
            Method::PATCH,
            &format!("/api/v1/notes/{id}"),
            Some(json!({"append": "## STP\nÉvite les boucles.\n", "add_tags": ["examen"]})),
            T,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{note}");
    assert_eq!(
        note["body_md"],
        "# VLAN\n\nLe **trunk** transporte plusieurs VLAN.\n\n## STP\nÉvite les boucles."
    );
    assert_eq!(note["tags"], json!(["bts", "examen", "réseau"]));

    // Épingler et déplacer.
    let (_, note) = app
        .api(
            Method::PATCH,
            &format!("/api/v1/notes/{id}"),
            Some(json!({"pinned": true, "space": "perso"})),
            T,
        )
        .await;
    assert_eq!(note["pinned"], true);
    assert_eq!(note["space"], "perso");

    // Lecture complète.
    let (status, full) = app
        .api(Method::GET, &format!("/api/v1/notes/{id}"), None, T)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(full["title"], "Cours réseau");

    // Pas de suppression de note par l'API.
    let (status, _) = app
        .api(Method::DELETE, &format!("/api/v1/notes/{id}"), None, T)
        .await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn l_api_ne_voit_que_le_compte_configure() {
    let app = TestApp::new().await;
    // Bob écrit une note et une tâche depuis le navigateur.
    let mut bob = app.logged_in(BOB).await;
    let created = bob.post("/notes", &[]).await;
    let bob_note = common::note_id(&created);
    bob.post(
        &format!("/notes/{bob_note}"),
        &[("title", "Secret de Bob"), ("body_md", "confidentiel")],
    )
    .await;
    bob.post("/tasks", &[("title", "Tâche de Bob")]).await;
    let bob_task: i64 = sqlx::query_scalar("SELECT id FROM tasks WHERE title = 'Tâche de Bob'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();

    // L'API agit au nom d'Alice : rien de Bob n'est visible ni modifiable.
    let (_, notes) = app
        .api(Method::GET, "/api/v1/notes?q=confidentiel", None, T)
        .await;
    assert!(notes.as_array().unwrap().is_empty());
    let (status, _) = app
        .api(Method::GET, &format!("/api/v1/notes/{bob_note}"), None, T)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = app
        .api(
            Method::PATCH,
            &format!("/api/v1/notes/{bob_note}"),
            Some(json!({"append": "piraté"})),
            T,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = app
        .api(
            Method::PATCH,
            &format!("/api/v1/tasks/{bob_task}"),
            Some(json!({"done": true})),
            T,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = app
        .api(
            Method::DELETE,
            &format!("/api/v1/tasks/{bob_task}"),
            None,
            T,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, tasks) = app
        .api(Method::GET, "/api/v1/tasks?status=all", None, T)
        .await;
    assert!(tasks.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn agenda_en_lecture() {
    let app = TestApp::new().await;
    let alice = app.user_id(ALICE).await;
    // Les espaces naissent au premier appel de l'API.
    app.api(Method::GET, "/api/v1/spaces", None, T).await;
    let ecole = app.space_id(ALICE, "ecole").await;

    let source: i64 = sqlx::query_scalar(
        "INSERT INTO calendar_sources (user_id, space_id, name, ical_url, created_at)
         VALUES (?, ?, 'Pronote', 'https://example.test/x.ics', 0) RETURNING id",
    )
    .bind(alice)
    .bind(ecole)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    // Lundi 16 novembre 2026, 8 h à Paris = 7 h UTC.
    let monday_8h = 1_794_812_400_i64;
    for (uid, title, start) in [
        ("a", "MATHS - DUPONT J. - Salle 12", monday_8h),
        ("b", "RÉSEAU - MARTIN P.", monday_8h + 7 * 86_400),
    ] {
        sqlx::query(
            "INSERT INTO calendar_events (source_id, uid, title, location, starts_at, ends_at)
             VALUES (?, ?, ?, 'B204', ?, ?)",
        )
        .bind(source)
        .bind(uid)
        .bind(title)
        .bind(start)
        .bind(start + 3600)
        .execute(&app.state.db)
        .await
        .unwrap();
    }

    let (status, events) = app
        .api(
            Method::GET,
            "/api/v1/calendar?from=2026-11-16&to=2026-11-20",
            None,
            T,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{events}");
    let events = events.as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["subject"], "MATHS");
    assert_eq!(events[0]["details"], "DUPONT J. · Salle 12");
    assert_eq!(events[0]["start"], "2026-11-16T08:00:00+01:00");
    assert_eq!(events[0]["space"], "ecole");

    // Perso réunit les agendas de tous les espaces, comme dans l'application ;
    // l'événement garde le slug de son espace. Un autre espace filtre vraiment.
    let (_, perso) = app
        .api(
            Method::GET,
            "/api/v1/calendar?from=2026-11-16&to=2026-11-20&space=perso",
            None,
            T,
        )
        .await;
    let perso = perso.as_array().unwrap();
    assert_eq!(perso.len(), 1);
    assert_eq!(perso[0]["space"], "ecole");
    let (_, taff) = app
        .api(
            Method::GET,
            "/api/v1/calendar?from=2026-11-16&to=2026-11-20&space=taff",
            None,
            T,
        )
        .await;
    assert!(taff.as_array().unwrap().is_empty());

    let (status, body) = app
        .api(
            Method::GET,
            "/api/v1/calendar?from=2026-01-01&to=2026-12-31",
            None,
            T,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("trop longue"));
}

#[tokio::test]
async fn les_erreurs_restent_en_json() {
    let app = TestApp::new().await;

    let (status, body) = app
        .api(
            Method::POST,
            "/api/v1/tasks",
            Some(json!({"title": "x", "space": "vacances"})),
            T,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("perso, ecole, taff")
    );

    let (status, body) = app
        .api_raw(
            Method::POST,
            "/api/v1/tasks",
            Some("{pas du json".into()),
            T,
            &[],
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("JSON"));

    let (status, _) = app
        .api(
            Method::POST,
            "/api/v1/tasks",
            Some(json!({"title": "x", "space": "perso", "due_date": "demain"})),
            T,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body) = app.api(Method::GET, "/api/v1/nimporte-quoi", None, T).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"].is_string());

    let (status, _) = app.api(Method::GET, "/api/v1/tasks/abc", None, T).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Une écriture par l'API n'a pas besoin de jeton CSRF (pas de cookie).
    let (status, _) = app
        .api(
            Method::POST,
            "/api/v1/notes",
            Some(json!({"title": "Sans CSRF", "space": "perso"})),
            T,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
}
