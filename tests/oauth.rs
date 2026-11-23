//! OAuth pour le connecteur Claude (SPEC, « Connecteur Claude (MCP) »,
//! étape 3) : découverte, enregistrement, autorisation, jetons, révocation.

mod common;

use axum::http::{Method, StatusCode};
use common::{ALICE, API_TOKEN, BOB, Client, TestApp};
use serde_json::{Value, json};

const CLAUDE_CALLBACK: &str = "https://claude.ai/api/mcp/auth_callback";
const RESOURCE: &str = "http://localhost:3000/mcp";
/// Exemple de la RFC 7636 (annexe B) : le secret PKCE et son empreinte.
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

/// Enregistre une application, comme le fait Claude.
async fn register(app: &TestApp, redirect: &str) -> String {
    let (status, body) = app
        .api(
            Method::POST,
            "/oauth/register",
            Some(json!({
                "client_name": "Claude",
                "redirect_uris": [redirect],
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
                "token_endpoint_auth_method": "none",
            })),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["token_endpoint_auth_method"], "none");
    body["client_id"].as_str().unwrap().to_string()
}

fn authorize_url(client_id: &str, redirect: &str, challenge: &str) -> String {
    let query: String = form_urlencoded::Serializer::new(String::new())
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", "etat-de-claude")
        .append_pair("resource", RESOURCE)
        .finish();
    format!("/oauth/authorize?{query}")
}

/// Paramètre d'une adresse de retour (`code`, `state`, `error`).
fn param(location: &str, name: &str) -> Option<String> {
    let url = url::Url::parse(location).expect("adresse de retour");
    url.query_pairs()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
}

/// Un compte connecté autorise l'application : renvoie le code.
async fn authorize(browser: &mut Client, client_id: &str, redirect: &str) -> String {
    let page = browser
        .get(&authorize_url(client_id, redirect, CHALLENGE))
        .await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    let back = browser
        .post("/oauth/authorize", &[("decision", "allow")])
        .await;
    assert_eq!(back.status, StatusCode::SEE_OTHER, "{}", back.body);
    assert!(back.location().starts_with(redirect), "{}", back.location());
    assert_eq!(
        param(back.location(), "state").as_deref(),
        Some("etat-de-claude")
    );
    param(back.location(), "code").expect("code d'autorisation")
}

/// `POST /oauth/token`, en formulaire comme le veut OAuth.
async fn token(app: &TestApp, form: &[(&str, &str)]) -> (StatusCode, Value) {
    let body = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form)
        .finish();
    app.api_raw(Method::POST, "/oauth/token", Some(body), None, &[])
        .await
}

async fn exchange(
    app: &TestApp,
    client_id: &str,
    redirect: &str,
    code: &str,
) -> (StatusCode, Value) {
    token(
        app,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", VERIFIER),
            ("client_id", client_id),
            ("redirect_uri", redirect),
            ("resource", RESOURCE),
        ],
    )
    .await
}

async fn refresh(app: &TestApp, client_id: &str, refresh_token: &str) -> (StatusCode, Value) {
    token(
        app,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
        ],
    )
    .await
}

/// Statut d'un appel à `/mcp` avec ce jeton.
async fn mcp_status(app: &TestApp, access: &str) -> StatusCode {
    app.api(
        Method::POST,
        "/mcp",
        Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })),
        Some(access),
    )
    .await
    .0
}

/// Autorisation complète pour un compte : (client, jeton d'accès, jeton de renouvellement).
async fn full_grant(app: &TestApp, email: &str) -> (String, String, String) {
    let client_id = register(app, CLAUDE_CALLBACK).await;
    let mut browser = app.logged_in(email).await;
    let code = authorize(&mut browser, &client_id, CLAUDE_CALLBACK).await;
    let (status, tokens) = exchange(app, &client_id, CLAUDE_CALLBACK, &code).await;
    assert_eq!(status, StatusCode::OK, "{tokens}");
    (
        client_id,
        tokens["access_token"].as_str().unwrap().to_string(),
        tokens["refresh_token"].as_str().unwrap().to_string(),
    )
}

