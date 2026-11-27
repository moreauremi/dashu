//! Événements créés dans dashu et leur copie dans Google Agenda (SPEC,
//! « Événements et Google Agenda »). Google est remplacé par un faux serveur
//! local qui imite l'échange du code, l'adresse du compte et l'API Agenda, et
//! note chaque appel.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use common::{ALICE, API_TOKEN, BOB, Client, TestApp, TestResponse};
use serde_json::{Value, json};

/// Le jeton de rafraîchissement que donne le faux Google.
const REFRESH: &str = "1//jeton-de-rafraichissement-secret";
const CALENDAR: &str = "https://www.googleapis.com/auth/calendar.events";

#[derive(Default)]
struct Fake {
    /// (méthode, chemin, corps) de chaque appel reçu.
    calls: Vec<(String, String, Value)>,
    events: HashMap<String, Value>,
    next: u32,
    /// Google indisponible pour les créations.
    fail_inserts: bool,
}

type Shared = Arc<Mutex<Fake>>;

fn bearer(headers: &HeaderMap) -> String {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

async fn token(State(fake): State<Shared>, Form(form): Form<HashMap<String, String>>) -> Response {
    fake.lock()
        .unwrap()
        .calls
        .push(("POST".into(), "/token".into(), json!(form)));
    let refused = || {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_grant" })),
        )
            .into_response()
    };
    match form.get("grant_type").map(String::as_str) {
        Some("authorization_code") => {
            // PKCE : le secret doit accompagner le code.
            if form.get("code_verifier").is_none_or(|v| v.len() < 43) {
                return refused();
            }
            let scope = match form.get("code").map(String::as_str) {
                Some("bon-code") => format!("openid email {CALENDAR}"),
                Some("sans-permission") => "openid email".into(),
                _ => return refused(),
            };
            Json(json!({
                "access_token": "acces-1", "refresh_token": REFRESH, "expires_in": 3599,
                "scope": scope, "token_type": "Bearer",
            }))
            .into_response()
        }
        Some("refresh_token") if form.get("refresh_token").map(String::as_str) == Some(REFRESH) => {
            Json(json!({ "access_token": "acces-2", "expires_in": 3599, "scope": CALENDAR, "token_type": "Bearer" }))
                .into_response()
        }
        _ => refused(),
    }
}

