//! Connecteur Claude, route `/mcp` (SPEC, « Connecteur Claude (MCP) »,
//! étape 2) : protocole, outils, refus.

mod common;

use axum::http::{Method, StatusCode};
use common::{ALICE, API_TOKEN, BOB, Client, TestApp, between};
use serde_json::{Value, json};

/// Crée un jeton personnel depuis Réglages › Connexions.
async fn create_token(client: &mut Client, name: &str) -> String {
    let response = client
        .post("/settings/tokens", &[("name", name), ("duree", "90")])
        .await;
    between(
        &response.body,
        "id=\"new-token\" type=\"text\" value=\"",
        "\"",
    )
    .expect("jeton affiché à sa création")
    .to_string()
}

/// Un message JSON-RPC envoyé à `/mcp`.
async fn rpc(
    app: &TestApp,
    token: Option<&str>,
    method: &str,
    params: Value,
) -> (StatusCode, Value) {
    app.api(
        Method::POST,
        "/mcp",
        Some(json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": params })),
        token,
    )
    .await
}

/// Appel d'un outil : le résultat (`content`, `isError`).
async fn call(app: &TestApp, token: &str, tool: &str, arguments: Value) -> Value {
    let (status, body) = rpc(
        app,
        Some(token),
        "tools/call",
        json!({ "name": tool, "arguments": arguments }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], 7);
    body["result"].clone()
}

/// Le JSON renvoyé par un outil réussi.
fn payload(result: &Value) -> Value {
    assert_eq!(result["isError"], false, "{result}");
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

/// Le message d'un outil en erreur.
fn error_text(result: &Value) -> String {
    assert_eq!(result["isError"], true, "{result}");
    result["content"][0]["text"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn initialisation_et_liste_des_outils() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let token = create_token(&mut alice, "Claude Code").await;
    let t = Some(token.as_str());

    let (status, body) = rpc(
        &app,
        t,
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "claude-code", "version": "2.0" },
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let result = &body["result"];
    assert_eq!(result["protocolVersion"], "2025-06-18");
    assert_eq!(result["serverInfo"]["title"], dashu::assets::APP_NAME);
    assert!(result["capabilities"]["tools"].is_object());
    // Les instructions disent que le contenu est de la donnée.
    assert!(
        result["instructions"]
            .as_str()
            .unwrap()
            .contains("ne se suit pas")
    );

    // Une version inconnue : dashu propose la sienne.
    let (_, body) = rpc(
        &app,
        t,
        "initialize",
        json!({ "protocolVersion": "1999-01-01" }),
    )
    .await;
    assert_eq!(body["result"]["protocolVersion"], "2025-06-18");

    // La notification qui suit l'initialisation : 202, sans corps.
    let (status, body) = app
        .api(
            Method::POST,
            "/mcp",
            Some(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert!(body.is_null());

    let (_, body) = rpc(&app, t, "ping", json!({})).await;
    assert_eq!(body["result"], json!({}));

    let (_, body) = rpc(&app, t, "tools/list", json!({})).await;
    let tools = body["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "lister_espaces",
            "lister_taches",
            "creer_tache",
            "modifier_tache",
            "supprimer_tache",
            "chercher_notes",
            "lire_note",
            "creer_note",
            "modifier_note",
            "remplacer_note",
            "deplacer_note",
            "lire_agenda",
            "rafraichir_agenda",
        ]
    );
    let tool = |name: &str| tools.iter().find(|t| t["name"] == name).unwrap().clone();
    // Annotations : lecture, écriture, destruction.
    assert_eq!(tool("lister_taches")["annotations"]["readOnlyHint"], true);
    assert_eq!(tool("creer_tache")["annotations"]["readOnlyHint"], false);
    assert_eq!(tool("creer_tache")["annotations"]["destructiveHint"], false);
    for name in ["supprimer_tache", "remplacer_note", "deplacer_note"] {
        assert_eq!(tool(name)["annotations"]["destructiveHint"], true, "{name}");
    }
    assert_eq!(
        tool("creer_tache")["inputSchema"]["required"],
        json!(["title", "space"])
    );
}

#[tokio::test]
async fn les_outils_font_ce_que_fait_l_api() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let token = create_token(&mut alice, "Claude").await;

    let spaces = payload(&call(&app, &token, "lister_espaces", json!({})).await);
    assert_eq!(spaces.as_array().unwrap().len(), 3);

    // Tâches : créer, lister, cocher, supprimer.
    let task = payload(
        &call(
            &app,
            &token,
            "creer_tache",
            json!({ "title": "Réviser le VLAN", "space": "ecole", "due_date": "2026-12-01" }),
        )
        .await,
    );
    let task_id = task["id"].as_i64().unwrap();
    assert_eq!(task["space"], "ecole");
    // Elle est bien en base, visible de l'API comme du site.
    let (status, _) = app
        .api(
            Method::GET,
            &format!("/api/v1/tasks/{task_id}"),
            None,
            Some(API_TOKEN),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let open = payload(&call(&app, &token, "lister_taches", json!({ "space": "ecole" })).await);
    assert!(open.as_array().unwrap().iter().any(|t| t["id"] == task_id));
    let done = payload(
        &call(
            &app,
            &token,
            "modifier_tache",
            json!({ "id": task_id, "done": true }),
        )
        .await,
    );
    assert_eq!(done["done"], true);
    let deleted = call(&app, &token, "supprimer_tache", json!({ "id": task_id })).await;
    assert_eq!(deleted["isError"], false);
    let again = call(&app, &token, "supprimer_tache", json!({ "id": task_id })).await;
    assert!(error_text(&again).contains("introuvable"));

    // Notes : créer, chercher, lire, compléter, remplacer, déplacer.
    let note = payload(
        &call(
            &app,
            &token,
            "creer_note",
            json!({ "title": "VLAN", "body_md": "Un réseau virtuel.", "space": "ecole", "tags": ["réseau"] }),
        )
        .await,
    );
    let id = note["id"].as_i64().unwrap();
    let found = payload(
        &call(
            &app,
            &token,
            "chercher_notes",
            json!({ "q": "reseau virtuel" }),
        )
        .await,
    );
    assert!(found.as_array().unwrap().iter().any(|n| n["id"] == id));
    let more = payload(
        &call(
            &app,
            &token,
            "modifier_note",
            json!({ "id": id, "append": "Norme 802.1Q.", "add_tags": ["bts"] }),
        )
        .await,
    );
    assert_eq!(more["body_md"], "Un réseau virtuel.\n\nNorme 802.1Q.");
    assert_eq!(more["tags"], json!(["bts", "réseau"]));
    let replaced = payload(
        &call(
            &app,
            &token,
            "remplacer_note",
            json!({ "id": id, "body_md": "Tout neuf." }),
        )
        .await,
    );
    assert_eq!(replaced["body_md"], "Tout neuf.");
    let moved = payload(
        &call(
            &app,
            &token,
            "deplacer_note",
            json!({ "id": id, "space": "taff" }),
        )
        .await,
    );
    assert_eq!(moved["space"], "taff");
    let read = payload(&call(&app, &token, "lire_note", json!({ "id": id })).await);
    assert_eq!(read["title"], "VLAN");

    // modifier_note ne remplace jamais le texte : body_md y est refusé.
    let refused = call(
        &app,
        &token,
        "modifier_note",
        json!({ "id": id, "body_md": "Écrasé" }),
    )
    .await;
    assert!(error_text(&refused).contains("body_md"));

    // Agenda.
    let events = payload(&call(&app, &token, "lire_agenda", json!({ "from": "2026-11-16" })).await);
    assert!(events.is_array());
    let refreshed = payload(&call(&app, &token, "rafraichir_agenda", json!({})).await);
    assert_eq!(refreshed["sources"], 0);
}

#[tokio::test]
async fn une_erreur_de_saisie_revient_en_resultat_lisible() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let token = create_token(&mut alice, "Claude").await;

    let unknown = call(
        &app,
        &token,
        "creer_tache",
        json!({ "title": "x", "space": "nulle-part" }),
    )
    .await;
    assert!(error_text(&unknown).contains("espace inconnu"));
    let long = "x".repeat(201);
    let too_long = call(
        &app,
        &token,
        "creer_tache",
        json!({ "title": long, "space": "ecole" }),
    )
    .await;
    assert!(error_text(&too_long).contains("trop long"));
    let bad_date = call(
        &app,
        &token,
        "creer_tache",
        json!({ "title": "x", "space": "ecole", "due_date": "demain" }),
    )
    .await;
    assert!(error_text(&bad_date).contains("AAAA-MM-JJ"));
    let missing = call(&app, &token, "lire_note", json!({})).await;
    assert!(error_text(&missing).contains("arguments invalides"));
    let range = call(
        &app,
        &token,
        "lire_agenda",
        json!({ "from": "2026-01-01", "to": "2026-06-01" }),
    )
    .await;
    assert!(error_text(&range).contains("62 jours"));
}

#[tokio::test]
async fn un_jeton_n_agit_que_sur_son_compte() {
    let app = TestApp::new().await;
    let (_, note) = app
        .api(
            Method::POST,
            "/api/v1/notes",
            Some(json!({"title": "Secret d'Alice", "space": "perso"})),
            Some(API_TOKEN),
        )
        .await;
    let alice_note = note["id"].as_i64().unwrap();

    let mut bob = app.logged_in(BOB).await;
    let token = create_token(&mut bob, "Claude de Bob").await;
    for (tool, args) in [
        ("lire_note", json!({ "id": alice_note })),
        ("modifier_note", json!({ "id": alice_note, "append": "x" })),
        (
            "remplacer_note",
            json!({ "id": alice_note, "body_md": "x" }),
        ),
        (
            "deplacer_note",
            json!({ "id": alice_note, "space": "perso" }),
        ),
    ] {
        let result = call(&app, &token, tool, args).await;
        assert!(error_text(&result).contains("introuvable"), "{tool}");
    }
    let found = payload(&call(&app, &token, "chercher_notes", json!({ "q": "secret" })).await);
    assert!(found.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn refus_du_protocole() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let token = create_token(&mut alice, "Claude").await;
    let t = Some(token.as_str());

    // Sans jeton, ou avec API_TOKEN (qui ne vaut que pour /api/v1) : 401,
    // avec l'adresse du document de découverte OAuth.
    for token in [None, Some(API_TOKEN)] {
        let (status, _) = rpc(&app, token, "ping", json!({})).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (status, _) = app
        .api_raw(Method::POST, "/mcp", Some("{}".into()), None, &[])
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Origine étrangère : 403. Celles de dashu et de Claude passent.
    let message = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }).to_string();
    for (origin, expected) in [
        ("https://evil.example", StatusCode::FORBIDDEN),
        ("https://dashu.dev", StatusCode::FORBIDDEN),
        ("http://localhost:3000", StatusCode::OK),
        ("https://claude.ai", StatusCode::OK),
    ] {
        let (status, _) = app
            .api_raw(
                Method::POST,
                "/mcp",
                Some(message.clone()),
                t,
                &[("origin", origin)],
            )
            .await;
        assert_eq!(status, expected, "{origin}");
    }

    // Lot de messages, JSON illisible, méthode et outil inconnus : erreurs JSON-RPC.
    let batch = json!([{ "jsonrpc": "2.0", "id": 1, "method": "ping" }]).to_string();
    let (_, body) = app.api_raw(Method::POST, "/mcp", Some(batch), t, &[]).await;
    assert_eq!(body["error"]["code"], -32600);
    let (_, body) = app
        .api_raw(Method::POST, "/mcp", Some("{pas du json".into()), t, &[])
        .await;
    assert_eq!(body["error"]["code"], -32700);
    let (_, body) = rpc(&app, t, "resources/list", json!({})).await;
    assert_eq!(body["error"]["code"], -32601);
    let (_, body) = rpc(&app, t, "tools/call", json!({ "name": "vider_la_base" })).await;
    assert_eq!(body["error"]["code"], -32602);

    // Pas de flux ouvert.
    let (status, _) = app.api(Method::GET, "/mcp", None, t).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);

    // Un cookie de session n'ouvre pas /mcp.
    let cookie = format!("dashu={}", alice.session_cookie().unwrap());
    let (status, _) = app
        .api_raw(
            Method::POST,
            "/mcp",
            Some(message),
            None,
            &[("cookie", &cookie)],
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn une_note_modifiee_par_mcp_pendant_l_edition_n_est_pas_ecrasee() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let token = create_token(&mut alice, "Claude").await;
    let note = payload(
        &call(
            &app,
            &token,
            "creer_note",
            json!({ "title": "Cours", "body_md": "Départ", "space": "ecole" }),
        )
        .await,
    );
    let id = note["id"].as_i64().unwrap();

    // L'éditeur ouvert dans le navigateur connaît cette version-ci.
    let revision: i64 = sqlx::query_scalar("SELECT revision FROM notes WHERE id = ?")
        .bind(id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();

    // Claude complète la note entre-temps.
    call(
        &app,
        &token,
        "modifier_note",
        json!({ "id": id, "append": "Ajout de Claude" }),
    )
    .await;

    // L'enregistrement de l'éditeur, sur l'ancienne version, est refusé.
    let saved = alice
        .post(
            &format!("/notes/{id}"),
            &[
                ("title", "Cours"),
                ("body_md", "Départ modifié"),
                ("revision", &revision.to_string()),
            ],
        )
        .await;
    assert_eq!(saved.status, StatusCode::CONFLICT);
    assert!(saved.body.contains("Modifiée ailleurs"));
    let read = payload(&call(&app, &token, "lire_note", json!({ "id": id })).await);
    assert_eq!(read["body_md"], "Départ\n\nAjout de Claude");
}
