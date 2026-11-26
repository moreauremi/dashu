//! Dossiers sur cinq niveaux (SPEC, « Dossiers : cinq niveaux ») : l'espace,
//! puis quatre niveaux de dossiers. La limite est vérifiée par le serveur
//! partout (site, API, connecteur), au déplacement comme à la création, et
//! tout ce qui parcourt l'arborescence (compte, recherche, corbeille,
//! changement d'espace) descend jusqu'en bas.

mod common;

use axum::http::{Method, StatusCode};
use common::{ALICE, BOB, Client, TestApp, between};
use serde_json::{Value, json};

/// Identifiant d'un dossier, par son nom (unique dans ces tests).
async fn folder(app: &TestApp, name: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM folders WHERE name = ?")
        .bind(name)
        .fetch_one(&app.state.db)
        .await
        .unwrap_or_else(|_| panic!("dossier « {name} » absent"))
}

/// (espace, dossier parent) d'un dossier.
async fn folder_place(app: &TestApp, id: i64) -> (i64, Option<i64>) {
    sqlx::query_as("SELECT space_id, parent_id FROM folders WHERE id = ?")
        .bind(id)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

/// (espace, dossier) d'une ligne de `table` (`notes` par titre, `files` par nom).
async fn place_of(app: &TestApp, table: &str, name: &str) -> (i64, Option<i64>) {
    let column = if table == "notes" { "title" } else { "name" };
    sqlx::query_as(&format!(
        "SELECT space_id, folder_id FROM {table} WHERE {column} = ?"
    ))
    .bind(name)
    .fetch_one(&app.state.db)
    .await
    .unwrap_or_else(|_| panic!("« {name} » absent de {table}"))
}

/// Vrai si la ligne existe hors de la corbeille.
async fn alive(app: &TestApp, table: &str, name: &str) -> bool {
    let column = if table == "notes" { "title" } else { "name" };
    sqlx::query_scalar::<_, i64>(&format!(
        "SELECT count(*) FROM {table} WHERE {column} = ? AND deleted_at IS NULL"
    ))
    .bind(name)
    .fetch_one(&app.state.db)
    .await
    .unwrap()
        > 0
}

async fn count_folders(app: &TestApp) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM folders WHERE deleted_at IS NULL")
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

/// Crée un dossier dans un espace et rend son identifiant.
async fn in_space(app: &TestApp, client: &mut Client, space: i64, name: &str) -> i64 {
    let created = client
        .post(
            "/folders",
            &[("name", name), ("space_id", &space.to_string())],
        )
        .await;
    assert_eq!(created.status, StatusCode::SEE_OTHER, "{}", created.body);
    folder(app, name).await
}

/// Crée un sous-dossier et rend son identifiant.
async fn in_folder(app: &TestApp, client: &mut Client, parent: i64, name: &str) -> i64 {
    let created = client
        .post(
            "/folders",
            &[("name", name), ("parent_id", &parent.to_string())],
        )
        .await;
    assert_eq!(created.status, StatusCode::SEE_OTHER, "{}", created.body);
    folder(app, name).await
}

/// École / Math / Analyse / Chapitre 1 / Exercices : cinq niveaux, l'espace
/// compris. Rend les quatre dossiers, du haut vers le bas.
async fn deep_chain(app: &TestApp, alice: &mut Client) -> [i64; 4] {
    let ecole = app.space_id(ALICE, "ecole").await;
    let math = in_space(app, alice, ecole, "Math").await;
    let analyse = in_folder(app, alice, math, "Analyse").await;
    let chapitre = in_folder(app, alice, analyse, "Chapitre 1").await;
    let exercices = in_folder(app, alice, chapitre, "Exercices").await;
    [math, analyse, chapitre, exercices]
}

/// Crée une note titrée dans l'espace actif et rend son identifiant.
async fn note(client: &mut Client, title: &str) -> i64 {
    let created = client.post("/notes", &[]).await;
    let id = common::note_id(&created);
    client
        .post(
            &format!("/notes/{id}"),
            &[("title", title), ("body_md", "contenu")],
        )
        .await;
    id
}

/// Range une note par la fenêtre « Ranger ».
async fn file_note(client: &mut Client, id: i64, space: i64, folder: i64) {
    let moved = client
        .post(
            &format!("/notes/{id}/move"),
            &[
                ("space_id", &space.to_string()),
                ("folder_id", &folder.to_string()),
            ],
        )
        .await;
    assert_eq!(moved.status, StatusCode::SEE_OTHER, "{}", moved.body);
}

/// Déplace un dossier par la fenêtre « Ranger » ; rend la page d'arrivée
/// (avec le message).
async fn move_folder(client: &mut Client, id: i64, space: i64, parent: Option<i64>) -> String {
    let parent = parent.map(|p| p.to_string()).unwrap_or_default();
    let moved = client
        .post(
            &format!("/folders/{id}/move"),
            &[("space_id", &space.to_string()), ("folder_id", &parent)],
        )
        .await;
    assert_eq!(moved.status, StatusCode::SEE_OTHER, "{}", moved.body);
    client.follow(moved).await.body
}

/// Le bouton de la fenêtre « Ranger » qui mène au dossier `id`, s'il y est.
fn dest_button(picker: &str, id: i64) -> Option<&str> {
    between(
        picker,
        &format!("name=\"folder_id\" value=\"{id}\""),
        "</button>",
    )
}

#[tokio::test]
async fn cinq_niveaux_de_dossiers_au_plus() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.post("/space", &[("slug", "ecole")]).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    let [math, analyse, chapitre, exercices] = deep_chain(&app, &mut alice).await;

    // Chacun sous le précédent : quatre niveaux de dossiers sous École.
    assert_eq!(folder_place(&app, math).await, (ecole, None));
    assert_eq!(folder_place(&app, analyse).await, (ecole, Some(math)));
    assert_eq!(folder_place(&app, chapitre).await, (ecole, Some(analyse)));
    assert_eq!(folder_place(&app, exercices).await, (ecole, Some(chapitre)));

    // Au quatrième niveau, « Nouveau dossier » n'est plus proposé ; au
    // troisième, il l'est encore.
    let page = alice.get(&format!("/files?dossier={chapitre}")).await;
    assert!(page.body.contains("Nouveau dossier"));
    assert!(
        page.body
            .contains(&format!("name=\"parent_id\" value=\"{chapitre}\""))
    );
    let page = alice.get(&format!("/files?dossier={exercices}")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(!page.body.contains("Nouveau dossier"));

    // Et le serveur le refuse, pas seulement caché : rien n'est créé.
    let refused = alice
        .post(
            "/folders",
            &[("name", "Trop loin"), ("parent_id", &exercices.to_string())],
        )
        .await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    assert!(
        refused
            .body
            .contains("Cinq niveaux au plus : pas de dossier de plus dans « Exercices »."),
        "{}",
        refused.body
    );
    assert_eq!(count_folders(&app).await, 4);

    // Le fil d'Ariane montre tout le chemin : Fichiers › École › Math ›
    // Analyse › Chapitre 1, puis le dossier ouvert en titre.
    let crumbs = between(&page.body, "<nav class=\"crumbs\"", "</nav>").unwrap();
    assert!(crumbs.contains("href=\"/files?dossier=racine\""));
    assert!(crumbs.contains("href=\"/e/ecole/files?dossier=espace\""));
    for id in [math, analyse, chapitre] {
        assert!(
            crumbs.contains(&format!("href=\"/e/ecole/files?dossier={id}\"")),
            "{id}"
        );
    }
    assert!(page.body.contains("<h1>Exercices</h1>"));

    // La fenêtre « Ranger » d'une note propose les quatre niveaux, chacun
    // avec son retrait.
    let cours = note(&mut alice, "Cours").await;
    let picker = alice
        .get_fragment(&format!("/destinations?note={cours}"))
        .await;
    for (id, depth) in [(math, 1), (analyse, 2), (chapitre, 3), (exercices, 4)] {
        let button = dest_button(&picker.body, id).unwrap_or_else(|| panic!("{id} absent"));
        assert!(
            button.contains(&format!("dest dest-d{depth}\"")),
            "{button}"
        );
    }

    // La note se range au niveau le plus profond, et l'éditeur dit où elle est.
    file_note(&mut alice, cours, ecole, exercices).await;
    assert_eq!(
        place_of(&app, "notes", "Cours").await,
        (ecole, Some(exercices))
    );
    let editor = alice.get(&format!("/notes/{cours}")).await;
    assert!(
        editor
            .body
            .contains("École / Math / Analyse / Chapitre 1 / Exercices")
    );
}

/// Un message JSON-RPC envoyé à `/mcp` ; le résultat d'un outil.
async fn mcp_call(app: &TestApp, token: &str, tool: &str, arguments: Value) -> Value {
    let (status, body) = app
        .api(
            Method::POST,
            "/mcp",
            Some(json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": tool, "arguments": arguments }
            })),
            Some(token),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["result"].clone()
}

