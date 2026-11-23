//! Jetons par compte (SPEC, « Connecteur Claude (MCP) », étape 1) :
//! Réglages › Connexions, et l'API au nom du compte du jeton.

mod common;

use axum::http::{Method, StatusCode};
use common::{ALICE, API_TOKEN, BOB, Client, TestApp, between};
use serde_json::json;

/// Crée un jeton depuis Réglages › Connexions et le lit dans la réponse,
/// seul endroit où il s'affiche en entier.
async fn create_token(client: &mut Client, name: &str) -> String {
    let response = client
        .post("/settings/tokens", &[("name", name), ("duree", "90")])
        .await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    between(
        &response.body,
        "id=\"new-token\" type=\"text\" value=\"",
        "\"",
    )
    .expect("jeton affiché à sa création")
    .to_string()
}

async fn token_id(app: &TestApp, name: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM access_tokens WHERE name = ?")
        .bind(name)
        .fetch_one(&app.state.db)
        .await
        .expect("jeton en base")
}

async fn count_tokens(app: &TestApp) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM access_tokens")
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn un_jeton_s_affiche_une_fois_et_seule_son_empreinte_est_gardee() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;

    let token = create_token(&mut alice, "Claude Code du Mac").await;
    assert!(token.starts_with("dashu_"));
    assert_eq!(token.len(), 70);

    let (name, stored): (String, String) =
        sqlx::query_as("SELECT name, token_hash FROM access_tokens")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(name, "Claude Code du Mac");
    assert_eq!(stored, dashu::auth::tokens::hash(&token));
    assert_ne!(stored, token);

    // Réaffichée, la carte donne le nom, jamais le jeton.
    let page = alice.get("/settings").await;
    assert!(page.body.contains("Claude Code du Mac"));
    assert!(page.body.contains("jamais utilisé"));
    assert!(!page.body.contains(&token));

    // Le jeton ouvre l'API.
    let (status, _) = app
        .api(Method::GET, "/api/v1/spaces", None, Some(&token))
        .await;
    assert_eq!(status, StatusCode::OK);
    let used: Option<i64> = sqlx::query_scalar("SELECT last_used_at FROM access_tokens")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert!(used.is_some(), "dernière utilisation notée");
    let page = alice.get("/settings").await;
    assert!(
        page.body.contains("utilisé à l&#39;instant") || page.body.contains("utilisé à l'instant")
    );
}

#[tokio::test]
async fn nom_vide_duree_inconnue_et_vingt_et_unieme_jeton_refuses() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;

    let response = alice
        .post("/settings/tokens", &[("name", "   "), ("duree", "90")])
        .await;
    assert!(response.body.contains("Donne un nom au jeton"));
    let response = alice
        .post("/settings/tokens", &[("name", "Script"), ("duree", "7")])
        .await;
    assert!(response.body.contains("Durée inconnue"));
    let long = "x".repeat(61);
    let response = alice
        .post("/settings/tokens", &[("name", &long), ("duree", "90")])
        .await;
    assert!(response.body.contains("Nom trop long"));
    assert_eq!(count_tokens(&app).await, 0);

    for n in 1..=20 {
        create_token(&mut alice, &format!("Jeton {n}")).await;
    }
    let response = alice
        .post("/settings/tokens", &[("name", "Jeton 21"), ("duree", "30")])
        .await;
    assert!(response.body.contains("20 jetons au plus"));
    assert!(!response.body.contains("new-token"));
    assert_eq!(count_tokens(&app).await, 20);
}

