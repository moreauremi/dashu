//! Dossiers (SPEC, « Dossiers ») : l'onglet Fichiers en arborescence, les
//! notes rangées depuis l'éditeur, la fenêtre « Ranger », la suppression d'un
//! dossier, et l'isolement entre deux comptes.

mod common;

use std::path::Path;

use axum::http::{Method, StatusCode};
use common::{ALICE, API_TOKEN, BOB, Client, TestApp};
use dashu::tools::{self, JobSpec, worker};
use serde_json::json;

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

async fn count_folders(app: &TestApp) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM folders")
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

async fn exists(app: &TestApp, table: &str, name: &str) -> bool {
    let column = if table == "notes" { "title" } else { "name" };
    sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table} WHERE {column} = ?"))
        .bind(name)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
        > 0
}

/// Crée un dossier dans un espace.
async fn create_in_space(client: &mut Client, space: i64, name: &str) {
    let created = client
        .post(
            "/folders",
            &[("name", name), ("space_id", &space.to_string())],
        )
        .await;
    assert_eq!(created.status, StatusCode::SEE_OTHER, "{}", created.body);
}

/// Crée un sous-dossier.
async fn create_in_folder(client: &mut Client, parent: i64, name: &str) {
    let created = client
        .post(
            "/folders",
            &[("name", name), ("parent_id", &parent.to_string())],
        )
        .await;
    assert_eq!(created.status, StatusCode::SEE_OTHER, "{}", created.body);
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

/// Range une note dans un dossier de son espace (sélecteur de l'éditeur).
async fn file_note(client: &mut Client, id: i64, folder: i64) {
    let set = client
        .post(
            &format!("/notes/{id}/folder"),
            &[("folder_id", &folder.to_string())],
        )
        .await;
    assert_eq!(set.status, StatusCode::OK, "{}", set.body);
}

#[tokio::test]
async fn la_racine_montre_les_espaces_et_temporaire() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;

    // Depuis Perso, l'onglet s'ouvre sur la racine : un dossier par espace,
    // et Temporaire.
    let root = alice.get("/files").await;
    assert_eq!(root.status, StatusCode::OK);
    assert!(root.body.contains("<h1>Fichiers</h1>"));
    for (slug, name) in [("perso", "Perso"), ("ecole", "École"), ("taff", "Taff")] {
        let row = common::between(
            &root.body,
            &format!("href=\"/e/{slug}/files?dossier=espace\""),
            "</li>",
        )
        .unwrap_or_else(|| panic!("{slug} absent de la racine"));
        assert!(row.contains(name), "{slug}");
        // Le dossier d'un espace ne se renomme ni ne se supprime ici.
        assert!(!row.contains("/folders/"), "{slug}");
    }
    assert!(root.body.contains("href=\"/files?dossier=temporaire\""));

    // Depuis École, le dossier d'École, avec le chemin pour remonter.
    alice.post("/space", &[("slug", "ecole")]).await;
    let page = alice.get("/files").await;
    assert!(page.body.contains("<h1>École</h1>"));
    assert!(page.body.contains("href=\"/files?dossier=racine\""));
    assert!(page.body.contains("Nouveau dossier"));

    // Renommer un espace renomme son dossier ; archivé, il quitte la racine.
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
    let root = alice.get("/files?dossier=racine").await;
    let row =
        common::between(&root.body, "href=\"/e/taff/files?dossier=espace\"", "</li>").unwrap();
    assert!(row.contains("Entreprise"));
    alice
        .post(&format!("/settings/spaces/{taff}/archive"), &[])
        .await;
    let root = alice.get("/files?dossier=racine").await;
    assert!(!root.body.contains("href=\"/e/taff/files?dossier=espace\""));
}

#[tokio::test]
async fn des_dossiers_sur_trois_niveaux_au_plus() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let (ecole, taff) = (
        app.space_id(ALICE, "ecole").await,
        app.space_id(ALICE, "taff").await,
    );

    // Un dossier dans École (le nom est nettoyé), puis retour à École.
    let created = alice
        .post(
            "/folders",
            &[("name", "  Math \t"), ("space_id", &ecole.to_string())],
        )
        .await;
    assert_eq!(created.location(), "/e/ecole/files?dossier=espace");
    let page = alice.follow(created).await;
    assert!(page.body.contains("Dossier « Math » créé"));
    let math = folder(&app, "Math").await;
    assert_eq!(folder_place(&app, math).await, (ecole, None));
    assert!(
        page.body
            .contains(&format!("href=\"/e/ecole/files?dossier={math}\""))
    );

    // Un sous-dossier dans Math : retour à Math.
    let created = alice
        .post(
            "/folders",
            &[("name", "Exercices"), ("parent_id", &math.to_string())],
        )
        .await;
    assert_eq!(created.location(), format!("/e/ecole/files?dossier={math}"));
    let exercices = folder(&app, "Exercices").await;
    assert_eq!(folder_place(&app, exercices).await, (ecole, Some(math)));

    // Pas de troisième niveau : refusé par le serveur, et pas proposé.
    let refused = alice
        .post(
            "/folders",
            &[("name", "Trop loin"), ("parent_id", &exercices.to_string())],
        )
        .await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    assert_eq!(count_folders(&app).await, 2);
    alice.post("/space", &[("slug", "ecole")]).await;
    let page = alice.get(&format!("/files?dossier={math}")).await;
    assert!(
        page.body
            .contains(&format!("name=\"parent_id\" value=\"{math}\""))
    );
    assert!(page.body.contains("Exercices"));
    let page = alice.get(&format!("/files?dossier={exercices}")).await;
    assert!(!page.body.contains("Nouveau dossier"));
    // Le chemin pour remonter : Fichiers › École › Math.
    let crumbs = common::between(&page.body, "<nav class=\"crumbs\"", "</nav>").unwrap();
    assert!(crumbs.contains("href=\"/files?dossier=racine\""));
    assert!(crumbs.contains("href=\"/e/ecole/files?dossier=espace\""));
    assert!(crumbs.contains(&format!("href=\"/e/ecole/files?dossier={math}\"")));
    assert!(page.body.contains("<h1>Exercices</h1>"));

    // Deux voisins ne portent pas le même nom, majuscules comprises.
    let twice = alice
        .post(
            "/folders",
            &[("name", "MATH"), ("space_id", &ecole.to_string())],
        )
        .await;
    let page = alice.follow(twice).await;
    assert!(page.body.contains("Un dossier « MATH » existe déjà ici."));
    create_in_space(&mut alice, ecole, "Économie").await;
    alice
        .post(
            "/folders",
            &[("name", "économie"), ("space_id", &ecole.to_string())],
        )
        .await;
    assert_eq!(count_folders(&app).await, 3);
    // Un nom vide est refusé.
    let empty = alice
        .post(
            "/folders",
            &[("name", "  "), ("space_id", &ecole.to_string())],
        )
        .await;
    assert!(
        alice
            .follow(empty)
            .await
            .body
            .contains("Donne un nom au dossier.")
    );
    assert_eq!(count_folders(&app).await, 3);

    // Le même nom ailleurs est permis : dans un autre espace, dans un dossier.
    create_in_space(&mut alice, taff, "Math").await;
    create_in_folder(&mut alice, math, "Math").await;
    assert_eq!(count_folders(&app).await, 5);

    // Renommer : nom libre accepté, nom d'un voisin refusé.
    let renamed = alice
        .post(&format!("/folders/{exercices}/rename"), &[("name", "TD")])
        .await;
    assert_eq!(renamed.location(), format!("/e/ecole/files?dossier={math}"));
    assert!(
        alice
            .follow(renamed)
            .await
            .body
            .contains("Dossier renommé en « TD »")
    );
    let clash = alice
        .post(&format!("/folders/{exercices}/rename"), &[("name", "math")])
        .await;
    assert!(
        alice
            .follow(clash)
            .await
            .body
            .contains("Un dossier « math » existe déjà ici.")
    );
    let name: String = sqlx::query_scalar("SELECT name FROM folders WHERE id = ?")
        .bind(exercices)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(name, "TD");
}