async fn userinfo(headers: HeaderMap) -> Response {
    if bearer(&headers) != "Bearer acces-1" {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(json!({ "email": "alice.perso@gmail.test", "email_verified": true })).into_response()
}

async fn revoke(State(fake): State<Shared>, Form(form): Form<HashMap<String, String>>) {
    fake.lock()
        .unwrap()
        .calls
        .push(("POST".into(), "/revoke".into(), json!(form)));
}

async fn insert(
    State(fake): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let mut fake = fake.lock().unwrap();
    fake.calls
        .push(("POST".into(), "/events".into(), body.clone()));
    if bearer(&headers) != "Bearer acces-2" {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if fake.fail_inserts {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    fake.next += 1;
    let id = format!("g{}", fake.next);
    fake.events.insert(id.clone(), body);
    Json(json!({ "id": id })).into_response()
}

async fn update(
    State(fake): State<Shared>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> StatusCode {
    let mut fake = fake.lock().unwrap();
    fake.calls
        .push(("PUT".into(), format!("/events/{id}"), body.clone()));
    match fake.events.get_mut(&id) {
        Some(event) => {
            *event = body;
            StatusCode::OK
        }
        None => StatusCode::NOT_FOUND,
    }
}

async fn remove(State(fake): State<Shared>, Path(id): Path<String>) -> StatusCode {
    let mut fake = fake.lock().unwrap();
    fake.calls
        .push(("DELETE".into(), format!("/events/{id}"), Value::Null));
    fake.events.remove(&id);
    StatusCode::NO_CONTENT
}

/// Lance le faux Google ; rend sa racine et son état.
async fn fake_google() -> (String, Shared) {
    let fake: Shared = Arc::default();
    let app = Router::new()
        .route("/token", post(token))
        .route("/userinfo", get(userinfo))
        .route("/revoke", post(revoke))
        .route("/calendar/v3/calendars/primary/events", post(insert))
        .route(
            "/calendar/v3/calendars/primary/events/{id}",
            put(update).delete(remove),
        )
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), fake)
}

/// Appels reçus par le faux Google pour un chemin donné.
fn calls(fake: &Shared, method: &str, path: &str) -> Vec<Value> {
    fake.lock()
        .unwrap()
        .calls
        .iter()
        .filter(|(m, p, _)| m == method && p == path)
        .map(|(_, _, body)| body.clone())
        .collect()
}

/// Relie Google Agenda au compte de `client`, comme le ferait le navigateur.
async fn connect(client: &mut Client, base: &str, code: &str) -> TestResponse {
    let start = client.get("/settings/google-calendar/connect").await;
    assert_eq!(start.status, StatusCode::SEE_OTHER);
    let location = start.location().to_string();
    assert!(location.starts_with(&format!("{base}/auth?")), "{location}");
    let url = url::Url::parse(&location).unwrap();
    let param = |name: &str| {
        url.query_pairs()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default()
    };
    assert!(param("scope").contains(CALENDAR));
    assert_eq!(param("access_type"), "offline");
    assert_eq!(param("code_challenge_method"), "S256");
    assert_eq!(
        param("redirect_uri"),
        "http://localhost:3000/auth/google/callback"
    );
    client
        .get(&format!(
            "/auth/google/callback?state={}&code={code}",
            param("state")
        ))
        .await
}

/// Un événement du mercredi 18 novembre 2026, 14:00 – 15:30.
async fn add_event(client: &mut Client, space: i64, title: &str, google: bool) -> TestResponse {
    let space = space.to_string();
    let mut form = vec![
        ("title", title),
        ("date", "2026-11-18"),
        ("start", "14:00"),
        ("end", "15:30"),
        ("location", "B204"),
        ("space_id", space.as_str()),
    ];
    if google {
        form.push(("google", "on"));
    }
    client.post("/calendar/events", &form).await
}

async fn event_id(app: &TestApp, title: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM events WHERE title = ?")
        .bind(title)
        .fetch_one(&app.state.db)
        .await
        .unwrap_or_else(|_| panic!("événement « {title} » absent"))
}

#[tokio::test]
async fn creer_modifier_supprimer_un_evenement() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await;

    // Depuis Perso, qui réunit tous les agendas : on y reste.
    let created = add_event(&mut alice, ecole, "Réunion projet", false).await;
    assert_eq!(created.status, StatusCode::SEE_OTHER, "{}", created.body);
    assert_eq!(created.location(), "/calendar?view=week&date=2026-11-18");
    let id = event_id(&app, "Réunion projet").await;

    // Dans l'agenda de la semaine, cliquable, avec son heure et son lieu.
    let week = alice
        .get("/e/ecole/calendar?view=week&date=2026-11-18")
        .await;
    assert!(week.body.contains("<strong>Réunion projet</strong>"));
    assert!(
        week.body
            .contains(&format!("href=\"/calendar/events/{id}\""))
    );
    assert!(week.body.contains("14:00 – 15:30 · B204"));
    assert!(week.body.contains("Nouvel événement"));

    // Le formulaire s'ouvre avec ses valeurs ; on le modifie.
    let form = alice.get(&format!("/calendar/events/{id}")).await;
    assert_eq!(form.status, StatusCode::OK);
    assert!(form.body.contains("value=\"Réunion projet\""));
    assert!(form.body.contains("value=\"14:00\""));
    let space = ecole.to_string();
    let saved = alice
        .post(
            &format!("/calendar/events/{id}"),
            &[
                ("title", "Réunion de projet"),
                ("date", "2026-11-19"),
                ("all_day", "on"),
                ("location", ""),
                ("space_id", &space),
            ],
        )
        .await;
    assert_eq!(saved.status, StatusCode::SEE_OTHER, "{}", saved.body);
    // Journée entière : dans le bandeau du jeudi, comme lien.
    let week = alice
        .get("/e/ecole/calendar?view=week&date=2026-11-19")
        .await;
    assert!(week.body.contains("cal-allday is-own"));
    assert!(week.body.contains(">Réunion de projet</a>"));

    let deleted = alice
        .post(&format!("/calendar/events/{id}/delete"), &[])
        .await;
    assert_eq!(deleted.status, StatusCode::SEE_OTHER);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM events")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn un_formulaire_invalide_est_refuse() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await.to_string();
    let base = |title: &'static str, date: &'static str, start: &'static str, end: &'static str| {
        vec![
            ("title", title),
            ("date", date),
            ("start", start),
            ("end", end),
            ("space_id", ecole.as_str()),
        ]
    };
    for (form, message) in [
        (base("  ", "2026-11-18", "14:00", "15:00"), "Donne un titre"),
        (
            base("Cours", "18/11/2026", "14:00", "15:00"),
            "pas une date valide",
        ),
        (
            base("Cours", "2026-11-18", "15:00", "14:00"),
            "La fin doit venir après le début",
        ),
        (base("Cours", "2026-11-18", "", "15:00"), "heure de début"),
    ] {
        let response = alice.post("/calendar/events", &form).await;
        assert_eq!(
            response.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{message}"
        );
        assert!(response.body.contains(message), "{message}");
        // Ce qui a été tapé reste dans le formulaire.
        assert!(response.body.contains(&format!("value=\"{}\"", form[1].1)));
    }
    // Un espace d'un autre compte est refusé.
    app.logged_in(BOB).await;
    let bob_space = app.space_id(BOB, "perso").await.to_string();
    let response = alice
        .post(
            "/calendar/events",
            &[
                ("title", "Cours"),
                ("date", "2026-11-18"),
                ("start", "14:00"),
                ("end", "15:00"),
                ("space_id", &bob_space),
            ],
        )
        .await;
    assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM events")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn un_evenement_reste_a_son_auteur() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let mut bob = app.logged_in(BOB).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    let slug = app.share_space(ALICE, "ecole", BOB).await;
    add_event(&mut alice, ecole, "Rendez-vous perso", false).await;
    let id = event_id(&app, "Rendez-vous perso").await;

    // Même membre de l'espace, Bob ne le voit pas et ne l'ouvre pas.
    let week = bob
        .get(&format!("/e/{slug}/calendar?view=week&date=2026-11-18"))
        .await;
    assert!(!week.body.contains("Rendez-vous perso"));
    assert_eq!(
        bob.get(&format!("/calendar/events/{id}")).await.status,
        StatusCode::NOT_FOUND
    );
    let response = bob
        .post(&format!("/calendar/events/{id}/delete"), &[])
        .await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);

    // Le Perso d'Alice le réunit, avec le nom de son espace.
    let perso = alice
        .get("/e/perso/calendar?view=week&date=2026-11-18")
        .await;
    assert!(perso.body.contains("Rendez-vous perso"));
    assert!(perso.body.contains(">École</b>"));

    // L'API (au nom d'Alice) le voit aussi.
    let (status, body) = app
        .api(
            Method::GET,
            "/api/v1/calendar?from=2026-11-16&to=2026-11-20",
            None,
            Some(API_TOKEN),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.to_string().contains("Rendez-vous perso"));

    // Un événement de Bob dans l'espace partagé revient dans son Perso
    // quand il le quitte.
    add_event(&mut bob, ecole, "Révision de Bob", false).await;
    let left = bob
        .post(&format!("/settings/spaces/{ecole}/leave"), &[])
        .await;
    assert_eq!(left.status, StatusCode::SEE_OTHER);
    let space: i64 =
        sqlx::query_scalar("SELECT space_id FROM events WHERE title = 'Révision de Bob'")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(space, app.space_id(BOB, "perso").await);
}

