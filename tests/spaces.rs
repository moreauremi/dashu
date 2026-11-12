//! Espaces : création, modification et suppression depuis les réglages,
//! avec transfert ou suppression du contenu. Et la migration 0007, qui
//! reconstruit la table spaces sans rien perdre.

mod common;

use std::borrow::Cow;
use std::str::FromStr;

use axum::http::StatusCode;
use common::{ALICE, BOB, PASSWORD, TestApp};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection};

type Row = (String, String, String, String);

/// Espaces d'un compte : (slug, nom, couleur, icône), dans l'ordre.
async fn spaces_of(app: &TestApp, email: &str) -> Vec<Row> {
    sqlx::query_as(
        "SELECT s.slug, s.name, s.color, s.icon FROM spaces s JOIN users u ON u.id = s.user_id
         WHERE u.email = ? ORDER BY s.position, s.id",
    )
    .bind(email)
    .fetch_all(&app.state.db)
    .await
    .unwrap()
}

fn row(slug: &str, name: &str, color: &str, icon: &str) -> Row {
    (slug.into(), name.into(), color.into(), icon.into())
}

/// Nombre de lignes de `table` dans un espace.
async fn count_in(app: &TestApp, table: &str, space_id: i64) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE space_id = ?"))
        .bind(space_id)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn creer_puis_modifier_un_espace() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;

    let created = alice
        .post(
            "/settings/spaces",
            &[
                ("name", "  Vie de   famille "),
                ("color", "violet"),
                ("icon", "heart"),
            ],
        )
        .await;
    assert_eq!(created.status, StatusCode::SEE_OTHER);
    assert_eq!(created.location(), "/settings#espaces");
    let page = alice.follow(created).await;
    assert!(page.body.contains("Espace « Vie de famille » créé"));
    // Il devient l'espace actif, et l'île le montre avec sa couleur et son icône.
    assert!(page.body.contains("class=\"app\" data-accent=\"violet\""));
    assert!(page.body.contains("#heart\""));
    assert_eq!(
        spaces_of(&app, ALICE).await.last().unwrap(),
        &row("vie-de-famille", "Vie de famille", "violet", "heart")
    );

    // Renommer garde le slug : l'API et la session continuent de le reconnaître.
    let id = app.space_id(ALICE, "vie-de-famille").await;
    let renamed = alice
        .post(
            &format!("/settings/spaces/{id}"),
            &[("name", "Famille"), ("color", "teal"), ("icon", "users")],
        )
        .await;
    let page = alice.follow(renamed).await;
    assert!(page.body.contains("appelle maintenant « Famille »"));
    assert_eq!(
        spaces_of(&app, ALICE).await.last().unwrap(),
        &row("vie-de-famille", "Famille", "teal", "users")
    );
    // Taff renommé Entreprise, sans changer de slug.
    let taff = app.space_id(ALICE, "taff").await;
    alice
        .post(
            &format!("/settings/spaces/{taff}"),
            &[
                ("name", "Entreprise"),
                ("color", "green"),
                ("icon", "building"),
            ],
        )
        .await;
    assert_eq!(
        spaces_of(&app, ALICE).await[2],
        row("taff", "Entreprise", "green", "building")
    );
}