/// Un jeton personnel, créé depuis Réglages › Connexions.
async fn token(app: &TestApp, email: &str) -> String {
    let mut client = app.logged_in(email).await;
    let response = client
        .post("/settings/tokens", &[("name", "Script"), ("duree", "90")])
        .await;
    between(
        &response.body,
        "id=\"new-token\" type=\"text\" value=\"",
        "\"",
    )
    .expect("jeton affiché")
    .to_string()
}

#[tokio::test]
async fn l_api_et_le_connecteur_suivent_les_cinq_niveaux() {
    let app = TestApp::new().await;
    let tk = token(&app, ALICE).await;
    let t = Some(tk.as_str());

    // L'API crée quatre niveaux de dossiers sous l'espace.
    let (status, created) = app
        .api(
            Method::POST,
            "/api/v1/folders",
            Some(json!({ "name": "Math", "space": "ecole" })),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let mut parent = created["id"].as_i64().unwrap();
    let math = parent;
    for name in ["Analyse", "Chapitre 1"] {
        let (status, created) = app
            .api(
                Method::POST,
                "/api/v1/folders",
                Some(json!({ "name": name, "parent": parent })),
                t,
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        assert_eq!(created["parent"], parent);
        assert_eq!(created["space"], "ecole");
        parent = created["id"].as_i64().unwrap();
    }
    let chapitre = parent;

    // Le connecteur crée le quatrième niveau…
    let result = mcp_call(
        &app,
        &tk,
        "creer_dossier",
        json!({ "name": "Exercices", "parent": chapitre }),
    )
    .await;
    assert_eq!(result["isError"], false, "{result}");
    let exercices = folder(&app, "Exercices").await;
    assert_eq!(
        folder_place(&app, exercices).await,
        (app.space_id(ALICE, "ecole").await, Some(chapitre))
    );

    // … et refuse le cinquième, comme l'API, avec le même message.
    let result = mcp_call(
        &app,
        &tk,
        "creer_dossier",
        json!({ "name": "Trop loin", "parent": exercices }),
    )
    .await;
    assert_eq!(result["isError"], true, "{result}");
    let message = "Cinq niveaux au plus : pas de dossier de plus dans « Exercices ».";
    assert!(result.to_string().contains(message), "{result}");
    let (status, refused) = app
        .api(
            Method::POST,
            "/api/v1/folders",
            Some(json!({ "name": "Trop loin", "parent": exercices })),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(refused.to_string().contains(message), "{refused}");
    assert_eq!(count_folders(&app).await, 4);

    // Déplacer : Math et ses sous-dossiers (quatre étages) ne tiennent pas dans un dossier
    // (refus, rien ne bouge) ; Chapitre 1 et Exercices, si.
    let (_, other) = app
        .api(
            Method::POST,
            "/api/v1/folders",
            Some(json!({ "name": "Autre", "space": "taff" })),
            t,
        )
        .await;
    let other = other["id"].as_i64().unwrap();
    let (status, refused) = app
        .api(
            Method::PATCH,
            &format!("/api/v1/folders/{math}"),
            Some(json!({ "space": "taff", "parent": other })),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert!(refused.to_string().contains("Cinq niveaux au plus"));
    assert_eq!(
        folder_place(&app, math).await,
        (app.space_id(ALICE, "ecole").await, None)
    );
    let (status, moved) = app
        .api(
            Method::PATCH,
            &format!("/api/v1/folders/{chapitre}"),
            Some(json!({ "space": "taff", "parent": other })),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    assert_eq!(moved["parent"], other);
    let taff = app.space_id(ALICE, "taff").await;
    assert_eq!(folder_place(&app, exercices).await, (taff, Some(chapitre)));

    // La liste du connecteur donne chaque parent.
    let result = mcp_call(&app, &tk, "lister_dossiers", json!({ "space": "taff" })).await;
    let text = result["content"][0]["text"].as_str().unwrap();
    let list: Value = serde_json::from_str(text).unwrap();
    let parents: Vec<(i64, Option<i64>)> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["id"].as_i64().unwrap(), f["parent"].as_i64()))
        .collect();
    assert!(parents.contains(&(exercices, Some(chapitre))));
    assert!(parents.contains(&(chapitre, Some(other))));
}

#[tokio::test]
async fn deplacer_un_dossier_avec_tous_ses_niveaux() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.post("/space", &[("slug", "ecole")]).await;
    let (ecole, taff) = (
        app.space_id(ALICE, "ecole").await,
        app.space_id(ALICE, "taff").await,
    );
    let [math, analyse, chapitre, exercices] = deep_chain(&app, &mut alice).await;
    let projet = in_space(&app, &mut alice, taff, "Projet").await;
    let lot = in_folder(&app, &mut alice, projet, "Lot A").await;

    // La fenêtre d'Analyse (lui, Chapitre 1 et Exercices : trois étages) : les
    // espaces et les dossiers du premier niveau seulement, sans lui ni ce
    // qu'il contient.
    let picker = alice
        .get_fragment(&format!("/destinations?dossier={analyse}"))
        .await;
    assert!(dest_button(&picker.body, projet).is_some());
    assert!(dest_button(&picker.body, math).is_some());
    for id in [lot, analyse, chapitre, exercices] {
        assert!(dest_button(&picker.body, id).is_none(), "{id}");
    }
    // Celle de Chapitre 1 (deux niveaux) va jusqu'au deuxième.
    let picker = alice
        .get_fragment(&format!("/destinations?dossier={chapitre}"))
        .await;
    assert!(dest_button(&picker.body, lot).unwrap().contains("dest-d2"));
    assert!(!picker.body.contains("dest-d3") && !picker.body.contains("dest-d4"));
    assert!(dest_button(&picker.body, exercices).is_none());

    // Analyse ne tient pas dans Lot A (il descendrait au sixième niveau) :
    // refus dit, rien ne bouge.
    let page = move_folder(&mut alice, analyse, taff, Some(lot)).await;
    assert!(
        page.contains(
            "Cinq niveaux au plus : pas de place pour « Analyse » dans « Taff / Projet / Lot A »."
        ),
        "{page}"
    );
    assert_eq!(folder_place(&app, analyse).await, (ecole, Some(math)));
    // Ni dans un de ses sous-dossiers, ni dans lui-même.
    let page = move_folder(&mut alice, analyse, ecole, Some(exercices)).await;
    assert!(page.contains("Un dossier ne va pas dans un de ses sous-dossiers."));
    let page = move_folder(&mut alice, analyse, ecole, Some(analyse)).await;
    assert!(page.contains("Un dossier ne va pas dans lui-même."));
    assert_eq!(folder_place(&app, analyse).await, (ecole, Some(math)));
    assert_eq!(folder_place(&app, exercices).await, (ecole, Some(chapitre)));

    // Dans Projet, il tient : il y va avec ses deux niveaux.
    let page = move_folder(&mut alice, analyse, taff, Some(projet)).await;
    assert!(page.contains("Dossier « Analyse » déplacé dans « Taff / Projet »"));
    assert_eq!(folder_place(&app, analyse).await, (taff, Some(projet)));
    assert_eq!(folder_place(&app, chapitre).await, (taff, Some(analyse)));
    assert_eq!(folder_place(&app, exercices).await, (taff, Some(chapitre)));
    // Et Chapitre 1 (deux niveaux) tient dans Lot A, au deuxième niveau.
    move_folder(&mut alice, chapitre, taff, Some(lot)).await;
    assert_eq!(folder_place(&app, chapitre).await, (taff, Some(lot)));
    assert_eq!(folder_place(&app, exercices).await, (taff, Some(chapitre)));
}

#[tokio::test]
async fn changer_d_espace_emmene_tous_les_niveaux() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.post("/space", &[("slug", "ecole")]).await;
    let (ecole, taff) = (
        app.space_id(ALICE, "ecole").await,
        app.space_id(ALICE, "taff").await,
    );
    let [math, analyse, chapitre, exercices] = deep_chain(&app, &mut alice).await;
    // Une note et un fichier à chaque niveau.
    for (level, id) in [math, analyse, chapitre, exercices].into_iter().enumerate() {
        let title = format!("Note {}", level + 1);
        let n = note(&mut alice, &title).await;
        file_note(&mut alice, n, ecole, id).await;
        let sent = alice
            .upload_in_folder(&format!("niveau{}.pdf", level + 1), b"%PDF", id)
            .await;
        assert!(sent.status.is_success(), "{}", sent.body);
    }

    // Math part dans Taff : tout le suit, jusqu'en bas.
    let page = move_folder(&mut alice, math, taff, None).await;
    assert!(page.contains("Dossier « Math » déplacé dans « Taff »"));
    for (level, id) in [math, analyse, chapitre, exercices].into_iter().enumerate() {
        let n = level + 1;
        assert_eq!(folder_place(&app, id).await.0, taff, "dossier {n}");
        assert_eq!(
            place_of(&app, "notes", &format!("Note {n}")).await,
            (taff, Some(id))
        );
        assert_eq!(
            place_of(&app, "files", &format!("niveau{n}.pdf")).await,
            (taff, Some(id))
        );
    }
    // École n'a plus rien.
    let left: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM folders WHERE space_id = ?1)
              + (SELECT count(*) FROM files WHERE space_id = ?1)",
    )
    .bind(ecole)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn le_contenu_compte_tous_les_niveaux() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.post("/space", &[("slug", "ecole")]).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    let [math, analyse, chapitre, exercices] = deep_chain(&app, &mut alice).await;
    let francais = in_space(&app, &mut alice, ecole, "Français").await;
    let cours = note(&mut alice, "Cours de limites").await;
    file_note(&mut alice, cours, ecole, analyse).await;
    let exo = note(&mut alice, "Exo de limites").await;
    file_note(&mut alice, exo, ecole, exercices).await;
    alice
        .upload_in_folder("corrige.pdf", b"%PDF corrige", chapitre)
        .await;

    // Le compte de Math et d'Analyse descend jusqu'en bas.
    let root = alice.get("/files?dossier=espace").await;
    let row = between(&root.body, &format!("dossier={math}\""), "</li>").unwrap();
    assert!(row.contains("3 dossiers, 2 notes et 1 fichier"), "{row}");
    let page = alice.get(&format!("/files?dossier={math}")).await;
    let row = between(&page.body, &format!("dossier={analyse}\""), "</li>").unwrap();
    assert!(row.contains("2 dossiers, 2 notes et 1 fichier"), "{row}");

    // La recherche dans Math trouve le fichier du troisième niveau ; celle
    // dans Français, non.
    let found = alice.get(&format!("/files?dossier={math}&q=corrige")).await;
    assert!(found.body.contains("corrige.pdf"), "{}", found.body);
    let elsewhere = alice
        .get(&format!("/files?dossier={francais}&q=corrige"))
        .await;
    assert!(!elsewhere.body.contains("corrige.pdf"));

    // L'onglet Notes : la note du quatrième niveau est dans la section de
    // Math, avec le chemin qui y mène.
    let list = alice.get("/notes").await.body;
    let section = &list[list.find("aria-label=\"Dossier Math\"").unwrap()..];
    let row = between(section, "Exo de limites", "</li>").unwrap();
    assert!(row.contains("class=\"note-sub\""));
    assert!(row.contains("Analyse / Chapitre 1 / Exercices"), "{row}");
    let row = between(section, "Cours de limites", "</li>").unwrap();
    assert!(row.contains("Analyse"));
}