#[tokio::test]
async fn sans_google_pas_de_google_agenda() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let settings = alice.get("/settings").await;
    assert!(!settings.body.contains("id=\"google-agenda\""));
    assert_eq!(
        alice.get("/settings/google-calendar/connect").await.status,
        StatusCode::NOT_FOUND
    );
    let form = alice.get("/calendar/events/new?date=2026-11-18").await;
    assert!(!form.body.contains("Google Agenda"));
}

#[tokio::test]
async fn relier_google_agenda() {
    let (base, fake) = fake_google().await;
    let app = TestApp::with_google(&base).await;
    let mut alice = app.logged_in(ALICE).await;

    // Pas encore relié : la carte propose de le faire.
    let settings = alice.get("/settings").await;
    assert!(settings.body.contains("Connecter Google Agenda"));

    // Un retour de Google qui n'est pas le nôtre ne relie rien.
    let stray = alice
        .get("/auth/google/callback?state=autre&code=bon-code")
        .await;
    assert_eq!(stray.location(), "/login?e=google");

    // Permission « agenda » décochée chez Google : refusé.
    let back = connect(&mut alice, &base, "sans-permission").await;
    assert_eq!(back.location(), "/settings#google-agenda");
    let page = alice.follow(back).await;
    assert!(page.body.contains("pas été accordée"));

    let back = connect(&mut alice, &base, "bon-code").await;
    assert_eq!(back.location(), "/settings#google-agenda");
    let page = alice.follow(back).await;
    assert!(page.body.contains("alice.perso@gmail.test"));
    assert!(page.body.contains("Déconnecter Google Agenda"));

    // L'échange du code a envoyé le secret PKCE.
    let exchange = calls(&fake, "POST", "/token")
        .into_iter()
        .find(|f| f["code"] == "bon-code")
        .unwrap();
    assert_eq!(exchange["grant_type"], "authorization_code");
    assert!(exchange["code_verifier"].as_str().unwrap().len() >= 43);

    // Le jeton est chiffré en base.
    let sealed: Vec<u8> = sqlx::query_scalar("SELECT refresh_token FROM google_accounts")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert!(
        !sealed
            .windows(REFRESH.len())
            .any(|w| w == REFRESH.as_bytes())
    );
    assert_eq!(
        dashu::calendar::google::open(&app.state, &sealed).as_deref(),
        Some(REFRESH)
    );

    // Déconnecter retire la permission chez Google et oublie le jeton.
    let out = alice
        .post("/settings/google-calendar/disconnect", &[])
        .await;
    assert_eq!(out.location(), "/settings#google-agenda");
    assert_eq!(calls(&fake, "POST", "/revoke")[0]["token"], REFRESH);
    let accounts: i64 = sqlx::query_scalar("SELECT count(*) FROM google_accounts")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(accounts, 0);
}

