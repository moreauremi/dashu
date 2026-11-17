//! Réglages communs à tous les espaces : pas d'île pour changer d'espace
//! (donc pas de balayage non plus, que l'île porte), et un accent neutre, le
//! texte lui-même, au lieu de la couleur de l'espace actif.

mod common;

use axum::http::StatusCode;
use common::{ALICE, TestApp};

/// L'attribut `data-accent` de `#app`.
fn app_accent(body: &str) -> &str {
    let app = common::between(body, "<div id=\"app\"", ">").expect("#app");
    common::between(app, "data-accent=\"", "\"").expect("data-accent")
}

#[tokio::test]
async fn les_reglages_n_ont_ni_ile_ni_couleur_d_espace() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await;

    // Ailleurs : l'île et la couleur de l'espace actif (École, bleu).
    let dashboard = alice.get("/e/ecole/").await;
    assert!(dashboard.body.contains("data-island"));
    assert!(dashboard.body.contains("action=\"/space\""));
    assert_eq!(app_accent(&dashboard.body), "blue");

    // Toutes les pages de Réglages, même ouvertes depuis l'espace École.
    for path in [
        "/settings".to_string(),
        format!("/settings/spaces/{ecole}/members"),
        format!("/settings/spaces/{ecole}/delete"),
    ] {
        let page = alice.get(&path).await;
        assert_eq!(page.status, StatusCode::OK, "{path}");
        assert!(!page.body.contains("data-island"), "île sur {path}");
        assert!(
            !page.body.contains("action=\"/space\""),
            "changement d'espace sur {path}"
        );
        assert_eq!(app_accent(&page.body), "neutral", "{path}");
    }

    // Les espaces gardent leur couleur dans la liste des réglages.
    let settings = alice.get("/settings").await;
    assert!(settings.body.contains("data-accent=\"blue\""));

    // Retour à une page d'espace : l'île revient.
    let notes = alice.get("/e/ecole/notes").await;
    assert!(notes.body.contains("data-island"));
}
