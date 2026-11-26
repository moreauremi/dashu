//! 1Life › Clients (SPEC, « 1Life — Clients ») : le carnet de clients d'un
//! compte autorisé à 1Life. Mêmes contrôles que le reste de 1Life (404 pour
//! un compte non autorisé ou sans session), clients propres à chaque compte,
//! texte brut jamais interprété, rien dans l'API ni le connecteur.
//!
//! Dépôt public : tous les noms ci-dessous sont inventés.

mod common;

use axum::http::{Method, StatusCode};
use common::{ALICE, BOB, Client, TestApp, between};
use serde_json::{Value, json};

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

const LIST: &str = "/e/taff/1life/clients";

/// Crée un client ; rend son identifiant.
async fn create(client: &mut Client, form: &[(&str, &str)]) -> i64 {
    let created = client.post(LIST, form).await;
    assert_eq!(created.status, StatusCode::SEE_OTHER, "{}", created.body);
    created
        .location()
        .rsplit('/')
        .next()
        .unwrap()
        .parse()
        .unwrap_or_else(|_| panic!("adresse de la fiche : {}", created.location()))
}

async fn count(app: &TestApp) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM onelife_clients")
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

/// (nom, contact, e-mail, téléphone, notes, archivé) d'un client.
async fn stored(app: &TestApp, id: i64) -> (String, String, String, String, String, bool) {
    sqlx::query_as(
        "SELECT name, contact, email, phone, notes, archived FROM onelife_clients WHERE id = ?",
    )
    .bind(id)
    .fetch_one(&app.state.db)
    .await
    .unwrap()
}

/// Les noms de la liste, dans l'ordre.
fn names(body: &str) -> Vec<String> {
    body.split("<span class=\"client-name\">")
        .skip(1)
        .filter_map(|part| part.split("</span>").next())
        .map(str::to_string)
        .collect()
}

