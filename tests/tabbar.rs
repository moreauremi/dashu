//! Barre de raccourcis du téléphone : choix par défaut, réglage, limites.

mod common;

use axum::http::StatusCode;
use common::{ALICE, BOB, TestApp, between};

/// Libellés des entrées de la barre, dans l'ordre de la page.
fn tabbar(body: &str) -> Vec<String> {
    let nav = between(body, r#"<nav class="tabbar""#, "</nav>").expect("barre de raccourcis");
    nav.split(r#"class="tab""#)
        .skip(1)
        .filter_map(|item| between(item, "<span>", "</span>"))
        .map(str::to_string)
        .collect()
}

#[tokio::test]
async fn quatre_raccourcis_par_defaut() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let page = alice.get("/notes").await;
    assert_eq!(
        tabbar(&page.body),
        ["Dashboard", "Notes", "Agenda", "Commandes"]
    );
    // La page courante est signalée, « Commandes » ouvre la barre de commandes.
    assert!(
        page.body
            .contains(r#"<a class="tab" href="/e/perso/notes" aria-current="page">"#)
    );
    assert!(
        page.body
            .contains(r#"<button class="tab" type="button" data-cmd-open>"#)
    );
}

#[tokio::test]
async fn choix_enregistre_sur_le_compte() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;

    let saved = alice
        .post(
            "/settings/tabs",
            &[
                ("tab_settings", "on"),
                ("tab_calendar", "on"),
                ("tab_new-note", "on"),
            ],
        )
        .await;
    assert_eq!(saved.status, StatusCode::SEE_OTHER);
    let page = alice.follow(saved).await;
    // Toujours dans le même ordre, quel que soit l'ordre des cases.
    assert_eq!(tabbar(&page.body), ["Agenda", "Nouvelle note", "Réglages"]);
    assert!(page.body.contains("Raccourcis enregistrés"));
    // Les cases cochées reflètent le choix.
    assert!(page.body.contains(r#"name="tab_calendar" checked"#));
    assert!(!page.body.contains(r#"name="tab_notes" checked"#));

    // Le choix est propre au compte : Bob garde la barre par défaut.
    let mut bob = app.logged_in(BOB).await;
    assert_eq!(
        tabbar(&bob.get("/").await.body),
        ["Dashboard", "Notes", "Agenda", "Commandes"]
    );
}

#[tokio::test]
async fn ni_vide_ni_plus_de_quatre() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;

    let five = [
        ("tab_dashboard", "on"),
        ("tab_notes", "on"),
        ("tab_calendar", "on"),
        ("tab_search", "on"),
        ("tab_settings", "on"),
    ];
    let refused = alice.post("/settings/tabs", &five).await;
    let page = alice.follow(refused).await;
    assert!(page.body.contains("4 raccourcis au plus."));
    assert_eq!(tabbar(&page.body).len(), 4);

    // Rien de coché, ou seulement des entrées inconnues : refusé aussi.
    let refused = alice.post("/settings/tabs", &[("tab_inconnu", "on")]).await;
    let page = alice.follow(refused).await;
    assert!(page.body.contains("Choisis au moins un raccourci."));
    assert_eq!(
        tabbar(&page.body),
        ["Dashboard", "Notes", "Agenda", "Commandes"]
    );
}

#[tokio::test]
async fn reglage_protege_par_csrf() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let refused = alice
        .post_without_csrf("/settings/tabs", &[("tab_settings", "on")])
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
}