/// Le faux service de conversion traite la dernière tâche, puis dashu la
/// relève (comme dans tests/conversions.rs).
async fn run_last_job(app: &TestApp) {
    let job: i64 = sqlx::query_scalar("SELECT MAX(id) FROM jobs WHERE kind = 'convert'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    let dir = tools::job_dir(&app.work_dir, job);
    std::fs::write(dir.join("started"), "").unwrap();
    let fake = |spec: &JobSpec, dir: &Path, _: &dyn Fn(u8)| {
        std::fs::write(dir.join(&spec.output), vec![7u8; 900]).map_err(|e| e.to_string())
    };
    worker::process_job(&dir, &fake);
    tools::collect(&app.state).await.unwrap();
}

#[tokio::test]
async fn un_fichier_envoye_dans_un_dossier_y_arrive_avec_sa_copie() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    create_in_space(&mut alice, ecole, "Math").await;
    let math = folder(&app, "Math").await;

    // Envoyé depuis le dossier : il y arrive, dans l'espace du dossier.
    let sent = alice.upload_in_folder("td.pdf", b"%PDF td", math).await;
    assert_eq!(sent.status, StatusCode::CREATED, "{}", sent.body);
    assert_eq!(place_of(&app, "files", "td.pdf").await, (ecole, Some(math)));

    // Il se voit dans le dossier, pas à la racine de l'espace (qui compte
    // ce que contient le dossier).
    let page = alice.get(&format!("/e/ecole/files?dossier={math}")).await;
    assert!(page.body.contains("td.pdf"));
    assert!(page.body.contains(&format!("data-folder=\"{math}\"")));
    // Dans un dossier, l'envoi n'offre pas de choisir un autre espace.
    assert!(!page.body.contains("data-upload-space"));
    let root = alice.get("/e/ecole/files").await;
    assert!(!root.body.contains("td.pdf</span>"));
    let row = common::between(&root.body, &format!("dossier={math}\""), "</li>").unwrap();
    assert!(row.contains("1 fichier"));

    // La copie convertie arrive dans le dossier de l'original.
    alice.upload_in_folder("IMG_1.HEIC", b"heic", math).await;
    let photo: i64 = sqlx::query_scalar("SELECT id FROM files WHERE name = 'IMG_1.HEIC'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    alice
        .post(&format!("/files/{photo}/convert"), &[("to", "jpg")])
        .await;
    run_last_job(&app).await;
    assert_eq!(
        place_of(&app, "files", "IMG_1.jpg").await,
        (ecole, Some(math))
    );

    // L'aperçu d'un fichier rangé ramène à son dossier.
    let view = alice.get(&format!("/e/ecole/files/{photo}")).await;
    assert!(
        view.body
            .contains(&format!("href=\"/e/ecole/files?dossier={math}\""))
    );
    assert!(view.body.contains("École / Math"));

    // « Remplacer l'original » : la copie, rangée entre-temps à la racine,
    // prend le nom et le dossier de l'original.
    let copy: i64 = sqlx::query_scalar("SELECT id FROM files WHERE name = 'IMG_1.jpg'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    alice
        .post(
            &format!("/files/{copy}/move"),
            &[("space_id", &ecole.to_string())],
        )
        .await;
    assert_eq!(place_of(&app, "files", "IMG_1.jpg").await, (ecole, None));
    let job: i64 = sqlx::query_scalar("SELECT MAX(id) FROM jobs WHERE kind = 'convert'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    let replaced = alice.post(&format!("/files/jobs/{job}/replace"), &[]).await;
    assert_eq!(replaced.status, StatusCode::SEE_OTHER, "{}", replaced.body);
    assert!(!exists(&app, "files", "IMG_1.HEIC").await);
    assert_eq!(
        place_of(&app, "files", "IMG_1.jpg").await,
        (ecole, Some(math))
    );
    let view = alice.get(&format!("/e/ecole/files/{copy}")).await;
    assert!(
        view.body
            .contains(&format!("href=\"/e/ecole/files?dossier={math}\""))
    );
    assert!(view.body.contains("École / Math"));
}

#[tokio::test]
async fn temporaire_puis_range_dans_un_dossier() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let (ecole, taff) = (
        app.space_id(ALICE, "ecole").await,
        app.space_id(ALICE, "taff").await,
    );
    create_in_space(&mut alice, ecole, "Math").await;
    let math = folder(&app, "Math").await;

    // Un fichier déposé dans Outils : dans Temporaire, et nulle part ailleurs.
    let sent = alice.upload_to_tools("scan.pdf", b"%PDF scan").await;
    assert_eq!(sent.status, StatusCode::CREATED, "{}", sent.body);
    let scan: i64 = sqlx::query_scalar("SELECT id FROM files WHERE name = 'scan.pdf'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    let temp = alice.get("/files?dossier=temporaire").await;
    assert!(temp.body.contains("<h1>Temporaire</h1>"));
    let row = common::between(&temp.body, "scan.pdf</span>", "</li>").unwrap();
    assert!(row.contains("effacé dans"));
    assert!(
        !alice
            .get("/files?dossier=espace")
            .await
            .body
            .contains("scan.pdf")
    );
    let root = alice.get("/files").await;
    let row = common::between(&root.body, "href=\"/files?dossier=temporaire\"", "</li>").unwrap();
    assert!(row.contains("1 fichier · effacés 24 heures après leur dépôt"));

    // La fenêtre « Ranger » propose de le garder, dans un espace ou un dossier.
    let picker = alice
        .get_fragment(&format!(
            "/destinations?fichier={scan}&next=/files?dossier=temporaire"
        ))
        .await;
    assert_eq!(picker.status, StatusCode::OK);
    assert!(
        picker
            .body
            .contains(&format!("action=\"/files/{scan}/keep\""))
    );
    assert!(
        picker
            .body
            .contains(&format!("name=\"folder_id\" value=\"{math}\""))
    );
    assert!(!picker.body.contains("disabled"));

    // Un dossier qui n'est pas dans l'espace demandé : refusé.
    let refused = alice
        .post(
            &format!("/files/{scan}/keep"),
            &[
                ("space_id", &taff.to_string()),
                ("folder_id", &math.to_string()),
            ],
        )
        .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);

    let kept = alice
        .post(
            &format!("/files/{scan}/keep"),
            &[
                ("space_id", &ecole.to_string()),
                ("folder_id", &math.to_string()),
                ("next", "/files?dossier=temporaire"),
            ],
        )
        .await;
    assert_eq!(kept.location(), "/files?dossier=temporaire");
    let page = alice.follow(kept).await;
    assert!(page.body.contains("Fichier gardé dans École / Math"));
    assert!(page.body.contains("Rien en attente dans Temporaire"));
    assert_eq!(
        place_of(&app, "files", "scan.pdf").await,
        (ecole, Some(math))
    );
    let expires: Option<i64> = sqlx::query_scalar("SELECT expires_at FROM files WHERE id = ?")
        .bind(scan)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(expires, None);
}

/// École : Math (avec Exercices) et Français ; Taff : Projet.
async fn school_folders(app: &TestApp, alice: &mut Client) -> [i64; 4] {
    let (ecole, taff) = (
        app.space_id(ALICE, "ecole").await,
        app.space_id(ALICE, "taff").await,
    );
    create_in_space(alice, ecole, "Math").await;
    create_in_space(alice, ecole, "Français").await;
    let math = folder(app, "Math").await;
    create_in_folder(alice, math, "Exercices").await;
    create_in_space(alice, taff, "Projet").await;
    [
        math,
        folder(app, "Français").await,
        folder(app, "Exercices").await,
        folder(app, "Projet").await,
    ]
}

#[tokio::test]
async fn le_selecteur_de_l_editeur_range_la_note() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let [math, francais, exercices, projet] = school_folders(&app, &mut alice).await;
    alice.post("/space", &[("slug", "ecole")]).await;
    let id = note(&mut alice, "Cours VLAN").await;

    // Sans choix, la note est à la racine de son espace.
    let ecole = app.space_id(ALICE, "ecole").await;
    assert_eq!(place_of(&app, "notes", "Cours VLAN").await, (ecole, None));

    // Le sélecteur, à côté des tags : la racine et les dossiers du premier
    // niveau de l'espace de la note ; ni les sous-dossiers, ni un autre espace.
    let editor = alice.get(&format!("/notes/{id}")).await;
    let control = common::between(&editor.body, "id=\"note-folder\"", "Voir plus").unwrap();
    assert!(control.contains("aria-label=\"Dossier de la note : École\""));
    assert!(control.contains("École (racine)"));
    assert!(control.contains(&format!("value=\"{math}\"")));
    assert!(control.contains(&format!("value=\"{francais}\"")));
    assert!(!control.contains(&format!("value=\"{exercices}\"")));
    assert!(!control.contains(&format!("value=\"{projet}\"")));
    assert!(editor.body.contains("id=\"note-tags\""));

    // Un choix range la note et renvoie le sélecteur à jour.
    let set = alice
        .post(
            &format!("/notes/{id}/folder"),
            &[("folder_id", &math.to_string())],
        )
        .await;
    assert_eq!(set.status, StatusCode::OK);
    assert!(set.body.contains("<span>École / Math</span>"));
    assert_eq!(
        place_of(&app, "notes", "Cours VLAN").await,
        (ecole, Some(math))
    );

    // Un dossier d'un autre espace : refusé, la note ne bouge pas.
    let refused = alice
        .post(
            &format!("/notes/{id}/folder"),
            &[("folder_id", &projet.to_string())],
        )
        .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
    assert_eq!(
        place_of(&app, "notes", "Cours VLAN").await,
        (ecole, Some(math))
    );

    // Vide : retour à la racine.
    alice
        .post(&format!("/notes/{id}/folder"), &[("folder_id", "")])
        .await;
    assert_eq!(place_of(&app, "notes", "Cours VLAN").await, (ecole, None));
}

#[tokio::test]
async fn voir_plus_range_dans_un_autre_espace() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let [_, _, exercices, projet] = school_folders(&app, &mut alice).await;
    let (ecole, taff) = (
        app.space_id(ALICE, "ecole").await,
        app.space_id(ALICE, "taff").await,
    );
    alice.post("/space", &[("slug", "ecole")]).await;
    let id = note(&mut alice, "Compte rendu").await;

    // « Voir plus » : tous les espaces et tous les dossiers, sous-dossiers
    // compris ; l'endroit actuel de la note est grisé.
    let editor = alice.get(&format!("/notes/{id}")).await;
    assert!(
        editor
            .body
            .contains(&format!("hx-get=\"/destinations?note={id}\""))
    );
    let picker = alice
        .get_fragment(&format!("/destinations?note={id}"))
        .await;
    for name in [
        "Perso",
        "École",
        "Taff",
        "Math",
        "Français",
        "Exercices",
        "Projet",
    ] {
        assert!(
            picker
                .body
                .contains(&format!("<span class=\"dest-name\">{name}</span>")),
            "{name}"
        );
    }
    assert!(
        picker
            .body
            .contains(&format!("action=\"/notes/{id}/move\""))
    );
    assert!(
        picker
            .body
            .contains(&format!("name=\"next\" value=\"/notes/{id}\""))
    );
    assert_eq!(picker.body.matches(" disabled").count(), 1);
    let here = common::between(
        &picker.body,
        &format!("name=\"space_id\" value=\"{ecole}\""),
        "</li>",
    )
    .unwrap();
    assert!(here.contains("disabled") && here.contains("Ici"));
    assert!(picker.body.contains("dest-d2"));

    // Un dossier d'un autre espace : la note y part, et s'ouvre dans cet espace.
    let moved = alice
        .post(
            &format!("/notes/{id}/move"),
            &[
                ("space_id", &taff.to_string()),
                ("folder_id", &projet.to_string()),
                ("next", &format!("/notes/{id}")),
            ],
        )
        .await;
    assert_eq!(moved.location(), format!("/notes/{id}"));
    let opened = alice.follow(moved).await;
    assert_eq!(opened.location(), format!("/e/taff/notes/{id}"));
    let page = alice.follow(opened).await;
    assert!(page.body.contains("Note rangée dans Taff / Projet"));
    assert!(
        page.body
            .contains("aria-label=\"Dossier de la note : Taff / Projet\"")
    );
    assert_eq!(
        place_of(&app, "notes", "Compte rendu").await,
        (taff, Some(projet))
    );

    // Un sous-dossier, par la même fenêtre.
    alice
        .post(
            &format!("/notes/{id}/move"),
            &[
                ("space_id", &ecole.to_string()),
                ("folder_id", &exercices.to_string()),
            ],
        )
        .await;
    assert_eq!(
        place_of(&app, "notes", "Compte rendu").await,
        (ecole, Some(exercices))
    );
    let editor = alice.get(&format!("/e/ecole/notes/{id}")).await;
    assert!(
        editor
            .body
            .contains("<span>École / Math / Exercices</span>")
    );

    // Un dossier qui n'est pas dans l'espace demandé : refusé.
    let refused = alice
        .post(
            &format!("/notes/{id}/move"),
            &[
                ("space_id", &taff.to_string()),
                ("folder_id", &exercices.to_string()),
            ],
        )
        .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
    // Sans dossier : la racine de l'espace (comme /move de la barre).
    alice
        .post(
            &format!("/notes/{id}/move"),
            &[("space_id", &taff.to_string())],
        )
        .await;
    assert_eq!(place_of(&app, "notes", "Compte rendu").await, (taff, None));
}

