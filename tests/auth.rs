//! Connexion, sessions, CSRF, anti-bruteforce, espaces.

mod common;

use axum::http::StatusCode;
use common::{ALICE, BOB, PASSWORD, TestApp};

const BAD_CREDENTIALS: &str = "Email ou mot de passe incorrect.";

#[tokio::test]
async fn health_repond_sans_authentification() {
    let app = TestApp::new().await;
    let response = app.client().get("/health").await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, "ok");
}

#[tokio::test]
async fn pages_protegees_renvoient_vers_la_connexion() {
    let app = TestApp::new().await;
    let mut visitor = app.client();
    for path in [
        "/",
        "/notes",
        "/notes/new",
        "/notes/1",
        "/calendar",
        "/settings",
        "/styleguide",
        "/widgets/tasks",
    ] {
        let response = visitor.get(path).await;
        assert_eq!(response.status, StatusCode::SEE_OTHER, "{path}");
        assert_eq!(response.location(), "/login", "{path}");
    }
    // Un fragment htmx reçoit un 401 et l'ordre de naviguer vers /login.
    let response = visitor.get_fragment("/widgets/tasks").await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    assert_eq!(response.header("hx-redirect"), "/login");
}

#[tokio::test]
async fn connexion_par_identifiants() {
    let app = TestApp::new().await;
    let mut alice = app.client();

    let response = alice.login(ALICE, PASSWORD).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(response.location(), "/");

    // Le cookie de session est inaccessible au JavaScript et limité au site.
    let cookie = response.header("set-cookie");
    assert!(cookie.contains("HttpOnly"), "{cookie}");
    assert!(cookie.contains("SameSite=Lax"), "{cookie}");

    let dashboard = alice.get("/").await;
    assert_eq!(dashboard.status, StatusCode::OK);
    assert!(dashboard.body.contains(ALICE));
}