#[tokio::test]
async fn documents_de_decouverte() {
    let app = TestApp::new().await;
    // /mcp sans jeton désigne le document de la ressource protégée.
    let (status, _) = app
        .api_raw(
            Method::POST,
            "/mcp",
            Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }).to_string()),
            None,
            &[],
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    for path in [
        "/.well-known/oauth-protected-resource",
        "/.well-known/oauth-protected-resource/mcp",
    ] {
        let (status, body) = app.api(Method::GET, path, None, None).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(body["resource"], RESOURCE);
        assert_eq!(
            body["authorization_servers"],
            json!(["http://localhost:3000"])
        );
    }
    let (status, body) = app
        .api(
            Method::GET,
            "/.well-known/oauth-authorization-server",
            None,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["issuer"], "http://localhost:3000");
    assert_eq!(
        body["authorization_endpoint"],
        "http://localhost:3000/oauth/authorize"
    );
    assert_eq!(body["token_endpoint"], "http://localhost:3000/oauth/token");
    assert_eq!(
        body["registration_endpoint"],
        "http://localhost:3000/oauth/register"
    );
    assert_eq!(body["code_challenge_methods_supported"], json!(["S256"]));
}

#[tokio::test]
async fn de_l_enregistrement_aux_outils() {
    let app = TestApp::new().await;
    let client_id = register(&app, CLAUDE_CALLBACK).await;

    // Pas encore connecté : la connexion d'abord, qui revient à l'autorisation.
    let mut browser = app.client();
    let first = browser
        .get(&authorize_url(&client_id, CLAUDE_CALLBACK, CHALLENGE))
        .await;
    assert_eq!(first.status, StatusCode::SEE_OTHER);
    assert_eq!(first.location(), "/login");
    let login = browser.get("/login").await;
    assert!(login.body.contains("autoriser « Claude »"));
    let signed = browser.login(ALICE, common::PASSWORD).await;
    assert_eq!(signed.status, StatusCode::SEE_OTHER);
    assert_eq!(signed.location(), "/oauth/authorize");

    // La page « Autoriser » : l'application, le compte, l'adresse de retour.
    let consent = browser.get("/oauth/authorize").await;
    assert_eq!(consent.status, StatusCode::OK, "{}", consent.body);
    assert!(consent.body.contains("Autoriser « Claude » ?"));
    assert!(consent.body.contains(ALICE));
    assert!(consent.body.contains("claude.ai"));
    let csp = consent.header("content-security-policy");
    assert!(
        csp.contains("form-action 'self' https://claude.ai;"),
        "{csp}"
    );
    assert!(csp.contains("frame-ancestors 'none'"));

    let back = browser
        .post("/oauth/authorize", &[("decision", "allow")])
        .await;
    assert_eq!(back.status, StatusCode::SEE_OTHER);
    let location = back.location().to_string();
    assert!(location.starts_with(CLAUDE_CALLBACK));
    assert_eq!(param(&location, "state").as_deref(), Some("etat-de-claude"));
    let code = param(&location, "code").unwrap();
    assert!(code.starts_with("dashu_"));

    // Le code contre des jetons, une seule fois.
    let (status, tokens) = exchange(&app, &client_id, CLAUDE_CALLBACK, &code).await;
    assert_eq!(status, StatusCode::OK, "{tokens}");
    assert_eq!(tokens["token_type"], "Bearer");
    assert_eq!(tokens["expires_in"], 3600);
    let access = tokens["access_token"].as_str().unwrap();
    assert!(access.starts_with("dashu_"));
    assert!(
        tokens["refresh_token"]
            .as_str()
            .unwrap()
            .starts_with("dashu_")
    );
    let (status, again) = exchange(&app, &client_id, CLAUDE_CALLBACK, &code).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(again["error"], "invalid_grant");

    // Le jeton d'accès ouvre /mcp, pas /api/v1.
    assert_eq!(mcp_status(&app, access).await, StatusCode::OK);
    let (status, _) = app
        .api(Method::GET, "/api/v1/spaces", None, Some(access))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // L'autorisation apparaît dans Réglages › Connexions.
    let settings = browser.get("/settings").await;
    assert!(settings.body.contains("application autorisée"));
    assert!(settings.body.contains("autorisé le"));
}

