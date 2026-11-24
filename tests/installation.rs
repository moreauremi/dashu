//! Premier lancement dans le navigateur (SPEC, « Installation simple »,
//! étape 4) : code d'installation, compte administrateur, adresse et Google
//! rangés en base, environnement prioritaire, secret de session créé une fois.

mod common;

use axum::http::StatusCode;
use common::{ALICE, Client, TestApp, between};
use dashu::config::{Config, Env};

const EMAIL: &str = "admin@example.test";
const MDP: &str = "un mot de passe solide";

/// Lit le jeton CSRF du formulaire de la page (session anonyme).
fn csrf_from(client: &mut Client, body: &str) {
    let token = between(body, "name=\"_csrf\" value=\"", "\"").expect("jeton CSRF dans la page");
    client.csrf = Some(token.to_string());
}

/// Donne le code d'installation : la suite de l'assistant s'ouvre.
async fn give_code(app: &TestApp, client: &mut Client) {
    let page = client.get("/setup").await;
    assert_eq!(page.status, StatusCode::OK);
    csrf_from(client, &page.body);
    let code = app.state.setup_code.to_lowercase().replace('-', " ");
    let r = client.post("/setup/code", &[("code", &code)]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    let form = client.get("/setup").await;
    assert!(form.body.contains("1. Ton compte"), "{}", form.body);
    csrf_from(client, &form.body);
}

async fn users(app: &TestApp) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users")
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn sans_compte_tout_mene_au_premier_lancement() {
    let app = TestApp::first_launch().await;
    let mut client = app.client();

    for path in ["/", "/login", "/notes", "/settings", "/s/inconnu"] {
        let r = client.get(path).await;
        assert_eq!(r.status, StatusCode::SEE_OTHER, "{path}");
        assert_eq!(r.location(), "/setup", "{path}");
    }
    // L'API, le connecteur Claude et OAuth : 503 en JSON.
    for path in [
        "/api/v1/spaces",
        "/mcp",
        "/.well-known/oauth-authorization-server",
    ] {
        let r = client.get(path).await;
        assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE, "{path}");
        assert!(r.body.contains("not_installed"), "{path} : {}", r.body);
    }
    // Ce qui reste ouvert.
    assert_eq!(client.get("/health").await.status, StatusCode::OK);
    let page = client.get("/setup").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("code d'installation"));
    // Le code n'est jamais dans la page.
    assert!(!page.body.contains(&*app.state.setup_code));
}

#[tokio::test]
async fn sans_le_bon_code_rien_ne_se_cree() {
    let app = TestApp::first_launch().await;
    let mut client = app.client();
    let page = client.get("/setup").await;
    csrf_from(&mut client, &page.body);

    // Le formulaire final sans avoir donné le code : renvoyé au code.
    let form = [
        ("email", EMAIL),
        ("password", MDP),
        ("password2", MDP),
        ("base_url", "http://localhost:3000"),
    ];
    let r = client.post("/setup", &form).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(r.location(), "/setup");
    assert_eq!(users(&app).await, 0);

    // Un mauvais code : refusé ; le code dans l'adresse ne compte pas.
    let r = client
        .post("/setup/code", &[("code", "AAAA-BBBB-CCCC")])
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = client
        .get(&format!("/setup?code={}", app.state.setup_code))
        .await;
    assert!(
        r.body.contains("XXXX-XXXX-XXXX"),
        "toujours la page du code"
    );
    let r = client.post("/setup", &form).await;
    assert_eq!(r.location(), "/setup");
    assert_eq!(users(&app).await, 0);

    // Sans jeton CSRF : refusé.
    let r = client
        .post_without_csrf("/setup/code", &[("code", &app.state.setup_code)])
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    // Les essais sont limités, comme sur /login (5 d'un coup).
    let mut limited = false;
    for _ in 0..8 {
        let r = client
            .post("/setup/code", &[("code", "AAAA-BBBB-CCCC")])
            .await;
        if r.status == StatusCode::TOO_MANY_REQUESTS {
            limited = true;
            break;
        }
    }
    assert!(limited, "les essais de code doivent être limités");
}