#[tokio::test]
async fn creer_retrouver_modifier_un_client() {
    let app = TestApp::new().await;
    let mut alice = granted(&app, ALICE).await;

    // La liste vide propose d'ajouter le premier.
    let page = alice.get(LIST).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("Aucun client pour l'instant"));
    assert!(page.body.contains("href=\"/e/taff/1life/clients/new\""));
    let form = alice.get("/e/taff/1life/clients/new").await;
    assert_eq!(form.status, StatusCode::OK);
    assert!(form.body.contains("action=\"/e/taff/1life/clients\""));

    // Créer : la fiche s'ouvre, avec ses liens pour écrire et appeler.
    let created = alice
        .post(
            LIST,
            &[
                ("name", "  Atelier   Élan "),
                ("contact", "Camille Exemple"),
                ("email", "bonjour@atelier.exemple.test"),
                ("phone", "+33 1 23 45 67 89"),
                ("notes", "Rappeler en janvier.\r\nDevis envoyé."),
            ],
        )
        .await;
    assert_eq!(created.status, StatusCode::SEE_OTHER);
    let id: i64 = created
        .location()
        .rsplit('/')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(created.location(), format!("/e/taff/1life/clients/{id}"));
    let fiche = alice.follow(created).await;
    assert!(fiche.body.contains("Client « Atelier Élan » créé"));
    assert!(fiche.body.contains("<h1>Atelier Élan</h1>"));
    assert!(
        fiche
            .body
            .contains("href=\"mailto:bonjour@atelier.exemple.test\"")
    );
    assert!(fiche.body.contains("href=\"tel:+33123456789\""));
    assert_eq!(
        stored(&app, id).await,
        (
            "Atelier Élan".into(),
            "Camille Exemple".into(),
            "bonjour@atelier.exemple.test".into(),
            "+33 1 23 45 67 89".into(),
            "Rappeler en janvier.\nDevis envoyé.".into(),
            false
        )
    );

    // La liste, par nom ; la recherche ignore accents et majuscules, et
    // cherche aussi dans le contact, l'adresse et le numéro.
    create(&mut alice, &[("name", "Boulangerie Exemple")]).await;
    create(
        &mut alice,
        &[("name", "agence Delta"), ("contact", "Dominique Test")],
    )
    .await;
    let page = alice.get(LIST).await;
    assert_eq!(
        names(&page.body),
        ["agence Delta", "Atelier Élan", "Boulangerie Exemple"]
    );
    for (q, found) in [
        ("elan", vec!["Atelier Élan"]),
        ("ATELIER", vec!["Atelier Élan"]),
        ("dominique", vec!["agence Delta"]),
        ("45 67", vec!["Atelier Élan"]),
        ("exemple.test", vec!["Atelier Élan"]),
        ("personne", vec![]),
    ] {
        let page = alice
            .get(&format!("{LIST}?q={}", q.replace(' ', "+")))
            .await;
        assert_eq!(names(&page.body), found, "{q}");
        if found.is_empty() {
            assert!(page.body.contains("Aucun client ne correspond"));
        }
    }

    // Modifier : la fiche se réenregistre.
    let saved = alice
        .post(
            &format!("{LIST}/{id}"),
            &[
                ("name", "Atelier Élan"),
                ("contact", "Camille Exemple"),
                ("email", ""),
                ("phone", "01 98 76 54 32"),
                ("notes", "Devis signé."),
            ],
        )
        .await;
    assert_eq!(saved.location(), format!("/e/taff/1life/clients/{id}"));
    let fiche = alice.follow(saved).await;
    assert!(fiche.body.contains("Fiche enregistrée"));
    assert!(!fiche.body.contains("mailto:"));
    assert_eq!(
        stored(&app, id).await,
        (
            "Atelier Élan".into(),
            "Camille Exemple".into(),
            String::new(),
            "01 98 76 54 32".into(),
            "Devis signé.".into(),
            false
        )
    );
    // Le même nom, inchangé, n'est pas un doublon de lui-même ; la fiche se
    // lit aussi sans l'espace dans l'adresse.
    assert_eq!(
        alice.get(&format!("/1life/clients/{id}")).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn une_saisie_refusee_revient_avec_ce_qui_a_ete_tape() {
    let app = TestApp::new().await;
    let mut alice = granted(&app, ALICE).await;
    let id = create(&mut alice, &[("name", "Atelier Elan")]).await;
    let long_name = "n".repeat(81);
    let long_notes = "x".repeat(4001);

    for (form, message) in [
        (vec![("name", "   ")], "Donne un nom au client."),
        (
            vec![("name", "atelier élan")],
            "Un client « atelier élan » existe déjà.",
        ),
        (
            vec![("name", "Studio Test"), ("email", "pas-une-adresse")],
            "Cette adresse e-mail n&#39;a pas l&#39;air valide.",
        ),
        (
            vec![("name", "Studio Test"), ("phone", "appeler le matin")],
            "Ce numéro de téléphone n&#39;a pas l&#39;air valide",
        ),
        (
            vec![("name", long_name.as_str())],
            "Le nom fait 80 caractères au plus.",
        ),
        (
            vec![("name", "Studio Test"), ("notes", long_notes.as_str())],
            "Les notes font 4000 caractères au plus.",
        ),
    ] {
        let mut form = form;
        form.push(("contact", "Sacha Tapé"));
        let refused = alice.post(LIST, &form).await;
        assert_eq!(
            refused.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{message}"
        );
        assert!(
            refused.body.contains(message),
            "{message}\n{}",
            refused.body
        );
        // Ce qui a été tapé est encore là.
        assert!(refused.body.contains("value=\"Sacha Tapé\""), "{message}");
        assert_eq!(count(&app).await, 1, "{message}");
    }

    // Modifier vers le nom d'un autre client : refusé, rien ne change.
    let other = create(&mut alice, &[("name", "Studio Test")]).await;
    let refused = alice
        .post(&format!("{LIST}/{other}"), &[("name", "ATELIER ELAN")])
        .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        refused
            .body
            .contains("Un client « ATELIER ELAN » existe déjà.")
    );
    assert!(
        refused
            .body
            .contains(&format!("action=\"/e/taff/1life/clients/{other}\""))
    );
    assert_eq!(stored(&app, other).await.0, "Studio Test");
    assert_eq!(stored(&app, id).await.0, "Atelier Elan");
}