#[tokio::test]
async fn formulaire_d_espace_verifie() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;

    for (name, color, icon, message) in [
        ("   ", "blue", "home", "Donne un nom à l"),
        ("Perso", "blue", "home", "appelle déjà « Perso »"),
        ("ÉCOLE", "blue", "home", "appelle déjà « ÉCOLE »"),
        (
            "Un nom bien trop long pour l'île",
            "blue",
            "home",
            "tient en 24 caractères",
        ),
        ("Jeux", "noir", "home", "Couleur inconnue"),
        ("Jeux", "blue", "skull", "Icône inconnue"),
    ] {
        let refused = alice
            .post(
                "/settings/spaces",
                &[("name", name), ("color", color), ("icon", icon)],
            )
            .await;
        let page = alice.follow(refused).await;
        assert!(page.body.contains(message), "{name} : {message}");
    }
    assert_eq!(spaces_of(&app, ALICE).await.len(), 3);

    // Huit espaces au plus : l'île doit tenir sur un téléphone.
    for n in 4..=8 {
        let name = format!("Espace {n}");
        alice
            .post(
                "/settings/spaces",
                &[("name", &name), ("color", "rose"), ("icon", "folder")],
            )
            .await;
    }
    assert_eq!(spaces_of(&app, ALICE).await.len(), 8);
    let refused = alice
        .post(
            "/settings/spaces",
            &[("name", "Neuvième"), ("color", "rose"), ("icon", "folder")],
        )
        .await;
    let page = alice.follow(refused).await;
    assert!(page.body.contains("8 espaces au plus"));
    assert!(!page.body.contains("action=\"/settings/spaces\""));
    assert_eq!(spaces_of(&app, ALICE).await.len(), 8);
}

#[tokio::test]
async fn supprimer_un_espace_en_transferant_son_contenu() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.post("/space", &[("slug", "taff")]).await;
    alice
        .post("/tasks", &[("title", "Préparer la réunion")])
        .await;
    let created = alice.post("/notes", &[]).await;
    let note = common::note_id(&created);
    alice
        .post(
            &format!("/notes/{note}"),
            &[("title", "Compte rendu"), ("body_md", "Budget validé")],
        )
        .await;
    let sent = alice.upload("devis.pdf", b"%PDF-1.4 devis").await;
    assert_eq!(sent.status, StatusCode::CREATED, "{}", sent.body);
    let taff = app.space_id(ALICE, "taff").await;
    let ecole = app.space_id(ALICE, "ecole").await;

    // La confirmation dit ce que contient l'espace, prévient que les fichiers
    // seraient effacés, et propose de tout transférer dans un autre espace.
    let page = alice.get(&format!("/settings/spaces/{taff}/delete")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("Cet espace contient 1 fichier ("));
    assert!(page.body.contains("1 note et 1 tâche"));
    assert!(page.body.contains("seront effacés du serveur"));
    assert!(page.body.contains("Le transférer dans"));
    assert!(
        page.body
            .contains(&format!("<option value=\"{ecole}\">École</option>"))
    );
    assert!(!page.body.contains(&format!("<option value=\"{taff}\"")));

    let deleted = alice
        .post(
            &format!("/settings/spaces/{taff}/delete"),
            &[("content", "move"), ("target", &ecole.to_string())],
        )
        .await;
    assert_eq!(deleted.status, StatusCode::SEE_OTHER);
    let page = alice.follow(deleted).await;
    assert!(
        page.body
            .contains("son contenu est maintenant dans « École »")
    );
    // École reçoit tout et devient l'espace actif ; le fichier reste sur le disque.
    assert!(page.body.contains("class=\"app\" data-accent=\"blue\""));
    let slugs: Vec<String> = spaces_of(&app, ALICE)
        .await
        .into_iter()
        .map(|s| s.0)
        .collect();
    assert_eq!(slugs, ["perso", "ecole"]);
    for table in ["notes", "tasks", "files"] {
        assert_eq!(count_in(&app, table, ecole).await, 1, "{table}");
    }
    assert_eq!(app.stored_files().len(), 1);
    assert!(alice.get("/files").await.body.contains("devis.pdf"));
    assert!(alice.get("/notes").await.body.contains("Compte rendu"));
}

