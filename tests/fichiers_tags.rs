//! Tags et recherche dans les fichiers (SPEC v2, « Page Fichiers ») : les
//! mêmes tags que les notes, posés depuis l'aperçu d'un fichier ; recherche
//! sur le nom et filtre par tag dans l'endroit montré de l'onglet Fichiers.

mod common;

use axum::http::StatusCode;
use common::{ALICE, BOB, Client, TestApp};

async fn file_id(app: &TestApp, name: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM files WHERE name = ?")
        .bind(name)
        .fetch_one(&app.state.db)
        .await
        .unwrap_or_else(|_| panic!("fichier « {name} » absent"))
}

async fn folder(app: &TestApp, name: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM folders WHERE name = ?")
        .bind(name)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

async fn tag_exists(app: &TestApp, name: &str) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tags WHERE name = ?")
        .bind(name)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
        > 0
}

async fn tag_id(app: &TestApp, name: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM tags WHERE name = ?")
        .bind(name)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

/// École : Réseaux (avec TP) ; cours-vlan.pdf dans Réseaux, tp-ospf.pdf dans
/// TP, Planning-École.pdf à la racine d'École ; photo-vacances.png dans Perso.
async fn setup(app: &TestApp, alice: &mut Client) -> (i64, i64, i64) {
    let ecole = app.space_id(ALICE, "ecole").await;
    alice
        .post(
            "/folders",
            &[("name", "Réseaux"), ("space_id", &ecole.to_string())],
        )
        .await;
    let reseaux = folder(app, "Réseaux").await;
    alice
        .post(
            "/folders",
            &[("name", "TP"), ("parent_id", &reseaux.to_string())],
        )
        .await;
    let tp = folder(app, "TP").await;
    alice
        .upload_in_folder("cours-vlan.pdf", b"%PDF vlan", reseaux)
        .await;
    alice
        .upload_in_folder("tp-ospf.pdf", b"%PDF ospf", tp)
        .await;
    let start = alice.start_upload_in("Planning-École.pdf", 4, ecole).await;
    alice.put_chunk(&start.body, 0, b"%PDF", true).await;
    alice.upload("photo-vacances.png", b"png").await;
    (ecole, reseaux, tp)
}

#[tokio::test]
async fn des_tags_sur_un_fichier() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.upload("cours.pdf", b"%PDF").await;
    let id = file_id(&app, "cours.pdf").await;

    // Posé depuis l'aperçu : « #Réseau » devient « réseau », comme pour une note.
    let view = alice.get(&format!("/files/{id}")).await;
    assert!(view.body.contains(&format!("hx-post=\"/files/{id}/tags\"")));
    let added = alice
        .post(&format!("/files/{id}/tags"), &[("name", "  #Réseau ")])
        .await;
    assert_eq!(added.status, StatusCode::OK);
    assert!(added.body.contains("#réseau"));
    assert!(
        alice
            .get(&format!("/files/{id}"))
            .await
            .body
            .contains("#réseau")
    );
    // Il se voit dans la liste, avec le fichier.
    let list = alice.get("/files?dossier=espace").await;
    let row = common::between(&list.body, "cours.pdf</span>", "</li>").unwrap();
    assert!(row.contains("<span class=\"tag\">#réseau</span>"));

    // Un tag partagé avec une note : le retirer du fichier ne l'efface pas
    // pour la note, et inversement.
    let created = alice.post("/notes", &[]).await;
    let note = common::note_id(&created);
    alice
        .post(&format!("/notes/{note}/tags"), &[("name", "réseau")])
        .await;
    let tag = tag_id(&app, "réseau").await;
    alice.delete(&format!("/notes/{note}/tags/{tag}")).await;
    assert!(
        tag_exists(&app, "réseau").await,
        "le tag du fichier a disparu"
    );
    assert!(
        alice
            .get(&format!("/files/{id}"))
            .await
            .body
            .contains("#réseau")
    );
    alice
        .post(&format!("/notes/{note}/tags"), &[("name", "réseau")])
        .await;
    let removed = alice.delete(&format!("/files/{id}/tags/{tag}")).await;
    assert_eq!(removed.status, StatusCode::OK);
    assert!(!removed.body.contains("#réseau</li>"));
    assert!(
        tag_exists(&app, "réseau").await,
        "le tag de la note a disparu"
    );
    // Plus porté par personne : il disparaît.
    alice.delete(&format!("/notes/{note}/tags/{tag}")).await;
    assert!(!tag_exists(&app, "réseau").await);

    // Un nom vide ne crée rien.
    alice
        .post(&format!("/files/{id}/tags"), &[("name", " # ")])
        .await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM file_tags")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn rechercher_par_le_nom_dans_l_endroit_montre() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let (_, reseaux, tp) = setup(&app, &mut alice).await;
    alice.post("/space", &[("slug", "ecole")]).await;

    // Dans École : tout l'espace, dossiers compris ; chaque résultat dit où
    // il est rangé.
    let found = alice.get("/files?dossier=espace&q=vlan").await.body;
    assert!(found.contains("1 fichier trouvé dans École"));
    let row = common::between(&found, "cours-vlan.pdf</span>", "</li>").unwrap();
    assert!(row.contains("École / Réseaux"));
    assert!(!found.contains("Planning-"));
    // Pendant une recherche, ni les dossiers ni les notes de l'endroit.
    assert!(!found.contains("class=\"file-row folder-row\""));
    // Accents ignorés, début de mot suffisant.
    let found = alice.get("/files?dossier=espace&q=ecole").await.body;
    assert!(found.contains("Planning-École.pdf"));
    let found = alice.get("/files?dossier=espace&q=osp").await.body;
    assert!(found.contains("tp-ospf.pdf"));
    assert!(found.contains("École / Réseaux / TP"));

    // Dans un dossier : lui et ses sous-dossiers seulement.
    let found = alice
        .get(&format!("/files?dossier={reseaux}&q=pdf"))
        .await
        .body;
    assert!(found.contains("cours-vlan.pdf") && found.contains("tp-ospf.pdf"));
    assert!(!found.contains("Planning-"));
    let found = alice.get(&format!("/files?dossier={tp}&q=vlan")).await.body;
    assert!(found.contains("Aucun fichier ne correspond"));

    // Un autre espace n'est pas cherché depuis École ; la racine cherche partout.
    let found = alice.get("/files?dossier=espace&q=vacances").await.body;
    assert!(found.contains("Aucun fichier ne correspond"));
    let found = alice.get("/files?dossier=racine&q=vacances").await.body;
    assert!(found.contains("photo-vacances.png") && found.contains("Perso"));

    // La barre de recherche porte l'endroit ; pas de recherche dans Temporaire.
    let page = alice.get(&format!("/files?dossier={reseaux}")).await.body;
    assert!(page.contains(&format!("name=\"dossier\" value=\"{reseaux}\"")));
    assert!(page.contains("placeholder=\"Rechercher dans Réseaux\""));
    assert!(
        !alice
            .get("/files?dossier=temporaire")
            .await
            .body
            .contains("role=\"search\"")
    );

    // Un fichier renommé (ici, en base) se trouve sous son nouveau nom.
    sqlx::query("UPDATE files SET name = 'routage-statique.pdf' WHERE name = 'tp-ospf.pdf'")
        .execute(&app.state.db)
        .await
        .unwrap();
    let found = alice.get("/files?dossier=espace&q=routage").await.body;
    assert!(found.contains("routage-statique.pdf"));
    assert!(
        alice
            .get("/files?dossier=espace&q=ospf")
            .await
            .body
            .contains("Aucun fichier ne correspond")
    );
}

