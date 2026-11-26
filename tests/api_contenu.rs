//! API, lot « contenu » (SPEC, « API complète et corbeille ») : notes au
//! complet, fichiers, liens de partage, dossiers, corbeille. Pour chaque
//! route : le cas normal, un élément d'un autre compte (404), une saisie
//! invalide. Et les mêmes actions par le connecteur Claude.

mod common;

use axum::http::{Method, StatusCode};
use common::{ALICE, BOB, Client, TestApp, between};
use serde_json::{Value, json};

/// Un jeton personnel, créé dans Réglages › Connexions.
async fn token(app: &TestApp, email: &str, name: &str) -> String {
    let mut client: Client = app.logged_in(email).await;
    let response = client
        .post("/settings/tokens", &[("name", name), ("duree", "90")])
        .await;
    between(
        &response.body,
        "id=\"new-token\" type=\"text\" value=\"",
        "\"",
    )
    .expect("jeton affiché")
    .to_string()
}

/// Envoie un fichier par l'API, en deux morceaux.
async fn upload(app: &TestApp, t: &str, name: &str, content: &[u8], place: Value) -> Value {
    let mut body = json!({ "name": name, "size": content.len() });
    for (k, v) in place.as_object().unwrap() {
        body[k] = v.clone();
    }
    let (status, started) = app
        .api(Method::POST, "/api/v1/files/uploads", Some(body), Some(t))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{started}");
    let id = started["upload"].as_str().unwrap().to_string();
    let half = content.len() / 2;
    let (status, _, first) = app
        .api_bytes(
            Method::PUT,
            &format!("/api/v1/files/uploads/{id}?offset=0"),
            Some(content[..half].to_vec()),
            Some(t),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&first)
    );
    let (status, _, done) = app
        .api_bytes(
            Method::PUT,
            &format!("/api/v1/files/uploads/{id}?offset={half}"),
            Some(content[half..].to_vec()),
            Some(t),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&done)
    );
    serde_json::from_slice(&done).unwrap()
}