#[tokio::test]
async fn supprimer_un_dossier_au_milieu_de_l_arborescence() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.post("/space", &[("slug", "ecole")]).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    let [math, analyse, chapitre, exercices] = deep_chain(&app, &mut alice).await;
    // Un autre « Chapitre 1 » est déjà dans Math.
    let homonyme = in_folder(&app, &mut alice, math, "chapitre 1").await;
    let resume = note(&mut alice, "Résumé").await;
    file_note(&mut alice, resume, ecole, analyse).await;
    let exo = note(&mut alice, "Exo").await;
    file_note(&mut alice, exo, ecole, exercices).await;
    alice
        .upload_in_folder("td.pdf", b"%PDF td", exercices)
        .await;

    // Transférer Analyse : son contenu va dans Math, et Chapitre 1 monte d'un
    // niveau dans Math (pas à la racine d'École), renommé car le nom est pris.
    let moved = alice
        .post(
            &format!("/folders/{analyse}/delete"),
            &[("contenu", "remonter")],
        )
        .await;
    let page = alice.follow(moved).await;
    assert!(page.body.contains(
        "Dossier « Analyse » mis dans la corbeille : son contenu est dans « École / Math »"
    ));
    assert_eq!(place_of(&app, "notes", "Résumé").await, (ecole, Some(math)));
    assert_eq!(folder_place(&app, chapitre).await, (ecole, Some(math)));
    assert_eq!(folder_place(&app, homonyme).await, (ecole, Some(math)));
    let name: String = sqlx::query_scalar("SELECT name FROM folders WHERE id = ?")
        .bind(chapitre)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(name, "Chapitre 1 (2)");
    // Ce qu'il contenait garde sa place.
    assert_eq!(folder_place(&app, exercices).await, (ecole, Some(chapitre)));
    assert_eq!(
        place_of(&app, "notes", "Exo").await,
        (ecole, Some(exercices))
    );

    // Tout supprimer Math : tout part dans la corbeille, à tous les niveaux ;
    // le disque se vide quand on l'y efface.
    let stored = app.stored_files().len();
    let erased = alice
        .post(
            &format!("/folders/{math}/delete"),
            &[("contenu", "supprimer")],
        )
        .await;
    assert!(
        alice
            .follow(erased)
            .await
            .body
            .contains("Dossier « Math » mis dans la corbeille, avec son contenu")
    );
    for (table, name) in [
        ("notes", "Résumé"),
        ("notes", "Exo"),
        ("files", "td.pdf"),
        ("folders", "Chapitre 1 (2)"),
        ("folders", "Exercices"),
        ("folders", "chapitre 1"),
    ] {
        assert!(!alive(&app, table, name).await, "{table} {name}");
    }
    let trash = alice.get("/settings/trash").await;
    assert!(
        trash.body.contains("2 notes, 1 fichier, 3 sous-dossiers"),
        "{}",
        trash.body
    );
    assert_eq!(app.stored_files().len(), stored);
    alice
        .post(&format!("/settings/trash/dossier/{math}/erase"), &[])
        .await;
    assert_eq!(app.stored_files().len(), stored - 1);
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM folders")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    // Seul Analyse, mis seul dans la corbeille, y reste.
    assert_eq!(left, 1);
}