#[tokio::test]
async fn archiver_puis_supprimer_un_client() {
    let app = TestApp::new().await;
    let mut alice = granted(&app, ALICE).await;
    let id = create(&mut alice, &[("name", "Atelier Elan")]).await;
    create(&mut alice, &[("name", "Studio Test")]).await;

    // Archiver : le client quitte la liste sans être effacé.
    let archived = alice
        .post(&format!("{LIST}/{id}/archive"), &[("archived", "1")])
        .await;
    assert_eq!(archived.location(), LIST);
    let page = alice.follow(archived).await;
    assert!(page.body.contains("Client « Atelier Elan » archivé"));
    assert_eq!(names(&page.body), ["Studio Test"]);
    assert!(stored(&app, id).await.5);
    // « Archivés » le montre, avec les nombres de chaque liste.
    let page = alice.get(&format!("{LIST}?archives=1")).await;
    assert_eq!(names(&page.body), ["Atelier Elan"]);
    let chips = between(&page.body, "aria-label=\"Clients montrés\"", "</nav>").unwrap();
    assert!(chips.contains("Actifs<span class=\"chip-count\">1</span>"));
    assert!(chips.contains("Archivés<span class=\"chip-count\">1</span>"));
    // La fiche le dit, et propose de le remettre.
    let fiche = alice.get(&format!("{LIST}/{id}")).await;
    assert!(fiche.body.contains("Archivé"));
    assert!(fiche.body.contains("Remettre dans la liste"));
    alice
        .post(&format!("{LIST}/{id}/archive"), &[("archived", "0")])
        .await;
    assert!(!stored(&app, id).await.5);
    assert_eq!(
        names(&alice.get(LIST).await.body),
        ["Atelier Elan", "Studio Test"]
    );

    // Supprimer : le formulaire demande une confirmation ; la fiche est
    // effacée pour de bon.
    let fiche = alice.get(&format!("{LIST}/{id}")).await;
    let delete = between(
        &fiche.body,
        &format!("action=\"/e/taff/1life/clients/{id}/delete\""),
        "</form>",
    )
    .unwrap();
    assert!(delete.contains("hx-confirm=\"Supprimer « Atelier Elan » pour de bon ?"));
    let deleted = alice.post(&format!("{LIST}/{id}/delete"), &[]).await;
    assert_eq!(deleted.location(), LIST);
    let page = alice.follow(deleted).await;
    assert!(page.body.contains("Client « Atelier Elan » supprimé"));
    assert_eq!(names(&page.body), ["Studio Test"]);
    assert_eq!(count(&app).await, 1);
    assert_eq!(
        alice.get(&format!("{LIST}/{id}")).await.status,
        StatusCode::NOT_FOUND
    );
}

/// Toutes les adresses de Clients (pages), avec et sans l'espace.
fn pages(id: i64) -> Vec<String> {
    let mut out = Vec::new();
    for base in ["", "/e/taff"] {
        for rest in [
            "/1life/clients".to_string(),
            "/1life/clients?archives=1".to_string(),
            "/1life/clients/new".to_string(),
            format!("/1life/clients/{id}"),
            "/1life/clients/abc".to_string(),
        ] {
            out.push(format!("{base}{rest}"));
        }
    }
    out
}

/// Les formulaires de Clients, avec et sans l'espace.
fn forms(id: i64) -> Vec<String> {
    let mut out = Vec::new();
    for base in ["", "/e/taff"] {
        for rest in [
            "/1life/clients".to_string(),
            format!("/1life/clients/{id}"),
            format!("/1life/clients/{id}/archive"),
            format!("/1life/clients/{id}/delete"),
            "/1life/clients/abc/delete".to_string(),
        ] {
            out.push(format!("{base}{rest}"));
        }
    }
    out
}