#[tokio::test]
async fn un_jeton_agit_au_nom_de_son_compte() {
    let app = TestApp::new().await;
    // Une note d'Alice, dans son espace École (par l'accès d'Hermès, qui
    // agit au nom d'Alice).
    let (status, note) = app
        .api(
            Method::POST,
            "/api/v1/notes",
            Some(json!({"title": "Cours d'Alice", "space": "ecole"})),
            Some(API_TOKEN),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let alice_note = note["id"].as_i64().unwrap();

    let mut bob = app.logged_in(BOB).await;
    let token = create_token(&mut bob, "Script de Bob").await;
    let t = Some(token.as_str());

    // Bob ne voit que ses espaces, et pas la note d'Alice.
    let (status, spaces) = app.api(Method::GET, "/api/v1/spaces", None, t).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(spaces.as_array().unwrap().len(), 3);
    let (status, _) = app
        .api(Method::GET, &format!("/api/v1/notes/{alice_note}"), None, t)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = app
        .api(
            Method::PATCH,
            &format!("/api/v1/notes/{alice_note}"),
            Some(json!({"append": "piratage"})),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Un espace partagé rejoint : Bob y voit et y écrit, par son jeton.
    let slug = app.share_space(ALICE, "taff", BOB).await;
    let (status, shared) = app
        .api(
            Method::POST,
            "/api/v1/notes",
            Some(json!({"title": "Compte rendu commun", "space": "taff"})),
            Some(API_TOKEN),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, found) = app
        .api(Method::GET, &format!("/api/v1/notes?space={slug}"), None, t)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        found
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["id"] == shared["id"])
    );
    let (status, task) = app
        .api(
            Method::POST,
            "/api/v1/tasks",
            Some(json!({"title": "Relire le compte rendu", "space": slug})),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{task}");
}

#[tokio::test]
async fn revoque_expire_tout_revoquer_ou_compte_retire_coupent_l_acces() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let first = create_token(&mut alice, "Premier").await;
    let second = create_token(&mut alice, "Deuxième").await;
    let third = create_token(&mut alice, "Troisième").await;
    let ok = |token: &str| {
        let token = token.to_string();
        let app = &app;
        async move {
            app.api(Method::GET, "/api/v1/tasks", None, Some(&token))
                .await
                .0
        }
    };
    assert_eq!(ok(&first).await, StatusCode::OK);

    // Un autre compte ne révoque pas le jeton d'Alice.
    let mut bob = app.logged_in(BOB).await;
    let id = token_id(&app, "Premier").await;
    let response = bob
        .post(&format!("/settings/tokens/{id}/revoke"), &[])
        .await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert_eq!(ok(&first).await, StatusCode::OK);

    // Révoqué : refusé à la requête suivante, les autres marchent toujours.
    let response = alice
        .post(&format!("/settings/tokens/{id}/revoke"), &[])
        .await;
    assert_eq!(response.status, StatusCode::OK);
    assert!(!response.body.contains("Premier"));
    let (status, body) = app
        .api(Method::GET, "/api/v1/tasks", None, Some(&first))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body["error"].as_str().unwrap().contains("jeton"));
    assert_eq!(ok(&second).await, StatusCode::OK);

    // Expiré : refusé, mais toujours dans la liste, à supprimer.
    sqlx::query("UPDATE access_tokens SET expires_at = 1 WHERE name = 'Deuxième'")
        .execute(&app.state.db)
        .await
        .unwrap();
    assert_eq!(ok(&second).await, StatusCode::UNAUTHORIZED);
    let page = alice.get("/settings").await;
    assert!(page.body.contains("Deuxième"));
    assert!(page.body.contains("expiré"));
    assert!(page.body.contains(">Supprimer</button>"));

    // Tout révoquer.
    assert_eq!(ok(&third).await, StatusCode::OK);
    let response = alice.post("/settings/tokens/revoke-all", &[]).await;
    assert_eq!(response.status, StatusCode::OK);
    assert!(response.body.contains("Aucun accès"));
    assert_eq!(ok(&third).await, StatusCode::UNAUTHORIZED);
    assert_eq!(count_tokens(&app).await, 0);

    // Un compte qui n'est plus dans ALLOWED_EMAILS : son jeton ne vaut plus rien.
    sqlx::query("INSERT INTO users (email, created_at) VALUES ('carol@example.test', 0)")
        .execute(&app.state.db)
        .await
        .unwrap();
    let carol = dashu::auth::tokens::generate();
    sqlx::query(
        "INSERT INTO access_tokens (user_id, name, token_hash, created_at, expires_at)
         SELECT id, 'Ancien', ?, 0, 9999999999 FROM users WHERE email = 'carol@example.test'",
    )
    .bind(dashu::auth::tokens::hash(&carol))
    .execute(&app.state.db)
    .await
    .unwrap();
    assert_eq!(ok(&carol).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn un_jeton_mal_forme_ou_inconnu_est_refuse() {
    let app = TestApp::new().await;
    for token in [
        "dashu_".to_string(),
        format!("dashu_{}", "0".repeat(64)),
        dashu::auth::tokens::generate(),
    ] {
        let (status, _) = app
            .api(Method::GET, "/api/v1/spaces", None, Some(&token))
            .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{token}");
    }
}