#[tokio::test]
async fn une_note_rangee_se_voit_dans_fichiers_et_dans_notes() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let [math, francais, exercices, _] = school_folders(&app, &mut alice).await;
    alice.post("/space", &[("slug", "ecole")]).await;
    let vlan = note(&mut alice, "Cours VLAN").await;
    file_note(&mut alice, vlan, math).await;
    let exo = note(&mut alice, "Exo routage").await;
    file_note(&mut alice, exo, exercices).await;
    let dictee = note(&mut alice, "Dictée").await;
    file_note(&mut alice, dictee, francais).await;
    note(&mut alice, "Idées en vrac").await;

    // Dans Fichiers : la note est dans son dossier, à côté des fichiers.
    let page = alice.get(&format!("/files?dossier={math}")).await;
    assert!(page.body.contains(&format!("href=\"/notes/{vlan}\"")));
    assert!(page.body.contains("Cours VLAN"));
    assert!(
        page.body
            .contains(&format!("hx-get=\"/destinations?note={vlan}"))
    );
    assert!(!page.body.contains("Idées en vrac"));
    assert!(!page.body.contains("Exo routage"));
    let root = alice.get("/files").await;
    assert!(root.body.contains("Idées en vrac"));
    assert!(!root.body.contains("Cours VLAN"));
    // Math compte sa note, son sous-dossier et la note de celui-ci.
    let row = common::between(&root.body, &format!("dossier={math}\""), "</li>").unwrap();
    assert!(row.contains("1 dossier et 2 notes"));

    // Dans Notes : sans dossier d'abord, puis une section par dossier du
    // premier niveau, par nom ; la note d'un sous-dossier dans celle de son
    // dossier, avec le nom du sous-dossier.
    let list = alice.get("/notes").await.body;
    let loose = list.find("<span>Sans dossier</span>").unwrap();
    let french = list.find("aria-label=\"Dossier Français\"").unwrap();
    let maths = list.find("aria-label=\"Dossier Math\"").unwrap();
    assert!(loose < french && french < maths);
    assert!(list[loose..french].contains("Idées en vrac"));
    assert!(list[french..maths].contains("Dictée"));
    let section = &list[maths..];
    assert!(section.contains("Cours VLAN") && section.contains("Exo routage"));
    let row = common::between(section, "Exo routage", "</li>").unwrap();
    assert!(row.contains("class=\"note-sub\""));
    assert!(row.contains("Exercices"));
    // La recherche garde les sections.
    let found = alice.get("/notes?q=vlan").await.body;
    assert!(found.contains("aria-label=\"Dossier Math\"") && found.contains("Cours"));
    assert!(!found.contains("Dictée"));

    // Perso montre tout : la section d'un dossier d'École porte son nom.
    alice.post("/space", &[("slug", "perso")]).await;
    let list = alice.get("/notes").await.body;
    let section = common::between(&list, "aria-label=\"Dossier Math\"", "</h2>").unwrap();
    assert!(section.contains("École</span>"));

    // Un espace sans dossier : la liste d'un seul tenant, sans titre.
    alice.post("/space", &[("slug", "taff")]).await;
    note(&mut alice, "Réunion").await;
    let list = alice.get("/notes").await.body;
    assert!(list.contains("Réunion"));
    assert!(!list.contains("note-group-title"));
}