#[tokio::test]
async fn refuser_renvoie_une_erreur_a_claude() {
    let app = TestApp::new().await;
    let client_id = register(&app, CLAUDE_CALLBACK).await;
    let mut browser = app.logged_in(ALICE).await;
    browser
        .get(&authorize_url(&client_id, CLAUDE_CALLBACK, CHALLENGE))
        .await;
    let back = browser
        .post("/oauth/authorize", &[("decision", "deny")])
        .await;
    assert_eq!(back.status, StatusCode::SEE_OTHER);
    assert_eq!(
        param(back.location(), "error").as_deref(),
        Some("access_denied")
    );
    assert!(param(back.location(), "code").is_none());
    // La demande est consommée : un second envoi n'a plus rien à autoriser.
    let again = browser
        .post("/oauth/authorize", &[("decision", "allow")])
        .await;
    assert_eq!(again.status, StatusCode::BAD_REQUEST);
    // Sans jeton CSRF, rien ne passe.
    browser
        .get(&authorize_url(&client_id, CLAUDE_CALLBACK, CHALLENGE))
        .await;
    let forged = browser
        .post_without_csrf("/oauth/authorize", &[("decision", "allow")])
        .await;
    assert_eq!(forged.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn enregistrement_limite_aux_adresses_de_claude() {
    let app = TestApp::new().await;
    for redirect in [
        "https://evil.example/api/mcp/auth_callback",
        "https://claude.ai.evil.example/api/mcp/auth_callback",
        "https://claude.ai/autre",
        "http://192.168.1.10/callback",
        "https://localhost/callback",
    ] {
        let (status, body) = app
            .api(
                Method::POST,
                "/oauth/register",
                Some(json!({ "client_name": "Faux Claude", "redirect_uris": [redirect] })),
                None,
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{redirect}");
        assert_eq!(body["error"], "invalid_redirect_uri");
    }
    let (status, _) = app
        .api(
            Method::POST,
            "/oauth/register",
            Some(json!({
                "redirect_uris": [CLAUDE_CALLBACK],
                "token_endpoint_auth_method": "client_secret_basic",
            })),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Claude Code revient sur une adresse locale, n'importe quel port.
    register(&app, "http://localhost:33418/callback").await;
    register(&app, "http://127.0.0.1:5000/callback").await;
}

#[tokio::test]
async fn autorisation_stricte() {
    let app = TestApp::new().await;
    let client_id = register(&app, CLAUDE_CALLBACK).await;
    let other = register(&app, "http://localhost:4000/callback").await;
    let mut browser = app.logged_in(ALICE).await;

    // Client inconnu, ou adresse de retour qui n'est pas la sienne : une
    // page d'erreur, jamais de redirection.
    let unknown = browser
        .get(&authorize_url("client_inconnu", CLAUDE_CALLBACK, CHALLENGE))
        .await;
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST);
    let wrong = browser
        .get(&authorize_url(&other, CLAUDE_CALLBACK, CHALLENGE))
        .await;
    assert_eq!(wrong.status, StatusCode::BAD_REQUEST);
    assert!(wrong.location().is_empty());

    // PKCE absent ou en « plain », ressource étrangère : erreur rendue à Claude.
    let no_pkce = format!(
        "/oauth/authorize?response_type=code&client_id={client_id}&redirect_uri={}&state=s",
        form_urlencoded::byte_serialize(CLAUDE_CALLBACK.as_bytes()).collect::<String>()
    );
    let response = browser.get(&no_pkce).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(
        param(response.location(), "error").as_deref(),
        Some("invalid_request")
    );
    assert_eq!(param(response.location(), "state").as_deref(), Some("s"));
    let plain = authorize_url(&client_id, CLAUDE_CALLBACK, CHALLENGE)
        .replace("code_challenge_method=S256", "code_challenge_method=plain");
    let response = browser.get(&plain).await;
    assert_eq!(
        param(response.location(), "error").as_deref(),
        Some("invalid_request")
    );
    let foreign = authorize_url(&client_id, CLAUDE_CALLBACK, CHALLENGE).replace(
        &form_urlencoded::byte_serialize(RESOURCE.as_bytes()).collect::<String>(),
        "https%3A%2F%2Fautre.example%2Fmcp",
    );
    let response = browser.get(&foreign).await;
    assert_eq!(
        param(response.location(), "error").as_deref(),
        Some("invalid_target")
    );

    // Mauvais secret PKCE : refusé, et le code est perdu.
    let code = authorize(&mut browser, &client_id, CLAUDE_CALLBACK).await;
    let (status, body) = token(
        &app,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            (
                "code_verifier",
                "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXl",
            ),
            ("client_id", &client_id),
            ("redirect_uri", CLAUDE_CALLBACK),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_grant");
    let (_, body) = exchange(&app, &client_id, CLAUDE_CALLBACK, &code).await;
    assert_eq!(body["error"], "invalid_grant");

    // Adresse de retour ou client différents de ceux du code.
    let code = authorize(&mut browser, &client_id, CLAUDE_CALLBACK).await;
    let (_, body) = exchange(
        &app,
        &client_id,
        "https://claude.com/api/mcp/auth_callback",
        &code,
    )
    .await;
    assert_eq!(body["error"], "invalid_grant");
    let code = authorize(&mut browser, &client_id, CLAUDE_CALLBACK).await;
    let (_, body) = exchange(&app, &other, CLAUDE_CALLBACK, &code).await;
    assert_eq!(body["error"], "invalid_grant");

    // Code expiré.
    let code = authorize(&mut browser, &client_id, CLAUDE_CALLBACK).await;
    sqlx::query("UPDATE oauth_codes SET expires_at = 1")
        .execute(&app.state.db)
        .await
        .unwrap();
    let (_, body) = exchange(&app, &client_id, CLAUDE_CALLBACK, &code).await;
    assert_eq!(body["error"], "invalid_grant");

    // Type d'échange inconnu.
    let (_, body) = token(&app, &[("grant_type", "password")]).await;
    assert_eq!(body["error"], "unsupported_grant_type");
}

#[tokio::test]
async fn un_jeton_de_renouvellement_reutilise_revoque_tout() {
    let app = TestApp::new().await;
    let (client_id, access, first_refresh) = full_grant(&app, ALICE).await;

    // Renouvellement : de nouveaux jetons, l'ancien jeton de renouvellement ne vaut plus.
    let (status, renewed) = refresh(&app, &client_id, &first_refresh).await;
    assert_eq!(status, StatusCode::OK, "{renewed}");
    let new_access = renewed["access_token"].as_str().unwrap().to_string();
    let new_refresh = renewed["refresh_token"].as_str().unwrap().to_string();
    assert_ne!(new_refresh, first_refresh);
    assert_eq!(mcp_status(&app, &new_access).await, StatusCode::OK);
    // Un autre client ne renouvelle pas ce jeton.
    let stranger = register(&app, CLAUDE_CALLBACK).await;
    let (_, body) = refresh(&app, &stranger, &new_refresh).await;
    assert_eq!(body["error"], "invalid_grant");

    // L'ancien revient : il a été copié, toute l'autorisation tombe.
    let (status, body) = refresh(&app, &client_id, &first_refresh).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_grant");
    assert_eq!(mcp_status(&app, &access).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        mcp_status(&app, &new_access).await,
        StatusCode::UNAUTHORIZED
    );
    let (_, body) = refresh(&app, &client_id, &new_refresh).await;
    assert_eq!(body["error"], "invalid_grant");
}

#[tokio::test]
async fn revoquer_dans_reglages_coupe_claude() {
    let app = TestApp::new().await;
    let (client_id, access, refresh_token) = full_grant(&app, ALICE).await;
    let grant: i64 = sqlx::query_scalar("SELECT id FROM oauth_grants")
        .fetch_one(&app.state.db)
        .await
        .unwrap();

    // Un autre compte ne révoque pas l'autorisation d'Alice.
    let mut bob = app.logged_in(BOB).await;
    let response = bob
        .post(&format!("/settings/grants/{grant}/revoke"), &[])
        .await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert_eq!(mcp_status(&app, &access).await, StatusCode::OK);

    let mut alice = app.logged_in(ALICE).await;
    let response = alice
        .post(&format!("/settings/grants/{grant}/revoke"), &[])
        .await;
    assert_eq!(response.status, StatusCode::OK);
    assert!(!response.body.contains("application autorisée"));
    assert_eq!(mcp_status(&app, &access).await, StatusCode::UNAUTHORIZED);
    let (_, body) = refresh(&app, &client_id, &refresh_token).await;
    assert_eq!(body["error"], "invalid_grant");

    // « Tout révoquer » emporte aussi les autorisations.
    let (_, access, _) = full_grant(&app, ALICE).await;
    assert_eq!(mcp_status(&app, &access).await, StatusCode::OK);
    alice.post("/settings/tokens/revoke-all", &[]).await;
    assert_eq!(mcp_status(&app, &access).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn claude_se_deconnecte_par_la_revocation() {
    let app = TestApp::new().await;
    let (_, access, refresh_token) = full_grant(&app, ALICE).await;
    let revoke = |token: String| {
        let app = &app;
        async move {
            app.api_raw(
                Method::POST,
                "/oauth/revoke",
                Some(format!("token={token}")),
                None,
                &[],
            )
            .await
            .0
        }
    };
    // Un jeton inconnu : 200 quand même (RFC 7009).
    assert_eq!(revoke("dashu_inconnu".into()).await, StatusCode::OK);
    assert_eq!(mcp_status(&app, &access).await, StatusCode::OK);
    assert_eq!(revoke(refresh_token).await, StatusCode::OK);
    assert_eq!(mcp_status(&app, &access).await, StatusCode::UNAUTHORIZED);

    // Révoquer le jeton d'accès emporte aussi l'autorisation.
    let (client_id, access, refresh_token) = full_grant(&app, ALICE).await;
    assert_eq!(revoke(access.clone()).await, StatusCode::OK);
    assert_eq!(mcp_status(&app, &access).await, StatusCode::UNAUTHORIZED);
    let (_, body) = refresh(&app, &client_id, &refresh_token).await;
    assert_eq!(body["error"], "invalid_grant");
}

#[tokio::test]
async fn un_second_compte_branche_son_propre_claude() {
    let app = TestApp::new().await;
    let (_, note) = app
        .api(
            Method::POST,
            "/api/v1/notes",
            Some(json!({ "title": "Note d'Alice", "space": "perso" })),
            Some(API_TOKEN),
        )
        .await;
    let alice_note = note["id"].as_i64().unwrap();

    let (_, bob_access, _) = full_grant(&app, BOB).await;
    let (_, body) = app
        .api(
            Method::POST,
            "/mcp",
            Some(json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": "lire_note", "arguments": { "id": alice_note } },
            })),
            Some(&bob_access),
        )
        .await;
    assert_eq!(body["result"]["isError"], true);
    assert!(
        body["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("introuvable")
    );
    // Les autorisations de Bob ne paraissent pas chez Alice.
    let mut alice = app.logged_in(ALICE).await;
    let settings = alice.get("/settings").await;
    assert!(!settings.body.contains("application autorisée"));
}

#[tokio::test]
async fn un_compte_retire_ne_renouvelle_plus() {
    let app = TestApp::new().await;
    let (client_id, _, _) = full_grant(&app, ALICE).await;
    // Un compte hors de ALLOWED_EMAILS, avec une autorisation en base.
    sqlx::query("INSERT INTO users (email, created_at) VALUES ('carol@example.test', 0)")
        .execute(&app.state.db)
        .await
        .unwrap();
    let carol_refresh = dashu::auth::tokens::generate();
    sqlx::query(
        "INSERT INTO oauth_grants (user_id, client_id, refresh_hash, created_at, expires_at)
         SELECT id, ?, ?, 0, 9999999999 FROM users WHERE email = 'carol@example.test'",
    )
    .bind(&client_id)
    .bind(dashu::auth::tokens::hash(&carol_refresh))
    .execute(&app.state.db)
    .await
    .unwrap();
    let (_, body) = refresh(&app, &client_id, &carol_refresh).await;
    assert_eq!(body["error"], "invalid_grant");
}