#[tokio::test]
async fn un_evenement_suit_dans_google_agenda() {
    let (base, fake) = fake_google().await;
    let app = TestApp::with_google(&base).await;
    let mut alice = app.logged_in(ALICE).await;
    let user = app.user_id(ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    connect(&mut alice, &base, "bon-code").await;

    // Le formulaire propose Google Agenda, coché.
    let form = alice.get("/calendar/events/new?date=2026-11-18").await;
    assert!(
        form.body
            .contains("Aussi dans Google Agenda (alice.perso@gmail.test)")
    );
    assert!(form.body.contains("name=\"google\" checked"));

    add_event(&mut alice, ecole, "Réunion projet", true).await;
    dashu::calendar::google::push_user(&app.state, user).await;
    let inserted = calls(&fake, "POST", "/events");
    assert_eq!(inserted.len(), 1, "une seule copie créée");
    assert_eq!(inserted[0]["summary"], "Réunion projet");
    assert_eq!(inserted[0]["location"], "B204");
    assert_eq!(
        inserted[0]["start"]["dateTime"],
        "2026-11-18T14:00:00+01:00"
    );
    assert_eq!(inserted[0]["end"]["timeZone"], "Europe/Paris");
    let id = event_id(&app, "Réunion projet").await;
    let google_id: Option<String> =
        sqlx::query_scalar("SELECT google_event_id FROM events WHERE id = ?")
            .bind(id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(google_id.as_deref(), Some("g1"));
    let edit = alice.get(&format!("/calendar/events/{id}")).await;
    assert!(edit.body.contains("Dans Google Agenda"));

    // Une modification suit.
    let space = ecole.to_string();
    alice
        .post(
            &format!("/calendar/events/{id}"),
            &[
                ("title", "Réunion projet BTS"),
                ("date", "2026-11-18"),
                ("start", "16:00"),
                ("end", "17:00"),
                ("location", "C02"),
                ("space_id", &space),
                ("google", "on"),
            ],
        )
        .await;
    dashu::calendar::google::push_user(&app.state, user).await;
    let updated = calls(&fake, "PUT", "/events/g1");
    assert_eq!(updated.last().unwrap()["summary"], "Réunion projet BTS");
    assert_eq!(
        updated.last().unwrap()["start"]["dateTime"],
        "2026-11-18T16:00:00+01:00"
    );

    // Le même événement, revenu par le flux iCal de l'agenda Google, ne
    // s'affiche qu'une fois.
    sqlx::query(
        "INSERT INTO calendar_sources (id, user_id, space_id, name, ical_url, created_at)
         VALUES (50, ?, ?, 'Google', 'https://exemple.test/agenda.ics', 0)",
    )
    .bind(user)
    .bind(ecole)
    .execute(&app.state.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO calendar_events (source_id, uid, title, location, starts_at, ends_at)
         SELECT 50, 'g1@google.com', title, location, starts_at, ends_at FROM events WHERE id = ?",
    )
    .bind(id)
    .execute(&app.state.db)
    .await
    .unwrap();
    let week = alice
        .get("/e/ecole/calendar?view=week&date=2026-11-18")
        .await;
    assert_eq!(
        week.body
            .matches("<strong>Réunion projet BTS</strong>")
            .count(),
        1
    );

    // La suppression suit.
    alice
        .post(&format!("/calendar/events/{id}/delete"), &[])
        .await;
    dashu::calendar::google::push_user(&app.state, user).await;
    assert_eq!(calls(&fake, "DELETE", "/events/g1").len(), 1);
    let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM google_deletions")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(pending, 0);
}

#[tokio::test]
async fn google_indisponible_puis_reprise() {
    let (base, fake) = fake_google().await;
    let app = TestApp::with_google(&base).await;
    let mut alice = app.logged_in(ALICE).await;
    let user = app.user_id(ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    connect(&mut alice, &base, "bon-code").await;

    fake.lock().unwrap().fail_inserts = true;
    add_event(&mut alice, ecole, "Oral blanc", true).await;
    dashu::calendar::google::push_user(&app.state, user).await;
    let id = event_id(&app, "Oral blanc").await;
    let (dirty, error): (i64, Option<String>) =
        sqlx::query_as("SELECT google_dirty, google_error FROM events WHERE id = ?")
            .bind(id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert!(dirty > 0, "toujours à envoyer");
    assert_eq!(error.as_deref(), Some("Google répond 503"));
    let edit = alice.get(&format!("/calendar/events/{id}")).await;
    assert!(edit.body.contains("Pas encore dans Google Agenda"));
    let settings = alice.get("/settings").await;
    assert!(
        settings
            .body
            .contains("1 événement pas encore dans Google Agenda")
    );

    // Google revient : le passage suivant l'envoie.
    fake.lock().unwrap().fail_inserts = false;
    dashu::calendar::google::push_user(&app.state, user).await;
    let (dirty, google_id): (i64, Option<String>) =
        sqlx::query_as("SELECT google_dirty, google_event_id FROM events WHERE id = ?")
            .bind(id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(dirty, 0);
    assert!(google_id.is_some());
}
