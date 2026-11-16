//! Notes en temps réel (SPEC, « Notes en temps réel ») : le canal WebSocket
//! `/notes/{id}/live`, sur un vrai serveur local. Le serveur arbitre les
//! étapes de ProseMirror sans les lire : les tests en envoient de fausses,
//! seules comptent leur numérotation et leur diffusion.

mod common;

use std::net::SocketAddr;
use std::time::Duration;

use axum::http::{Method, StatusCode};
use common::{ALICE, API_TOKEN, BOB, Client, TestApp};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Ouvre le canal d'une note avec les cookies de `client`. `origin` : l'en-tête
/// `Origin` (celui du serveur par défaut). Une poignée de main refusée rend
/// son statut HTTP.
async fn open(
    addr: SocketAddr,
    client: &Client,
    note: i64,
    origin: Option<&str>,
) -> Result<Ws, StatusCode> {
    let mut request = format!("ws://{addr}/notes/{note}/live")
        .into_client_request()
        .unwrap();
    let headers = request.headers_mut();
    let cookies = client.cookie_header();
    if !cookies.is_empty() {
        headers.insert("cookie", cookies.parse().unwrap());
    }
    let own = format!("http://{addr}");
    if let Some(origin) = origin.or(Some(own.as_str())).filter(|o| !o.is_empty()) {
        headers.insert("origin", origin.parse().unwrap());
    }
    match tokio_tungstenite::connect_async(request).await {
        Ok((ws, _)) => Ok(ws),
        Err(tungstenite::Error::Http(response)) => {
            Err(StatusCode::from_u16(response.status().as_u16()).unwrap())
        }
        Err(other) => panic!("connexion : {other}"),
    }
}

/// Prochain message JSON (les signes de vie sont sautés). `Null` si le canal
/// se ferme ; échec au bout de 3 secondes.
async fn recv(ws: &mut Ws) -> Value {
    loop {
        let next = tokio::time::timeout(Duration::from_secs(3), ws.next())
            .await
            .expect("aucun message reçu");
        match next {
            Some(Ok(Message::Text(text))) => return serde_json::from_str(&text).unwrap(),
            Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return Value::Null,
            Some(Ok(_)) => continue,
        }
    }
}

/// Prochain message d'un type donné (les autres, comme `peers`, sont sautés).
async fn recv_type(ws: &mut Ws, kind: &str) -> Value {
    loop {
        let message = recv(ws).await;
        if message.is_null() || message["type"] == kind {
            return message;
        }
    }
}

/// Rien ne doit arriver (hors `peers`) pendant un court instant.
async fn silent(ws: &mut Ws) {
    let wait = tokio::time::timeout(Duration::from_millis(300), async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Text(text))) => {
                    let message: Value = serde_json::from_str(&text).unwrap();
                    if message["type"] != "peers" {
                        return message;
                    }
                }
                Some(Ok(_)) => continue,
                _ => return Value::Null,
            }
        }
    })
    .await;
    if let Ok(message) = wait {
        panic!("message inattendu : {message}");
    }
}

async fn send(ws: &mut Ws, message: Value) {
    ws.send(Message::Text(message.to_string().into()))
        .await
        .unwrap();
}

/// Une fausse étape ProseMirror (le serveur ne la lit pas).
fn step(n: usize) -> Value {
    json!({ "stepType": "replace", "from": n, "to": n, "slice": { "content": [] } })
}

/// Une note dans l'espace `slug` de `owner`.
async fn note_in(app: &TestApp, owner: &str, slug: &str, body: &str) -> i64 {
    let user = app.user_id(owner).await;
    let space = app.space_id(owner, slug).await;
    sqlx::query_scalar(
        "INSERT INTO notes (user_id, space_id, title, body_md, created_at, updated_at)
         VALUES (?, ?, 'Projet', ?, 0, 0) RETURNING id",
    )
    .bind(user)
    .bind(space)
    .bind(body)
    .fetch_one(&app.state.db)
    .await
    .unwrap()
}

