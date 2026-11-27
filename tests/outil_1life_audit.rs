//! 1Life › Audit (SPEC, « 1Life — Audit » et « 1Life — Audit en Word ») :
//! des notes d'audit au document, par l'IA d'Anthropic. Un test (au moins)
//! par critère d'acceptation.
//!
//! L'API d'Anthropic est remplacée par un faux serveur local
//! (`ANTHROPIC_BASE_URL`) : aucun appel réel. Il note chaque demande et
//! rejoue la réponse qu'on lui donne.
//!
//! Dépôt public : tous les noms ci-dessous sont inventés, et aucun contenu de
//! 1Life n'y figure.

mod common;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use common::{ALICE, BOB, Client, TestApp, between};
use dashu::tools::{Conversion, JobSpec, worker};
use serde_json::{Value, json};

const LIST: &str = "/e/taff/1life/audit";
const CLIENTS: &str = "/e/taff/1life/clients";
const DATE: &str = "2026-11-27";

// ---------------------------------------------------------------------
// Le faux serveur Anthropic
// ---------------------------------------------------------------------

/// Ce que le faux serveur répond à la prochaine demande.
#[derive(Clone)]
enum Reply {
    /// Une réponse complète, avec ce texte.
    Text(String),
    /// `stop_reason: max_tokens`.
    Cut,
    /// Une erreur HTTP, avec son type.
    Error(u16, &'static str),
    /// Rien avant ce délai (plus long que celui des tests, 2 secondes).
    Slow(Duration),
}

/// Une demande reçue : ses en-têtes et son corps.
#[derive(Clone)]
struct Seen {
    api_key: String,
    version: String,
    body: Value,
}

#[derive(Default)]
struct FakeState {
    replies: VecDeque<Reply>,
    seen: Vec<Seen>,
}

#[derive(Clone)]
struct Fake {
    base: String,
    state: Arc<Mutex<FakeState>>,
}

impl Fake {
    fn reply(&self, reply: Reply) {
        self.state.lock().unwrap().replies.push_back(reply);
    }

