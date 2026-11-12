//! Dashboard modulaire : disposition propre à chaque espace (ordre et taille
//! des widgets), enregistrement, refus, et widgets qui s'adaptent à leur taille.

mod common;

use axum::http::StatusCode;
use common::{ALICE, BOB, TestApp};

/// Widgets de la page, dans l'ordre : (type, classes de taille).
fn widgets(body: &str) -> Vec<(String, String)> {
    body.match_indices("class=\"card widget dw ")
        .map(|(at, _)| {
            let rest = &body[at..];
            let size = common::between(rest, "class=\"card widget dw ", "\"").unwrap();
            let kind = common::between(rest, "data-kind=\"", "\"").unwrap();
            (kind.to_string(), size.to_string())
        })
        .collect()
}

fn layout(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(k, s)| (k.to_string(), s.to_string()))
        .collect()
}

#[tokio::test]
async fn disposition_par_espace() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;

    // Par défaut : agenda, tâches, notes, fichiers.
    let page = alice.get("/").await;
    assert_eq!(page.status, StatusCode::OK);
    let default = layout(&[
        ("courses", "w2 h2"),
        ("tasks", "w2 h2"),
        ("notes", "w2 h1"),
        ("files", "w2 h1"),
    ]);
    assert_eq!(widgets(&page.body), default);
    // Le catalogue propose les cinq outils, en trois tailles.
    for kind in ["courses", "tasks", "notes", "files", "settings"] {
        assert!(
            page.body.contains(&format!(
                "data-dash-pick data-kind=\"{kind}\" data-w=\"2\" data-h=\"2\""
            )),
            "{kind}"
        );
    }

    // Une disposition à soi : l'ordre et les tailles sont gardés, chaque
    // widget demande son contenu à sa taille.
    let saved = alice
        .post(
            "/dashboard/layout",
            &[("layout", "notes:1x1,settings:2x1,courses:4x2")],
        )
        .await;
    assert_eq!(saved.status, StatusCode::NO_CONTENT);
    let page = alice.get("/").await;
    assert_eq!(
        widgets(&page.body),
        layout(&[
            ("notes", "w1 h1"),
            ("settings", "w2 h1"),
            ("courses", "w4 h2")
        ])
    );
    assert!(
        page.body
            .contains("hx-get=\"/widgets/courses?w=4&amp;h=2\"")
    );
    // La grille seule (rechargée après un ajout) suit la même disposition.
    let grid = alice.get_fragment("/dashboard/grid").await;
    assert_eq!(grid.status, StatusCode::OK);
    assert_eq!(widgets(&grid.body), widgets(&page.body));

    // Un autre espace garde la sienne ; un autre compte aussi.
    alice.post("/space", &[("slug", "ecole")]).await;
    assert_eq!(widgets(&alice.get("/").await.body), default);
    let mut bob = app.logged_in(BOB).await;
    assert_eq!(widgets(&bob.get("/").await.body), default);

    // Tout retirer : le dashboard reste vide, avec de quoi ajouter.
    alice
        .post("/dashboard/layout", &[("layout", ""), ("empty", "1")])
        .await;
    let page = alice.get("/").await;
    assert!(widgets(&page.body).is_empty());
    assert!(page.body.contains("Aucun widget dans cet espace"));

    // Retour à la disposition par défaut.
    alice.post("/dashboard/layout", &[("layout", "")]).await;
    assert_eq!(widgets(&alice.get("/").await.body), default);
}

#[tokio::test]
async fn disposition_refusee() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice
        .post("/dashboard/layout", &[("layout", "tasks:1x1")])
        .await;

    for bad in [
        "meteo:1x1",
        "tasks:1x1,tasks:2x2",
        "tasks:5x1",
        "tasks:0x1",
        "tasks",
    ] {
        let refused = alice.post("/dashboard/layout", &[("layout", bad)]).await;
        assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{bad}");
    }
    let refused = alice
        .post_without_csrf("/dashboard/layout", &[("layout", "notes:1x1")])
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert_eq!(
        widgets(&alice.get("/").await.body),
        layout(&[("tasks", "w1 h1")])
    );
}

#[tokio::test]
async fn les_widgets_suivent_leur_taille() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    for n in 1..=8 {
        let created = alice.post("/notes", &[]).await;
        let id = common::note_id(&created);
        alice
            .post(
                &format!("/notes/{id}"),
                &[("title", &format!("Note {n}")), ("body_md", "")],
            )
            .await;
    }
    let rows = |body: &str| body.matches("class=\"note-link-title\"").count();

    // Petit carré : une seule note ; grand : toutes celles qui tiennent.
    assert_eq!(
        rows(&alice.get_fragment("/widgets/notes?w=1&h=1").await.body),
        1
    );
    assert_eq!(
        rows(&alice.get_fragment("/widgets/notes?w=2&h=1").await.body),
        2
    );
    assert_eq!(
        rows(&alice.get_fragment("/widgets/notes?w=2&h=4").await.body),
        8
    );
    // Sans taille : comme avant le dashboard modulaire.
    assert_eq!(rows(&alice.get_fragment("/widgets/notes").await.body), 5);

    // Fichiers : les derniers de l'espace, et le total s'il y en a plus.
    for name in ["a.txt", "b.txt", "c.txt"] {
        alice.upload(name, b"contenu").await;
    }
    let files = alice.get_fragment("/widgets/files?w=2&h=1").await;
    assert_eq!(files.status, StatusCode::OK);
    assert!(files.body.contains("c.txt") && files.body.contains("b.txt"));
    assert!(!files.body.contains("a.txt"));
    assert!(files.body.contains("3 fichiers dans cet espace"));

    // Réglages rapides : le thème, les agendas, la place prise.
    let settings = alice.get_fragment("/widgets/settings?w=2&h=2").await;
    assert_eq!(settings.status, StatusCode::OK);
    assert!(
        settings
            .body
            .contains("value=\"system\" aria-pressed=\"true\"")
    );
    assert!(settings.body.contains("Ajouter un agenda iCal"));
    assert!(settings.body.contains("3 fichiers · "));
    // En petit carré, seulement le thème.
    let small = alice.get_fragment("/widgets/settings?w=1&h=1").await;
    assert!(small.body.contains("action=\"/settings/theme\""));
    assert!(!small.body.contains("Ajouter un agenda iCal"));
}

#[tokio::test]
async fn sans_fichiers_le_widget_disparait() {
    let app = TestApp::without_files().await;
    let mut alice = app.logged_in(ALICE).await;
    let page = alice.get("/").await;
    assert!(!page.body.contains("data-kind=\"files\""));
    assert_eq!(
        widgets(&page.body),
        layout(&[("courses", "w2 h2"), ("tasks", "w2 h2"), ("notes", "w2 h1")])
    );
    let widget = alice.get_fragment("/widgets/files").await;
    assert!(widget.body.contains("pas activés"));
}
