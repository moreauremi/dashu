//! Réglages › Instance (SPEC, « Installation simple », étape 5) : adresse,
//! Google et adresses autorisées, pour l'administrateur seulement ; une
//! valeur de l'environnement reste verrouillée.

mod common;

use axum::http::StatusCode;
use common::{ALICE, BOB, PASSWORD, TestApp};

async fn stored(app: &TestApp, key: &str) -> Option<String> {
    sqlx::query_scalar::<_, String>("SELECT value FROM instance_settings WHERE key = ?")
        .bind(key)
        .fetch_optional(&app.state.db)
        .await
        .unwrap()
}

async fn allowed(app: &TestApp) -> Vec<String> {
    sqlx::query_scalar::<_, String>("SELECT email FROM allowed_emails ORDER BY email")
        .fetch_all(&app.state.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn seul_l_administrateur_voit_et_change_l_instance() {
    let app = TestApp::by_wizard().await;

    let mut alice = app.client();
    alice.login(ALICE, PASSWORD).await;
    let page = alice.get("/settings").await;
    assert!(
        page.body.contains("id=\"instance\""),
        "Alice est l'administratrice"
    );

    let mut bob = app.client();
    bob.login(BOB, PASSWORD).await;
    let page = bob.get("/settings").await;
    assert!(!page.body.contains("id=\"instance\""));
    for (path, form) in [
        (
            "/settings/instance/address",
            vec![("base_url", "https://pirate.example")],
        ),
        (
            "/settings/instance/google",
            vec![("client_id", "x"), ("client_secret", "y")],
        ),
        ("/settings/instance/google/remove", vec![]),
        (
            "/settings/instance/emails",
            vec![("email", "intrus@example.test")],
        ),
        ("/settings/instance/emails/remove", vec![("email", ALICE)]),
    ] {
        let r = bob.post(path, &form).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}");
    }
    assert_eq!(stored(&app, "base_url").await, None);
    assert_eq!(
        allowed(&app).await,
        vec![ALICE.to_string(), BOB.to_string()]
    );
    assert!(!app.state.restart.asked());
}

#[tokio::test]
async fn adresse_google_et_adresses_se_changent_puis_dashu_se_relance() {
    let app = TestApp::by_wizard().await;
    let mut alice = app.client();
    alice.login(ALICE, PASSWORD).await;
    alice.get("/settings").await;

    // L'adresse.
    let r = alice
        .post(
            "/settings/instance/address",
            &[("base_url", "https://dashu.exemple.fr/")],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(r.location(), "/settings#instance");
    assert_eq!(
        stored(&app, "base_url").await.as_deref(),
        Some("https://dashu.exemple.fr")
    );
    assert!(app.state.restart.asked(), "un changement relance dashu");
    let page = alice.get("/settings").await;
    assert!(
        page.body.contains("redémarre"),
        "message après l'enregistrement"
    );
    assert!(
        page.body
            .contains("https://dashu.exemple.fr/auth/google/callback")
    );

    // Google : le secret est chiffré et jamais réaffiché.
    alice
        .post(
            "/settings/instance/google",
            &[
                ("client_id", "client.apps.googleusercontent.com"),
                ("client_secret", "secret-google"),
            ],
        )
        .await;
    let sealed = stored(&app, "google_client_secret").await.unwrap();
    assert!(!sealed.contains("secret-google"));
    let page = alice.get("/settings").await;
    assert!(page.body.contains("client.apps.googleusercontent.com"));
    assert!(!page.body.contains("secret-google"));

    // Une adresse de plus, une de moins.
    alice
        .post(
            "/settings/instance/emails",
            &[("email", " Carole@Example.test ")],
        )
        .await;
    alice
        .post("/settings/instance/emails/remove", &[("email", BOB)])
        .await;
    assert_eq!(
        allowed(&app).await,
        vec![ALICE.to_string(), "carole@example.test".to_string()]
    );

    // Après la relance : tout vient de la base, et Bob n'entre plus.
    let after = app.restarted(&[]).await;
    let config = &after.state.config;
    assert_eq!(config.base_url, "https://dashu.exemple.fr");
    assert_eq!(
        config.google.as_ref().unwrap().client_secret,
        "secret-google"
    );
    assert!(config.is_allowed("carole@example.test"));
    assert!(!config.is_allowed(BOB));
    let mut bob = after.client();
    let r = bob.login(BOB, PASSWORD).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);

    // Retirer Google : plus de bouton après la relance.
    let mut alice = after.client();
    alice.login(ALICE, PASSWORD).await;
    alice.get("/settings").await;
    alice.post("/settings/instance/google/remove", &[]).await;
    assert_eq!(stored(&after, "google_client_id").await, None);
    let again = after.restarted(&[]).await;
    assert!(again.state.google.is_none());
}

#[tokio::test]
async fn garde_fous() {
    let app = TestApp::by_wizard().await;
    let mut alice = app.client();
    alice.login(ALICE, PASSWORD).await;
    alice.get("/settings").await;

    // Saisies invalides : rien ne change, pas de relance.
    for (path, form, message) in [
        (
            "/settings/instance/address",
            vec![("base_url", "https://exemple.fr/chemin")],
            "Adresse invalide",
        ),
        (
            "/settings/instance/emails",
            vec![("email", "pas-une-adresse")],
            "Adresse email invalide",
        ),
        (
            "/settings/instance/google",
            vec![("client_id", "seul"), ("client_secret", "")],
            "et le secret du client Google",
        ),
        (
            "/settings/instance/emails/remove",
            vec![("email", ALICE)],
            "ta propre adresse",
        ),
    ] {
        let r = alice.post(path, &form).await;
        assert_eq!(r.status, StatusCode::SEE_OTHER, "{path}");
        let page = alice.get("/settings").await;
        assert!(page.body.contains(message), "{message}");
    }
    assert!(!app.state.restart.asked());
    assert_eq!(
        allowed(&app).await,
        vec![ALICE.to_string(), BOB.to_string()]
    );

    // Bob retiré, Alice reste seule : sa propre adresse ne se retire pas,
    // et c'est de toute façon la dernière.
    alice
        .post("/settings/instance/emails/remove", &[("email", BOB)])
        .await;
    assert_eq!(allowed(&app).await, vec![ALICE.to_string()]);
    let page = alice.get("/settings").await;
    assert!(page.body.contains("ton adresse"));
}

#[tokio::test]
async fn l_environnement_verrouille_ses_reglages() {
    // Comme le serveur de Rémi : adresse et comptes dans dashu.env.
    let app = TestApp::new().await;
    let mut alice = app.client();
    alice.login(ALICE, PASSWORD).await;
    let page = alice.get("/settings").await;
    assert!(page.body.contains("id=\"instance\""));
    assert!(page.body.contains("Définie sur le serveur"));
    assert!(page.body.contains("Fixées sur le serveur"));
    assert!(!page.body.contains("action=\"/settings/instance/address\""));

    let r = alice
        .post(
            "/settings/instance/address",
            &[("base_url", "https://autre.example")],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let r = alice
        .post(
            "/settings/instance/emails",
            &[("email", "carole@example.test")],
        )
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let page = alice.get("/settings").await;
    assert!(page.body.contains("défini sur le serveur"));
    assert_eq!(stored(&app, "base_url").await, None);
    assert!(allowed(&app).await.is_empty());
    assert!(!app.state.restart.asked());
}