#[tokio::test]
async fn l_email_est_insensible_a_la_casse_et_aux_espaces() {
    let app = TestApp::new().await;
    let response = app.client().login("  Alice@Example.TEST ", PASSWORD).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn meme_message_que_le_compte_existe_ou_non() {
    let app = TestApp::new().await;

    let wrong_password = app.client().login(ALICE, "pas le bon mot de passe").await;
    let unknown_email = app.client().login("personne@example.test", PASSWORD).await;

    for response in [&wrong_password, &unknown_email] {
        assert_eq!(response.status, StatusCode::UNAUTHORIZED);
        assert!(response.body.contains(BAD_CREDENTIALS));
    }
    // Les deux pages d'erreur sont identiques, au jeton CSRF près.
    let strip = |body: &str| {
        let token = common::between(body, "name=\"_csrf\" value=\"", "\"")
            .unwrap_or("")
            .to_string();
        body.replace(&token, "")
            .replace("personne@example.test", "")
            .replace(ALICE, "")
    };
    assert_eq!(strip(&wrong_password.body), strip(&unknown_email.body));
}

#[tokio::test]
async fn email_hors_liste_refuse_meme_avec_le_bon_mot_de_passe() {
    let app = TestApp::new().await;
    // Le compte existe en base, mais son email n'est pas dans ALLOWED_EMAILS.
    app.add_user("intrus@example.test").await;

    let mut intrus = app.client();
    let response = intrus.login("intrus@example.test", PASSWORD).await;
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    assert!(response.body.contains(BAD_CREDENTIALS));
    assert_eq!(intrus.get("/").await.status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn compte_google_refuse_message_clair() {
    // Retour de Google pour un email hors de ALLOWED_EMAILS (src/auth/google.rs).
    let app = TestApp::new().await;
    let page = app.client().get("/login?e=refuse").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(
        page.body
            .contains("Ce compte Google n&#39;a pas accès à dashu.")
    );
    assert!(!page.body.contains(BAD_CREDENTIALS));
}

#[tokio::test]
async fn sixieme_tentative_en_une_minute_bloquee() {
    let app = TestApp::new().await;
    let mut attacker = app.client();

    for attempt in 1..=5 {
        let response = attacker.login(ALICE, "mauvais mot de passe").await;
        assert_eq!(
            response.status,
            StatusCode::UNAUTHORIZED,
            "tentative {attempt}"
        );
    }
    let blocked = attacker.login(ALICE, "mauvais mot de passe").await;
    assert_eq!(blocked.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(blocked.body.contains("Trop de tentatives"));

    // Même le bon mot de passe est refusé pendant le blocage…
    assert_eq!(
        attacker.login(ALICE, PASSWORD).await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
    // … mais le blocage est par adresse IP : un autre visiteur n'est pas touché.
    assert_eq!(
        app.client().login(ALICE, PASSWORD).await.status,
        StatusCode::SEE_OTHER
    );
}

#[tokio::test]
async fn l_identifiant_de_session_change_a_la_connexion() {
    let app = TestApp::new().await;
    let mut alice = app.client();

    alice.get("/login").await;
    let before = alice.session_cookie().cloned().expect("session anonyme");
    alice.login(ALICE, PASSWORD).await;
    let after = alice.session_cookie().cloned().expect("session connectée");
    assert_ne!(before, after);

    // L'ancien identifiant (qu'un attaquant aurait pu fixer) ne donne accès à rien.
    let mut attacker = app.client();
    attacker.set_session_cookie(&before);
    assert_eq!(attacker.get("/").await.status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn deconnexion() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let cookie = alice.session_cookie().cloned().unwrap();

    let response = alice.post("/logout", &[]).await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(response.location(), "/login");
    assert_eq!(alice.get("/").await.status, StatusCode::SEE_OTHER);

    // La session est détruite côté serveur : rejouer l'ancien cookie ne marche pas.
    let mut replay = app.client();
    replay.set_session_cookie(&cookie);
    assert_eq!(replay.get("/").await.status, StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn requete_modifiante_sans_jeton_csrf_refusee() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;

    // Sans jeton : ce qu'enverrait un formulaire hébergé sur un autre site.
    let response = alice.post_without_csrf("/notes", &[]).await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    let response = alice
        .post_without_csrf("/tasks", &[("title", "injectée")])
        .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);

    // Avec un jeton inventé, dans l'en-tête ou dans le formulaire.
    let real = alice.csrf.replace("0".repeat(64));
    assert_eq!(
        alice.post("/notes", &[]).await.status,
        StatusCode::FORBIDDEN
    );
    let forged = "0".repeat(64);
    let response = alice
        .post_without_csrf("/notes", &[("_csrf", forged.as_str())])
        .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);

    // Avec le vrai jeton, la même requête passe.
    alice.csrf = real;
    assert_eq!(
        alice.post("/notes", &[]).await.status,
        StatusCode::SEE_OTHER
    );

    let tasks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(
        tasks, 0,
        "aucune tâche n'a été créée par les requêtes refusées"
    );
}

#[tokio::test]
async fn en_tetes_de_securite() {
    let app = TestApp::new().await;
    let response = app.client().get("/login").await;

    let csp = response.header("content-security-policy");
    assert!(csp.contains("default-src 'self'"), "{csp}");
    assert!(csp.contains("script-src 'self'"), "{csp}");
    assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
    assert!(
        !csp.contains("unsafe-inline") && !csp.contains("unsafe-eval"),
        "{csp}"
    );
    assert_eq!(response.header("x-content-type-options"), "nosniff");
    assert_eq!(response.header("referrer-policy"), "same-origin");
    assert_eq!(response.header("x-frame-options"), "DENY");
}

#[tokio::test]
async fn fichiers_statiques_avec_empreinte_et_cache_d_un_an() {
    let app = TestApp::new().await;
    let mut visitor = app.client();
    let page = visitor.get("/login").await;

    let css =
        common::between(&page.body, "rel=\"stylesheet\" href=\"", "\"").expect("lien vers le CSS");
    assert!(
        css.starts_with("/static/css/app.") && css.ends_with(".css"),
        "{css}"
    );
    assert_ne!(
        css, "/static/css/app.css",
        "le nom doit contenir une empreinte"
    );

    let response = visitor.get(css).await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        response.header("cache-control"),
        "public, max-age=31536000, immutable"
    );
    assert!(response.header("content-type").starts_with("text/css"));
    // Les polices citées dans le CSS portent elles aussi une empreinte.
    assert!(!response.body.contains("/static/fonts/rethink-sans.woff2"));
}

#[tokio::test]
async fn manifeste_pour_l_ecran_d_accueil() {
    let app = TestApp::new().await;
    let mut visitor = app.client();
    let page = visitor.get("/login").await;

    let manifest = common::between(&page.body, "rel=\"manifest\" href=\"", "\"")
        .expect("lien vers le manifeste");
    assert!(
        manifest.starts_with("/static/manifest.") && manifest != "/static/manifest.webmanifest",
        "{manifest}"
    );
    let icon = common::between(&page.body, "rel=\"apple-touch-icon\" href=\"", "\"")
        .expect("icône pour iOS");
    assert_eq!(visitor.get(icon).await.header("content-type"), "image/png");

    let response = visitor.get(manifest).await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.header("content-type"), "application/manifest+json");
    let json: serde_json::Value = serde_json::from_str(&response.body).expect("JSON valide");
    assert_eq!(json["display"], "standalone");
    assert_eq!(json["start_url"], "/");
    // Les icônes citées portent une empreinte et existent.
    for icon in json["icons"].as_array().expect("icônes") {
        let src = icon["src"].as_str().unwrap();
        assert!(
            !src.ends_with("icon-192.png") && !src.ends_with("icon-512.png"),
            "{src}"
        );
        assert_eq!(visitor.get(src).await.status, StatusCode::OK, "{src}");
    }
}

#[tokio::test]
async fn trois_espaces_des_la_premiere_connexion() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;

    let slugs: Vec<String> = sqlx::query_scalar(
        "SELECT s.slug FROM spaces s JOIN users u ON u.id = s.user_id WHERE u.email = ? ORDER BY s.position",
    )
    .bind(ALICE)
    .fetch_all(&app.state.db)
    .await
    .unwrap();
    assert_eq!(slugs, ["perso", "ecole", "taff"]);

    let page = alice.get("/").await;
    for name in ["Perso", "École", "Taff"] {
        assert!(page.body.contains(name), "{name}");
    }
    // Bob ne s'est pas encore connecté : il n'a pas d'espace, et n'en reçoit pas d'Alice.
    let bob_spaces: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM spaces s JOIN users u ON u.id = s.user_id WHERE u.email = ?",
    )
    .bind(BOB)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(bob_spaces, 0);
}

#[tokio::test]
async fn le_changement_d_espace_filtre_le_contenu() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;

    // Une tâche et une note dans l'espace Perso (actif par défaut).
    alice
        .post("/tasks", &[("title", "Arroser les plantes")])
        .await;
    let created = alice.post("/notes", &[]).await;
    let id = common::note_id(&created);
    alice
        .post(
            &format!("/notes/{id}"),
            &[("title", "Liste de courses"), ("body_md", "pain")],
        )
        .await;

    assert!(
        alice
            .get("/widgets/tasks")
            .await
            .body
            .contains("Arroser les plantes")
    );
    assert!(alice.get("/notes").await.body.contains("Liste de courses"));
    assert!(
        alice
            .get("/widgets/notes")
            .await
            .body
            .contains("Liste de courses")
    );

    // Passage à l'espace École : la page revient où on était, sans ce contenu.
    let response = alice
        .post("/space", &[("slug", "ecole"), ("next", "/notes")])
        .await;
    assert_eq!(response.status, StatusCode::SEE_OTHER);
    assert_eq!(response.location(), "/notes");

    let notes = alice.get("/notes").await;
    assert!(notes.body.contains("data-accent=\"blue\""));
    assert!(!notes.body.contains("Liste de courses"));
    assert!(
        !alice
            .get("/widgets/tasks")
            .await
            .body
            .contains("Arroser les plantes")
    );
    assert!(
        !alice
            .get("/widgets/notes")
            .await
            .body
            .contains("Liste de courses")
    );

    // Retour à Perso : tout est là.
    alice.post("/space", &[("slug", "perso")]).await;
    assert!(alice.get("/notes").await.body.contains("Liste de courses"));

    // Un espace inconnu est refusé, et l'URL de retour ne sort jamais du site.
    assert_eq!(
        alice.post("/space", &[("slug", "admin")]).await.status,
        StatusCode::NOT_FOUND
    );
    let response = alice
        .post(
            "/space",
            &[("slug", "taff"), ("next", "https://evil.example/")],
        )
        .await;
    assert_eq!(response.location(), "/");
}