#[tokio::test]
async fn supprimer_un_espace_avec_son_contenu() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    // Une note dans Perso, qui doit survivre.
    let created = alice.post("/notes", &[]).await;
    let kept = common::note_id(&created);

    alice.post("/space", &[("slug", "taff")]).await;
    alice
        .post("/tasks", &[("title", "Relancer le client")])
        .await;
    let created = alice.post("/notes", &[]).await;
    let note = common::note_id(&created);
    alice
        .post(
            &format!("/notes/{note}"),
            &[("title", "Contrat"), ("body_md", "motconfidentiel")],
        )
        .await;
    alice.upload("contrat.pdf", b"%PDF-1.4 contrat").await;
    assert_eq!(app.stored_files().len(), 1);
    let taff = app.space_id(ALICE, "taff").await;

    let deleted = alice
        .post(
            &format!("/settings/spaces/{taff}/delete"),
            &[("content", "delete")],
        )
        .await;
    let page = alice.follow(deleted).await;
    assert!(
        page.body
            .contains("Espace « Taff » supprimé avec son contenu")
    );
    // L'espace actif a disparu : retour au premier.
    assert!(page.body.contains("class=\"app\" data-accent=\"orange\""));
    assert!(app.stored_files().is_empty(), "le fichier quitte le disque");
    for table in ["notes", "tasks", "files"] {
        assert_eq!(count_in(&app, table, taff).await, 0, "{table}");
    }
    // La recherche plein texte a suivi.
    let hits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notes_fts WHERE notes_fts MATCH 'motconfidentiel'",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(hits, 0);
    assert_eq!(
        alice.get(&format!("/notes/{kept}")).await.status,
        StatusCode::OK
    );

    // Un espace supprimé ne revient pas à la connexion suivante.
    let mut again = app.client();
    again.login(ALICE, PASSWORD).await;
    let slugs: Vec<String> = spaces_of(&app, ALICE)
        .await
        .into_iter()
        .map(|s| s.0)
        .collect();
    assert_eq!(slugs, ["perso", "ecole"]);
}

#[tokio::test]
async fn espace_vide_et_perso_protege() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let perso = app.space_id(ALICE, "perso").await;
    let ecole = app.space_id(ALICE, "ecole").await;
    let taff = app.space_id(ALICE, "taff").await;

    // Perso ne se supprime pas, même quand il reste d'autres espaces : pas de
    // bouton, une page qui explique, et un refus si on force.
    let settings = alice.get("/settings").await;
    assert!(
        !settings
            .body
            .contains(&format!("/settings/spaces/{perso}/delete"))
    );
    assert!(
        settings
            .body
            .contains(&format!("/settings/spaces/{ecole}/delete"))
    );
    assert!(settings.body.contains("il ne se supprime pas"));
    let page = alice.get(&format!("/settings/spaces/{perso}/delete")).await;
    assert!(page.body.contains("ne se supprime pas"));
    assert!(!page.body.contains("name=\"content\""));
    let refused = alice
        .post(
            &format!("/settings/spaces/{perso}/delete"),
            &[("content", "move"), ("target", &ecole.to_string())],
        )
        .await;
    let page = alice.follow(refused).await;
    assert!(page.body.contains("« Perso » ne se supprime pas"));
    assert_eq!(spaces_of(&app, ALICE).await.len(), 3);

    // Un espace vide : pas d'alerte, rien à transférer.
    let page = alice.get(&format!("/settings/spaces/{ecole}/delete")).await;
    assert!(page.body.contains("Cet espace est vide"));
    assert!(!page.body.contains("Le transférer dans"));
    for id in [ecole, taff] {
        let deleted = alice
            .post(
                &format!("/settings/spaces/{id}/delete"),
                &[("content", "delete")],
            )
            .await;
        assert_eq!(deleted.status, StatusCode::SEE_OTHER);
    }

    // Seul, Perso reste là ; renommé, il reste protégé.
    alice
        .post(
            &format!("/settings/spaces/{perso}"),
            &[("name", "Maison"), ("color", "orange"), ("icon", "home")],
        )
        .await;
    let refused = alice
        .post(
            &format!("/settings/spaces/{perso}/delete"),
            &[("content", "delete")],
        )
        .await;
    let page = alice.follow(refused).await;
    assert!(page.body.contains("« Maison » ne se supprime pas"));
    assert_eq!(
        spaces_of(&app, ALICE).await,
        [row("perso", "Maison", "orange", "home")]
    );
}