async fn note_row(app: &TestApp, id: i64) -> (String, String, i64) {
    sqlx::query_as("SELECT title, body_md, revision FROM notes WHERE id = ?")
        .bind(id)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

/// Alice et Bob connectés, École d'Alice partagée avec Bob, une note dedans.
async fn setup() -> (TestApp, SocketAddr, Client, Client, i64) {
    let app = TestApp::new().await;
    let alice = app.logged_in(ALICE).await;
    let bob = app.logged_in(BOB).await;
    app.share_space(ALICE, "ecole", BOB).await;
    let note = note_in(&app, ALICE, "ecole", "Bonjour").await;
    let addr = app.serve().await;
    (app, addr, alice, bob, note)
}

#[tokio::test]
async fn refus_sans_session_d_une_autre_origine_ou_sans_acces() {
    let (app, addr, alice, bob, note) = setup().await;

    let anonymous = app.client();
    let refused = open(addr, &anonymous, note, None).await.err();
    assert!(
        matches!(
            refused,
            Some(StatusCode::SEE_OTHER | StatusCode::UNAUTHORIZED)
        ),
        "sans session : {refused:?}"
    );

    // Une page d'un autre site, ou sans en-tête Origin.
    assert_eq!(
        open(addr, &alice, note, Some("https://dashu.dev"))
            .await
            .err(),
        Some(StatusCode::FORBIDDEN)
    );
    assert_eq!(
        open(addr, &alice, note, Some("")).await.err(),
        Some(StatusCode::FORBIDDEN)
    );

    // Une note de Taff (non partagé) : Bob n'y a pas accès.
    let private = note_in(&app, ALICE, "taff", "Secret").await;
    assert_eq!(
        open(addr, &bob, private, None).await.err(),
        Some(StatusCode::NOT_FOUND)
    );
    assert!(open(addr, &alice, private, None).await.is_ok());
}

#[tokio::test]
async fn les_etapes_sont_numerotees_et_diffusees() {
    let (_app, addr, alice, bob, note) = setup().await;

    let mut a = open(addr, &alice, note, None).await.unwrap();
    let init = recv(&mut a).await;
    assert_eq!(init["type"], "init");
    assert_eq!(init["version"], 0);
    assert_eq!(init["doc"], "Bonjour");
    assert_eq!(init["title"], "Projet");

    let mut b = open(addr, &bob, note, None).await.unwrap();
    assert_eq!(recv(&mut b).await["version"], 0);
    // Alice apprend que Bob est là.
    let peers = recv_type(&mut a, "peers").await;
    assert_eq!(peers["peers"].as_array().unwrap().len(), 2);

    // Alice envoie deux étapes : tout le monde les reçoit, elle comprise
    // (c'est sa confirmation).
    send(
        &mut a,
        json!({ "type": "steps", "version": 0, "steps": [step(1), step(2)], "clientID": "a" }),
    )
    .await;
    for ws in [&mut a, &mut b] {
        let steps = recv_type(ws, "steps").await;
        assert_eq!(steps["from"], 0);
        assert_eq!(steps["steps"], json!([step(1), step(2)]));
        assert_eq!(steps["clientIDs"], json!(["a", "a"]));
    }

    // Bob envoie sur la version 0, dépassée : ignoré.
    send(
        &mut b,
        json!({ "type": "steps", "version": 0, "steps": [step(9)], "clientID": "b" }),
    )
    .await;
    silent(&mut a).await;

    // Recalé sur la version 2, il passe.
    send(
        &mut b,
        json!({ "type": "steps", "version": 2, "steps": [step(3)], "clientID": "b" }),
    )
    .await;
    let steps = recv_type(&mut a, "steps").await;
    assert_eq!(steps["from"], 2);
    assert_eq!(steps["clientIDs"], json!(["b"]));

    // Une page qui arrive maintenant reçoit tout depuis la version 0.
    let mut late = open(addr, &alice, note, None).await.unwrap();
    let init = recv(&mut late).await;
    assert_eq!(init["version"], 3);
    assert_eq!(init["doc"], "Bonjour");
    assert_eq!(init["steps"], json!([step(1), step(2), step(3)]));
    assert_eq!(init["clientIDs"], json!(["a", "a", "b"]));
}

#[tokio::test]
async fn seule_la_derniere_version_est_enregistree() {
    let (app, addr, alice, bob, note) = setup().await;
    let (_, _, revision) = note_row(&app, note).await;
    let mut a = open(addr, &alice, note, None).await.unwrap();
    recv(&mut a).await;
    let mut b = open(addr, &bob, note, None).await.unwrap();
    recv(&mut b).await;

    send(
        &mut a,
        json!({ "type": "steps", "version": 0, "steps": [step(1)], "clientID": "a" }),
    )
    .await;
    recv_type(&mut a, "steps").await;
    recv_type(&mut b, "steps").await;

    // Le Markdown de la version 0 n'est plus le dernier : ignoré.
    send(
        &mut b,
        json!({ "type": "save", "version": 0, "doc": "Vieux", "title": "Vieux" }),
    )
    .await;
    silent(&mut a).await;
    assert_eq!(note_row(&app, note).await.1, "Bonjour");

    // Celui de la version 1 est écrit, et tout le monde le sait.
    send(
        &mut a,
        json!({ "type": "save", "version": 1, "doc": "Bonjour !\r\n", "title": "  Projet BTS " }),
    )
    .await;
    for ws in [&mut a, &mut b] {
        let saved = recv_type(ws, "saved").await;
        assert_eq!(saved["version"], 1);
        assert_eq!(saved["revision"], revision + 1);
    }
    assert_eq!(
        note_row(&app, note).await,
        ("Projet BTS".into(), "Bonjour !\n".into(), revision + 1)
    );

    // En quittant la page : les dernières étapes et le Markdown obtenu, en
    // un seul message.
    send(
        &mut b,
        json!({ "type": "steps", "version": 1, "steps": [step(2)], "clientID": "b",
                "doc": "Bonjour à tous", "title": "Projet BTS" }),
    )
    .await;
    let saved = recv_type(&mut a, "saved").await;
    assert_eq!(saved["version"], 2);
    assert_eq!(note_row(&app, note).await.1, "Bonjour à tous");

    // L'API et la recherche voient le texte enregistré.
    let (status, body) = app
        .api(
            Method::GET,
            &format!("/api/v1/notes/{note}"),
            None,
            Some(API_TOKEN),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["body_md"], "Bonjour à tous");
}

#[tokio::test]
async fn une_modification_exterieure_n_est_pas_ecrasee() {
    let (app, addr, alice, _bob, note) = setup().await;
    let mut a = open(addr, &alice, note, None).await.unwrap();
    recv(&mut a).await;

    // Hermès modifie la note par l'API pendant qu'Alice l'édite.
    let (status, _) = app
        .api(
            Method::PATCH,
            &format!("/api/v1/notes/{note}"),
            Some(json!({ "body_md": "Écrit par Hermès" })),
            Some(API_TOKEN),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    send(
        &mut a,
        json!({ "type": "steps", "version": 0, "steps": [step(1)], "clientID": "a",
                "doc": "Écrit par Alice", "title": "Projet" }),
    )
    .await;
    assert_eq!(recv_type(&mut a, "conflict").await["type"], "conflict");
    assert!(recv(&mut a).await.is_null(), "le canal se ferme");
    assert_eq!(note_row(&app, note).await.1, "Écrit par Hermès");

    // La page suivante repart de la note telle qu'elle est en base.
    let mut again = open(addr, &alice, note, None).await.unwrap();
    let init = recv(&mut again).await;
    assert_eq!(init["version"], 0);
    assert_eq!(init["doc"], "Écrit par Hermès");
}

#[tokio::test]
async fn curseurs_titre_et_presents_viennent_du_serveur() {
    let (app, addr, alice, bob, note) = setup().await;
    sqlx::query("UPDATE users SET first_name = 'Bob', avatar_color = 'teal' WHERE email = ?")
        .bind(BOB)
        .execute(&app.state.db)
        .await
        .unwrap();
    let mut a = open(addr, &alice, note, None).await.unwrap();
    recv(&mut a).await;
    let mut b = open(addr, &bob, note, None).await.unwrap();
    let init = recv(&mut b).await;
    let me = init["me"].clone();

    // Le nom et la couleur de Bob viennent de son profil.
    let peers = recv_type(&mut a, "peers").await;
    let bob_peer = peers["peers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == me)
        .unwrap()
        .clone();
    assert_eq!(bob_peer["name"], "Bob");
    assert_eq!(bob_peer["initials"], "B");
    assert_eq!(bob_peer["color"], "teal");

    // Son curseur part chez Alice, pas chez lui.
    send(
        &mut b,
        json!({ "type": "cursor", "cursor": { "anchor": 3, "head": 5, "name": "Usurpateur" } }),
    )
    .await;
    let cursor = recv_type(&mut a, "cursor").await;
    assert_eq!(cursor["peer"], me);
    assert_eq!(cursor["cursor"]["head"], 5);
    silent(&mut b).await;

    // Un curseur trop gros ou mal formé est ignoré.
    send(
        &mut b,
        json!({ "type": "cursor", "cursor": "x".repeat(500) }),
    )
    .await;
    send(&mut b, "pas du JSON".into()).await;
    silent(&mut a).await;

    // Le titre suit en direct.
    send(
        &mut b,
        json!({ "type": "title", "title": "  Nouveau titre " }),
    )
    .await;
    assert_eq!(recv_type(&mut a, "title").await["title"], "Nouveau titre");

    // Le départ de Bob est annoncé.
    drop(b);
    let peers = recv_type(&mut a, "peers").await;
    assert_eq!(peers["peers"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn un_membre_retire_perd_le_canal() {
    let (app, addr, alice, bob, note) = setup().await;
    let mut a = open(addr, &alice, note, None).await.unwrap();
    recv(&mut a).await;
    let mut b = open(addr, &bob, note, None).await.unwrap();
    recv(&mut b).await;

    sqlx::query("DELETE FROM space_members")
        .execute(&app.state.db)
        .await
        .unwrap();

    send(
        &mut b,
        json!({ "type": "steps", "version": 0, "steps": [step(1)], "clientID": "b" }),
    )
    .await;
    assert_eq!(recv_type(&mut b, "gone").await["type"], "gone");
    assert!(recv(&mut b).await.is_null(), "le canal se ferme");
    // Son étape n'a pas été diffusée.
    let next = recv(&mut a).await;
    assert_eq!(next["type"], "peers", "{next}");
}

#[tokio::test]
async fn une_reconnexion_retrouve_la_session() {
    let (app, addr, alice, _bob, note) = setup().await;
    let mut a = open(addr, &alice, note, None).await.unwrap();
    let first = recv(&mut a).await;
    send(
        &mut a,
        json!({ "type": "steps", "version": 0, "steps": [step(1)], "clientID": "a" }),
    )
    .await;
    recv_type(&mut a, "steps").await;
    drop(a);

    // La session reste une minute après le départ de la dernière page.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(app.state.live.open_rooms(), 1);
    let mut again = open(addr, &alice, note, None).await.unwrap();
    let init = recv(&mut again).await;
    assert_eq!(init["epoch"], first["epoch"]);
    assert_eq!(init["version"], 1);
}

#[tokio::test]
async fn une_note_supprimee_ferme_la_session() {
    let (app, addr, mut alice, _bob, note) = setup().await;
    let mut a = open(addr, &alice, note, None).await.unwrap();
    recv(&mut a).await;

    alice.get("/e/ecole/notes").await;
    let deleted = alice.post(&format!("/notes/{note}/delete"), &[]).await;
    assert_eq!(deleted.status, StatusCode::SEE_OTHER);

    send(
        &mut a,
        json!({ "type": "save", "version": 0, "doc": "Encore là ?", "title": "Projet" }),
    )
    .await;
    let message = recv(&mut a).await;
    assert!(
        message["type"] == "gone" || message.is_null(),
        "la note n'existe plus : {message}"
    );
    let _ = app;
}