#[tokio::test]
async fn filtrer_par_tag() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    setup(&app, &mut alice).await;
    alice.post("/space", &[("slug", "ecole")]).await;
    for name in ["cours-vlan.pdf", "tp-ospf.pdf"] {
        let id = file_id(&app, name).await;
        alice
            .post(&format!("/files/{id}/tags"), &[("name", "réseau")])
            .await;
    }
    let planning = file_id(&app, "Planning-École.pdf").await;
    alice
        .post(&format!("/files/{planning}/tags"), &[("name", "examens")])
        .await;

    // Les tags des fichiers de l'endroit, avec leur nombre.
    let page = alice.get("/files?dossier=espace").await.body;
    let chips = common::between(&page, "aria-label=\"Filtrer par tag\"", "</nav>").unwrap();
    assert!(chips.contains("#réseau<span class=\"chip-count\">2</span>"));
    assert!(chips.contains("#examens<span class=\"chip-count\">1</span>"));

    // Le filtre : les fichiers de ce tag, dans tout l'espace.
    let found = alice
        .get("/files?dossier=espace&tag=r%C3%A9seau")
        .await
        .body;
    assert!(found.contains("cours-vlan.pdf") && found.contains("tp-ospf.pdf"));
    assert!(!found.contains("Planning-"));
    assert!(found.contains("aria-current=\"true\">#réseau"));
    assert!(found.contains("Retirer le filtre"));
    // Recherche et tag ensemble.
    let found = alice
        .get("/files?dossier=espace&tag=r%C3%A9seau&q=ospf")
        .await
        .body;
    assert!(found.contains("tp-ospf.pdf") && !found.contains("cours-vlan.pdf"));
}

#[tokio::test]
async fn un_autre_compte_ne_voit_ni_ne_tague_les_fichiers() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.upload("secret-vlan.pdf", b"%PDF").await;
    let id = file_id(&app, "secret-vlan.pdf").await;
    alice
        .post(&format!("/files/{id}/tags"), &[("name", "privé")])
        .await;
    let tag = tag_id(&app, "privé").await;

    let mut bob = app.logged_in(BOB).await;
    assert_eq!(
        bob.post(&format!("/files/{id}/tags"), &[("name", "intrus")])
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        bob.delete(&format!("/files/{id}/tags/{tag}")).await.status,
        StatusCode::NOT_FOUND
    );
    for path in [
        "/files?dossier=racine&q=vlan",
        "/files?dossier=espace&q=vlan",
    ] {
        let page = bob.get(path).await.body;
        assert!(!page.contains("secret-vlan.pdf"), "{path}");
    }
    assert!(
        !bob.get("/files?dossier=espace")
            .await
            .body
            .contains("#privé")
    );
    assert!(!tag_exists(&app, "intrus").await);
    assert!(
        alice
            .get(&format!("/files/{id}"))
            .await
            .body
            .contains("#privé")
    );
}