#[tokio::test]
async fn restaurer_un_dossier_profond() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.post("/space", &[("slug", "ecole")]).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    let [_, analyse, chapitre, exercices] = deep_chain(&app, &mut alice).await;
    let exo = note(&mut alice, "Exo").await;
    file_note(&mut alice, exo, ecole, exercices).await;

    // Chapitre 1 part dans la corbeille avec Exercices, puis revient dans
    // Analyse : il y tient encore.
    alice
        .post(
            &format!("/folders/{chapitre}/delete"),
            &[("contenu", "supprimer")],
        )
        .await;
    assert!(!alive(&app, "folders", "Exercices").await);
    let restored = alice
        .post(&format!("/settings/trash/dossier/{chapitre}/restore"), &[])
        .await;
    assert_eq!(restored.status, StatusCode::SEE_OTHER);
    assert_eq!(folder_place(&app, chapitre).await, (ecole, Some(analyse)));
    assert_eq!(folder_place(&app, exercices).await, (ecole, Some(chapitre)));
    assert_eq!(
        place_of(&app, "notes", "Exo").await,
        (ecole, Some(exercices))
    );

    // De nouveau dans la corbeille ; entre-temps, Analyse descend d'un niveau
    // (dans Français / Grammaire). Chapitre 1 et Exercices n'y tiendraient
    // plus : ils reviennent à la racine d'École, ensemble.
    alice
        .post(
            &format!("/folders/{chapitre}/delete"),
            &[("contenu", "supprimer")],
        )
        .await;
    let francais = in_space(&app, &mut alice, ecole, "Français").await;
    let grammaire = in_folder(&app, &mut alice, francais, "Grammaire").await;
    move_folder(&mut alice, analyse, ecole, Some(grammaire)).await;
    assert_eq!(folder_place(&app, analyse).await, (ecole, Some(grammaire)));
    alice
        .post(&format!("/settings/trash/dossier/{chapitre}/restore"), &[])
        .await;
    assert_eq!(folder_place(&app, chapitre).await, (ecole, None));
    assert_eq!(folder_place(&app, exercices).await, (ecole, Some(chapitre)));
    assert_eq!(
        place_of(&app, "notes", "Exo").await,
        (ecole, Some(exercices))
    );
}

