//! 1Life, outil réservé (SPEC, « Outils par espace », PR 2) : seulement pour
//! les comptes autorisés par `dashu tools grant`, 404 partout ailleurs, puis
//! affiché ou masqué par espace comme les autres outils (masqué par défaut).
//!
//! Dépôt public : aucun nom de client ni aucune information de 1Life ici.

mod common;

use axum::http::{Method, StatusCode};
use common::{ALICE, API_TOKEN, BOB, TestApp, between, command_node};
use serde_json::json;

/// Toutes les adresses de 1Life, avec et sans l'espace.
const PAGES: &[&str] = &[
    "/1life",
    "/1life/cr",
    "/1life/audit",
    "/1life/clients",
    "/1life/taches",
    "/e/taff/1life",
    "/e/taff/1life/cr",
    "/e/taff/1life/audit",
    "/e/taff/1life/clients",
    "/e/taff/1life/taches",
];

/// Identifiants des outils de la barre latérale.
fn sidebar(body: &str) -> Vec<String> {
    let nav = between(body, r#"<div id="nav-tools""#, "</ul>").expect("outils de la barre");
    nav.split(r#"data-tool=""#)
        .skip(1)
        .filter_map(|item| item.split('"').next())
        .map(str::to_string)
        .collect()
}

fn go_words(tree: &serde_json::Value) -> Vec<String> {
    command_node(tree, &["go"])["children"]
        .as_array()
        .expect("/go")
        .iter()
        .filter_map(|n| n["name"].as_str().map(str::to_string))
        .collect()
}

#[tokio::test]
async fn un_compte_non_autorise_recoit_404_partout() {
    let app = TestApp::new().await;
    let mut bob = app.logged_in(BOB).await;
    let mut anonyme = app.client();
    let inexistante = bob.get("/nulle-part").await;
    assert_eq!(inexistante.status, StatusCode::NOT_FOUND);

    for page in PAGES {
        let response = bob.get(page).await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{page}");
        // La même page qu'une adresse qui n'existe pas : rien ne dit que
        // l'outil existe.
        assert_eq!(response.body, inexistante.body, "{page}");
        // Sans session non plus : ni page de connexion, ni redirection.
        let response = anonyme.get(page).await;
        assert_eq!(
            response.status,
            StatusCode::NOT_FOUND,
            "{page} sans session"
        );
    }
    // Un module inconnu, une autre méthode : 404 aussi.
    assert_eq!(
        bob.get("/1life/inconnu").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(bob.post("/1life", &[]).await.status, StatusCode::NOT_FOUND);

    // Ni dans les réglages, ni dans la barre de commandes, ni dans la barre.
    let settings = bob.get("/settings").await;
    assert!(!settings.body.contains("1life") && !settings.body.contains("1Life"));
    let tree = bob.catalogue("/").await;
    assert!(!go_words(&tree).contains(&"1life".to_string()));
    let page = bob.get("/").await;
    assert!(!sidebar(&page.body).contains(&"1life".to_string()));

    // Ni par le formulaire des réglages : comme un outil inconnu.
    let taff = app.space_id(BOB, "taff").await;
    let response = bob
        .post(
            &format!("/settings/spaces/{taff}/tools/1life"),
            &[("shown", "1")],
        )
        .await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    let page = bob.get("/e/taff/").await;
    assert!(!sidebar(&page.body).contains(&"1life".to_string()));
}

#[tokio::test]
async fn grant_puis_revoke_par_la_commande() {
    let app = TestApp::new().await;

    let (ok, out, _) = app.run_binary(&["tools", "list"]);
    assert!(ok);
    assert!(
        out.contains("1life (1Life) : aucun compte autorisé"),
        "{out}"
    );

    let (ok, out, err) = app.run_binary(&["tools", "grant", ALICE, "1life"]);
    assert!(ok, "{err}");
    assert!(out.contains("a maintenant accès à 1life"), "{out}");
    let (ok, out, _) = app.run_binary(&["tools", "grant", ALICE, "1life"]);
    assert!(ok && out.contains("avait déjà accès"), "{out}");
    let (_, out, _) = app.run_binary(&["tools", "list"]);
    assert!(out.contains(&format!("1life (1Life) : {ALICE}")), "{out}");

    // Autorisée : 1Life apparaît dans ses réglages, masqué par défaut.
    let mut alice = app.logged_in(ALICE).await;
    let taff = app.space_id(ALICE, "taff").await;
    let settings = alice.get("/settings").await;
    assert!(
        settings
            .body
            .contains(&format!("/settings/spaces/{taff}/tools/1life"))
    );
    let page = alice.get("/e/taff/").await;
    assert!(!sidebar(&page.body).contains(&"1life".to_string()));
    // Masqué dans l'espace : retour à l'accueil, comme un autre outil.
    let back = alice.get("/e/taff/1life").await;
    assert_eq!(back.status, StatusCode::SEE_OTHER);
    assert_eq!(back.location(), "/e/taff/");

    // Refus : compte inconnu, outil non réservé ou inconnu. Rien n'est écrit.
    for args in [
        ["tools", "grant", "personne@example.test", "1life"],
        ["tools", "grant", BOB, "notes"],
        ["tools", "grant", BOB, "inconnu"],
    ] {
        let (ok, _, err) = app.run_binary(&args);
        assert!(!ok, "{args:?} accepté");
        assert!(!err.is_empty());
    }
    let (_, out, _) = app.run_binary(&["tools", "list"]);
    assert!(!out.contains(BOB));

    let (ok, out, _) = app.run_binary(&["tools", "revoke", ALICE, "1life"]);
    assert!(ok && out.contains("n'a plus accès"), "{out}");
    let (_, out, _) = app.run_binary(&["tools", "list"]);
    assert!(out.contains("aucun compte autorisé"), "{out}");
    for page in PAGES {
        assert_eq!(
            alice.get(page).await.status,
            StatusCode::NOT_FOUND,
            "{page}"
        );
    }
    let settings = alice.get("/settings").await;
    assert!(!settings.body.contains("/tools/1life"));
}

#[tokio::test]
async fn un_compte_autorise_active_1life_par_espace() {
    let app = TestApp::new().await;
    dashu::registry::grant(&app.state.db, ALICE, "1life")
        .await
        .unwrap();
    let mut alice = app.logged_in(ALICE).await;
    let taff = app.space_id(ALICE, "taff").await;

    let shown = alice
        .post(
            &format!("/settings/spaces/{taff}/tools/1life"),
            &[("shown", "1")],
        )
        .await;
    assert_eq!(shown.status, StatusCode::SEE_OTHER);

    // Dans Taff seulement.
    let page = alice.get("/e/taff/").await;
    assert_eq!(
        sidebar(&page.body).last().map(String::as_str),
        Some("1life")
    );
    assert!(page.body.contains(r#"href="/e/taff/1life""#));
    alice.with_header("x-espace", "taff");
    assert!(go_words(&alice.catalogue("/").await).contains(&"1life".to_string()));
    for other in ["perso", "ecole"] {
        let page = alice.get(&format!("/e/{other}/")).await;
        assert!(
            !sidebar(&page.body).contains(&"1life".to_string()),
            "{other}"
        );
        assert_eq!(
            alice.get(&format!("/e/{other}/1life")).await.status,
            StatusCode::SEE_OTHER
        );
    }

    // Les cinq pages s'ouvrent : l'accueil et ses quatre cartes, puis chaque
    // module. Compte rendu, Audit et Tâches sont « Bientôt » (« Module à
    // venir ») ;
    // Clients est fait depuis le 26 novembre 2026 (SPEC, « 1Life — Clients ») :
    // sa page est la liste des clients, éprouvée dans
    // tests/outil_1life_clients.rs. Audit, depuis le 27 novembre 2026
    // (tests/outil_1life_audit.rs).
    let home = alice.get("/e/taff/1life").await;
    assert_eq!(home.status, StatusCode::OK);
    assert!(
        home.body
            .contains(r#"aria-current="page" data-tool="1life""#)
    );
    for (id, name, soon) in [
        ("cr", "Compte rendu", true),
        ("audit", "Audit", false),
        ("clients", "Clients", false),
        ("taches", "Tâches", true),
    ] {
        assert!(
            home.body.contains(&format!(r#"href="/e/taff/1life/{id}""#)),
            "{id}"
        );
        assert!(home.body.contains(name), "{name}");
        let module = alice.get(&format!("/e/taff/1life/{id}")).await;
        assert_eq!(module.status, StatusCode::OK, "{id}");
        assert_eq!(module.body.contains("Module à venir"), soon, "{id}");
        assert!(module.body.contains(name), "{id}");
        if id == "clients" {
            // Sans dossier des clients choisi : l'état vide de Clients
            // (SPEC, « 1Life — Clients »).
            assert!(
                module.body.contains("Choisir le dossier des clients"),
                "{id}"
            );
        }
    }
    // Compte rendu et Tâches.
    assert_eq!(home.body.matches("Bientôt").count(), 2);

    // Bob, non autorisé, ne le voit toujours pas.
    let mut bob = app.logged_in(BOB).await;
    assert_eq!(bob.get("/e/taff/1life").await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn l_api_ne_donne_pas_l_acces_et_traite_1life_comme_un_outil_inconnu() {
    let app = TestApp::new().await;
    let token = Some(API_TOKEN);
    // L'API agit au nom d'Alice, non autorisée.
    let (status, tools) = app
        .api(Method::GET, "/api/v1/spaces/taff/tools", None, token)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!tools.to_string().contains("1life"));
    let (status, refused) = app
        .api(
            Method::PUT,
            "/api/v1/spaces/taff/tools",
            Some(json!({ "tools": { "1life": true } })),
            token,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(refused["error"], "outil inconnu : 1life");
    let (status, unknown) = app
        .api(
            Method::PUT,
            "/api/v1/spaces/taff/tools",
            Some(json!({ "tools": { "inconnu": true } })),
            token,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(unknown["error"], "outil inconnu : inconnu");
    // Aucune route ne parle d'autorisation.
    for path in [
        "/api/v1/tools",
        "/api/v1/tools/1life/grants",
        "/api/v1/grants",
    ] {
        let (status, _) = app.api(Method::POST, path, Some(json!({})), token).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM tool_grants")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(rows, 0);

    // Autorisée, Alice règle 1Life par l'API comme un autre outil.
    dashu::registry::grant(&app.state.db, ALICE, "1life")
        .await
        .unwrap();
    let (status, tools) = app
        .api(
            Method::PUT,
            "/api/v1/spaces/taff/tools",
            Some(json!({ "tools": { "1life": true } })),
            token,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{tools}");
    let onelife = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["tool"] == "1life")
        .cloned()
        .unwrap();
    assert_eq!(onelife["shown"], true);
}