#[tokio::test]
async fn sans_autorisation_clients_repond_404_partout() {
    let app = TestApp::new().await;
    let mut alice = granted(&app, ALICE).await;
    let id = create(&mut alice, &[("name", "Atelier Elan")]).await;

    // Bob, connecté mais non autorisé : la même page qu'une adresse qui
    // n'existe pas, pour les pages comme pour les formulaires, même mal
    // formés (numéro illisible, corps qui n'est pas un formulaire).
    let mut bob = app.logged_in(BOB).await;
    let unknown = bob.get("/nulle-part").await;
    for page in pages(id) {
        let response = bob.get(&page).await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{page}");
        assert_eq!(response.body, unknown.body, "{page}");
    }
    for form in forms(id) {
        let response = bob
            .post(&form, &[("name", "Intrus"), ("archived", "1")])
            .await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{form}");
        assert_eq!(response.body, unknown.body, "{form}");
        let raw = bob.post_raw(&form, b"{\"name\": 1}").await;
        assert_eq!(raw.status, StatusCode::NOT_FOUND, "{form} brut");
    }
    // Une autre méthode non plus ne dit rien.
    assert_eq!(
        bob.delete(&format!("/1life/clients/{id}")).await.status,
        StatusCode::NOT_FOUND
    );

    // Sans session : 404 sur les pages, et un formulaire répond comme sur
    // une adresse qui n'existe pas.
    let mut anonymous = app.client();
    for page in pages(id) {
        assert_eq!(
            anonymous.get(&page).await.status,
            StatusCode::NOT_FOUND,
            "{page}"
        );
    }
    let unknown_post = anonymous.post("/nulle-part", &[("name", "x")]).await;
    for form in forms(id) {
        let response = anonymous.post(&form, &[("name", "x")]).await;
        assert_eq!(response.status, unknown_post.status, "{form}");
        assert_eq!(response.body, unknown_post.body, "{form}");
    }

    // Rien n'a bougé.
    assert_eq!(count(&app).await, 1);
    assert_eq!(stored(&app, id).await.0, "Atelier Elan");

    // Autorisée, mais 1Life masqué dans École : retour à l'accueil d'École,
    // comme les autres outils ; rien n'est écrit par un formulaire.
    let back = alice.get("/e/ecole/1life/clients").await;
    assert_eq!(back.status, StatusCode::SEE_OTHER);
    assert_eq!(back.location(), "/e/ecole/");
    let back = alice
        .post("/e/ecole/1life/clients", &[("name", "Pas ici")])
        .await;
    assert_eq!(back.status, StatusCode::SEE_OTHER);
    assert_eq!(count(&app).await, 1);
    // Un numéro illisible, pour elle : 404, pas 400.
    assert_eq!(
        alice.get("/e/taff/1life/clients/abc").await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn un_autre_compte_autorise_ne_touche_pas_aux_clients() {
    let app = TestApp::new().await;
    let mut alice = granted(&app, ALICE).await;
    let mut bob = granted(&app, BOB).await;
    let id = create(
        &mut alice,
        &[("name", "Atelier Elan"), ("contact", "Camille Exemple")],
    )
    .await;

    // La liste de Bob est vide ; il peut avoir son propre « Atelier Elan ».
    let page = bob.get(LIST).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(names(&page.body).is_empty());
    assert!(!page.body.contains("Camille Exemple"));
    let page = bob.get(&format!("{LIST}?q=elan")).await;
    assert!(names(&page.body).is_empty());
    let mine = create(&mut bob, &[("name", "Atelier Elan")]).await;
    assert_ne!(mine, id);

    // La fiche d'Alice : 404 pour la lire, la modifier, l'archiver, la
    // supprimer.
    assert_eq!(
        bob.get(&format!("{LIST}/{id}")).await.status,
        StatusCode::NOT_FOUND
    );
    for (path, form) in [
        (format!("{LIST}/{id}"), vec![("name", "Volé")]),
        (format!("{LIST}/{id}/archive"), vec![("archived", "1")]),
        (format!("{LIST}/{id}/delete"), vec![]),
    ] {
        let response = bob.post(&path, &form).await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{path}");
    }
    assert_eq!(
        stored(&app, id).await,
        (
            "Atelier Elan".into(),
            "Camille Exemple".into(),
            String::new(),
            String::new(),
            String::new(),
            false
        )
    );
    assert_eq!(names(&alice.get(LIST).await.body), ["Atelier Elan"]);
}

#[tokio::test]
async fn les_notes_d_un_client_restent_du_texte() {
    let app = TestApp::new().await;
    let mut alice = granted(&app, ALICE).await;
    let id = create(
        &mut alice,
        &[
            ("name", "Atelier <b>Elan</b>"),
            (
                "notes",
                "<script>alert(1)</script>\n**pas du gras** <img src=x onerror=alert(2)>",
            ),
        ],
    )
    .await;

    // Les chevrons arrivent échappés (askama), sous l'une ou l'autre forme.
    let escaped = |body: &str, text: &str| {
        let named = text.replace('<', "&lt;").replace('>', "&gt;");
        let numeric = text.replace('<', "&#60;").replace('>', "&#62;");
        body.contains(&named) || body.contains(&numeric)
    };
    let fiche = alice.get(&format!("{LIST}/{id}")).await;
    assert!(!fiche.body.contains("<script>alert(1)"));
    assert!(!fiche.body.contains("<img src=x"));
    assert!(!fiche.body.contains("<b>Elan</b>"));
    assert!(
        escaped(&fiche.body, "<script>alert(1)</script>"),
        "{}",
        fiche.body
    );
    assert!(fiche.body.contains("**pas du gras**"));
    assert!(escaped(&fiche.body, "Atelier <b>Elan</b>"));
    let list = alice.get(LIST).await;
    assert!(escaped(&list.body, "Atelier <b>Elan</b>"));
    assert!(!list.body.contains("<b>Elan</b>"));
    // Gardé tel quel en base : c'est l'affichage qui échappe.
    assert_eq!(stored(&app, id).await.0, "Atelier <b>Elan</b>");
}

/// Un jeton personnel, créé depuis Réglages › Connexions.
async fn token(client: &mut Client) -> String {
    let response = client
        .post("/settings/tokens", &[("name", "Script"), ("duree", "90")])
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
async fn clients_sort_du_squelette_mais_reste_hors_de_l_api() {
    let app = TestApp::new().await;
    let mut alice = granted(&app, ALICE).await;
    create(&mut alice, &[("name", "Atelier Elan")]).await;

    // L'accueil de 1Life : Clients n'est plus « Bientôt », les deux autres
    // modules le restent.
    let home = alice.get("/e/taff/1life").await;
    assert_eq!(home.status, StatusCode::OK);
    let card = between(&home.body, "href=\"/e/taff/1life/clients\"", "</a>").unwrap();
    assert!(card.contains("Clients") && !card.contains("Bientôt"));
    assert_eq!(home.body.matches("Bientôt").count(), 2);

    // Ni dans l'API…
    let tk = token(&mut alice).await;
    for path in [
        "/api/v1/1life",
        "/api/v1/1life/clients",
        "/api/v1/clients",
        "/api/v1/onelife/clients",
    ] {
        let (status, _) = app.api(Method::GET, path, None, Some(&tk)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        let (status, _) = app
            .api(Method::POST, path, Some(json!({ "name": "x" })), Some(&tk))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    // … ni dans le connecteur Claude…
    let (status, list) = app
        .api(
            Method::POST,
            "/mcp",
            Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} })),
            Some(&tk),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let tools: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(!tools.is_empty());
    assert!(
        tools
            .iter()
            .all(|t| !t.contains("client") && !t.contains("1life")),
        "{tools:?}"
    );
    // … ni dans la barre de commandes (seul `/go 1life` y mène) …
    alice.with_header("x-espace", "taff");
    let catalogue: Value = alice.catalogue("/").await;
    assert!(!catalogue.to_string().contains("Atelier Elan"));
    assert!(!catalogue.to_string().contains("/1life/clients"));
    // … ni dans les commandes d'Hermès.
    let hermes = std::fs::read_to_string("deploy/hermes/dashu").unwrap();
    assert!(!hermes.contains("1life") && !hermes.contains("onelife"));
}
