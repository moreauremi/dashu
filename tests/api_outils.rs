//! API, lot « outils » (SPEC, « API complète et corbeille ») : formats
//! possibles, convertir, compresser, suivre, remplacer l'original. Le
//! service de conversion est simulé, comme dans les tests du site.

mod common;

use std::path::Path;

use axum::http::{Method, StatusCode};
use common::{ALICE, BOB, Client, TestApp, between};
use dashu::tools::{self, JobSpec, worker};
use serde_json::{Value, json};

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

/// Un fichier envoyé par l'API, en un morceau.
async fn upload(app: &TestApp, t: &str, name: &str, content: &[u8]) -> i64 {
    let (_, started) = app
        .api(
            Method::POST,
            "/api/v1/files/uploads",
            Some(json!({ "name": name, "size": content.len(), "space": "perso" })),
            Some(t),
        )
        .await;
    let upload = started["upload"].as_str().unwrap();
    let (status, _, body) = app
        .api_bytes(
            Method::PUT,
            &format!("/api/v1/files/uploads/{upload}?offset=0"),
            Some(content.to_vec()),
            Some(t),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    serde_json::from_slice::<Value>(&body).unwrap()["id"]
        .as_i64()
        .unwrap()
}

/// Faux service : un résultat de `size` octets.
fn fake(size: usize) -> impl Fn(&JobSpec, &Path, &dyn Fn(u8)) -> Result<(), String> + Sync {
    move |spec, dir, _| {
        std::fs::write(dir.join(&spec.output), vec![7u8; size]).map_err(|e| e.to_string())
    }
}

/// Le service de conversion fait la tâche, puis dashu relève le résultat.
async fn run_job(app: &TestApp, job: i64) {
    let dir = tools::job_dir(&app.work_dir, job);
    std::fs::write(dir.join("started"), "").unwrap();
    worker::process_job(&dir, &fake(1000));
    tools::collect(&app.state).await.unwrap();
}

#[tokio::test]
async fn formats_convertir_suivre_remplacer() {
    let app = TestApp::new().await;
    let tk = token(&app, ALICE, "Script").await;
    let t = Some(tk.as_str());
    let video = upload(&app, &tk, "Vacances.MOV", &[9u8; 4096]).await;
    let note = upload(&app, &tk, "cours.md", b"# Cours").await;

    let (status, formats) = app
        .api(
            Method::GET,
            &format!("/api/v1/files/{video}/conversions"),
            None,
            t,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let keys: Vec<&str> = formats
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["to"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["compress", "mp4", "mp3"]);
    let (_, formats) = app
        .api(
            Method::GET,
            &format!("/api/v1/files/{note}/conversions"),
            None,
            t,
        )
        .await;
    assert_eq!(formats[0]["to"], "pdf");
    assert_eq!(formats[0]["label"], "En PDF");

    // Refus : sortie inconnue ou impossible pour ce type, pages manquantes.
    for (id, body) in [
        (video, json!({ "to": "gif" })),
        (video, json!({ "to": "pdf" })),
        (note, json!({ "to": "pages" })),
    ] {
        let (status, refused) = app
            .api(
                Method::POST,
                &format!("/api/v1/files/{id}/convert"),
                Some(body.clone()),
                t,
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body} {refused}");
    }

    // Compresser : en file, puis terminé avec le fichier produit.
    let (status, job) = app
        .api(
            Method::POST,
            &format!("/api/v1/files/{video}/compress"),
            None,
            t,
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{job}");
    assert_eq!(job["status"], "queued");
    assert_eq!(job["label"], "Compression");
    let job_id = job["id"].as_i64().unwrap();
    let (_, active) = app
        .api(Method::GET, "/api/v1/jobs?active=true", None, t)
        .await;
    assert_eq!(active.as_array().unwrap().len(), 1);
    run_job(&app, job_id).await;
    let (_, done) = app
        .api(Method::GET, &format!("/api/v1/jobs/{job_id}"), None, t)
        .await;
    assert_eq!(done["status"], "done", "{done}");
    assert_eq!(done["result"]["name"], "Vacances (compressé).mp4");
    let (_, active) = app
        .api(Method::GET, "/api/v1/jobs?active=true", None, t)
        .await;
    assert_eq!(active, json!([]));

    // Un autre compte ne voit ni la tâche ni les formats.
    let bob = token(&app, BOB, "Bob").await;
    for path in [
        format!("/api/v1/jobs/{job_id}"),
        format!("/api/v1/files/{video}/conversions"),
    ] {
        let (status, _) = app.api(Method::GET, &path, None, Some(&bob)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    let (status, _) = app
        .api(
            Method::POST,
            &format!("/api/v1/jobs/{job_id}/replace"),
            None,
            Some(&bob),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Remplacer l'original : la copie prend sa place, l'original part dans
    // la corbeille, avec le nom du jeton.
    let (status, replaced) = app
        .api(
            Method::POST,
            &format!("/api/v1/jobs/{job_id}/replace"),
            None,
            t,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{replaced}");
    assert_eq!(replaced["file"]["name"], "Vacances.mp4");
    assert_eq!(replaced["original_in_trash"], true);
    let (_, trash) = app.api(Method::GET, "/api/v1/trash", None, t).await;
    assert_eq!(trash[0]["id"], video);
    assert_eq!(trash[0]["via"], "Script");
    let (status, _) = app
        .api(
            Method::POST,
            &format!("/api/v1/jobs/{job_id}/replace"),
            None,
            t,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn les_outils_mcp_des_conversions() {
    let app = TestApp::new().await;
    let tk = token(&app, ALICE, "Claude").await;
    let doc = upload(&app, &tk, "notes.md", b"# Titre").await;
    let call = |name: &'static str, arguments: Value| {
        let app = &app;
        let tk = tk.clone();
        async move {
            let (_, body) = app
                .api(
                    Method::POST,
                    "/mcp",
                    Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                                 "params": { "name": name, "arguments": arguments } })),
                    Some(&tk),
                )
                .await;
            let result = body["result"].clone();
            assert_eq!(result["isError"], false, "{name} {result}");
            serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap()).unwrap()
        }
    };
    let formats = call("formats_possibles", json!({ "id": doc })).await;
    assert!(
        formats
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["to"] == "docx")
    );
    let job = call("convertir", json!({ "id": doc, "to": "html" })).await;
    let job_id = job["id"].as_i64().unwrap();
    run_job(&app, job_id).await;
    let done = call("suivre_conversion", json!({ "job": job_id })).await;
    assert_eq!(done["status"], "done");
    assert_eq!(done["result"]["name"], "notes.html");
    let listed = call("lister_conversions", json!({})).await;
    assert_eq!(listed.as_array().unwrap().len(), 1);
    let replaced = call("remplacer_original", json!({ "job": job_id })).await;
    assert_eq!(replaced["original_in_trash"], true);
}

#[tokio::test]
async fn sans_service_de_conversion_les_routes_n_existent_pas() {
    let app = TestApp::without_files().await;
    let tk = token(&app, ALICE, "Script").await;
    for (method, path) in [
        (Method::GET, "/api/v1/files/1/conversions"),
        (Method::POST, "/api/v1/files/1/compress"),
        (Method::GET, "/api/v1/jobs"),
        (Method::POST, "/api/v1/jobs/1/replace"),
    ] {
        let (status, body) = app.api(method.clone(), path, None, Some(&tk)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
        assert!(
            body["error"].as_str().unwrap().contains("WORK_DIR"),
            "{body}"
        );
    }
}