    fn seen(&self) -> Vec<Seen> {
        self.state.lock().unwrap().seen.clone()
    }
}

/// Le document que répond le faux serveur par défaut.
const DOCUMENT: &str = "# Rapport d'audit ERP\n\n## Contexte\n\nAtelier Test fabrique des meubles.\n\n## Constats\n\n- La saisie des commandes est faite deux fois.";

/// Une réponse comme celle de l'IA : le document, la ligne séparatrice, puis
/// les points non repris.
fn answer(document: &str, points: &[&str]) -> String {
    let mut text = format!("{document}\n\n=== POINTS NON REPRIS ===\n");
    for point in points {
        text.push_str(&format!("- {point}\n"));
    }
    text
}

async fn messages(
    State(state): State<Arc<Mutex<FakeState>>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let reply = {
        let mut state = state.lock().unwrap();
        state.seen.push(Seen {
            api_key: headers
                .get("x-api-key")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string(),
            version: headers
                .get("anthropic-version")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string(),
            body: body.clone(),
        });
        state
            .replies
            .pop_front()
            .unwrap_or_else(|| Reply::Text(answer(DOCUMENT, &[])))
    };
    let model = body["model"].clone();
    let usage = json!({ "input_tokens": 1234, "output_tokens": 567 });
    match reply {
        Reply::Text(text) => Json(json!({
            "id": "msg_test",
            "type": "message",
            "role": "assistant",
            "model": model,
            // Un bloc de réflexion vide, comme Haiku 5.5 en renvoie.
            "content": [{ "type": "thinking", "thinking": "" }, { "type": "text", "text": text }],
            "stop_reason": "end_turn",
            "usage": usage,
        }))
        .into_response(),
        Reply::Cut => Json(json!({
            "id": "msg_test",
            "type": "message",
            "role": "assistant",
            "model": model,
            "content": [{ "type": "text", "text": "# Rapport coupé" }],
            "stop_reason": "max_tokens",
            "usage": usage,
        }))
        .into_response(),
        Reply::Error(status, kind) => (
            StatusCode::from_u16(status).unwrap(),
            Json(json!({ "type": "error", "error": { "type": kind, "message": "faux refus" } })),
        )
            .into_response(),
        Reply::Slow(delay) => {
            tokio::time::sleep(delay).await;
            Json(json!({ "content": [], "stop_reason": "end_turn" })).into_response()
        }
    }
}

/// Démarre le faux serveur Anthropic sur un port libre.
async fn fake_anthropic() -> Fake {
    let state = Arc::new(Mutex::new(FakeState::default()));
    let router = axum::Router::new()
        .route("/v1/messages", axum::routing::post(messages))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Fake {
        base: format!("http://{addr}"),
        state,
    }
}

// ---------------------------------------------------------------------
// Aides
// ---------------------------------------------------------------------

/// Autorise un compte à 1Life et l'affiche dans Taff ; rend son client
/// connecté.
async fn granted(app: &TestApp, email: &str) -> Client {
    dashu::registry::grant(&app.state.db, email, "1life")
        .await
        .unwrap();
    let mut client = app.logged_in(email).await;
    let taff = app.space_id(email, "taff").await;
    let shown = client
        .post(
            &format!("/settings/spaces/{taff}/tools/1life"),
            &[("shown", "1")],
        )
        .await;
    assert_eq!(shown.status, StatusCode::SEE_OTHER);
    client
}

/// Crée un dossier dans Fichiers ; rend son identifiant.
async fn folder(
    app: &TestApp,
    client: &mut Client,
    space: i64,
    parent: Option<i64>,
    name: &str,
) -> i64 {
    let parent_raw = parent.map(|p| p.to_string()).unwrap_or_default();
    let space_raw = space.to_string();
    let mut form = vec![("name", name)];
    if parent.is_some() {
        form.push(("parent_id", parent_raw.as_str()));
    } else {
        form.push(("space_id", space_raw.as_str()));
    }
    let created = client.post("/folders", &form).await;
    assert_eq!(created.status, StatusCode::SEE_OTHER, "{}", created.body);
    sqlx::query_scalar("SELECT id FROM folders WHERE name = ? AND parent_id IS ? AND space_id = ?")
        .bind(name)
        .bind(parent)
        .bind(space)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

/// Un compte autorisé, avec « Taff / Clients » comme dossier des clients et
/// un client « Atelier Test » (dossier, plus Audit, CR et Fichiers) ; rend
/// (client connecté, espace Taff, dossier du client).
async fn ready(app: &TestApp, email: &str) -> (Client, i64, i64) {
    let mut client = granted(app, email).await;
    let taff = app.space_id(email, "taff").await;
    let book = folder(app, &mut client, taff, None, "Clients").await;
    let chosen = client
        .post(
            &format!("{CLIENTS}/dossier"),
            &[
                ("space_id", &taff.to_string()),
                ("folder_id", &book.to_string()),
            ],
        )
        .await;
    assert_eq!(chosen.status, StatusCode::SEE_OTHER);
    let created = client.post(CLIENTS, &[("name", "Atelier Test")]).await;
    assert_eq!(created.status, StatusCode::SEE_OTHER, "{}", created.body);
    let id = created
        .location()
        .rsplit('/')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    (client, taff, id)
}

/// Commence un audit pour le client ; rend (audit, note de notes).
async fn start(app: &TestApp, client: &mut Client, client_id: i64, template: &str) -> (i64, i64) {
    let started = client
        .post(
            LIST,
            &[
                ("client", &client_id.to_string()),
                ("date", DATE),
                ("titre", "Rapport d'audit ERP"),
                ("approbateur", "Dominique Exemple"),
                ("modele", template),
            ],
        )
        .await;
    assert_eq!(started.status, StatusCode::SEE_OTHER, "{}", started.body);
    let notes = common::note_id(&started);
    let audit: i64 = sqlx::query_scalar("SELECT id FROM onelife_audits WHERE notes_id = ?")
        .bind(notes)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    (audit, notes)
}

/// Ouvre une page, en suivant la redirection vers l'espace de la note s'il
/// y en a une.
async fn open(client: &mut Client, path: &str) -> common::TestResponse {
    let page = client.get(path).await;
    if page.status == StatusCode::SEE_OTHER {
        client.follow(page).await
    } else {
        page
    }
}

/// Écrit les notes de l'audit, comme l'éditeur.
async fn write_notes(client: &mut Client, notes: i64, body: &str) {
    let saved = client
        .post(
            &format!("/notes/{notes}"),
            &[("title", "Notes d'audit"), ("body_md", body)],
        )
        .await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.body);
}

/// « Générer l'audit » avec ce modèle d'IA ; rend la réponse.
async fn generate(client: &mut Client, audit: i64, model: &str) -> common::TestResponse {
    client
        .post(&format!("{LIST}/{audit}/generer"), &[("modele_ia", model)])
        .await
}

/// Attend la fin de la dernière génération ; rend (statut, message).
async fn finished(app: &TestApp, audit: i64) -> (String, String) {
    for _ in 0..200 {
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT status, message FROM onelife_audit_runs WHERE audit_id = ? ORDER BY id DESC LIMIT 1",
        )
        .bind(audit)
        .fetch_optional(&app.state.db)
        .await
        .unwrap();
        if let Some(row) = row.filter(|(status, _)| status != "running") {
            return row;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("la génération ne finit pas");
}

/// Les notes du compte : (titre, texte, dossier), par identifiant.
async fn notes(app: &TestApp) -> Vec<(i64, String, String, Option<i64>)> {
    sqlx::query_as("SELECT id, title, body_md, folder_id FROM notes ORDER BY id")
        .fetch_all(&app.state.db)
        .await
        .unwrap()
}

async fn count(app: &TestApp, sql: &str) -> i64 {
    sqlx::query_scalar(sql)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

/// Le sous-dossier « Audit » d'un client.
async fn audit_folder(app: &TestApp, client_id: i64) -> i64 {
    sqlx::query_scalar("SELECT id FROM folders WHERE parent_id = ? AND name = 'Audit'")
        .bind(client_id)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

// ---------------------------------------------------------------------
// Les critères
// ---------------------------------------------------------------------

#[tokio::test]
async fn commencer_un_audit_cree_les_notes_dans_le_dossier_audit() {
    let fake = fake_anthropic().await;
    let app = TestApp::with_ai(&fake.base).await;
    let (mut alice, taff, client_id) = ready(&app, ALICE).await;

    // Le formulaire est prérempli : la date du jour, le titre par défaut,
    // le client demandé.
    let form = alice
        .get(&format!("{LIST}/nouveau?client={client_id}"))
        .await;
    assert_eq!(form.status, StatusCode::OK);
    let today = dashu::time::paris_date(dashu::time::now())
        .format("%Y-%m-%d")
        .to_string();
    assert!(
        form.body.contains(&format!("value=\"{today}\"")),
        "{}",
        form.body
    );
    assert!(form.body.contains("Rapport d&#39;audit ERP"));
    assert!(form.body.contains(&format!(
        "<option value=\"{client_id}\" selected>Atelier Test</option>"
    )));

    let (audit, notes_id) = start(&app, &mut alice, client_id, "integre").await;
    let all = notes(&app).await;
    let (_, title, body, place) = all.iter().find(|n| n.0 == notes_id).unwrap();
    assert_eq!(title, "Notes d'audit — Atelier Test — 27 novembre 2026");
    assert!(body.is_empty());
    assert_eq!(*place, Some(audit_folder(&app, client_id).await));

    // L'éditeur s'ouvre, avec « Générer l'audit » et le retour vers l'audit.
    let editor = open(
        &mut alice,
        &format!("/notes/{notes_id}?depuis={LIST}/{audit}"),
    )
    .await;
    assert_eq!(editor.status, StatusCode::OK);
    assert!(editor.body.contains("Générer l'audit"), "{}", editor.body);
    assert!(editor.body.contains(&format!("{LIST}/{audit}")));

    // Le sous-dossier Audit manque : il est créé.
    let book = count(
        &app,
        &format!("SELECT parent_id FROM folders WHERE id = {client_id}"),
    )
    .await;
    let bare = folder(&app, &mut alice, taff, Some(book), "Client Exemple").await;
    let (_, bare_notes) = start(&app, &mut alice, bare, "integre").await;
    let made = audit_folder(&app, bare).await;
    let all = notes(&app).await;
    assert_eq!(
        all.iter().find(|n| n.0 == bare_notes).unwrap().3,
        Some(made)
    );

    // La page Audit et la fiche client les listent, « Notes en cours ».
    let list = alice.get(LIST).await;
    assert_eq!(list.status, StatusCode::OK);
    assert!(list.body.contains("Atelier Test") && list.body.contains("Client Exemple"));
    assert!(list.body.contains("Notes en cours"));
    let fiche = alice.get(&format!("{CLIENTS}/{client_id}")).await;
    assert!(fiche.body.contains(&format!("{LIST}/{audit}")));
    assert!(!fiche.body.contains("Client Exemple"));
    // Le filtre par client.
    let filtered = alice.get(&format!("{LIST}?client={bare}")).await;
    assert!(filtered.body.contains("Client Exemple"));
    assert!(!filtered.body.contains(&format!("{LIST}/{audit}\"")));
}

#[tokio::test]
async fn generer_de_bout_en_bout() {
    let fake = fake_anthropic().await;
    fake.reply(Reply::Text(answer(DOCUMENT, &["Le stock du dépôt 2"])));
    let app = TestApp::with_ai(&fake.base).await;
    let (mut alice, _, client_id) = ready(&app, ALICE).await;
    let (audit, notes_id) = start(&app, &mut alice, client_id, "integre").await;
    write_notes(
        &mut alice,
        notes_id,
        "OP: les commandes sont saisies deux fois\n!! pas de lien avec la compta",
    )
    .await;

    // La page propose les modèles d'IA, Haiku 5.5 par défaut, avec leur coût.
    let page = alice.get(&format!("{LIST}/{audit}")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(
        page.body
            .contains("<option value=\"claude-haiku-5-5\" selected>Haiku 5.5 — "),
        "{}",
        page.body
    );
    assert!(page.body.contains("Sonnet 5.5 — "));
    assert!(page.body.contains(" au plus</option>"));

    let sent = generate(&mut alice, audit, "claude-haiku-5-5").await;
    assert_eq!(sent.status, StatusCode::SEE_OTHER);
    assert_eq!(sent.location(), format!("{LIST}/{audit}"));
    assert_eq!(finished(&app, audit).await, ("done".into(), String::new()));

    // Ce qui est parti : la clé, la version de l'API, le modèle, la
    // consigne système, le modèle rempli et les notes.
    let seen = fake.seen();
    assert_eq!(seen.len(), 1);
    let request = &seen[0];
    assert_eq!(request.api_key, "cle-de-test");
    assert_eq!(request.version, "2026-10-21");
    assert_eq!(request.body["model"], "claude-haiku-5-5");
    assert_eq!(request.body["max_tokens"], 16_000);
    assert_eq!(request.body["output_config"]["effort"], "low");
    assert!(
        request.body["system"]
            .as_str()
            .unwrap()
            .contains("vouvoyant")
    );
    assert!(request.body.get("tools").is_none(), "l'IA n'a aucun outil");
    let message = request.body["messages"][0]["content"].as_str().unwrap();
    assert!(
        message.contains("<modele>\n# Rapport d'audit ERP"),
        "{message}"
    );
    assert!(message.contains("- **Client :** Atelier Test"));
    assert!(message.contains("- **Approbateur :** Dominique Exemple"));
    assert!(message.contains(
        "<notes>\nOP: les commandes sont saisies deux fois\n!! pas de lien avec la compta\n</notes>"
    ));

    // Le résultat : une note dans le dossier Audit, avec le document seul.
    let all = notes(&app).await;
    let (version_note, title, body, folder) = all.last().unwrap().clone();
    assert_eq!(title, "Audit — Atelier Test — 27 novembre 2026");
    assert_eq!(body, DOCUMENT);
    assert_eq!(folder, Some(audit_folder(&app, client_id).await));
    let (model, input, output, cost): (String, i64, i64, i64) = sqlx::query_as(
        "SELECT model, input_tokens, output_tokens, cost_micro FROM onelife_audit_versions WHERE audit_id = ?",
    )
    .bind(audit)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(
        (model.as_str(), input, output),
        ("claude-haiku-5-5", 1234, 567)
    );
    // 1 234 × 0,10 $ + 567 × 0,50 $ par million de tokens.
    assert_eq!(cost, 407);

    // La page montre la version : modèle, tokens, coût ; la liste, « Généré ».
    let page = alice.get(&format!("{LIST}/{audit}")).await;
    assert!(
        page.body
            .contains("v1 · Audit — Atelier Test — 27 novembre 2026"),
        "{}",
        page.body
    );
    assert!(
        page.body
            .contains("Haiku 5.5 · 1\u{202f}234 + 567 tokens · 0,0004 $")
    );
    assert!(page.body.contains(&format!("/notes/{version_note}")));
    assert!(alice.get(LIST).await.body.contains("Généré"));
    // Le résultat est une note ordinaire, nettoyée : l'éditeur l'ouvre.
    let editor = open(&mut alice, &format!("/notes/{version_note}")).await;
    assert_eq!(editor.status, StatusCode::OK);
    assert!(
        editor.body.contains("<h1>Rapport d&#39;audit ERP</h1>")
            || editor.body.contains("Rapport d'audit ERP")
    );
    assert!(editor.body.contains("v1 · Haiku 5.5"));
}

#[tokio::test]
async fn regenerer_cree_v2_sans_toucher_v1_ni_les_notes() {
    let fake = fake_anthropic().await;
    fake.reply(Reply::Text(answer("# Première version", &[])));
    fake.reply(Reply::Text(answer("# Deuxième version", &[])));
    let app = TestApp::with_ai(&fake.base).await;
    let (mut alice, _, client_id) = ready(&app, ALICE).await;
    let (audit, notes_id) = start(&app, &mut alice, client_id, "integre").await;
    write_notes(&mut alice, notes_id, "Les notes d'origine.").await;

    generate(&mut alice, audit, "claude-haiku-5-5").await;
    finished(&app, audit).await;
    // Rémi retouche la v1 à la main.
    let v1: i64 = count(
        &app,
        "SELECT note_id FROM onelife_audit_versions WHERE number = 1",
    )
    .await;
    alice
        .post(
            &format!("/notes/{v1}"),
            &[
                ("title", "Audit — Atelier Test — 27 novembre 2026"),
                ("body_md", "# Première version, relue"),
            ],
        )
        .await;

    generate(&mut alice, audit, "claude-haiku-5-5").await;
    assert_eq!(finished(&app, audit).await.0, "done");
    let all = notes(&app).await;
    let find = |id: i64| all.iter().find(|n| n.0 == id).unwrap().clone();
    assert_eq!(find(v1).2, "# Première version, relue");
    assert_eq!(find(notes_id).2, "Les notes d'origine.");
    let v2: i64 = count(
        &app,
        "SELECT note_id FROM onelife_audit_versions WHERE number = 2",
    )
    .await;
    assert_eq!(find(v2).1, "Audit — Atelier Test — 27 novembre 2026 (v2)");
    assert_eq!(find(v2).2, "# Deuxième version");
    let page = alice.get(&format!("{LIST}/{audit}")).await;
    assert!(page.body.contains("v2 · ") && page.body.contains("v1 · "));
}

#[tokio::test]
async fn les_points_non_repris_restent_hors_de_la_note() {
    let fake = fake_anthropic().await;
    fake.reply(Reply::Text(answer(
        DOCUMENT,
        &["Le stock du dépôt 2", "[ ] La date de bascule"],
    )));
    let app = TestApp::with_ai(&fake.base).await;
    let (mut alice, _, client_id) = ready(&app, ALICE).await;
    let (audit, _) = start(&app, &mut alice, client_id, "integre").await;
    generate(&mut alice, audit, "claude-haiku-5-5").await;
    finished(&app, audit).await;

    let all = notes(&app).await;
    let body = &all.last().unwrap().2;
    assert!(
        !body.contains("POINTS NON REPRIS") && !body.contains("dépôt 2"),
        "{body}"
    );

    let page = alice.get(&format!("{LIST}/{audit}")).await;
    assert!(page.body.contains("Points non repris (v1)"));
    assert!(page.body.contains("Le stock du dépôt 2"));
    assert!(page.body.contains("La date de bascule"));
    assert!(!page.body.contains("[ ] La date"));

    // Cocher un point : l'état s'enregistre.
    let point: i64 = count(
        &app,
        "SELECT id FROM onelife_audit_points WHERE position = 0",
    )
    .await;
    alice.with_header("hx-request", "true");
    let checked = alice
        .post(&format!("{LIST}/{audit}/points/{point}"), &[("done", "1")])
        .await;
    assert_eq!(checked.status, StatusCode::OK);
    assert!(checked.body.contains("is-done") && checked.body.contains("checked"));
    assert_eq!(
        count(
            &app,
            &format!("SELECT done FROM onelife_audit_points WHERE id = {point}")
        )
        .await,
        1
    );
    let page = alice.get(&format!("{LIST}/{audit}")).await;
    assert!(page.body.contains("audit-point is-done"));
    // Décocher.
    alice.with_header("hx-request", "true");
    alice
        .post(&format!("{LIST}/{audit}/points/{point}"), &[])
        .await;
    assert_eq!(
        count(
            &app,
            &format!("SELECT done FROM onelife_audit_points WHERE id = {point}")
        )
        .await,
        0
    );
    // Un point d'un autre audit, ou inconnu : 404.
    assert_eq!(
        alice
            .post(&format!("{LIST}/{audit}/points/999"), &[("done", "1")])
            .await
            .status,
        StatusCode::NOT_FOUND
    );

    // Sans la ligne séparatrice : tout est le document, et la page le dit.
    fake.reply(Reply::Text("# Rapport sans liste".into()));
    generate(&mut alice, audit, "claude-haiku-5-5").await;
    let (status, message) = finished(&app, audit).await;
    assert_eq!(status, "done");
    assert!(message.contains("n'a pas séparé les points non repris"));
    let page = alice.get(&format!("{LIST}/{audit}")).await;
    assert!(
        page.body
            .contains("n&#39;a pas séparé les points non repris")
    );
}

#[tokio::test]
async fn les_consignes_sont_retirees_meme_recopiees() {
    let fake = fake_anthropic().await;
    fake.reply(Reply::Text(answer(
        "# Rapport\n\n## Contexte\n\n> Consigne : présenter le client (activité, secteur, effectif) et la raison de l'audit.\n\nAtelier Test fabrique des meubles.\n\n> **Consigne :** recopiée autrement\n\n> Une citation du client reste.",
        &[],
    )));
    let app = TestApp::with_ai(&fake.base).await;
    let (mut alice, _, client_id) = ready(&app, ALICE).await;
    let (audit, _) = start(&app, &mut alice, client_id, "integre").await;
    generate(&mut alice, audit, "claude-haiku-5-5").await;
    finished(&app, audit).await;

    // Les consignes partent chez l'IA : elles la guident.
    let message = fake.seen()[0].body["messages"][0]["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        message.contains("> Consigne : présenter le client"),
        "{message}"
    );
    // Elles ne restent pas dans le document.
    let body = notes(&app).await.last().unwrap().2.clone();
    assert_eq!(
        body,
        "# Rapport\n\n## Contexte\n\nAtelier Test fabrique des meubles.\n\n> Une citation du client reste."
    );
}

#[tokio::test]
async fn variables_vides_et_inconnues() {
    let fake = fake_anthropic().await;
    let app = TestApp::with_ai(&fake.base).await;
    let (mut alice, taff, client_id) = ready(&app, ALICE).await;
    // Un modèle en note, dans le dossier des modèles.
    let models = folder(&app, &mut alice, taff, None, "Modèles").await;
    let chosen = alice
        .post(
            "/e/taff/1life/reglages/modeles",
            &[
                ("space_id", &taff.to_string()),
                ("folder_id", &models.to_string()),
            ],
        )
        .await;
    assert_eq!(chosen.status, StatusCode::SEE_OTHER);
    let note = common::note_id(
        &alice
            .post(&format!("/e/taff/notes?dossier={models}"), &[])
            .await,
    );
    alice
        .post(
            &format!("/notes/{note}"),
            &[
                ("title", "Modèle court"),
                ("body_md", "# {{client.raison_sociale}}\n\nAdresse : {{client.adresse}}\nSIRET : {{ client.siret }}\nPar {{consultant.nom}}"),
            ],
        )
        .await;
    // La fiche : une raison sociale, pas d'adresse ; le profil a un nom.
    alice.with_header("hx-request", "true");
    let saved = alice
        .post(
            &format!("{CLIENTS}/{client_id}/identite"),
            &[("raison_sociale", "Atelier Test SAS"), ("adresse", "")],
        )
        .await;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.body);
    sqlx::query("UPDATE users SET first_name = 'Camille', last_name = 'Exemple' WHERE email = ?")
        .bind(ALICE)
        .execute(&app.state.db)
        .await
        .unwrap();

    let (audit, _) = start(&app, &mut alice, client_id, &format!("note:{note}")).await;
    // La page signale, avant de générer, ce qui reste à compléter.
    let page = alice.get(&format!("{LIST}/{audit}")).await;
    assert!(page.body.contains("Modèle : Modèle court"), "{}", page.body);
    let missing = between(&page.body, "<ul class=\"missing-list\">", "</ul>").unwrap();
    assert!(missing.contains("<li>Adresse</li>") && missing.contains("<li>client.siret</li>"));
    assert!(!missing.contains("Raison sociale"));

    generate(&mut alice, audit, "claude-haiku-5-5").await;
    finished(&app, audit).await;
    let message = fake.seen()[0].body["messages"][0]["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(message.contains("# Atelier Test SAS"), "{message}");
    assert!(message.contains("Adresse : [À compléter : Adresse]"));
    assert!(message.contains("SIRET : [À compléter : client.siret]"));
    assert!(message.contains("Par Camille Exemple"));
    assert!(!message.contains("{{"));
}

#[tokio::test]
async fn le_modele_d_ia_choisi_est_celui_envoye() {
    let fake = fake_anthropic().await;
    let app = TestApp::with_ai(&fake.base).await;
    let (mut alice, _, client_id) = ready(&app, ALICE).await;
    let (audit, _) = start(&app, &mut alice, client_id, "integre").await;

    generate(&mut alice, audit, "claude-sonnet-5-5").await;
    finished(&app, audit).await;
    assert_eq!(fake.seen()[0].body["model"], "claude-sonnet-5-5");
    let page = alice.get(&format!("{LIST}/{audit}")).await;
    // 1 234 × 2 $ + 567 × 10 $ par million de tokens.
    assert!(
        page.body
            .contains("Sonnet 5.5 · 1\u{202f}234 + 567 tokens · 0,0081 $"),
        "{}",
        page.body
    );

    // Un modèle hors de ONELIFE_AI_MODELS : refusé, rien n'est envoyé.
    let refused = generate(&mut alice, audit, "claude-autre").await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    assert_eq!(fake.seen().len(), 1);

    // Le modèle d'IA par défaut des réglages est présélectionné.
    let saved = alice
        .post(
            "/e/taff/1life/reglages",
            &[("ai_model", "claude-sonnet-5-5"), ("template", "integre")],
        )
        .await;
    assert_eq!(saved.status, StatusCode::SEE_OTHER);
    let page = alice.get(&format!("{LIST}/{audit}")).await;
    assert!(
        page.body
            .contains("<option value=\"claude-sonnet-5-5\" selected>")
    );
}

#[tokio::test]
async fn sans_cle_rien_n_est_envoye() {
    let fake = fake_anthropic().await;
    let app = TestApp::with_ai_without_key(&fake.base).await;
    let (mut alice, _, client_id) = ready(&app, ALICE).await;
    let (audit, notes_id) = start(&app, &mut alice, client_id, "integre").await;

    let page = alice.get(&format!("{LIST}/{audit}")).await;
    assert!(page.body.contains("IA non configurée"), "{}", page.body);
    assert!(page.body.contains("README"));
    let form = between(&page.body, "class=\"stack generate-form\"", "</form>").unwrap();
    assert!(form.contains("type=\"submit\" disabled"), "{form}");
    // Dans l'éditeur aussi.
    let editor = open(&mut alice, &format!("/notes/{notes_id}")).await;
    assert!(editor.body.contains("IA non configurée"));

    let sent = generate(&mut alice, audit, "claude-haiku-5-5").await;
    assert_eq!(sent.status, StatusCode::SEE_OTHER);
    assert!(fake.seen().is_empty());
    assert_eq!(
        count(&app, "SELECT COUNT(*) FROM onelife_audit_runs").await,
        0
    );
    assert_eq!(
        count(&app, "SELECT COUNT(*) FROM onelife_audit_versions").await,
        0
    );
}

#[tokio::test]
async fn reponse_coupee_et_erreurs_de_l_api_sans_rien_creer() {
    let fake = fake_anthropic().await;
    let app = TestApp::with_ai(&fake.base).await;
    let (mut alice, _, client_id) = ready(&app, ALICE).await;
    let (audit, _) = start(&app, &mut alice, client_id, "integre").await;
    let before = notes(&app).await.len();

    for (reply, expected) in [
        (Reply::Cut, "La réponse a été coupée"),
        (
            Reply::Error(401, "authentication_error"),
            "Clé refusée par Anthropic",
        ),
        (Reply::Error(402, "billing_error"), "plus de crédit"),
        (
            Reply::Error(429, "rate_limit_error"),
            "Quota de l'API atteint",
        ),
        (
            Reply::Error(529, "overloaded_error"),
            "surchargée ou en panne",
        ),
        (Reply::Error(404, "not_found_error"), "Modèle d'IA inconnu"),
        (
            Reply::Slow(Duration::from_secs(4)),
            "Pas de réponse de l'IA en 2 secondes",
        ),
    ] {
        fake.reply(reply);
        let sent = generate(&mut alice, audit, "claude-haiku-5-5").await;
        assert_eq!(sent.status, StatusCode::SEE_OTHER);
        let (status, message) = finished(&app, audit).await;
        assert_eq!(status, "failed", "{expected}");
        assert!(message.contains(expected), "{message}");
        assert!(
            message.contains("Rien n'est créé") || message.contains("rien n'est créé"),
            "{message}"
        );
        // Rien de créé : ni note, ni version, ni point.
        assert_eq!(notes(&app).await.len(), before, "{expected}");
        assert_eq!(
            count(&app, "SELECT COUNT(*) FROM onelife_audit_versions").await,
            0
        );
        // La page le dit ; la clé n'y est jamais.
        let page = alice.get(&format!("{LIST}/{audit}")).await;
        assert!(page.body.contains("alert alert-error"), "{expected}");
        assert!(!page.body.contains("cle-de-test"));
    }
    assert_eq!(
        count(&app, "SELECT COUNT(*) FROM onelife_audit_points").await,
        0
    );
}

#[tokio::test]
async fn limites_de_taille_par_jour_et_une_a_la_fois() {
    let fake = fake_anthropic().await;
    let app = TestApp::with_ai(&fake.base).await;
    let (mut alice, _, client_id) = ready(&app, ALICE).await;
    let (audit, notes_id) = start(&app, &mut alice, client_id, "integre").await;
    let user = app.user_id(ALICE).await;

    // Plus de 80 000 tokens estimés : rien n'est envoyé.
    write_notes(
        &mut alice,
        notes_id,
        &"Une ligne de notes assez longue. ".repeat(8_000),
    )
    .await;
    let page = alice.get(&format!("{LIST}/{audit}")).await;
    assert!(page.body.contains("Demande trop longue"), "{}", page.body);
    let refused = generate(&mut alice, audit, "claude-haiku-5-5").await;
    assert_eq!(refused.status, StatusCode::SEE_OTHER);
    let page = alice.follow(refused).await;
    assert!(page.body.contains("Demande trop longue"));
    assert!(fake.seen().is_empty());
    assert_eq!(
        count(&app, "SELECT COUNT(*) FROM onelife_audit_runs").await,
        0
    );
    write_notes(&mut alice, notes_id, "Des notes courtes.").await;

    // Une à la fois : une génération en cours bloque la suivante.
    let now = dashu::time::now();
    sqlx::query("INSERT INTO onelife_audit_runs (user_id, audit_id, model, status, started_at) VALUES (?, ?, 'claude-haiku-5-5', 'running', ?)")
        .bind(user)
        .bind(audit)
        .bind(now)
        .execute(&app.state.db)
        .await
        .unwrap();
    let busy = generate(&mut alice, audit, "claude-haiku-5-5").await;
    let busy = alice.follow(busy).await;
    assert!(
        busy.body.contains("Une génération est déjà en cours"),
        "{}",
        busy.body
    );
    // La page suit la génération en cours, toutes les deux secondes.
    assert!(busy.body.contains("hx-trigger=\"every 2s\""));
    assert!(fake.seen().is_empty());
    // Au redémarrage, une génération coupée est marquée échouée.
    dashu::onelife::audit::recover(&app.state.db).await.unwrap();
    assert_eq!(finished(&app, audit).await.0, "failed");
    let status = alice
        .get_fragment(&format!("{LIST}/{audit}/generation"))
        .await;
    assert_eq!(status.header("hx-refresh"), "true");

    // Vingt par jour : la 21e n'est pas envoyée.
    for _ in 0..19 {
        sqlx::query("INSERT INTO onelife_audit_runs (user_id, audit_id, model, status, started_at) VALUES (?, ?, 'claude-haiku-5-5', 'done', ?)")
            .bind(user)
            .bind(audit)
            .bind(now)
            .execute(&app.state.db)
            .await
            .unwrap();
    }
    let page = alice.get(&format!("{LIST}/{audit}")).await;
    assert!(
        page.body
            .contains("Générations aujourd&#39;hui : 20 sur 20")
            || page.body.contains("Générations aujourd'hui : 20 sur 20"),
        "{}",
        page.body
    );
    let over = generate(&mut alice, audit, "claude-haiku-5-5").await;
    let over = alice.follow(over).await;
    assert!(
        over.body.contains("20 générations par jour au plus"),
        "{}",
        over.body
    );
    assert!(fake.seen().is_empty());
}

#[tokio::test]
async fn le_modele_integre_sert_sans_dossier_des_modeles() {
    let fake = fake_anthropic().await;
    let app = TestApp::with_ai(&fake.base).await;
    let (mut alice, _, client_id) = ready(&app, ALICE).await;
    let (audit, _) = start(&app, &mut alice, client_id, "integre").await;
    let page = alice.get(&format!("{LIST}/{audit}")).await;
    assert!(page.body.contains("Modèle : Modèle intégré"));
    generate(&mut alice, audit, "claude-haiku-5-5").await;
    finished(&app, audit).await;
    let message = fake.seen()[0].body["messages"][0]["content"]
        .as_str()
        .unwrap()
        .to_string();
    for heading in [
        "## Contexte",
        "## Périmètre",
        "## Constats",
        "## Recommandations",
        "## Prochaines étapes",
    ] {
        assert!(message.contains(heading), "{heading}");
    }
    // Rien de 1Life dans le modèle intégré ni dans la consigne système.
    assert!(!dashu::onelife::templates::BUILTIN.contains("1Life"));
    assert!(!dashu::onelife::audit::SYSTEM.contains("1Life"));

    // L'aide montre la syntaxe, les variables du registre et ce modèle.
    let help = alice.get(&format!("{LIST}/modeles")).await;
    assert_eq!(help.status, StatusCode::OK);
    for field in dashu::onelife::fields::FIELDS {
        assert!(
            help.body.contains(&format!("{{{{client.{}}}}}", field.key)),
            "{}",
            field.key
        );
    }
    assert!(help.body.contains("{{consultant.email}}") && help.body.contains("OP:"));
    assert!(help.body.contains("## Prochaines étapes"));
}

#[tokio::test]
async fn un_fichier_md_du_dossier_des_modeles_sert_de_modele() {
    let fake = fake_anthropic().await;
    let app = TestApp::with_ai(&fake.base).await;
    let (mut alice, taff, client_id) = ready(&app, ALICE).await;
    let models = folder(&app, &mut alice, taff, None, "Modèles").await;
    alice
        .post(
            "/e/taff/1life/reglages/modeles",
            &[
                ("space_id", &taff.to_string()),
                ("folder_id", &models.to_string()),
            ],
        )
        .await;
    let uploaded = alice
        .upload_in_folder(
            "modele-audit.md",
            "# Audit de {{client.raison_sociale}}\n\n## Logistique\n\n> Consigne : les flux de stock.\n".as_bytes(),
            models,
        )
        .await;
    assert_eq!(uploaded.status, StatusCode::CREATED, "{}", uploaded.body);
    let file: i64 = count(&app, "SELECT id FROM files WHERE name = 'modele-audit.md'").await;

    // Il est proposé dans les réglages et au début d'un audit.
    let settings = alice.get("/e/taff/1life/reglages").await;
    assert!(
        settings.body.contains(&format!(
            "<option value=\"fichier:{file}\">modele-audit.md</option>"
        )),
        "{}",
        settings.body
    );
    assert!(settings.body.contains("Taff / Modèles"));
    let (audit, _) = start(&app, &mut alice, client_id, &format!("fichier:{file}")).await;
    generate(&mut alice, audit, "claude-haiku-5-5").await;
    finished(&app, audit).await;
    let message = fake.seen()[0].body["messages"][0]["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        message.contains("# Audit de Atelier Test\n\n## Logistique"),
        "{message}"
    );

    // Le fichier mis à la corbeille : le modèle intégré le remplace, et la
    // page le dit.
    let deleted = alice.post(&format!("/files/{file}/delete"), &[]).await;
    assert!(deleted.status.is_redirection() || deleted.status.is_success());
    let page = alice.get(&format!("{LIST}/{audit}")).await;
    assert!(
        page.body.contains("n&#39;existe plus") || page.body.contains("n'existe plus"),
        "{}",
        page.body
    );
}

#[tokio::test]
async fn deux_comptes_ne_voient_pas_les_audits_l_un_de_l_autre() {
    let fake = fake_anthropic().await;
    let app = TestApp::with_ai(&fake.base).await;
    let (mut alice, _, client_id) = ready(&app, ALICE).await;
    let (audit, notes_id) = start(&app, &mut alice, client_id, "integre").await;
    generate(&mut alice, audit, "claude-haiku-5-5").await;
    finished(&app, audit).await;

    let (mut bob, _, _) = ready(&app, BOB).await;
    assert!(
        !bob.get(LIST)
            .await
            .body
            .contains(&format!("{LIST}/{audit}"))
    );
    for path in [
        format!("{LIST}/{audit}"),
        format!("{LIST}/{audit}/generation"),
        format!("{LIST}/{audit}/word"),
    ] {
        assert_eq!(bob.get(&path).await.status, StatusCode::NOT_FOUND, "{path}");
    }
    for path in [
        format!("{LIST}/{audit}/generer"),
        format!("{LIST}/{audit}/points/1"),
    ] {
        assert_eq!(
            bob.post(&path, &[("modele_ia", "claude-haiku-5-5")])
                .await
                .status,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    // Bob ne commence pas d'audit sur le client d'Alice.
    let refused = bob
        .post(
            LIST,
            &[
                ("client", &client_id.to_string()),
                ("date", DATE),
                ("titre", "x"),
                ("modele", "integre"),
            ],
        )
        .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
    assert_eq!(
        bob.get(&format!("/notes/{notes_id}")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(fake.seen().len(), 1);
}

#[tokio::test]
async fn sans_autorisation_le_module_repond_404_partout() {
    let fake = fake_anthropic().await;
    let app = TestApp::with_ai(&fake.base).await;
    let (mut alice, _, client_id) = ready(&app, ALICE).await;
    let (audit, _) = start(&app, &mut alice, client_id, "integre").await;

    // Bob n'est pas autorisé ; un visiteur n'a pas de session.
    let mut bob = app.logged_in(BOB).await;
    let mut visitor = app.client();
    for prefix in ["", "/e/taff"] {
        let gets = [
            format!("{prefix}/1life/audit"),
            format!("{prefix}/1life/audit/nouveau"),
            format!("{prefix}/1life/audit/modeles"),
            format!("{prefix}/1life/audit/{audit}"),
            format!("{prefix}/1life/audit/{audit}/generation"),
            format!("{prefix}/1life/audit/{audit}/word"),
            format!("{prefix}/1life/audit/abc"),
            format!("{prefix}/1life/reglages"),
        ];
        for path in &gets {
            assert_eq!(bob.get(path).await.status, StatusCode::NOT_FOUND, "{path}");
            assert_eq!(
                visitor.get(path).await.status,
                StatusCode::NOT_FOUND,
                "{path}"
            );
        }
        let posts = [
            format!("{prefix}/1life/audit"),
            format!("{prefix}/1life/audit/{audit}/generer"),
            format!("{prefix}/1life/audit/{audit}/points/1"),
            format!("{prefix}/1life/reglages"),
            format!("{prefix}/1life/reglages/modeles"),
            format!("{prefix}/1life/clients/{client_id}/identite"),
        ];
        for path in &posts {
            assert_eq!(
                bob.post(path, &[("modele_ia", "claude-haiku-5-5")])
                    .await
                    .status,
                StatusCode::NOT_FOUND,
                "{path}"
            );
        }
    }
    assert_eq!(
        bob.get("/destinations?modeles=1").await.status,
        StatusCode::NOT_FOUND
    );
    assert!(fake.seen().is_empty());
    // Rien de l'audit dans l'API.
    let (status, _) = app
        .api(axum::http::Method::GET, "/api/v1/1life/audit", None, None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn corbeille_et_restauration() {
    let fake = fake_anthropic().await;
    let app = TestApp::with_ai(&fake.base).await;
    let (mut alice, _, client_id) = ready(&app, ALICE).await;
    let (audit, notes_id) = start(&app, &mut alice, client_id, "integre").await;
    generate(&mut alice, audit, "claude-haiku-5-5").await;
    finished(&app, audit).await;
    let page = format!("{LIST}/{audit}");

    // Les notes à la corbeille : l'audit disparaît, de la liste et de la fiche.
    let deleted = alice.post(&format!("/notes/{notes_id}/delete"), &[]).await;
    assert_eq!(deleted.status, StatusCode::SEE_OTHER);
    assert_eq!(alice.get(&page).await.status, StatusCode::NOT_FOUND);
    assert!(!alice.get(LIST).await.body.contains(&page));
    assert!(
        !alice
            .get(&format!("{CLIENTS}/{client_id}"))
            .await
            .body
            .contains(&page)
    );
    // Restaurées : il revient, avec sa version.
    let restored = alice
        .post(&format!("/settings/trash/note/{notes_id}/restore"), &[])
        .await;
    assert_eq!(restored.status, StatusCode::SEE_OTHER);
    let back = alice.get(&page).await;
    assert_eq!(back.status, StatusCode::OK);
    assert!(back.body.contains("v1 · "));

    // Une version à la corbeille ne compte plus : « Notes en cours ».
    let v1: i64 = count(&app, "SELECT note_id FROM onelife_audit_versions").await;
    alice.post(&format!("/notes/{v1}/delete"), &[]).await;
    assert!(alice.get(LIST).await.body.contains("Notes en cours"));
    alice
        .post(&format!("/settings/trash/note/{v1}/restore"), &[])
        .await;
    assert!(alice.get(LIST).await.body.contains("Généré"));

    // Le client entier à la corbeille, puis restauré.
    let gone = alice
        .post(
            &format!("/folders/{client_id}/delete"),
            &[("contenu", "supprimer")],
        )
        .await;
    assert_eq!(gone.status, StatusCode::SEE_OTHER);
    assert_eq!(alice.get(&page).await.status, StatusCode::NOT_FOUND);
    alice
        .post(&format!("/settings/trash/dossier/{client_id}/restore"), &[])
        .await;
    assert_eq!(alice.get(&page).await.status, StatusCode::OK);
}

// ---------------------------------------------------------------------
// Word
// ---------------------------------------------------------------------

/// Tâches prises par le faux service de conversion : leur `job.json` et le
/// document de référence trouvé à côté.
type Jobs = Arc<Mutex<Vec<(JobSpec, Option<Vec<u8>>)>>>;

/// Faux service de conversion : prend les tâches prêtes, note le document de
/// référence et écrit un faux `.docx`.
fn fake_worker(work_dir: PathBuf) -> (tokio::task::JoinHandle<()>, Jobs) {
    let jobs: Jobs = Arc::default();
    let log = jobs.clone();
    let handle = tokio::spawn(async move {
        let dir_of_jobs = work_dir.join("jobs");
        loop {
            for entry in std::fs::read_dir(&dir_of_jobs)
                .into_iter()
                .flatten()
                .flatten()
            {
                let dir = entry.path();
                if !dir.join("job.json").exists() || dir.join("started").exists() {
                    continue;
                }
                std::fs::write(dir.join("started"), "").unwrap();
                let spec: JobSpec =
                    serde_json::from_slice(&std::fs::read(dir.join("job.json")).unwrap()).unwrap();
                let reference = std::fs::read(dir.join(dashu::tools::REFERENCE_DOCX)).ok();
                log.lock().unwrap().push((spec, reference));
                worker::process_job(&dir, &|spec: &JobSpec, dir: &Path, _| {
                    std::fs::write(dir.join(&spec.output), b"PK\x03\x04 faux docx")
                        .map_err(|e| e.to_string())
                });
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    });
    (handle, jobs)
}

/// Un vrai petit fichier ZIP (entrées stockées, sans compression), comme un
/// `.docx` : rien de 1Life, fabriqué ici.
fn tiny_docx() -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for byte in data {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    let entries: [(&str, &[u8]); 2] = [
        (
            "[Content_Types].xml",
            br#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>"#,
        ),
        (
            "word/styles.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"/>"#,
        ),
    ];
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let size = data.len() as u32;
        let mut local = Vec::new();
        local.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        local.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        local.extend_from_slice(&crc.to_le_bytes());
        local.extend_from_slice(&size.to_le_bytes());
        local.extend_from_slice(&size.to_le_bytes());
        local.extend_from_slice(&(name.len() as u16).to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes());
        local.extend_from_slice(name.as_bytes());
        local.extend_from_slice(data);
        out.extend_from_slice(&local);
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0; 12]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let start = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0, 2, 0, 2, 0]);
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

#[tokio::test]
async fn exporter_en_word_avec_le_document_de_reference() {
    let fake = fake_anthropic().await;
    let app = TestApp::with_ai(&fake.base).await;
    let (mut alice, taff, client_id) = ready(&app, ALICE).await;
    let (audit, notes_id) = start(&app, &mut alice, client_id, "integre").await;
    write_notes(&mut alice, notes_id, "Des notes.").await;
    generate(&mut alice, audit, "claude-haiku-5-5").await;
    finished(&app, audit).await;
    let v1: i64 = count(&app, "SELECT note_id FROM onelife_audit_versions").await;
    let (worker, jobs) = fake_worker(app.work_dir.clone());

    // « Exporter en Word » sur la page de l'audit et dans l'en-tête des notes.
    let page = alice.get(&format!("{LIST}/{audit}")).await;
    assert!(page.body.contains(&format!("{LIST}/{audit}/word")));
    let editor = open(&mut alice, &format!("/notes/{v1}")).await;
    assert!(
        editor
            .body
            .contains(&format!("{LIST}/{audit}/word?note={v1}")),
        "{}",
        editor.body
    );
    assert!(editor.body.contains("Exporter en Word"));

    // Sans dossier des modèles : le Word de pandoc, sans référence.
    let plain = alice.get(&format!("{LIST}/{audit}/word")).await;
    assert_eq!(plain.status, StatusCode::OK, "{}", plain.body);
    assert_eq!(
        plain.header("content-type"),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    );
    assert!(plain.header("content-disposition").contains(".docx"));
    assert_eq!(plain.bytes, b"PK\x03\x04 faux docx");

    // Un .docx dans le dossier des modèles : il sert de référence.
    let models = folder(&app, &mut alice, taff, None, "Modèles").await;
    alice
        .post(
            "/e/taff/1life/reglages/modeles",
            &[
                ("space_id", &taff.to_string()),
                ("folder_id", &models.to_string()),
            ],
        )
        .await;
    let reference = tiny_docx();
    assert_eq!(
        alice
            .upload_in_folder("reference.docx", &reference, models)
            .await
            .status,
        StatusCode::CREATED
    );
    let styled = alice
        .get(&format!("{LIST}/{audit}/word?note={notes_id}"))
        .await;
    assert_eq!(styled.status, StatusCode::OK);
    worker.abort();

    let jobs = jobs.lock().unwrap().clone();
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].0.to, Some(Conversion::Docx));
    assert_eq!(jobs[0].1, None);
    assert_eq!(jobs[1].1.as_deref(), Some(reference.as_slice()));

    // Une note qui n'est pas de cet audit : 404, pas d'export.
    let other = common::note_id(&alice.post("/notes", &[]).await);
    assert_eq!(
        alice
            .get(&format!("{LIST}/{audit}/word?note={other}"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    // Une note ordinaire n'a ni « Exporter en Word » ni « Générer l'audit ».
    let ordinary = open(&mut alice, &format!("/notes/{other}")).await;
    assert_eq!(ordinary.status, StatusCode::OK);
    assert!(
        !ordinary.body.contains("Exporter en Word") && !ordinary.body.contains("Générer l'audit")
    );
}