/// Perso a pu être supprimé avant d'être protégé : la migration 0008 le rend,
/// vide et en tête, aux comptes qui ne l'ont plus.
#[tokio::test]
async fn migration_0008_rend_perso() {
    let path = std::env::temp_dir().join(format!("dashu-migration-0008-{}.db", std::process::id()));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    let url = format!("sqlite://{}", path.display());
    let mut conn = SqliteConnectOptions::from_str(&url)
        .unwrap()
        .create_if_missing(true)
        .connect()
        .await
        .unwrap();
    let mut before = sqlx::migrate!();
    before.migrations = Cow::Owned(
        before
            .migrations
            .iter()
            .filter(|m| m.version < 8)
            .cloned()
            .collect(),
    );
    before.run(&mut conn).await.unwrap();
    // Compte 1 : Perso supprimé. Compte 2 : Perso supprimé, puis un espace
    // renommé « Perso ». Compte 3 : intact. Compte 4 : jamais connecté.
    sqlx::raw_sql(
        "INSERT INTO users (id, email, created_at) VALUES
           (1, 'a@example.test', 0), (2, 'b@example.test', 0), (3, 'c@example.test', 0),
           (4, 'd@example.test', 0);
         INSERT INTO spaces (user_id, slug, name, color, icon, position) VALUES
           (1, 'ecole', 'École', 'blue', 'school', 1), (1, 'taff', 'Taff', 'green', 'work', 2),
           (2, 'taff', 'Perso', 'green', 'work', 2),
           (3, 'perso', 'Perso', 'orange', 'home', 0), (3, 'ecole', 'École', 'blue', 'school', 1);",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    conn.close().await.unwrap();

    let db = dashu::db::connect(&url).await.expect("migration 0008");
    let spaces: Vec<(i64, String, String, i64)> = sqlx::query_as(
        "SELECT user_id, slug, name, position FROM spaces ORDER BY user_id, position",
    )
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!(
        spaces,
        [
            (1, "perso".into(), "Perso".into(), 0),
            (1, "ecole".into(), "École".into(), 1),
            (1, "taff".into(), "Taff".into(), 2),
            (2, "perso".into(), "Vue d'ensemble".into(), 1),
            (2, "taff".into(), "Perso".into(), 2),
            (3, "perso".into(), "Perso".into(), 0),
            (3, "ecole".into(), "École".into(), 1),
        ]
    );
    db.close().await;
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

#[tokio::test]
async fn un_autre_compte_ne_touche_pas_aux_espaces() {
    let app = TestApp::new().await;
    let _alice = app.logged_in(ALICE).await;
    let mut bob = app.logged_in(BOB).await;
    let alice_perso = app.space_id(ALICE, "perso").await;
    let alice_taff = app.space_id(ALICE, "taff").await;
    let bob_taff = app.space_id(BOB, "taff").await;
    let fields = [("name", "Piraté"), ("color", "blue"), ("icon", "home")];

    assert_eq!(
        bob.post(&format!("/settings/spaces/{alice_taff}"), &fields)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        bob.get(&format!("/settings/spaces/{alice_taff}/delete"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        bob.post(
            &format!("/settings/spaces/{alice_taff}/delete"),
            &[("content", "delete")]
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    // Bob ne peut pas verser le contenu d'un de ses espaces dans celui d'Alice.
    bob.post("/space", &[("slug", "taff")]).await;
    bob.post("/tasks", &[("title", "Tâche de Bob")]).await;
    let refused = bob
        .post(
            &format!("/settings/spaces/{bob_taff}/delete"),
            &[("content", "move"), ("target", &alice_perso.to_string())],
        )
        .await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    assert_eq!(count_in(&app, "tasks", bob_taff).await, 1);
    assert_eq!(count_in(&app, "tasks", alice_perso).await, 0);
    assert_eq!(
        spaces_of(&app, ALICE).await,
        [
            row("perso", "Perso", "orange", "home"),
            row("ecole", "École", "blue", "school"),
            row("taff", "Taff", "green", "work"),
        ]
    );
}

/// Mise à jour d'un serveur en service : une base aux migrations 1 à 6, avec
/// du contenu, passe la migration 0007 (reconstruction de la table spaces)
/// sans rien perdre, et les suppressions en cascade marchent toujours.
#[tokio::test]
async fn migration_0007_garde_tout_le_contenu() {
    let path = std::env::temp_dir().join(format!("dashu-migration-0007-{}.db", std::process::id()));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    let url = format!("sqlite://{}", path.display());

    // Base d'avant : clés étrangères actives, comme en service.
    let mut conn = SqliteConnectOptions::from_str(&url)
        .unwrap()
        .create_if_missing(true)
        .foreign_keys(true)
        .connect()
        .await
        .unwrap();
    let mut before = sqlx::migrate!();
    before.migrations = Cow::Owned(
        before
            .migrations
            .iter()
            .filter(|m| m.version < 7)
            .cloned()
            .collect(),
    );
    before.run(&mut conn).await.unwrap();
    sqlx::raw_sql(
        "INSERT INTO users (id, email, created_at) VALUES (1, 'a@example.test', 0);
         INSERT INTO spaces (id, user_id, slug, name, color, position)
           VALUES (1, 1, 'perso', 'Perso', 'orange', 0), (2, 1, 'ecole', 'École', 'blue', 1),
                  (3, 1, 'taff', 'Taff', 'green', 2);
         INSERT INTO notes (id, user_id, space_id, title, created_at, updated_at)
           VALUES (1, 1, 1, 'n1', 0, 0), (2, 1, 2, 'n2', 0, 0), (3, 1, 3, 'n3', 0, 0);
         INSERT INTO tags (id, user_id, name) VALUES (1, 1, 'bts');
         INSERT INTO note_tags (note_id, tag_id) VALUES (2, 1);
         INSERT INTO tasks (user_id, space_id, title, created_at) VALUES (1, 2, 't', 0), (1, 3, 't', 0);
         INSERT INTO calendar_sources (id, user_id, space_id, name, ical_url, created_at)
           VALUES (1, 1, 2, 'Pronote', 'https://exemple.test/a.ics', 0);
         INSERT INTO calendar_events (source_id, uid, title, starts_at, ends_at) VALUES (1, 'u', 'Cours', 0, 1);
         INSERT INTO files (user_id, space_id, name, mime, size, sha256, storage_key, created_at, updated_at)
           VALUES (1, 3, 'f.pdf', 'application/pdf', 1, 'h', 'cle', 0, 0);",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    conn.close().await.unwrap();

    let db = dashu::db::connect(&url).await.expect("migration 0007");
    let count = |table: &'static str| {
        let db = db.clone();
        async move {
            sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(&db)
                .await
                .unwrap()
        }
    };
    for (table, expected) in [
        ("spaces", 3),
        ("notes", 3),
        ("note_tags", 1),
        ("tasks", 2),
        ("calendar_sources", 1),
        ("calendar_events", 1),
        ("files", 1),
    ] {
        assert_eq!(count(table).await, expected, "{table}");
    }
    let icons: Vec<(String, String)> =
        sqlx::query_as("SELECT slug, icon FROM spaces ORDER BY position")
            .fetch_all(&db)
            .await
            .unwrap();
    assert_eq!(
        icons,
        [
            ("perso".into(), "home".into()),
            ("ecole".into(), "school".into()),
            ("taff".into(), "work".into()),
        ]
    );
    // Les références suivent la nouvelle table : supprimer École emporte ses
    // notes, tâches et agendas, et rien d'autre.
    sqlx::query("DELETE FROM spaces WHERE id = 2")
        .execute(&db)
        .await
        .unwrap();
    assert_eq!(count("notes").await, 2);
    assert_eq!(count("note_tags").await, 0);
    assert_eq!(count("tasks").await, 1);
    assert_eq!(count("calendar_events").await, 0);
    assert_eq!(count("files").await, 1);
    db.close().await;
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}