#[tokio::test]
async fn supprimer_un_dossier_vide_ou_non() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    alice.post("/space", &[("slug", "ecole")]).await;
    // Math : une note, un fichier, et Exercices (une note, un fichier). Un
    // autre « Exercices » est déjà à la racine d'École.
    create_in_space(&mut alice, ecole, "Vide").await;
    create_in_space(&mut alice, ecole, "Math").await;
    let math = folder(&app, "Math").await;
    create_in_folder(&mut alice, math, "Exercices").await;
    let sub: i64 = sqlx::query_scalar("SELECT id FROM folders WHERE parent_id = ?")
        .bind(math)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    create_in_space(&mut alice, ecole, "Exercices").await;
    let cours = note(&mut alice, "Cours").await;
    file_note(&mut alice, cours, math).await;
    let exo = note(&mut alice, "Exo").await;
    file_note(&mut alice, exo, sub).await;
    alice.upload_in_folder("td.pdf", b"%PDF td", math).await;
    alice.upload_in_folder("exo.pdf", b"%PDF exo", sub).await;

    // Un dossier vide : une simple confirmation.
    let vide = folder(&app, "Vide").await;
    let page = alice.get("/files").await;
    let row = common::between(&page.body, &format!("dossier={vide}\""), "</li>").unwrap();
    assert!(row.contains("hx-confirm=\"Supprimer le dossier « Vide » ?\""));
    let deleted = alice.post(&format!("/folders/{vide}/delete"), &[]).await;
    assert_eq!(deleted.location(), "/e/ecole/files?dossier=espace");
    assert!(
        alice
            .follow(deleted)
            .await
            .body
            .contains("Dossier « Vide » supprimé")
    );

    // Un dossier non vide : l'avertissement dit ce qu'il contient et propose
    // les deux choix.
    let page = alice.get("/files").await;
    let warning =
        common::between(&page.body, &format!("id=\"pd-delete-{math}\""), "Annuler").unwrap();
    assert!(warning.contains("Il contient 1 dossier, 2 notes et 2 fichiers."));
    assert!(warning.contains("Transférer dans « École »"));
    assert!(warning.contains("value=\"supprimer\""));

    // Sans choix, le serveur refuse : rien ne bouge.
    let refused = alice.post(&format!("/folders/{math}/delete"), &[]).await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    assert_eq!(count_folders(&app).await, 3);

    // Transférer : le contenu monte à la racine d'École, Exercices aussi,
    // renommé car le nom est pris.
    let stored = app.stored_files().len();
    let moved = alice
        .post(
            &format!("/folders/{math}/delete"),
            &[("contenu", "remonter")],
        )
        .await;
    let page = alice.follow(moved).await;
    assert!(
        page.body
            .contains("Dossier « Math » supprimé : son contenu est dans « École »")
    );
    assert_eq!(place_of(&app, "notes", "Cours").await, (ecole, None));
    assert_eq!(place_of(&app, "files", "td.pdf").await, (ecole, None));
    assert_eq!(place_of(&app, "notes", "Exo").await, (ecole, Some(sub)));
    assert_eq!(place_of(&app, "files", "exo.pdf").await, (ecole, Some(sub)));
    assert_eq!(folder_place(&app, sub).await, (ecole, None));
    let name: String = sqlx::query_scalar("SELECT name FROM folders WHERE id = ?")
        .bind(sub)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(name, "Exercices (2)");
    assert_eq!(app.stored_files().len(), stored);

    // Un sous-dossier transféré : son contenu va dans son dossier parent.
    create_in_folder(&mut alice, sub, "Chapitre 1").await;
    let chapter = folder(&app, "Chapitre 1").await;
    let lecon = note(&mut alice, "Leçon").await;
    file_note_any(&app, lecon, chapter).await;
    alice
        .post(
            &format!("/folders/{chapter}/delete"),
            &[("contenu", "remonter")],
        )
        .await;
    assert_eq!(place_of(&app, "notes", "Leçon").await, (ecole, Some(sub)));

    // Tout supprimer : les notes, les fichiers (disque compris) et les
    // sous-dossiers partent avec lui.
    create_in_folder(&mut alice, sub, "Annexe").await;
    let erased = alice
        .post(
            &format!("/folders/{sub}/delete"),
            &[("contenu", "supprimer")],
        )
        .await;
    assert!(
        alice
            .follow(erased)
            .await
            .body
            .contains("Dossier « Exercices (2) » supprimé, avec son contenu")
    );
    assert!(!exists(&app, "notes", "Exo").await);
    assert!(!exists(&app, "notes", "Leçon").await);
    assert!(!exists(&app, "files", "exo.pdf").await);
    assert!(!exists(&app, "folders", "Annexe").await);
    assert_eq!(app.stored_files().len(), stored - 1);
    // Le reste d'École est là.
    assert!(exists(&app, "notes", "Cours").await && exists(&app, "files", "td.pdf").await);
}

