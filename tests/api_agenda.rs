//! API, lot « agenda » (SPEC, « API complète et corbeille ») : événements
//! créés dans dashu et sources iCal. Pour chaque route : le cas normal, un
//! élément d'un autre compte (404), une saisie invalide.

mod common;

use axum::http::{Method, StatusCode};
use common::{ALICE, BOB, Client, TestApp, between};
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

#[tokio::test]
async fn evenements_creer_lire_modifier_supprimer() {
    let app = TestApp::new().await;
    let tk = token(&app, ALICE, "Script").await;
    let t = Some(tk.as_str());

    let (status, event) = app
        .api(
            Method::POST,
            "/api/v1/events",
            Some(
                json!({ "title": "Oral de BTS", "date": "2026-12-03", "start": "09:00",
                         "end": "09:30", "location": "Salle 12", "space": "ecole" }),
            ),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{event}");
    let id = event["id"].as_i64().unwrap();
    assert_eq!(event["start"], "2026-12-03T09:00:00+01:00");
    assert_eq!(event["space"], "ecole");
    // Sans Google Agenda relié, rien ne part chez Google.
    assert_eq!(event["google"], false);

    // L'agenda le montre, avec son identifiant.
    let (_, agenda) = app
        .api(
            Method::GET,
            "/api/v1/calendar?from=2026-12-01&to=2026-12-05",
            None,
            t,
        )
        .await;
    let found = agenda
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["title"] == "Oral de BTS")
        .expect("dans l'agenda")
        .clone();
    assert_eq!(found["event"], id);

    // Modifier : les champs absents restent.
    let (status, changed) = app
        .api(
            Method::PATCH,
            &format!("/api/v1/events/{id}"),
            Some(json!({ "end": "10:00", "space": "taff" })),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["title"], "Oral de BTS");
    assert_eq!(changed["end"], "2026-12-03T10:00:00+01:00");
    assert_eq!(changed["space"], "taff");
    let (_, whole_day) = app
        .api(
            Method::PATCH,
            &format!("/api/v1/events/{id}"),
            Some(json!({ "all_day": true })),
            t,
        )
        .await;
    assert_eq!(whole_day["all_day"], true);

    // Refus, sans rien créer ni changer.
    for body in [
        json!({ "title": " ", "date": "2026-12-03", "start": "09:00", "end": "10:00", "space": "ecole" }),
        json!({ "title": "x", "date": "3 décembre", "start": "09:00", "end": "10:00", "space": "ecole" }),
        json!({ "title": "x", "date": "2026-12-03", "start": "10:00", "end": "09:00", "space": "ecole" }),
        json!({ "title": "x", "date": "2026-12-03", "space": "ecole" }),
        json!({ "title": "x", "date": "2026-12-03", "all_day": true, "space": "nulle-part" }),
        json!({ "title": "x", "date": "2026-12-03", "all_day": true }),
    ] {
        let (status, refused) = app
            .api(Method::POST, "/api/v1/events", Some(body.clone()), t)
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body} {refused}");
    }
    let (status, _) = app
        .api(
            Method::PATCH,
            &format!("/api/v1/events/{id}"),
            Some(json!({ "all_day": false, "start": "11:00", "end": "10:00" })),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Un événement est à son auteur.
    let bob = token(&app, BOB, "Bob").await;
    for method in [Method::GET, Method::PATCH, Method::DELETE] {
        let (status, _) = app
            .api(
                method.clone(),
                &format!("/api/v1/events/{id}"),
                (method == Method::PATCH).then(|| json!({ "title": "x" })),
                Some(&bob),
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method}");
    }

    let (status, _) = app
        .api(Method::DELETE, &format!("/api/v1/events/{id}"), None, t)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = app
        .api(Method::GET, &format!("/api/v1/events/{id}"), None, t)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn sources_ical_adresses_publiques_seulement() {
    let app = TestApp::new().await;
    let tk = token(&app, ALICE, "Script").await;
    let t = Some(tk.as_str());

    let (status, source) = app
        .api(
            Method::POST,
            "/api/v1/calendar/sources",
            Some(json!({ "name": "Pronote", "space": "ecole",
                         "url": "https://agenda.example.invalid/ical/Edt.ics?jeton=secret" })),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{source}");
    // Le jeton de l'adresse n'est jamais redonné.
    assert!(!source.to_string().contains("secret"));
    assert_eq!(source["space"], "ecole");
    let id = source["id"].as_i64().unwrap();
    let (_, list) = app
        .api(Method::GET, "/api/v1/calendar/sources", None, t)
        .await;
    assert_eq!(list.as_array().unwrap().len(), 1);

    // Une adresse du réseau local, de la machine, ou pas en HTTPS : refusée.
    for url in [
        "https://192.168.1.10/agenda.ics",
        "https://127.0.0.1/agenda.ics",
        "https://localhost/agenda.ics",
        "https://nas.local/agenda.ics",
        "http://agenda.example.com/agenda.ics",
        "pas une adresse",
    ] {
        let (status, refused) = app
            .api(
                Method::POST,
                "/api/v1/calendar/sources",
                Some(json!({ "name": "Piège", "space": "ecole", "url": url })),
                t,
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{url} {refused}");
    }
    let (status, _) = app
        .api(
            Method::POST,
            "/api/v1/calendar/sources",
            Some(json!({ "name": " ", "space": "ecole", "url": "https://agenda.example.invalid/x.ics" })),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, list) = app
        .api(Method::GET, "/api/v1/calendar/sources", None, t)
        .await;
    assert_eq!(list.as_array().unwrap().len(), 1);

    let bob = token(&app, BOB, "Bob").await;
    let (status, _) = app
        .api(
            Method::DELETE,
            &format!("/api/v1/calendar/sources/{id}"),
            None,
            Some(&bob),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = app
        .api(
            Method::DELETE,
            &format!("/api/v1/calendar/sources/{id}"),
            None,
            t,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn les_outils_mcp_de_l_agenda() {
    let app = TestApp::new().await;
    let tk = token(&app, ALICE, "Claude").await;
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
            body["result"].clone()
        }
    };
    let ok = |result: Value| {
        assert_eq!(result["isError"], false, "{result}");
        serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap()).unwrap()
    };
    let event = ok(call(
        "creer_evenement",
        json!({ "title": "Dentiste", "date": "2026-12-10", "start": "14:00", "end": "14:30", "space": "perso" }),
    )
    .await);
    let id = event["id"].as_i64().unwrap();
    let agenda = ok(call(
        "lire_agenda",
        json!({ "from": "2026-12-10", "to": "2026-12-10" }),
    )
    .await);
    assert_eq!(agenda[0]["event"], id);
    let moved = ok(call(
        "modifier_evenement",
        json!({ "id": id, "start": "15:00", "end": "15:30" }),
    )
    .await);
    assert_eq!(moved["start"], "2026-12-10T15:00:00+01:00");
    ok(call("supprimer_evenement", json!({ "id": id })).await);
    let refused = call(
        "ajouter_source_agenda",
        json!({ "name": "Piège", "url": "https://10.0.0.1/a.ics", "space": "perso" }),
    )
    .await;
    assert_eq!(refused["isError"], true);
    assert!(
        refused["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("réseau local")
    );
    let sources = ok(call("lister_sources_agenda", json!({})).await);
    assert_eq!(sources, json!([]));
}