#[tokio::test]
async fn changement_de_mot_de_passe() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let new_password = "un nouveau mot de passe solide";

    // Mauvais mot de passe actuel, puis nouveau trop court : rien ne change.
    alice
        .post(
            "/settings/password",
            &[
                ("current", "faux"),
                ("new", new_password),
                ("confirm", new_password),
            ],
        )
        .await;
    alice
        .post(
            "/settings/password",
            &[
                ("current", PASSWORD),
                ("new", "court"),
                ("confirm", "court"),
            ],
        )
        .await;
    assert_eq!(
        app.client().login(ALICE, PASSWORD).await.status,
        StatusCode::SEE_OTHER
    );

    let response = alice
        .post(
            "/settings/password",
            &[
                ("current", PASSWORD),
                ("new", new_password),
                ("confirm", new_password),
            ],
        )
        .await;
    assert!(
        alice
            .follow(response)
            .await
            .body
            .contains("Mot de passe enregistré<")
    );

    assert_eq!(
        app.client().login(ALICE, PASSWORD).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.client().login(ALICE, new_password).await.status,
        StatusCode::SEE_OTHER
    );
}

#[tokio::test]
async fn theme_enregistre_par_utilisateur() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;

    // Par défaut : pas de thème forcé, le navigateur suit le système.
    assert!(alice.get("/").await.body.contains("<html lang=\"fr\">"));

    alice
        .post("/settings/theme", &[("theme", "light"), ("next", "/")])
        .await;
    assert!(
        alice
            .get("/")
            .await
            .body
            .contains("<html lang=\"fr\" data-theme=\"light\">")
    );

    // Le réglage est lié au compte : il est là sur un autre navigateur.
    let mut elsewhere = app.logged_in(ALICE).await;
    assert!(
        elsewhere
            .get("/")
            .await
            .body
            .contains("data-theme=\"light\"")
    );
    // Et il ne concerne pas Bob.
    let mut bob = app.logged_in(BOB).await;
    assert!(bob.get("/").await.body.contains("<html lang=\"fr\">"));
}