/// Range une note dans un sous-dossier, comme « Voir plus » (le sélecteur de
/// l'éditeur ne propose que le premier niveau, mais accepte tout dossier de
/// l'espace).
async fn file_note_any(app: &TestApp, id: i64, folder: i64) {
    sqlx::query("UPDATE notes SET folder_id = ? WHERE id = ?")
        .bind(folder)
        .bind(id)
        .execute(&app.state.db)
        .await
        .unwrap();
}

#[tokio::test]
async fn un_autre_compte_ne_touche_pas_aux_dossiers() {
    let app = TestApp::new().await;
    let mut bob = app.logged_in(BOB).await;
    let bob_note = note(&mut bob, "Note de Bob").await;
    let bob_perso = app.space_id(BOB, "perso").await;
    let mut alice = app.logged_in(ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    create_in_space(&mut alice, ecole, "Math").await;
    let math = folder(&app, "Math").await;
    alice.post("/space", &[("slug", "ecole")]).await;
    let alice_note = note(&mut alice, "Cours").await;
    file_note(&mut alice, alice_note, math).await;
    alice.upload_in_folder("td.pdf", b"%PDF td", math).await;
    let td: i64 = sqlx::query_scalar("SELECT id FROM files WHERE name = 'td.pdf'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();

    // Bob ne voit, ne remplit, ne renomme ni ne supprime le dossier d'Alice.
    assert_eq!(
        bob.get(&format!("/files?dossier={math}")).await.status,
        StatusCode::NOT_FOUND
    );
    let math_id = math.to_string();
    for (path, form) in [
        (
            "/folders".to_string(),
            vec![("name", "Intrus"), ("parent_id", math_id.as_str())],
        ),
        (format!("/folders/{math}/rename"), vec![("name", "Intrus")]),
        (
            format!("/folders/{math}/delete"),
            vec![("contenu", "supprimer")],
        ),
        (
            format!("/notes/{bob_note}/folder"),
            vec![("folder_id", math_id.as_str())],
        ),
    ] {
        assert_eq!(
            bob.post(&path, &form).await.status,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    let bob_space = bob_perso.to_string();
    let moved = bob
        .post(
            &format!("/notes/{bob_note}/move"),
            &[
                ("space_id", bob_space.as_str()),
                ("folder_id", math_id.as_str()),
            ],
        )
        .await;
    assert_eq!(moved.status, StatusCode::NOT_FOUND);
    assert_eq!(
        bob.upload_in_folder("intrus.txt", b"x", math).await.status,
        StatusCode::NOT_FOUND
    );
    for path in [
        format!("/destinations?note={alice_note}"),
        format!("/destinations?fichier={td}"),
    ] {
        assert_eq!(
            bob.get_fragment(&path).await.status,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    // Sa propre fenêtre « Ranger » ne montre que ses espaces et ses dossiers.
    let picker = bob
        .get_fragment(&format!("/destinations?note={bob_note}"))
        .await;
    assert!(!picker.body.contains("Math") && !picker.body.contains(&format!("value=\"{ecole}\"")));
    // Le dossier d'Alice est intact, avec son contenu.
    assert_eq!(count_folders(&app).await, 1);
    assert_eq!(place_of(&app, "notes", "Cours").await, (ecole, Some(math)));
    assert_eq!(
        place_of(&app, "notes", "Note de Bob").await,
        (bob_perso, None)
    );
    assert!(exists(&app, "files", "td.pdf").await && !exists(&app, "files", "intrus.txt").await);
}

#[tokio::test]
async fn supprimer_un_espace_emmene_ses_dossiers() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let (ecole, taff) = (
        app.space_id(ALICE, "ecole").await,
        app.space_id(ALICE, "taff").await,
    );
    create_in_space(&mut alice, ecole, "Math").await;
    let ecole_math = folder(&app, "Math").await;
    create_in_space(&mut alice, taff, "Clients").await;
    create_in_space(&mut alice, taff, "MATH").await;
    let taff_math = folder(&app, "MATH").await;
    create_in_folder(&mut alice, taff_math, "Statistiques").await;
    let stats = folder(&app, "Statistiques").await;
    alice.post("/space", &[("slug", "taff")]).await;
    let rapport = note(&mut alice, "Rapport").await;
    file_note_any(&app, rapport, stats).await;

    let deleted = alice
        .post(
            &format!("/settings/spaces/{taff}/delete"),
            &[("content", "move"), ("target", &ecole.to_string())],
        )
        .await;
    assert_eq!(deleted.status, StatusCode::SEE_OTHER);
    // Les dossiers suivent ; un nom déjà pris dans École change, les
    // sous-dossiers gardent le leur et leur contenu.
    assert_eq!(
        folder_place(&app, folder(&app, "Clients").await).await,
        (ecole, None)
    );
    assert_eq!(folder_place(&app, taff_math).await, (ecole, None));
    let name: String = sqlx::query_scalar("SELECT name FROM folders WHERE id = ?")
        .bind(taff_math)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(name, "MATH (2)");
    assert_eq!(folder_place(&app, ecole_math).await, (ecole, None));
    assert_eq!(folder_place(&app, stats).await, (ecole, Some(taff_math)));
    assert_eq!(
        place_of(&app, "notes", "Rapport").await,
        (ecole, Some(stats))
    );
}

#[tokio::test]
async fn l_api_range_a_la_racine_du_nouvel_espace() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let (ecole, taff) = (
        app.space_id(ALICE, "ecole").await,
        app.space_id(ALICE, "taff").await,
    );
    create_in_space(&mut alice, ecole, "Math").await;
    let math = folder(&app, "Math").await;
    alice.post("/space", &[("slug", "ecole")]).await;
    let id = note(&mut alice, "Cours").await;
    file_note(&mut alice, id, math).await;

    // Même espace : la note garde son dossier.
    let path = format!("/api/v1/notes/{id}");
    let (status, _) = app
        .api(
            Method::PATCH,
            &path,
            Some(json!({"space": "ecole"})),
            Some(API_TOKEN),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(place_of(&app, "notes", "Cours").await, (ecole, Some(math)));
    // Autre espace : à sa racine (un dossier appartient à un espace).
    let (status, _) = app
        .api(
            Method::PATCH,
            &path,
            Some(json!({"space": "taff"})),
            Some(API_TOKEN),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(place_of(&app, "notes", "Cours").await, (taff, None));
}

#[tokio::test]
async fn deplacer_un_dossier_entier() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let [math, francais, exercices, projet] = school_folders(&app, &mut alice).await;
    let (ecole, taff) = (
        app.space_id(ALICE, "ecole").await,
        app.space_id(ALICE, "taff").await,
    );
    // Math : une note et un fichier ; Exercices (dans Math) : une note.
    alice.post("/space", &[("slug", "ecole")]).await;
    let cours = note(&mut alice, "Cours").await;
    file_note(&mut alice, cours, math).await;
    let exo = note(&mut alice, "Exo").await;
    file_note_any(&app, exo, exercices).await;
    alice.upload_in_folder("td.pdf", b"%PDF td", math).await;

    // La fenêtre d'un dossier qui a des sous-dossiers : les espaces seulement.
    let picker = alice
        .get_fragment(&format!("/destinations?dossier={math}"))
        .await;
    assert!(
        picker
            .body
            .contains(&format!("action=\"/folders/{math}/move\""))
    );
    assert!(!picker.body.contains("dest-d1") && !picker.body.contains("dest-d2"));
    // Sans sous-dossier : aussi les dossiers du premier niveau, sauf lui-même.
    let picker = alice
        .get_fragment(&format!("/destinations?dossier={francais}"))
        .await;
    assert!(
        picker
            .body
            .contains(&format!("name=\"folder_id\" value=\"{projet}\""))
    );
    assert!(
        picker
            .body
            .contains(&format!("name=\"folder_id\" value=\"{math}\""))
    );
    assert!(
        !picker
            .body
            .contains(&format!("name=\"folder_id\" value=\"{francais}\""))
    );
    assert!(!picker.body.contains("dest-d2"));
    // La liste propose de le déplacer.
    let page = alice.get("/files").await;
    assert!(
        page.body
            .contains(&format!("hx-get=\"/destinations?dossier={math}"))
    );

    // Math part dans Taff avec tout son contenu.
    let moved = alice
        .post(
            &format!("/folders/{math}/move"),
            &[
                ("space_id", &taff.to_string()),
                ("folder_id", ""),
                ("next", "/e/ecole/files?dossier=espace"),
            ],
        )
        .await;
    assert_eq!(moved.location(), "/e/ecole/files?dossier=espace");
    let page = alice.follow(moved).await;
    assert!(page.body.contains("Dossier « Math » déplacé dans « Taff »"));
    assert!(!page.body.contains(&format!("dossier={math}\"")));
    assert_eq!(folder_place(&app, math).await, (taff, None));
    assert_eq!(folder_place(&app, exercices).await, (taff, Some(math)));
    assert_eq!(place_of(&app, "notes", "Cours").await, (taff, Some(math)));
    assert_eq!(
        place_of(&app, "notes", "Exo").await,
        (taff, Some(exercices))
    );
    assert_eq!(place_of(&app, "files", "td.pdf").await, (taff, Some(math)));

    // Un dossier sans sous-dossier devient le sous-dossier d'un autre.
    alice
        .post(
            &format!("/folders/{francais}/move"),
            &[
                ("space_id", &taff.to_string()),
                ("folder_id", &projet.to_string()),
            ],
        )
        .await;
    assert_eq!(folder_place(&app, francais).await, (taff, Some(projet)));
    // Et un sous-dossier remonte à la racine d'un espace.
    alice
        .post(
            &format!("/folders/{exercices}/move"),
            &[("space_id", &ecole.to_string()), ("folder_id", "")],
        )
        .await;
    assert_eq!(folder_place(&app, exercices).await, (ecole, None));
    assert_eq!(
        place_of(&app, "notes", "Exo").await,
        (ecole, Some(exercices))
    );

    // Refus dits, rien ne bouge : un dossier qui a des sous-dossiers dans un
    // dossier, un dossier dans un sous-dossier, dans lui-même, à côté d'un
    // homonyme.
    create_in_folder(&mut alice, projet, "Annexes").await;
    let refusals = [
        (projet, taff, Some(math), "contient des sous-dossiers"),
        (
            math,
            taff,
            Some(francais),
            "Pas de dossier dans un sous-dossier",
        ),
        (
            math,
            taff,
            Some(math),
            "Un dossier ne va pas dans lui-même.",
        ),
    ];
    for (folder_id, space, parent, message) in refusals {
        let parent = parent.map(|p| p.to_string()).unwrap_or_default();
        let refused = alice
            .post(
                &format!("/folders/{folder_id}/move"),
                &[("space_id", &space.to_string()), ("folder_id", &parent)],
            )
            .await;
        assert_eq!(refused.status, StatusCode::SEE_OTHER, "{message}");
        assert!(
            alice.follow(refused).await.body.contains(message),
            "{message}"
        );
    }
    create_in_space(&mut alice, ecole, "Projet").await;
    let refused = alice
        .post(
            &format!("/folders/{projet}/move"),
            &[("space_id", &ecole.to_string()), ("folder_id", "")],
        )
        .await;
    assert!(
        alice
            .follow(refused)
            .await
            .body
            .contains("Un dossier « Projet » existe déjà dans « École ».")
    );
    assert_eq!(folder_place(&app, projet).await, (taff, None));
    assert_eq!(folder_place(&app, math).await, (taff, None));

    // Un autre compte ne déplace ni ce dossier ni le sien chez Alice.
    let mut bob = app.logged_in(BOB).await;
    let bob_perso = app.space_id(BOB, "perso").await;
    let refused = bob
        .post(
            &format!("/folders/{math}/move"),
            &[("space_id", &bob_perso.to_string()), ("folder_id", "")],
        )
        .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
    create_in_space(&mut bob, bob_perso, "Perso de Bob").await;
    let bob_folder = folder(&app, "Perso de Bob").await;
    for (space, parent) in [(taff, String::new()), (bob_perso, math.to_string())] {
        let refused = bob
            .post(
                &format!("/folders/{bob_folder}/move"),
                &[("space_id", &space.to_string()), ("folder_id", &parent)],
            )
            .await;
        assert_eq!(refused.status, StatusCode::NOT_FOUND);
    }
    assert_eq!(
        bob.get_fragment(&format!("/destinations?dossier={math}"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(folder_place(&app, bob_folder).await, (bob_perso, None));
}

#[tokio::test]
async fn creer_une_note_depuis_fichiers() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let [_, _, exercices, projet] = school_folders(&app, &mut alice).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    alice.post("/space", &[("slug", "ecole")]).await;

    // Les boutons de la page Fichiers (la barre latérale a aussi le sien).
    async fn actions(client: &mut Client, path: &str) -> String {
        let page = client.get(path).await.body;
        common::between(&page, "<div class=\"files-actions\">", "</label>")
            .unwrap_or_default()
            .to_string()
    }

    // Le bouton, dans le dossier d'un espace et dans un dossier.
    let here = actions(&mut alice, "/files").await;
    assert!(here.contains("<form method=\"post\" action=\"/notes\">"));
    assert!(here.contains("Nouvelle note"));
    let here = actions(&mut alice, &format!("/files?dossier={exercices}")).await;
    assert!(here.contains(&format!("action=\"/notes?dossier={exercices}\"")));
    // Pas à la racine de l'onglet ni dans Temporaire.
    assert!(
        !actions(&mut alice, "/files?dossier=racine")
            .await
            .contains("Nouvelle note")
    );
    let temp = alice.get("/files?dossier=temporaire").await.body;
    let temp = common::between(&temp, "<div class=\"files-actions\">", "</div>").unwrap();
    assert!(!temp.contains("Nouvelle note"));

    // La note naît dans le dossier ouvert, puis l'éditeur s'ouvre.
    let created = alice
        .post(&format!("/notes?dossier={exercices}"), &[])
        .await;
    let id = common::note_id(&created);
    let editor = alice.get(&format!("/notes/{id}")).await;
    assert!(
        editor
            .body
            .contains("<span>École / Math / Exercices</span>")
    );
    let folder_of: Option<i64> = sqlx::query_scalar("SELECT folder_id FROM notes WHERE id = ?")
        .bind(id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(folder_of, Some(exercices));

    // Encore vierge, elle est réutilisée par la création suivante : à la
    // racine, elle quitte son dossier.
    let again = alice.post("/notes", &[]).await;
    assert_eq!(common::note_id(&again), id);
    let place: (i64, Option<i64>) =
        sqlx::query_as("SELECT space_id, folder_id FROM notes WHERE id = ?")
            .bind(id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(place, (ecole, None));

    // Un dossier d'un autre espace : refusé, rien n'est créé.
    let count = |app: &TestApp| {
        let db = app.state.db.clone();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM notes")
                .fetch_one(&db)
                .await
                .unwrap()
        }
    };
    let before = count(&app).await;
    let refused = alice.post(&format!("/notes?dossier={projet}"), &[]).await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
    assert_eq!(count(&app).await, before);
}

#[tokio::test]
async fn glisser_deposer_dans_l_onglet_fichiers() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let [math, francais, _, _] = school_folders(&app, &mut alice).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    alice.post("/space", &[("slug", "ecole")]).await;
    let id = note(&mut alice, "Cours").await;
    alice.upload("td.pdf", b"%PDF td").await;
    let td: i64 = sqlx::query_scalar("SELECT id FROM files WHERE name = 'td.pdf'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    alice.upload_to_tools("scan.pdf", b"%PDF scan").await;

    // Ce qui se glisse porte l'adresse de son rangement ; les dossiers et le
    // fil d'Ariane disent où ils rangent.
    let page = alice.get("/files").await.body;
    assert!(page.contains(&format!(
        "draggable=\"true\" data-drag-move=\"/notes/{id}/move\""
    )));
    assert!(page.contains(&format!(
        "draggable=\"true\" data-drag-move=\"/files/{td}/move\""
    )));
    assert!(page.contains(&format!(
        "data-drop-space=\"{ecole}\" data-drop-folder=\"{math}\" draggable=\"true\" data-drag-move=\"/folders/{math}/move\" data-drag-folder=\"{math}\""
    )));
    let page = alice.get(&format!("/files?dossier={math}")).await.body;
    let crumbs = common::between(&page, "<nav class=\"crumbs\"", "</nav>").unwrap();
    assert!(crumbs.contains(&format!(
        "data-drop-space=\"{ecole}\" data-drop-folder=\"\""
    )));
    // À la racine, le dossier d'un espace reçoit (sa racine), Temporaire non ;
    // rien n'y est glissable.
    let root = alice.get("/files?dossier=racine").await.body;
    assert!(root.contains(&format!(
        "data-drop-space=\"{ecole}\" data-drop-folder=\"\""
    )));
    let temp_row = common::between(&root, "<li class=\"file-row folder-row\">", "</li>").unwrap();
    assert!(temp_row.contains("Temporaire"));
    assert!(!root.contains("data-drag-move"));
    // Un fichier de Temporaire se garde, il ne se glisse pas.
    let temp = alice.get("/files?dossier=temporaire").await.body;
    assert!(temp.contains("scan.pdf") && !temp.contains("data-drag-move"));

    // Lâcher, c'est le formulaire de la fenêtre « Ranger » (files.js) : la
    // note et le fichier vont dans Français, Français dans Math.
    let here = "/e/ecole/files?dossier=espace";
    for url in [format!("/notes/{id}/move"), format!("/files/{td}/move")] {
        let moved = alice
            .post(
                &url,
                &[
                    ("space_id", &ecole.to_string()),
                    ("folder_id", &francais.to_string()),
                    ("next", here),
                ],
            )
            .await;
        assert_eq!(moved.location(), here, "{url}");
    }
    assert_eq!(
        place_of(&app, "notes", "Cours").await,
        (ecole, Some(francais))
    );
    assert_eq!(
        place_of(&app, "files", "td.pdf").await,
        (ecole, Some(francais))
    );
    alice
        .post(
            &format!("/folders/{francais}/move"),
            &[
                ("space_id", &ecole.to_string()),
                ("folder_id", &math.to_string()),
                ("next", here),
            ],
        )
        .await;
    assert_eq!(folder_place(&app, francais).await, (ecole, Some(math)));
    // Des fichiers de l'ordinateur lâchés sur un dossier y arrivent.
    let sent = alice.upload_in_folder("photo.png", b"png", francais).await;
    assert_eq!(sent.status, StatusCode::CREATED);
    assert_eq!(
        place_of(&app, "files", "photo.png").await,
        (ecole, Some(francais))
    );
}