#[tokio::test]
async fn fichiers_envoi_lecture_rangement_et_suppression() {
    let app = TestApp::new().await;
    let tk = token(&app, ALICE, "Script").await;
    let t = Some(tk.as_str());
    let content = b"%PDF-1.4 un devis assez long pour deux morceaux";

    // Envoi en morceaux ; un morceau hors d'ordre dit où reprendre.
    let (_, started) = app
        .api(
            Method::POST,
            "/api/v1/files/uploads",
            Some(json!({ "name": "devis.pdf", "size": content.len(), "space": "taff" })),
            t,
        )
        .await;
    let upload_id = started["upload"].as_str().unwrap();
    let (status, _, body) = app
        .api_bytes(
            Method::PUT,
            &format!("/api/v1/files/uploads/{upload_id}?offset=10"),
            Some(content.to_vec()),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["received"],
        0
    );
    let file = upload(&app, &tk, "devis.pdf", content, json!({ "space": "taff" })).await;
    let id = file["id"].as_i64().unwrap();
    assert_eq!(file["space"], "taff");
    assert_eq!(file["size"], content.len());
    assert_eq!(file["temporary"], false);

    // Téléchargé identique, en pièce jointe, jamais exécuté.
    let (status, headers, bytes) = app
        .api_bytes(
            Method::GET,
            &format!("/api/v1/files/{id}/download"),
            None,
            t,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, content);
    assert!(
        headers["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment")
    );
    assert!(
        headers["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("sandbox")
    );

    // Renommer (l'extension reste), ranger dans un dossier, taguer.
    let (_, folder) = app
        .api(
            Method::POST,
            "/api/v1/folders",
            Some(json!({ "name": "Clients", "space": "taff" })),
            t,
        )
        .await;
    let folder_id = folder["id"].as_i64().unwrap();
    let (status, edited) = app
        .api(
            Method::PATCH,
            &format!("/api/v1/files/{id}"),
            Some(json!({ "name": "Devis signé.exe", "folder": folder_id, "add_tags": ["client", "2026"] })),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(edited["name"], "Devis signé.exe.pdf");
    assert_eq!(edited["folder"], folder_id);
    assert_eq!(edited["tags"], json!(["2026", "client"]));
    let (_, edited) = app
        .api(
            Method::PATCH,
            &format!("/api/v1/files/{id}"),
            Some(json!({ "remove_tags": ["2026"], "space": "ecole" })),
            t,
        )
        .await;
    assert_eq!(edited["space"], "ecole");
    assert_eq!(edited["folder"], Value::Null);
    assert_eq!(edited["tags"], json!(["client"]));

    // Lister et chercher.
    for query in ["?q=devis", "?tag=client", "?space=ecole"] {
        let (_, list) = app
            .api(Method::GET, &format!("/api/v1/files{query}"), None, t)
            .await;
        assert!(
            list.as_array().unwrap().iter().any(|f| f["id"] == id),
            "{query}"
        );
    }
    let (_, none) = app
        .api(Method::GET, "/api/v1/files?space=perso", None, t)
        .await;
    assert_eq!(none, json!([]));

    // Saisies invalides.
    for (body, expected) in [
        (json!({ "name": "  " }), StatusCode::BAD_REQUEST),
        (json!({ "space": "nulle-part" }), StatusCode::BAD_REQUEST),
        (
            json!({ "keep": true, "space": "perso" }),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let (status, _) = app
            .api(
                Method::PATCH,
                &format!("/api/v1/files/{id}"),
                Some(body.clone()),
                t,
            )
            .await;
        assert_eq!(status, expected, "{body}");
    }
    let (status, _) = app
        .api(
            Method::POST,
            "/api/v1/files/uploads",
            Some(json!({ "name": "gros.bin", "size": 10_000_000, "space": "perso" })),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let (status, _) = app
        .api(
            Method::POST,
            "/api/v1/files/uploads",
            Some(json!({ "name": "sans.txt", "size": 3 })),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Un autre compte : 404 partout.
    let bob = token(&app, BOB, "Bob").await;
    let b = Some(bob.as_str());
    for (method, path) in [
        (Method::GET, format!("/api/v1/files/{id}")),
        (Method::PATCH, format!("/api/v1/files/{id}")),
        (Method::DELETE, format!("/api/v1/files/{id}")),
        (Method::GET, format!("/api/v1/files/{id}/shares")),
    ] {
        let (status, _) = app
            .api(
                method.clone(),
                &path,
                (method == Method::PATCH).then(|| json!({ "name": "x" })),
                b,
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
    }
    let (status, _, _) = app
        .api_bytes(
            Method::GET,
            &format!("/api/v1/files/{id}/download"),
            None,
            b,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Supprimer : dans la corbeille, avec le nom du jeton comme trace.
    let (status, _) = app
        .api(Method::DELETE, &format!("/api/v1/files/{id}"), None, t)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = app
        .api(Method::GET, &format!("/api/v1/files/{id}"), None, t)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, trash) = app.api(Method::GET, "/api/v1/trash", None, t).await;
    let entry = trash
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == id)
        .expect("dans la corbeille")
        .clone();
    assert_eq!(entry["kind"], "fichier");
    assert_eq!(entry["via"], "Script");
    let (status, restored) = app
        .api(
            Method::POST,
            &format!("/api/v1/trash/fichier/{id}/restore"),
            None,
            t,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{restored}");
    let (status, _) = app
        .api(Method::GET, &format!("/api/v1/files/{id}"), None, t)
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn temporaire_garde_ou_efface() {
    let app = TestApp::new().await;
    let tk = token(&app, ALICE, "Script").await;
    let t = Some(tk.as_str());
    let kept = upload(
        &app,
        &tk,
        "photo.jpg",
        b"jpeg jpeg",
        json!({ "temporary": true }),
    )
    .await;
    assert_eq!(kept["temporary"], true);
    let (_, list) = app
        .api(Method::GET, "/api/v1/files?temporary=true", None, t)
        .await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    let id = kept["id"].as_i64().unwrap();
    let (status, moved) = app
        .api(
            Method::PATCH,
            &format!("/api/v1/files/{id}"),
            Some(json!({ "space": "perso" })),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{moved}");
    let (_, kept) = app
        .api(
            Method::PATCH,
            &format!("/api/v1/files/{id}"),
            Some(json!({ "keep": true, "space": "perso" })),
            t,
        )
        .await;
    assert_eq!(kept["temporary"], false);
    assert_eq!(kept["space"], "perso");

    // Un fichier de Temporaire supprimé est effacé, pas mis dans la corbeille.
    let gone = upload(
        &app,
        &tk,
        "brouillon.txt",
        b"abc",
        json!({ "temporary": true }),
    )
    .await;
    let gone_id = gone["id"].as_i64().unwrap();
    app.api(Method::DELETE, &format!("/api/v1/files/{gone_id}"), None, t)
        .await;
    let (_, trash) = app.api(Method::GET, "/api/v1/trash", None, t).await;
    assert_eq!(trash, json!([]));
}

#[tokio::test]
async fn liens_de_partage() {
    let app = TestApp::new().await;
    let tk = token(&app, ALICE, "Script").await;
    let t = Some(tk.as_str());
    let file = upload(
        &app,
        &tk,
        "cours.pdf",
        b"%PDF cours",
        json!({ "space": "ecole" }),
    )
    .await;
    let id = file["id"].as_i64().unwrap();

    let (status, link) = app
        .api(
            Method::POST,
            &format!("/api/v1/files/{id}/shares"),
            Some(json!({ "duration": "1j" })),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{link}");
    let url = link["url"].as_str().unwrap();
    assert!(url.starts_with("http://localhost:3000/s/"));
    let path = &url["http://localhost:3000".len()..];
    let mut stranger = app.client();
    assert_eq!(stranger.get(path).await.status, StatusCode::OK);

    let (_, links) = app
        .api(Method::GET, &format!("/api/v1/files/{id}/shares"), None, t)
        .await;
    assert_eq!(links.as_array().unwrap().len(), 1);
    assert!(links[0].get("url").is_none(), "l'adresse ne se relit pas");

    let (status, _) = app
        .api(
            Method::POST,
            &format!("/api/v1/files/{id}/shares"),
            Some(json!({ "duration": "1an" })),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let bob = token(&app, BOB, "Bob").await;
    let link_id = link["id"].as_i64().unwrap();
    let (status, _) = app
        .api(
            Method::DELETE,
            &format!("/api/v1/files/{id}/shares/{link_id}"),
            None,
            Some(&bob),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = app
        .api(
            Method::DELETE,
            &format!("/api/v1/files/{id}/shares/{link_id}"),
            None,
            t,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(stranger.get(path).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn dossiers_creer_renommer_deplacer_supprimer() {
    let app = TestApp::new().await;
    let tk = token(&app, ALICE, "Script").await;
    let t = Some(tk.as_str());
    let (status, math) = app
        .api(
            Method::POST,
            "/api/v1/folders",
            Some(json!({ "name": "Math", "space": "ecole" })),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{math}");
    let math_id = math["id"].as_i64().unwrap();
    let (_, exos) = app
        .api(
            Method::POST,
            "/api/v1/folders",
            Some(json!({ "name": "Exercices", "parent": math_id })),
            t,
        )
        .await;
    assert_eq!(exos["space"], "ecole");
    assert_eq!(exos["parent"], math_id);
    let exos_id = exos["id"].as_i64().unwrap();

    // Refus : nom pris, ni espace ni parent, nom vide. (Le niveau de trop,
    // au-delà de cinq niveaux de dossiers, est éprouvé dans
    // tests/dossiers.rs, `l_api_et_le_connecteur_suivent_les_cinq_niveaux`.)
    for body in [
        json!({ "name": "math", "space": "ecole" }),
        json!({ "name": "Perdu" }),
        json!({ "name": "  ", "space": "ecole" }),
    ] {
        let (status, _) = app
            .api(Method::POST, "/api/v1/folders", Some(body.clone()), t)
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }

    // Une note rangée dedans.
    let (_, note) = app
        .api(
            Method::POST,
            "/api/v1/notes",
            Some(json!({ "title": "Théorème", "space": "ecole" })),
            t,
        )
        .await;
    let note_id = note["id"].as_i64().unwrap();
    let (status, _) = app
        .api(
            Method::PATCH,
            &format!("/api/v1/notes/{note_id}"),
            Some(json!({ "folder": exos_id })),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // Renommer, puis emmener dans Taff avec son contenu.
    let (_, renamed) = app
        .api(
            Method::PATCH,
            &format!("/api/v1/folders/{math_id}"),
            Some(json!({ "name": "Maths" })),
            t,
        )
        .await;
    assert_eq!(renamed["name"], "Maths");
    let (status, moved) = app
        .api(
            Method::PATCH,
            &format!("/api/v1/folders/{math_id}"),
            Some(json!({ "space": "taff" })),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    assert_eq!(moved["space"], "taff");
    let (_, note) = app
        .api(Method::GET, &format!("/api/v1/notes/{note_id}"), None, t)
        .await;
    assert_eq!(note["space"], "taff");
    // Un dossier ne va pas dans un de ses sous-dossiers.
    let (_, other) = app
        .api(
            Method::POST,
            "/api/v1/folders",
            Some(json!({ "name": "Autre", "space": "taff" })),
            t,
        )
        .await;
    assert!(other["id"].is_i64(), "{other}");
    let (status, refused) = app
        .api(
            Method::PATCH,
            &format!("/api/v1/folders/{math_id}"),
            Some(json!({ "parent": exos_id })),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert!(
        refused
            .to_string()
            .contains("Un dossier ne va pas dans un de ses sous-dossiers.")
    );

    let (_, list) = app
        .api(Method::GET, "/api/v1/folders?space=taff", None, t)
        .await;
    assert_eq!(list.as_array().unwrap().len(), 3);

    // Un autre compte ne voit ni ne touche.
    let bob = token(&app, BOB, "Bob").await;
    let (_, bobs) = app
        .api(Method::GET, "/api/v1/folders", None, Some(&bob))
        .await;
    assert_eq!(bobs, json!([]));
    for method in [Method::PATCH, Method::DELETE] {
        let (status, _) = app
            .api(
                method.clone(),
                &format!("/api/v1/folders/{math_id}"),
                (method == Method::PATCH).then(|| json!({ "name": "x" })),
                Some(&bob),
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method}");
    }

    // Supprimer : dans la corbeille avec tout son contenu, puis restaurer.
    let (status, _) = app
        .api(
            Method::DELETE,
            &format!("/api/v1/folders/{math_id}"),
            None,
            t,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = app
        .api(Method::GET, &format!("/api/v1/notes/{note_id}"), None, t)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, trash) = app.api(Method::GET, "/api/v1/trash", None, t).await;
    assert_eq!(trash[0]["kind"], "dossier");
    assert_eq!(trash[0]["contents"], "1 note, 1 sous-dossier");
    let (status, _) = app
        .api(
            Method::POST,
            &format!("/api/v1/trash/dossier/{math_id}/restore"),
            None,
            Some(&bob),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    app.api(
        Method::POST,
        &format!("/api/v1/trash/dossier/{math_id}/restore"),
        None,
        t,
    )
    .await;
    let (status, _) = app
        .api(Method::GET, &format!("/api/v1/notes/{note_id}"), None, t)
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = app
        .api(Method::POST, "/api/v1/trash/truc/1/restore", None, t)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn notes_ranger_detaguer_telecharger_supprimer() {
    let app = TestApp::new().await;
    let t = token(&app, ALICE, "Claude Code du Mac").await;
    let t = Some(t.as_str());
    let (_, note) = app
        .api(
            Method::POST,
            "/api/v1/notes",
            Some(json!({ "title": "Réseaux", "body_md": "VLAN et **trunk**", "space": "ecole", "tags": ["bts", "brouillon"] })),
            t,
        )
        .await;
    let id = note["id"].as_i64().unwrap();
    let (_, folder) = app
        .api(
            Method::POST,
            "/api/v1/folders",
            Some(json!({ "name": "Cours", "space": "ecole" })),
            t,
        )
        .await;
    let (_, filed) = app
        .api(
            Method::PATCH,
            &format!("/api/v1/notes/{id}"),
            Some(json!({ "folder": folder["id"], "remove_tags": ["brouillon"] })),
            t,
        )
        .await;
    assert_eq!(filed["tags"], json!(["bts"]));
    let in_folder: Option<i64> = sqlx::query_scalar("SELECT folder_id FROM notes WHERE id = ?")
        .bind(id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(in_folder, folder["id"].as_i64());
    app.api(
        Method::PATCH,
        &format!("/api/v1/notes/{id}"),
        Some(json!({ "folder": null })),
        t,
    )
    .await;
    let at_root: Option<i64> = sqlx::query_scalar("SELECT folder_id FROM notes WHERE id = ?")
        .bind(id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(at_root, None);
    // Un dossier d'un autre espace : refusé.
    let (_, elsewhere) = app
        .api(
            Method::POST,
            "/api/v1/folders",
            Some(json!({ "name": "Ailleurs", "space": "taff" })),
            t,
        )
        .await;
    let (status, _) = app
        .api(
            Method::PATCH,
            &format!("/api/v1/notes/{id}"),
            Some(json!({ "folder": elsewhere["id"] })),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Le Markdown, en pièce jointe.
    let (status, headers, bytes) = app
        .api_bytes(
            Method::GET,
            &format!("/api/v1/notes/{id}/download.md"),
            None,
            t,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers["content-disposition"]
            .to_str()
            .unwrap()
            .contains("attachment")
    );
    assert_eq!(
        String::from_utf8(bytes).unwrap(),
        "# Réseaux\n\nVLAN et **trunk**\n"
    );

    // Un autre compte.
    let bob = token(&app, BOB, "Bob").await;
    let (status, _) = app
        .api(
            Method::DELETE,
            &format!("/api/v1/notes/{id}"),
            None,
            Some(&bob),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = app
        .api_bytes(
            Method::GET,
            &format!("/api/v1/notes/{id}/download.md"),
            None,
            Some(&bob),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Supprimer : dans la corbeille ; la page du site dit par où.
    let (status, _) = app
        .api(Method::DELETE, &format!("/api/v1/notes/{id}"), None, t)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let mut alice = app.logged_in(ALICE).await;
    let trash = alice.get("/settings/trash").await;
    assert!(trash.body.contains("Réseaux"));
    assert!(trash.body.contains("depuis Claude Code du Mac"));
}

#[tokio::test]
async fn un_membre_supprime_dans_l_espace_partage_et_le_proprietaire_restaure() {
    let app = TestApp::new().await;
    // Les espaces naissent à la première connexion (celle qui crée le jeton).
    let alice = token(&app, ALICE, "Alice").await;
    let bob = token(&app, BOB, "Bob").await;
    let slug = app.share_space(ALICE, "ecole", BOB).await;
    let (_, note) = app
        .api(
            Method::POST,
            "/api/v1/notes",
            Some(json!({ "title": "Commune", "space": "ecole" })),
            Some(&alice),
        )
        .await;
    let id = note["id"].as_i64().unwrap();
    let (status, _) = app
        .api(
            Method::DELETE,
            &format!("/api/v1/notes/{id}"),
            None,
            Some(&bob),
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, trash) = app
        .api(Method::GET, "/api/v1/trash", None, Some(&alice))
        .await;
    assert_eq!(trash[0]["via"], "Bob");
    assert_eq!(trash[0]["deleted_by"], "bob@example.test");
    let (status, _) = app
        .api(
            Method::POST,
            &format!("/api/v1/trash/note/{id}/restore"),
            None,
            Some(&alice),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, found) = app
        .api(
            Method::GET,
            &format!("/api/v1/notes?space={slug}"),
            None,
            Some(&bob),
        )
        .await;
    assert!(found.as_array().unwrap().iter().any(|n| n["id"] == id));
}

/// Un appel d'outil MCP : le résultat (`content`, `isError`).
async fn tool(app: &TestApp, t: &str, name: &str, arguments: Value) -> Value {
    let (status, body) = app
        .api(
            Method::POST,
            "/mcp",
            Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                         "params": { "name": name, "arguments": arguments } })),
            Some(t),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["result"].clone()
}

fn payload(result: &Value) -> Value {
    assert_eq!(result["isError"], false, "{result}");
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn les_outils_mcp_du_contenu() {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    let app = TestApp::new().await;
    let t = token(&app, ALICE, "Claude").await;

    let (_, list) = app
        .api(
            Method::POST,
            "/mcp",
            Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })),
            Some(&t),
        )
        .await;
    let tools = list["result"]["tools"].as_array().unwrap();
    let destructive = |name: &str| {
        tools.iter().find(|t| t["name"] == name).unwrap()["annotations"]["destructiveHint"].clone()
    };
    for name in [
        "supprimer_note",
        "supprimer_fichier",
        "supprimer_dossier",
        "creer_lien",
    ] {
        assert_eq!(destructive(name), true, "{name}");
    }

    // Envoyer puis relire un fichier, en base64.
    let sent = payload(
        &tool(
            &app,
            &t,
            "envoyer_fichier",
            json!({ "name": "liste.txt", "space": "perso", "content_base64": b64.encode("lait, œufs") }),
        )
        .await,
    );
    let id = sent["id"].as_i64().unwrap();
    let read = payload(
        &tool(
            &app,
            &t,
            "lire_fichier",
            json!({ "id": id, "contenu": true }),
        )
        .await,
    );
    assert_eq!(
        b64.decode(read["content_base64"].as_str().unwrap())
            .unwrap(),
        "lait, œufs".as_bytes()
    );
    let refused = tool(
        &app,
        &t,
        "envoyer_fichier",
        json!({ "name": "gros.bin", "space": "perso", "content_base64": "A".repeat(14_000_001) }),
    )
    .await;
    assert_eq!(refused["isError"], true);
    assert!(
        refused["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("10 Mo")
    );

    // Dossier, rangement, corbeille, restauration.
    let folder = payload(
        &tool(
            &app,
            &t,
            "creer_dossier",
            json!({ "name": "Courses", "space": "perso" }),
        )
        .await,
    );
    let moved = payload(
        &tool(
            &app,
            &t,
            "modifier_fichier",
            json!({ "id": id, "folder": folder["id"], "add_tags": ["maison"] }),
        )
        .await,
    );
    assert_eq!(moved["folder"], folder["id"]);
    assert_eq!(moved["tags"], json!(["maison"]));
    let found = payload(&tool(&app, &t, "lister_fichiers", json!({ "tag": "maison" })).await);
    assert_eq!(found.as_array().unwrap().len(), 1);
    payload(&tool(&app, &t, "supprimer_dossier", json!({ "id": folder["id"] })).await);
    let trash = payload(&tool(&app, &t, "lister_corbeille", json!({})).await);
    assert_eq!(trash[0]["via"], "Claude");
    payload(
        &tool(
            &app,
            &t,
            "restaurer",
            json!({ "kind": "dossier", "id": folder["id"] }),
        )
        .await,
    );
    let back = payload(&tool(&app, &t, "lire_fichier", json!({ "id": id })).await);
    assert_eq!(back["folder"], folder["id"]);

    // Un lien de partage.
    let link = payload(
        &tool(
            &app,
            &t,
            "creer_lien",
            json!({ "id": id, "duration": "1h" }),
        )
        .await,
    );
    assert!(link["url"].as_str().unwrap().contains("/s/"));
    let links = payload(&tool(&app, &t, "lister_liens", json!({ "id": id })).await);
    assert_eq!(links.as_array().unwrap().len(), 1);
    payload(
        &tool(
            &app,
            &t,
            "revoquer_lien",
            json!({ "id": id, "link": link["id"] }),
        )
        .await,
    );

    // Une note : rangée, supprimée.
    let (_, note) = app
        .api(
            Method::POST,
            "/api/v1/notes",
            Some(json!({ "title": "Menu", "space": "perso" })),
            Some(&t),
        )
        .await;
    payload(&tool(&app, &t, "supprimer_note", json!({ "id": note["id"] })).await);
    let gone = tool(&app, &t, "lire_note", json!({ "id": note["id"] })).await;
    assert_eq!(gone["isError"], true);
}