#[tokio::test]
async fn l_assistant_cree_l_administrateur_et_relance_dashu() {
    let app = TestApp::first_launch().await;
    let mut client = app.client();
    give_code(&app, &mut client).await;

    // Saisies refusées, rien n'est créé.
    for (form, message) in [
        (
            vec![
                ("email", "pas-une-adresse"),
                ("password", MDP),
                ("password2", MDP),
                ("base_url", "http://localhost:3000"),
            ],
            "Adresse email invalide",
        ),
        (
            vec![
                ("email", EMAIL),
                ("password", "court"),
                ("password2", "court"),
                ("base_url", "http://localhost:3000"),
            ],
            "12 caractères",
        ),
        (
            vec![
                ("email", EMAIL),
                ("password", MDP),
                ("password2", "autre chose encore"),
                ("base_url", "http://localhost:3000"),
            ],
            "pas identiques",
        ),
        (
            vec![
                ("email", EMAIL),
                ("password", MDP),
                ("password2", MDP),
                ("base_url", "https://exemple.fr/chemin"),
            ],
            "Adresse invalide",
        ),
        (
            vec![
                ("email", EMAIL),
                ("password", MDP),
                ("password2", MDP),
                ("base_url", "https://dashu.exemple.fr"),
                ("google_client_id", "id-seul"),
            ],
            "aucun des deux",
        ),
    ] {
        let r = client.post("/setup", &form).await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{message}");
        assert!(r.body.contains(message), "{message} : {}", r.body);
        assert!(
            !r.body.contains(MDP),
            "le mot de passe ne revient pas dans la page"
        );
        csrf_from(&mut client, &r.body);
    }
    assert_eq!(users(&app).await, 0);

    // Le bon formulaire.
    let r = client
        .post(
            "/setup",
            &[
                ("email", " Admin@Example.test "),
                ("password", MDP),
                ("password2", MDP),
                ("base_url", "https://dashu.exemple.fr/"),
                ("google_client_id", "client.apps.googleusercontent.com"),
                ("google_client_secret", "secret-google"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(r.body.contains("C'est prêt"));
    assert!(app.state.restart.asked(), "dashu doit se relancer");

    // En base : l'administrateur, son adresse autorisée, l'adresse publique,
    // Google avec un secret chiffré.
    let (email, admin) = sqlx::query_as::<_, (String, i64)>("SELECT email, is_admin FROM users")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!((email.as_str(), admin), (EMAIL, 1));
    let stored: Vec<(String, String)> =
        sqlx::query_as("SELECT key, value FROM instance_settings ORDER BY key")
            .fetch_all(&app.state.db)
            .await
            .unwrap();
    assert!(stored.contains(&("base_url".into(), "https://dashu.exemple.fr".into())));
    let sealed = &stored
        .iter()
        .find(|(k, _)| k == "google_client_secret")
        .unwrap()
        .1;
    assert!(
        !sealed.contains("secret-google"),
        "le secret Google est chiffré"
    );

    // Le premier lancement est fini : /setup n'existe plus, même pour un
    // autre navigateur qui aurait le code (avec un jeton CSRF valable, pris
    // sur la page de connexion, qui revient).
    assert_eq!(client.get("/setup").await.status, StatusCode::NOT_FOUND);
    let mut other = app.client();
    let login = other.get("/login").await;
    assert_eq!(login.status, StatusCode::OK);
    csrf_from(&mut other, &login.body);
    assert_eq!(
        other
            .post("/setup/code", &[("code", &app.state.setup_code)])
            .await
            .status,
        StatusCode::NOT_FOUND
    );

    // Après la relance, la configuration vient de la base.
    let after = app.restarted(&[]).await;
    let config = &after.state.config;
    assert_eq!(config.base_url, "https://dashu.exemple.fr");
    assert_eq!(config.allowed_emails, vec![EMAIL.to_string()]);
    let google = config.google.as_ref().expect("Google configuré");
    assert_eq!(google.client_id, "client.apps.googleusercontent.com");
    assert_eq!(google.client_secret, "secret-google");
    assert!(after.state.google.is_some());
    // Et le compte se connecte.
    let mut browser = after.client();
    let r = browser.login(EMAIL, MDP).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert_eq!(r.location(), "/");
}

#[tokio::test]
async fn une_fois_un_compte_cree_setup_repond_404() {
    let app = TestApp::new().await;
    let mut client = app.client();
    assert_eq!(client.get("/setup").await.status, StatusCode::NOT_FOUND);
    let login = client.get("/login").await;
    assert_eq!(login.status, StatusCode::OK, "la connexion reste ouverte");
    csrf_from(&mut client, &login.body);
    let r = client
        .post("/setup/code", &[("code", &app.state.setup_code)])
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = client
        .post(
            "/setup",
            &[("email", EMAIL), ("password", MDP), ("password2", MDP)],
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(users(&app).await, 2);
    // L'API reste ouverte (pas de 503).
    assert_ne!(
        client.get("/api/v1/spaces").await.status,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn l_environnement_prime_sur_la_base() {
    let app = TestApp::first_launch().await;
    let mut client = app.client();
    give_code(&app, &mut client).await;
    let r = client
        .post(
            "/setup",
            &[
                ("email", EMAIL),
                ("password", MDP),
                ("password2", MDP),
                ("base_url", "https://dashu.exemple.fr"),
                ("google_client_id", "client-en-base"),
                ("google_client_secret", "secret-en-base"),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);

    let after = app
        .restarted(&[
            ("BASE_URL", "https://app.exemple.org"),
            ("ALLOWED_EMAILS", "alice@example.test"),
            ("GOOGLE_CLIENT_ID", "client-du-fichier"),
            ("GOOGLE_CLIENT_SECRET", "secret-du-fichier"),
        ])
        .await;
    let config = &after.state.config;
    assert_eq!(config.base_url, "https://app.exemple.org");
    assert_eq!(config.allowed_emails, vec![ALICE.to_string()]);
    assert_eq!(
        config.google.as_ref().unwrap().client_id,
        "client-du-fichier"
    );
    assert!(config.from_env.base_url && config.from_env.allowed_emails && config.from_env.google);
    // L'adresse de la base n'est plus autorisée : l'environnement l'emporte.
    assert!(!config.is_allowed(EMAIL));
}

#[tokio::test]
async fn sans_session_secret_le_secret_est_cree_une_fois() {
    let dir = std::env::temp_dir().join(format!("dashu-test-secret-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let url = format!("sqlite://{}", dir.join("dashu.db").display());
    let env = Env::from_pairs([("DATABASE_URL", url.as_str())]);

    let (first, pool) = Config::load(&env).await.expect("premier démarrage");
    pool.close().await;
    let file = dir.join("secret");
    let written = std::fs::read_to_string(&file).unwrap();
    assert_eq!(written.trim(), first.session_secret);
    assert!(first.session_secret.len() >= 64);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "lisible par dashu seul");
    }

    let (second, pool) = Config::load(&env).await.expect("second démarrage");
    pool.close().await;
    assert_eq!(
        second.session_secret, first.session_secret,
        "gardé au redémarrage"
    );

    // SESSION_SECRET donné : il l'emporte, le fichier ne change pas.
    let given = "f".repeat(64);
    let env = Env::from_pairs([
        ("DATABASE_URL", url.as_str()),
        ("SESSION_SECRET", given.as_str()),
    ]);
    let (third, pool) = Config::load(&env).await.unwrap();
    pool.close().await;
    assert_eq!(third.session_secret, given);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), written);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn le_code_change_a_chaque_demarrage() {
    let one = TestApp::first_launch().await;
    let two = TestApp::first_launch().await;
    assert_ne!(one.state.setup_code, two.state.setup_code);
}
