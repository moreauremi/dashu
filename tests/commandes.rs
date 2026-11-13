//! Barre de commandes « / » (SPEC, « Barre de commandes ») : la page la
//! propose à la place de la palette Ctrl+K, la liste des commandes suit la
//! page, et chaque ligne validée (`POST /cmd`) fait ce que font les boutons.
//! Lot 1 d'abord, puis lot 2 (tâches, note, fichier, réglages).

mod common;

use axum::http::StatusCode;
use chrono::Duration;
use common::{ALICE, BOB, TestApp, arg_values, command_node, hx_location};

fn today() -> chrono::NaiveDate {
    dashu::time::paris_date(dashu::time::now())
}

async fn count(app: &TestApp, sql: &str) -> i64 {
    sqlx::query_scalar(sql)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn la_barre_remplace_la_palette() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let page = alice.get("/").await;
    // « / » ouvre la barre : le bouton de la barre latérale le rappelle.
    assert!(page.body.contains(r#"<dialog id="cmdbar""#));
    assert!(page.body.contains(r#"hx-post="/cmd""#));
    assert!(page.body.contains("<kbd>/</kbd>"));
    // Plus de Ctrl+K ni de palette.
    assert!(!page.body.contains("Ctrl K"));
    assert!(!page.body.contains("data-palette-open"));
    assert_eq!(
        alice.get_fragment("/search/palette?q=").await.status,
        StatusCode::NOT_FOUND
    );
    // Une commande passe par le jeton CSRF, comme tout formulaire.
    let forged = alice
        .post_without_csrf("/cmd", &[("line", "/add task Piège"), ("page", "/")])
        .await;
    assert_eq!(forged.status, StatusCode::FORBIDDEN);
    assert_eq!(count(&app, "SELECT COUNT(*) FROM tasks").await, 0);
}

#[tokio::test]
async fn le_catalogue_suit_le_compte() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let tree = alice.catalogue("/").await;
    let names: Vec<&str> = tree
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["off"].is_null())
        .map(|n| n["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "help", "go", "open", "search", "space", "add", "theme", "size", "sidebar", "refresh",
            "logout"
        ]
    );
    // Les espaces vers lesquels changer : pas celui où l'on est.
    assert_eq!(
        arg_values(command_node(&tree, &["space"])),
        ["ecole", "taff"]
    );
    // L'option @espace d'une tâche : tous les espaces du compte.
    let options = arg_values(command_node(&tree, &["add", "task"]));
    assert_eq!(options[..3], ["@perso", "@ecole", "@taff"]);
    assert!(options.contains(&"+demain".to_string()));
    // Les commandes d'une note ou d'un fichier ne s'emploient pas ici, et le
    // disent ; ce que fait le navigateur seul est marqué.
    assert_eq!(
        command_node(&tree, &["move"])["off"],
        "S'emploie sur une note ou un fichier ouvert."
    );
    assert_eq!(command_node(&tree, &["add", "file"])["client"], "file");
    assert!(command_node(&tree, &["add", "file"])["off"].is_null());
    assert_eq!(command_node(&tree, &["help"])["client"], "help");

    // Un espace renommé se tape aussi par son nouveau nom.
    let ecole = app.space_id(ALICE, "ecole").await;
    alice
        .post(
            &format!("/settings/spaces/{ecole}"),
            &[
                ("name", "Lycée Hoche"),
                ("color", "blue"),
                ("icon", "school"),
            ],
        )
        .await;
    let tree = alice.catalogue("/").await;
    let space = &command_node(&tree, &["space"])["arg"]["values"][0];
    assert_eq!(space["v"], "ecole");
    assert_eq!(space["help"], "Lycée Hoche");
    assert_eq!(space["alias"][0], "lycee-hoche");
    let switched = alice.command("/space lycee", "/settings").await;
    assert_eq!(hx_location(&switched)["headers"]["X-Espace"], "ecole");

    // Bob ne voit que ses espaces.
    let mut bob = app.logged_in(BOB).await;
    let tree = bob.catalogue("/").await;
    assert_eq!(
        arg_values(command_node(&tree, &["space"])),
        ["ecole", "taff"]
    );
    assert!(!tree.to_string().contains("Lycée"));
}

#[tokio::test]
async fn aller_a_une_page() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;

    // La page s'ouvre comme un lien : dans l'espace de l'onglet, en page entière.
    let go = hx_location(&alice.command("/go notes", "/").await);
    assert_eq!(go["path"], "/e/perso/notes");
    assert_eq!(go["target"], "body");
    assert_eq!(go["headers"]["X-Espace"], "perso");
    assert_eq!(go["headers"]["HX-Boosted"], "true");
    // Un début de mot suffit, accents, majuscules et alias compris.
    assert_eq!(
        hx_location(&alice.command("/g ag", "/").await)["path"],
        "/e/perso/calendar"
    );
    assert_eq!(
        hx_location(&alice.command("/GO Réglages", "/").await)["path"],
        "/settings"
    );
    assert_eq!(
        hx_location(&alice.command("/go files", "/").await)["path"],
        "/e/perso/files"
    );
    // L'onglet sur École reste sur École.
    alice.with_header("x-espace", "ecole");
    let go = hx_location(&alice.command("/go dashboard", "/").await);
    assert_eq!(go["path"], "/e/ecole/");

    // L'agenda d'un jour : vue jour, sauf si la semaine est demandée.
    let tomorrow = (today() + Duration::days(1)).format("%Y-%m-%d");
    assert_eq!(
        hx_location(&alice.command("/go agenda demain", "/").await)["path"],
        format!("/e/perso/calendar?view=day&date={tomorrow}")
    );
    assert_eq!(
        hx_location(&alice.command("/go agenda semaine 25/12", "/").await)["path"],
        format!(
            "/e/perso/calendar?view=week&date={}-12-25",
            today().format("%Y")
        )
    );
    // La recherche dans les notes, adresse encodée.
    assert_eq!(
        hx_location(&alice.command("/search réseau vlan", "/").await)["path"],
        "/e/perso/notes?q=r%C3%A9seau+vlan"
    );

    // Ce qui ne peut pas aboutir est refusé avec un message clair.
    for (line, message) in [
        ("/zz", "Commande inconnue : /zz"),
        ("/s", "peut être : search, space"),
        ("/go", "Il manque la suite : /go dashboard"),
        ("/go agenda bientot", "Jour inconnu : bientot"),
        ("/search", "Il manque la suite : /search &lt;texte&gt;"),
        ("/convert jpg", "S&#39;emploie sur un fichier ouvert."),
        ("/help", "se lance depuis la barre"),
        ("/add file", "se lance depuis la barre"),
        ("/space nulle-part", "Espace inconnu"),
        ("go notes", "Une commande commence par"),
    ] {
        let refused = alice.command(line, "/").await;
        assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{line}");
        assert!(refused.body.contains(message), "{line} : {}", refused.body);
    }
}

#[tokio::test]
async fn ajouter_une_tache_ou_une_note() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await;

    // Depuis Perso, une tâche pour École, sans changer d'espace.
    let added = alice.command("/add task Réviser le bac @ec", "/").await;
    assert_eq!(hx_location(&added)["path"], "/e/perso/");
    let (space, title, due): (i64, String, String) =
        sqlx::query_as("SELECT space_id, title, due_date FROM tasks")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(
        (space, title.as_str(), due),
        (
            ecole,
            "Réviser le bac",
            today().format("%Y-%m-%d").to_string()
        )
    );
    // Le message habituel s'affiche sur la page qui suit.
    assert!(
        alice
            .get("/")
            .await
            .body
            .contains("Tâche ajoutée dans École")
    );
    // Sans titre, ou avec deux espaces : refusé.
    let refused = alice.command("/add task @ecole", "/").await;
    assert!(refused.body.contains("Il manque le titre"));
    let refused = alice.command("/add task a @ecole @taff", "/").await;
    assert!(refused.body.contains("Un seul espace"));
    // L'espace d'un autre compte n'existe pas pour Alice.
    let mut bob = app.logged_in(BOB).await;
    bob.post(
        "/settings/spaces",
        &[("name", "Secret"), ("color", "rose"), ("icon", "heart")],
    )
    .await;
    let refused = alice.command("/add task x @secret", "/").await;
    assert!(refused.body.contains("Espace inconnu"));
    assert_eq!(count(&app, "SELECT COUNT(*) FROM tasks").await, 1);

    // Une note titrée, ouverte dans l'éditeur, dans l'espace demandé.
    let created = hx_location(&alice.command("/add note Idée de projet @taff", "/").await);
    let path = created["path"].as_str().unwrap();
    assert_eq!(created["headers"]["X-Espace"], "taff");
    let id: i64 = path.trim_start_matches("/notes/").parse().unwrap();
    let (title, space): (String, i64) =
        sqlx::query_as("SELECT title, space_id FROM notes WHERE id = ?")
            .bind(id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(title, "Idée de projet");
    assert_eq!(space, app.space_id(ALICE, "taff").await);
    // (La note s'ouvre à l'adresse de son espace.)
    let opened = alice.get(path).await;
    assert_eq!(opened.location(), format!("/e/taff{path}"));
    assert!(alice.follow(opened).await.body.contains("Idée de projet"));
    // Sans titre : une note vide, comme le bouton « Nouvelle note ».
    let blank = hx_location(&alice.command("/add note", "/").await);
    assert!(blank["path"].as_str().unwrap().starts_with("/notes/"));
    assert_eq!(count(&app, "SELECT COUNT(*) FROM notes").await, 2);
}

#[tokio::test]
async fn ouvrir_une_note_ou_un_fichier() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let created = alice.post("/notes", &[]).await;
    let note = created.location().to_string();
    alice
        .post(
            &note,
            &[
                ("title", "Révisions"),
                ("body_md", "Le trunk porte les VLAN."),
            ],
        )
        .await;
    alice.upload("Relevé de notes.pdf", b"%PDF-1.4").await;
    let file: i64 = sqlx::query_scalar("SELECT id FROM files")
        .fetch_one(&app.state.db)
        .await
        .unwrap();

    // Suggestions : la note par un mot de son texte, le fichier par son nom
    // (sans accents ni majuscules).
    let found = alice.get_fragment("/cmd/suggest?q=trunk").await;
    assert!(found.body.contains(r#""v":"Révisions""#));
    assert!(found.body.contains(r#""help":"Note · Perso""#));
    let found = alice.get_fragment("/cmd/suggest?q=releve").await;
    assert!(found.body.contains(r#""v":"Relevé de notes.pdf""#));
    assert!(found.body.contains(&format!(r#""href":"/files/{file}""#)));
    // Sans texte : les notes récentes.
    assert!(
        alice
            .get_fragment("/cmd/suggest?q=")
            .await
            .body
            .contains("Révisions")
    );

    // Entrée sur la ligne : le premier résultat s'ouvre.
    assert_eq!(
        hx_location(&alice.command("/open vlan", "/").await)["path"],
        note
    );
    assert_eq!(
        hx_location(&alice.command("/open relevé", "/").await)["path"],
        format!("/files/{file}")
    );
    let refused = alice.command("/open introuvable", "/").await;
    assert!(refused.body.contains("Rien ne correspond"));

    // Rien pour Bob.
    let mut bob = app.logged_in(BOB).await;
    assert_eq!(
        bob.get_fragment("/cmd/suggest?q=releve").await.body,
        r#"{"items":[]}"#
    );
}

#[tokio::test]
async fn agir_sur_la_note_ou_le_fichier_ouvert() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let taff = app.space_id(ALICE, "taff").await;

    // Une note : /move la déplace, comme son bouton.
    let created = alice.post("/notes", &[]).await;
    let note = created.location().to_string();
    let moved = hx_location(&alice.command("/move taff", &note).await);
    assert_eq!(moved["path"], "/e/perso/notes");
    let space: i64 = sqlx::query_scalar("SELECT space_id FROM notes")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(space, taff);
    assert!(
        alice
            .get("/notes")
            .await
            .body
            .contains("Note déplacée vers Taff")
    );

    // Un fichier : conversions et compression, comme son menu, puis retour
    // sur sa page.
    alice.upload("photo.png", b"png").await;
    let file: i64 = sqlx::query_scalar("SELECT id FROM files")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    let page = format!("/files/{file}");
    let converted = hx_location(&alice.command("/convert jpeg", &page).await);
    assert_eq!(converted["path"], page);
    let compressed = hx_location(&alice.command("/compress", &page).await);
    assert_eq!(compressed["path"], page);
    let targets: Vec<String> = sqlx::query_scalar(
        "SELECT target FROM jobs WHERE kind IN ('convert', 'compress') ORDER BY id",
    )
    .fetch_all(&app.state.db)
    .await
    .unwrap();
    assert!(targets.contains(&"jpg".to_string()));
    assert!(targets.contains(&"compress".to_string()));
    // Une sortie que son type ne permet pas n'existe pas dans la barre.
    let refused = alice.command("/convert mp3", &page).await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    // Le déplacer.
    hx_location(&alice.command("/move ecole", &page).await);
    let space: i64 = sqlx::query_scalar("SELECT space_id FROM files")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(space, app.space_id(ALICE, "ecole").await);

    // Le fichier d'Alice, vu par Bob : ces commandes n'existent pas pour lui.
    let mut bob = app.logged_in(BOB).await;
    let refused = bob.command("/move taff", &page).await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    let refused = bob.command("/compress", &page).await;
    assert!(
        refused
            .body
            .contains("S&#39;emploie sur un fichier ouvert.")
    );
}

// ---------------------------------------------------------------------
// Lot 2
// ---------------------------------------------------------------------

/// Une valeur de la base, pour une requête qui n'en rend qu'une.
async fn scalar<T>(app: &TestApp, sql: &str) -> T
where
    T: for<'r> sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite> + Send + Unpin,
{
    sqlx::query_scalar(sql)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn les_taches_au_clavier() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;

    // Une tâche pour demain : rangée à sa date, pas encore dans le widget.
    hx_location(&alice.command("/add task Rendre le DM +dem", "/").await);
    let tomorrow = today() + Duration::days(1);
    assert_eq!(
        scalar::<String>(&app, "SELECT due_date FROM tasks").await,
        tomorrow.format("%Y-%m-%d").to_string()
    );
    assert!(alice.get("/").await.body.contains(&format!(
        "Tâche ajoutée pour le {}",
        dashu::time::date_short(tomorrow)
    )));
    assert!(
        !alice
            .get("/widgets/tasks")
            .await
            .body
            .contains("Rendre le DM")
    );
    let refused = alice.command("/add task x +bientot", "/").await;
    assert!(refused.body.contains("Jour inconnu : +bientot"));
    let refused = alice.command("/add task x +demain +lundi", "/").await;
    assert!(refused.body.contains("Un seul jour"));

    // Les tâches du jour se cochent et se décochent par leur titre.
    let tree = alice.catalogue("/").await;
    assert_eq!(
        command_node(&tree, &["done"])["off"],
        "Aucune tâche à cocher aujourd'hui."
    );
    alice.command("/add task Réviser le bac", "/").await;
    alice.command("/add task Appeler mamie", "/").await;
    let tree = alice.catalogue("/").await;
    assert_eq!(command_node(&tree, &["done"])["arg"]["mode"], "line");
    assert_eq!(
        arg_values(command_node(&tree, &["done"])),
        ["Réviser le bac", "Appeler mamie"]
    );
    hx_location(&alice.command("/done rev", "/").await);
    assert!(
        scalar::<bool>(
            &app,
            "SELECT done FROM tasks WHERE title = 'Réviser le bac'"
        )
        .await
    );
    assert!(
        alice
            .get("/")
            .await
            .body
            .contains("Tâche cochée : Réviser le bac")
    );
    let tree = alice.catalogue("/").await;
    assert_eq!(
        arg_values(command_node(&tree, &["done"])),
        ["Appeler mamie"]
    );
    assert_eq!(
        arg_values(command_node(&tree, &["undone"])),
        ["Réviser le bac"]
    );
    hx_location(&alice.command("/undone bac", "/").await);
    assert!(
        !scalar::<bool>(
            &app,
            "SELECT done FROM tasks WHERE title = 'Réviser le bac'"
        )
        .await
    );
    let refused = alice.command("/done zz", "/").await;
    assert!(
        refused
            .body
            .contains("Aucune tâche du jour ne correspond à « zz »")
    );

    // Supprimer une tâche (le navigateur demande d'abord confirmation).
    let tree = alice.catalogue("/").await;
    assert!(
        command_node(&tree, &["delete", "task"])["confirm"]
            .as_str()
            .unwrap()
            .contains("Entrée encore")
    );
    hx_location(&alice.command("/delete task appeler", "/").await);
    assert_eq!(count(&app, "SELECT COUNT(*) FROM tasks").await, 2);

    // Bob n'a rien à cocher, et ne touche pas aux tâches d'Alice.
    let mut bob = app.logged_in(BOB).await;
    let refused = bob.command("/done Réviser", "/").await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    assert!(
        !scalar::<bool>(
            &app,
            "SELECT done FROM tasks WHERE title = 'Réviser le bac'"
        )
        .await
    );
}

#[tokio::test]
async fn la_note_au_clavier() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let created = alice.post("/notes", &[]).await;
    let note = created.location().to_string();
    alice
        .post(&note, &[("title", "Réseaux"), ("body_md", "VLAN")])
        .await;

    // Tags : ajouter, puis retirer par un début de nom.
    let back = hx_location(&alice.command("/tag Cours", &note).await);
    assert_eq!(back["path"], note);
    assert!(alice.get(&note).await.body.contains("Tag « cours » ajouté"));
    let tree = alice.catalogue(&note).await;
    assert_eq!(arg_values(command_node(&tree, &["untag"])), ["cours"]);
    hx_location(&alice.command("/untag co", &note).await);
    assert_eq!(count(&app, "SELECT COUNT(*) FROM note_tags").await, 0);
    // Le tag sans note disparaît, comme avec le bouton.
    assert_eq!(count(&app, "SELECT COUNT(*) FROM tags").await, 0);

    // Épingler, désépingler.
    hx_location(&alice.command("/pin", &note).await);
    assert!(scalar::<bool>(&app, "SELECT pinned FROM notes").await);
    let tree = alice.catalogue(&note).await;
    assert_eq!(
        command_node(&tree, &["pin"])["help"],
        "Désépingler cette note"
    );
    hx_location(&alice.command("/pin", &note).await);
    assert!(!scalar::<bool>(&app, "SELECT pinned FROM notes").await);

    // Supprimer la note ouverte, après confirmation dans le navigateur.
    assert_eq!(
        command_node(&tree, &["delete"])["confirm"],
        "Supprimer la note « Réseaux » ? Entrée encore pour confirmer."
    );
    let deleted = hx_location(&alice.command("/delete", &note).await);
    assert_eq!(deleted["path"], "/e/perso/notes");
    assert_eq!(count(&app, "SELECT COUNT(*) FROM notes").await, 0);

    // Ailleurs que sur une note, ces commandes le disent.
    let refused = alice.command("/pin", "/").await;
    assert!(refused.body.contains("S&#39;emploie sur une note ouverte."));
}

#[tokio::test]
async fn le_fichier_au_clavier() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.upload("cours.pdf", b"%PDF-1.4").await;
    let file: i64 = scalar(&app, "SELECT id FROM files").await;
    let page = format!("/files/{file}");

    // Télécharger : une vraie navigation (HX-Redirect), pas une page htmx.
    let download = alice.command("/download", &page).await;
    assert_eq!(download.status, StatusCode::NO_CONTENT);
    assert_eq!(
        download.header("hx-redirect"),
        format!("/files/{file}/download")
    );
    let deleted = hx_location(&alice.command("/delete", &page).await);
    assert_eq!(deleted["path"], "/e/perso/files");
    assert_eq!(count(&app, "SELECT COUNT(*) FROM files").await, 0);

    // Un fichier de l'onglet Outils : se garde dans un espace, ne se
    // déplace pas avant.
    alice.upload_to_tools("photo.png", b"png").await;
    let file: i64 = scalar(&app, "SELECT id FROM files").await;
    let page = format!("/files/{file}");
    let refused = alice.command("/move ecole", &page).await;
    assert!(refused.body.contains("pas encore rangé"));
    let kept = hx_location(&alice.command("/keep ecole", &page).await);
    assert_eq!(kept["path"], "/tools");
    let (space, expires): (i64, Option<i64>) =
        sqlx::query_as("SELECT space_id, expires_at FROM files")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(space, app.space_id(ALICE, "ecole").await);
    assert_eq!(expires, None);
    let refused = alice.command("/keep taff", &page).await;
    assert!(refused.body.contains("déjà rangé"));
}

#[tokio::test]
async fn les_reglages_au_clavier() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let user = app.user_id(ALICE).await;
    let setting = |column: &str| format!("SELECT {column} FROM users WHERE id = {user}");

    // Thème, taille, barre latérale : enregistrés, puis retour à la page.
    let back = hx_location(&alice.command("/theme sombre", "/notes").await);
    assert_eq!(back["path"], "/e/perso/notes");
    assert_eq!(scalar::<String>(&app, &setting("theme")).await, "dark");
    hx_location(&alice.command("/theme sys", "/").await);
    assert_eq!(
        scalar::<Option<String>>(&app, &setting("theme")).await,
        None
    );
    hx_location(&alice.command("/size grand", "/").await);
    assert_eq!(scalar::<String>(&app, &setting("ui_size")).await, "large");
    hx_location(&alice.command("/sidebar", "/").await);
    assert!(scalar::<bool>(&app, &setting("sidebar_floating")).await);
    let tree = alice.catalogue("/").await;
    assert_eq!(
        command_node(&tree, &["sidebar"])["help"],
        "Ancrer la barre latérale"
    );
    hx_location(&alice.command("/sidebar", "/").await);
    assert!(!scalar::<bool>(&app, &setting("sidebar_floating")).await);

    // Resynchroniser l'agenda : on arrive sur l'agenda.
    assert_eq!(
        hx_location(&alice.command("/refresh", "/").await)["path"],
        "/e/perso/calendar"
    );

    // Créer un espace : on y entre. Un nom déjà pris : Réglages, avec le message.
    let created = hx_location(&alice.command("/add space Sport", "/").await);
    assert_eq!(created["path"], "/e/sport/");
    assert_eq!(created["headers"]["X-Espace"], "sport");
    let again = hx_location(&alice.command("/add space sport", "/").await);
    assert_eq!(again["path"], "/settings#espaces");
    assert!(
        alice
            .get("/settings")
            .await
            .body
            .contains("Un espace s&#39;appelle déjà")
    );

    // Un agenda : l'adresse passe par les mêmes vérifications que Réglages.
    let refused = alice.command("/add agenda Pronote", "/").await;
    assert!(
        refused
            .body
            .contains("Il manque l&#39;adresse du flux iCal")
    );
    hx_location(
        &alice
            .command(
                "/add agenda http://campus.example.test/a.ics @ecole Pronote",
                "/",
            )
            .await,
    );
    assert!(
        alice
            .get("/settings")
            .await
            .body
            .contains("L&#39;URL doit commencer par https://")
    );
    assert_eq!(
        count(&app, "SELECT COUNT(*) FROM calendar_sources").await,
        0
    );

    // Se déconnecter : une vraie navigation vers la connexion, session finie.
    let out = alice.command("/logout", "/").await;
    assert_eq!(out.header("hx-redirect"), "/login");
    assert_eq!(alice.get("/").await.location(), "/login");
}

// ---------------------------------------------------------------------
// Variables #date et #cours
// ---------------------------------------------------------------------

/// Un cours qui a commencé il y a dix minutes, dans l'agenda d'École.
async fn course_now(app: &TestApp, title: &str) {
    let now = dashu::time::now();
    let source: i64 = sqlx::query_scalar(
        "INSERT INTO calendar_sources (user_id, space_id, name, ical_url, created_at)
         VALUES (?, ?, 'Pronote', 'https://pronote.example.test/a.ics', ?) RETURNING id",
    )
    .bind(app.user_id(ALICE).await)
    .bind(app.space_id(ALICE, "ecole").await)
    .bind(now)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    let event = dashu::calendar::ical::ParsedEvent {
        uid: "cours".into(),
        title: title.into(),
        location: "Salle 12".into(),
        starts_at: now - 600,
        ends_at: now + 3000,
        all_day: false,
    };
    dashu::calendar::sync::store(&app.state.db, source, &[event], now)
        .await
        .unwrap();
}

#[tokio::test]
async fn date_et_cours_dans_les_textes() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let date = today().format("%d/%m/%Y").to_string();

    // #date : la date du jour, dans une tâche comme dans une note.
    hx_location(
        &alice
            .command("/add task Relire le cours du #date", "/")
            .await,
    );
    assert_eq!(
        scalar::<String>(&app, "SELECT title FROM tasks").await,
        format!("Relire le cours du {date}")
    );

    // Sans cours en ce moment, #cours est refusé, et la complétion le dit.
    let refused = alice.command("/add note #cours", "/").await;
    assert!(refused.body.contains("Aucun cours en ce moment"));
    let tree = alice.catalogue("/").await;
    let values = &command_node(&tree, &["add", "note"])["arg"]["values"];
    let help = |v: &str| {
        values
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["v"] == v)
            .map(|c| c["help"].as_str().unwrap().to_string())
            .unwrap()
    };
    assert_eq!(help("#date"), format!("Date du jour : {date}"));
    assert_eq!(help("#cours"), "Aucun cours en ce moment");

    // Pendant un cours d'École : sa matière, aussi depuis Perso.
    course_now(&app, "MATHS - DUPONT J. - Salle 12").await;
    let tree = alice.catalogue("/").await;
    let values = &command_node(&tree, &["add", "note"])["arg"]["values"];
    assert!(values.to_string().contains("Cours en cours : MATHS"));
    let created = hx_location(&alice.command("/add note #cours du #date @ecole", "/").await);
    let id: i64 = created["path"]
        .as_str()
        .unwrap()
        .trim_start_matches("/notes/")
        .parse()
        .unwrap();
    let title: String = sqlx::query_scalar("SELECT title FROM notes WHERE id = ?")
        .bind(id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(title, format!("MATHS du {date}"));
    // Un tag au nom du cours, sur la note ouverte.
    hx_location(&alice.command("/tag #cours", &format!("/notes/{id}")).await);
    assert_eq!(
        scalar::<String>(&app, "SELECT name FROM tags").await,
        "maths"
    );
    // Un autre # reste tel quel.
    hx_location(&alice.command("/add task Exercice #3", "/").await);
    let kept = "SELECT COUNT(*) FROM tasks WHERE title = 'Exercice #3'";
    assert_eq!(count(&app, kept).await, 1);
    // Depuis Taff, le cours d'École n'est pas « en cours ».
    alice.with_header("x-espace", "taff");
    let refused = alice.command("/add note #cours", "/").await;
    assert!(refused.body.contains("Aucun cours en ce moment"));
}
