//! L'espace dans l'adresse : `/e/<slug>/…`, l'espace de l'onglet sur chaque
//! requête htmx, et les redirections qui le gardent (SPEC, espaces et
//! outils, C).

mod common;

use axum::http::StatusCode;
use common::{ALICE, BOB, TestApp};

/// Crée une note dans l'espace actif, avec ce titre ; renvoie son adresse.
async fn note(client: &mut common::Client, title: &str) -> String {
    let created = client.post("/notes", &[]).await;
    let path = created.location().to_string();
    client
        .post(&path, &[("title", title), ("body_md", "contenu")])
        .await;
    path
}

#[tokio::test]
async fn une_adresse_ouvre_son_espace() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.post("/space", &[("slug", "ecole")]).await;
    note(&mut alice, "Cours de réseau").await;
    alice.post("/space", &[("slug", "taff")]).await;

    // La session est sur Taff ; l'adresse demande École.
    let page = alice.get("/e/ecole/notes").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(
        page.body
            .contains("<title>Notes · École · Hermes Dashboard</title>")
    );
    assert!(page.body.contains("Cours de réseau"));
    // Les liens de la page gardent l'espace, et htmx le joint à ses requêtes.
    assert!(page.body.contains("href=\"/e/ecole/notes\""));
    assert!(page.body.contains("href=\"/e/ecole/\""));
    assert!(page.body.contains("\"X-Espace\":\"ecole\""));
    // L'adresse devient le dernier espace utilisé.
    assert!(alice.get("/notes").await.body.contains("Notes · École"));

    // Le dashboard, avec ou sans barre finale.
    for path in ["/e/taff/", "/e/taff"] {
        let page = alice.get(path).await;
        assert!(page.body.contains("<title>Dashboard · Taff"), "{path}");
    }

    // Un espace inconnu, ou celui d'un autre compte : introuvable.
    let mut bob = app.logged_in(BOB).await;
    bob.post(
        "/settings/spaces",
        &[("name", "Sport"), ("color", "teal"), ("icon", "dumbbell")],
    )
    .await;
    assert_eq!(
        alice.get("/e/sport/notes").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(alice.get("/e/inconnu/").await.status, StatusCode::NOT_FOUND);
    assert_eq!(bob.get("/e/sport/notes").await.status, StatusCode::OK);
}

#[tokio::test]
async fn chaque_onglet_garde_son_espace() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    alice
        .post(
            "/tasks",
            &[("title", "Rendre le TP"), ("space_id", &ecole.to_string())],
        )
        .await;
    // Un autre onglet est passé sur Taff : c'est la session.
    alice.post("/space", &[("slug", "taff")]).await;

    // Le widget de l'onglet resté sur École suit son en-tête.
    let widget = alice
        .with_header("x-espace", "ecole")
        .get_fragment("/widgets/tasks")
        .await;
    assert!(widget.body.contains("Rendre le TP"));
    let widget = alice.get_fragment("/widgets/tasks").await;
    assert!(!widget.body.contains("Rendre le TP"));
    // Un en-tête qui ne désigne aucun espace du compte est ignoré.
    let widget = alice
        .with_header("x-espace", "sport")
        .get_fragment("/widgets/tasks")
        .await;
    assert_eq!(widget.status, StatusCode::OK);

    // Un lien sans préfixe suivi par htmx : l'historique garde l'espace.
    let page = alice
        .with_header("x-espace", "ecole")
        .get_boosted("/notes")
        .await;
    assert_eq!(page.header("hx-push-url"), "/e/ecole/notes");
}

#[tokio::test]
async fn les_redirections_gardent_l_espace() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.post("/space", &[("slug", "ecole")]).await;
    let path = note(&mut alice, "Brouillon").await;

    // Supprimer la note depuis un onglet sur École : retour aux notes d'École.
    let deleted = alice
        .with_header("x-espace", "ecole")
        .post(&format!("{path}/delete"), &[])
        .await;
    assert_eq!(deleted.location(), "/e/ecole/notes");
    // Sans espace d'onglet (formulaire classique), l'adresse ne change pas.
    let path = note(&mut alice, "Autre").await;
    let deleted = alice.post(&format!("{path}/delete"), &[]).await;
    assert_eq!(deleted.location(), "/notes");

    // Changer d'espace : l'adresse du nouvel espace, même page.
    let switched = alice
        .post("/space", &[("slug", "taff"), ("next", "/files")])
        .await;
    assert_eq!(switched.location(), "/e/taff/files");
    let switched = alice
        .post("/space", &[("slug", "perso"), ("next", "/notes/12")])
        .await;
    assert_eq!(switched.location(), "/e/perso/notes");
    let switched = alice
        .post("/space", &[("slug", "perso"), ("next", "/settings")])
        .await;
    assert_eq!(switched.location(), "/settings");
}

#[tokio::test]
async fn une_note_s_ouvre_dans_son_espace() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.post("/space", &[("slug", "ecole")]).await;
    let path = note(&mut alice, "Cours de réseau").await;
    alice.post("/space", &[("slug", "perso")]).await;

    // Depuis Perso (ou la palette), la note s'ouvre à l'adresse de son espace.
    let opened = alice.get(&path).await;
    assert_eq!(opened.location(), format!("/e/ecole{path}"));
    let page = alice.follow(opened).await;
    assert!(page.body.contains("data-accent=\"blue\""));
    assert!(page.body.contains("Cours de réseau"));
}