#[tokio::test]
async fn un_autre_compte_ne_touche_pas_aux_dossiers_profonds() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.post("/space", &[("slug", "ecole")]).await;
    let [_, _, chapitre, exercices] = deep_chain(&app, &mut alice).await;

    let mut bob = app.logged_in(BOB).await;
    let bob_perso = app.space_id(BOB, "perso").await;
    // Il ne crée rien dans un dossier profond d'Alice…
    let refused = bob
        .post(
            "/folders",
            &[("name", "Intrus"), ("parent_id", &chapitre.to_string())],
        )
        .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
    // … ne l'ouvre pas, ne voit pas sa fenêtre « Ranger »…
    assert_eq!(
        bob.get(&format!("/files?dossier={exercices}")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        bob.get_fragment(&format!("/destinations?dossier={exercices}"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    // … et n'y range pas son propre dossier.
    let mine = in_space(&app, &mut bob, bob_perso, "Perso de Bob").await;
    let refused = bob
        .post(
            &format!("/folders/{mine}/move"),
            &[
                ("space_id", &bob_perso.to_string()),
                ("folder_id", &chapitre.to_string()),
            ],
        )
        .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
    assert_eq!(folder_place(&app, mine).await, (bob_perso, None));
    assert_eq!(count_folders(&app).await, 5);
}
