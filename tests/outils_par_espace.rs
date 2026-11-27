//! Outils par espace (SPEC, « Outils par espace ») : chaque compte choisit,
//! espace par espace, les outils affichés dans la barre latérale. Masquer ne
//! supprime rien et ne retire aucun droit.

mod common;

use axum::http::{Method, StatusCode};
use common::{ALICE, API_TOKEN, BOB, TestApp, between, command_node};
use dashu::registry::TOOLS;
use serde_json::json;

const T: Option<&str> = Some(API_TOKEN);

/// Identifiants des outils de la barre latérale, dans l'ordre de la page.
fn sidebar(body: &str) -> Vec<String> {
    let nav = between(body, r#"<div id="nav-tools""#, "</ul>").expect("outils de la barre");
    nav.split(r#"data-tool=""#)
        .skip(1)
        .filter_map(|item| item.split('"').next())
        .map(str::to_string)
        .collect()
}

/// Libellés de la barre de raccourcis du téléphone.
fn tabbar(body: &str) -> Vec<String> {
    let nav = between(body, r#"class="tabbar""#, "</nav>").expect("barre de raccourcis");
    nav.split(r#"class="tab""#)
        .skip(1)
        .filter_map(|item| between(item, "<span>", "</span>"))
        .map(str::to_string)
        .collect()
}

/// Mots de `/go` dans la barre de commandes.
fn go_words(tree: &serde_json::Value) -> Vec<String> {
    command_node(tree, &["go"])["children"]
        .as_array()
        .expect("/go")
        .iter()
        .filter_map(|n| n["name"].as_str().map(str::to_string))
        .collect()
}

/// Affiche (`true`) ou masque un outil dans un espace, comme l'interrupteur
/// de Réglages sans htmx.
async fn toggle(
    client: &mut common::Client,
    space: i64,
    tool: &str,
    shown: bool,
) -> common::TestResponse {
    let form: &[(&str, &str)] = if shown { &[("shown", "1")] } else { &[] };
    client
        .post(&format!("/settings/spaces/{space}/tools/{tool}"), form)
        .await
}

#[tokio::test]
async fn masquer_puis_reafficher_un_outil_sans_rien_perdre() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await;

    // Un fichier dans École.
    alice.get("/e/ecole/").await;
    let sent = alice
        .upload("cours.txt", b"Le trunk transporte les VLAN.")
        .await;
    assert!(sent.status.is_success(), "{}", sent.body);
    assert!(alice.get("/e/ecole/files").await.body.contains("cours.txt"));

    // Tous les outils sont là au départ.
    let page = alice.get("/e/ecole/").await;
    assert_eq!(
        sidebar(&page.body),
        ["dashboard", "notes", "files", "calendar", "tools"]
    );

    let hidden = toggle(&mut alice, ecole, "files", false).await;
    assert_eq!(hidden.status, StatusCode::SEE_OTHER);
    assert_eq!(hidden.location(), "/settings#espaces");
    let settings = alice.follow(hidden).await;
    assert!(settings.body.contains("Fichiers masqué dans École"));

    // Plus dans la barre latérale ni dans /go d'École…
    let page = alice.get("/e/ecole/").await;
    assert_eq!(
        sidebar(&page.body),
        ["dashboard", "notes", "calendar", "tools"]
    );
    alice.with_header("x-espace", "ecole");
    let tree = alice.catalogue("/").await;
    assert!(!go_words(&tree).contains(&"fichiers".to_string()));
    // … et une commande tapée quand même n'est pas comprise.
    alice.with_header("x-espace", "ecole");
    let refused = alice.command("/go fichiers", "/").await;
    assert_ne!(refused.status, StatusCode::NO_CONTENT);

    // Le fichier est toujours en base, et s'ouvre toujours par son adresse :
    // masquer n'est pas une permission.
    let id: i64 = sqlx::query_scalar("SELECT id FROM files WHERE space_id = ?")
        .bind(ecole)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    let opened = alice.get(&format!("/files/{id}")).await;
    assert_eq!(opened.status, StatusCode::OK);
    assert!(opened.body.contains("cours.txt"));

    // Réaffiché : il revient, avec son contenu.
    toggle(&mut alice, ecole, "files", true).await;
    let page = alice.get("/e/ecole/").await;
    assert!(sidebar(&page.body).contains(&"files".to_string()));
    assert!(alice.get("/e/ecole/files").await.body.contains("cours.txt"));
}

#[tokio::test]
async fn notes_masquees_retirent_aussi_les_raccourcis_qui_en_dependent() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    alice
        .post(
            "/settings/tabs",
            &[
                ("tab_notes", "on"),
                ("tab_new-note", "on"),
                ("tab_calendar", "on"),
            ],
        )
        .await;

    toggle(&mut alice, ecole, "notes", false).await;
    let page = alice.get("/e/ecole/").await;
    assert_eq!(tabbar(&page.body), ["Agenda"]);
    assert!(!page.body.contains("btn btn-block new-note"));
    // Ailleurs, rien ne change.
    let page = alice.get("/e/perso/").await;
    assert_eq!(tabbar(&page.body), ["Notes", "Agenda", "Nouvelle note"]);
    assert!(page.body.contains("btn btn-block new-note"));
}

#[tokio::test]
async fn adresse_d_un_outil_masque_ramene_a_l_accueil() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    let perso = app.space_id(ALICE, "perso").await;
    toggle(&mut alice, ecole, "files", false).await;
    toggle(&mut alice, ecole, "calendar", false).await;
    toggle(&mut alice, perso, "tools", false).await;

    let back = alice.get("/e/ecole/files").await;
    assert_eq!(back.status, StatusCode::SEE_OTHER);
    assert_eq!(back.location(), "/e/ecole/");
    let home = alice.follow(back).await;
    assert_eq!(home.status, StatusCode::OK);
    assert!(home.body.contains("Cet outil est masqué dans cet espace."));
    assert!(
        home.body
            .contains(r#"<a href="/settings#espaces">Ouvrir les réglages</a>"#)
    );

    // Avec des paramètres, et pour l'agenda aussi.
    let back = alice.get("/e/ecole/calendar?view=week").await;
    assert_eq!(back.location(), "/e/ecole/");

    // L'onglet Outils (adresse sans espace) suit l'espace actif.
    alice.get("/e/perso/").await;
    assert_eq!(alice.get("/tools").await.status, StatusCode::SEE_OTHER);
    alice.get("/e/ecole/").await;
    assert_eq!(alice.get("/tools").await.status, StatusCode::OK);

    // Dans les autres espaces, les pages s'ouvrent.
    assert_eq!(alice.get("/e/taff/files").await.status, StatusCode::OK);
    assert_eq!(alice.get("/e/perso/calendar").await.status, StatusCode::OK);
}

#[tokio::test]
async fn reglages_independants_d_un_espace_et_d_un_compte_a_l_autre() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let mut bob = app.logged_in(BOB).await;
    let ecole = app.space_id(ALICE, "ecole").await;

    toggle(&mut alice, ecole, "files", false).await;
    toggle(&mut alice, ecole, "tools", false).await;

    let page = alice.get("/e/ecole/").await;
    assert_eq!(sidebar(&page.body), ["dashboard", "notes", "calendar"]);
    for other in ["perso", "taff"] {
        let page = alice.get(&format!("/e/{other}/")).await;
        assert_eq!(
            sidebar(&page.body),
            ["dashboard", "notes", "files", "calendar", "tools"],
            "{other}"
        );
    }
    // Bob, son propre École : rien de masqué. Et il ne règle pas l'espace
    // d'Alice.
    let page = bob.get("/e/ecole/").await;
    assert_eq!(sidebar(&page.body).len(), 5);
    assert_eq!(
        toggle(&mut bob, ecole, "notes", false).await.status,
        StatusCode::NOT_FOUND
    );

    // Espace partagé : chaque membre a son réglage.
    let slug = app.share_space(ALICE, "ecole", BOB).await;
    let page = bob.get(&format!("/e/{slug}/")).await;
    assert_eq!(sidebar(&page.body).len(), 5, "Bob garde tout");
    assert_eq!(
        toggle(&mut bob, ecole, "notes", false).await.status,
        StatusCode::SEE_OTHER
    );
    let page = bob.get(&format!("/e/{slug}/")).await;
    assert_eq!(
        sidebar(&page.body),
        ["dashboard", "files", "calendar", "tools"]
    );
    let page = alice.get("/e/ecole/").await;
    assert_eq!(sidebar(&page.body), ["dashboard", "notes", "calendar"]);
}

#[tokio::test]
async fn l_accueil_ne_se_masque_pas_et_un_outil_inconnu_est_refuse() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await;

    assert_eq!(
        toggle(&mut alice, ecole, "dashboard", false).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        toggle(&mut alice, ecole, "inconnu", false).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        toggle(&mut alice, 999_999, "files", false).await.status,
        StatusCode::NOT_FOUND
    );
    let page = alice.get("/e/ecole/").await;
    assert_eq!(sidebar(&page.body)[0], "dashboard");
    // Les réglages ne proposent pas d'interrupteur pour l'accueil.
    let settings = alice.get("/settings").await;
    assert!(!settings.body.contains("/tools/dashboard"));
}

#[tokio::test]
async fn interrupteur_htmx_renvoie_le_bloc_et_la_barre_laterale() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    let perso = app.space_id(ALICE, "perso").await;

    // Interrupteur d'École, depuis Réglages ouverts sur École.
    alice.with_header("hx-request", "true");
    alice.with_header("x-espace", "ecole");
    let response = toggle(&mut alice, ecole, "files", false).await;
    assert_eq!(response.status, StatusCode::OK);
    let block = between(
        &response.body,
        &format!(r#"id="space-tools-{ecole}""#),
        "</fieldset>",
    )
    .expect("bloc de l'espace");
    assert!(
        block.contains(r#"name="shown" value="1">"#),
        "Fichiers décoché"
    );
    assert!(!block.contains("/tools/dashboard"));
    // L'espace actif a changé : la barre latérale suit, sans Fichiers.
    assert!(response.body.contains(r#"hx-swap-oob="true""#));
    assert_eq!(
        sidebar(&response.body),
        ["dashboard", "notes", "calendar", "tools"]
    );

    // Un autre espace que l'actif : le bloc seul.
    alice.with_header("hx-request", "true");
    alice.with_header("x-espace", "ecole");
    let response = toggle(&mut alice, perso, "notes", false).await;
    assert_eq!(response.status, StatusCode::OK);
    assert!(!response.body.contains("hx-swap-oob"));
}

/// Chaque entrée du registre apparaît dans la barre latérale, la barre de
/// commandes et les réglages, sans rien écrire d'autre : ce test parcourt le
/// registre, et couvre donc tout outil qu'on y ajoutera.
#[tokio::test]
async fn chaque_outil_du_registre_apparait_partout() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await;

    let page = alice.get("/e/ecole/").await;
    let settings = alice.get("/settings").await;
    alice.with_header("x-espace", "ecole");
    let tree = alice.catalogue("/").await;
    let words = go_words(&tree);
    let shown = sidebar(&page.body);

    for tool in TOOLS.iter().filter(|t| !t.reserved && t.default_shown) {
        assert!(shown.contains(&tool.id.to_string()), "barre : {}", tool.id);
        assert!(
            words.contains(&tool.command.to_string()),
            "/go : {}",
            tool.id
        );
        assert!(
            page.body
                .contains(&format!(">{}</span></a></li>", tool.name)),
            "nom : {}",
            tool.id
        );
        if tool.maskable {
            assert!(
                settings
                    .body
                    .contains(&format!("/settings/spaces/{ecole}/tools/{}", tool.id)),
                "réglages : {}",
                tool.id
            );
        }
    }
}

#[tokio::test]
async fn la_migration_ne_masque_rien() {
    let app = TestApp::new().await;
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM space_tools")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(rows, 0);
    let mut alice = app.logged_in(ALICE).await;
    for space in ["perso", "ecole", "taff"] {
        let page = alice.get(&format!("/e/{space}/")).await;
        assert_eq!(
            sidebar(&page.body),
            ["dashboard", "notes", "files", "calendar", "tools"]
        );
    }
}

#[tokio::test]
async fn l_api_travaille_sur_un_outil_masque_et_regle_les_outils() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    toggle(&mut alice, ecole, "notes", false).await;

    // Notes masqué dans École : l'API crée et liste toujours.
    let (status, note) = app
        .api(
            Method::POST,
            "/api/v1/notes",
            Some(json!({ "title": "Cours réseau", "body_md": "VLAN", "space": "ecole" })),
            T,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{note}");
    let (status, list) = app
        .api(Method::GET, "/api/v1/notes?space=ecole", None, T)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list[0]["title"], "Cours réseau");

    // Lire le réglage.
    let (status, tools) = app
        .api(Method::GET, "/api/v1/spaces/ecole/tools", None, T)
        .await;
    assert_eq!(status, StatusCode::OK, "{tools}");
    assert_eq!(tools["space"], "ecole");
    let notes = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["tool"] == "notes")
        .unwrap();
    assert_eq!(notes["shown"], false);
    assert_eq!(tools["tools"][0]["tool"], "dashboard");
    assert_eq!(tools["tools"][0]["maskable"], false);

    // Le changer : seuls les outils nommés bougent.
    let (status, tools) = app
        .api(
            Method::PUT,
            "/api/v1/spaces/ecole/tools",
            Some(json!({ "tools": { "notes": true, "files": false } })),
            T,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{tools}");
    let page = alice.get("/e/ecole/").await;
    assert_eq!(
        sidebar(&page.body),
        ["dashboard", "notes", "calendar", "tools"]
    );

    // Refus, sans rien écrire : outil inconnu, accueil, espace d'un autre.
    for body in [
        json!({ "tools": { "files": true, "inconnu": false } }),
        json!({ "tools": { "files": true, "dashboard": false } }),
        json!({ "layout": "x" }),
    ] {
        let (status, _) = app
            .api(Method::PUT, "/api/v1/spaces/ecole/tools", Some(body), T)
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let page = alice.get("/e/ecole/").await;
    assert!(!sidebar(&page.body).contains(&"files".to_string()));
    let (status, _) = app
        .api(Method::GET, "/api/v1/spaces/nulle-part/tools", None, T)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn le_connecteur_claude_lit_et_regle_les_outils() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let response = alice
        .post("/settings/tokens", &[("name", "Claude"), ("duree", "90")])
        .await;
    let token = between(
        &response.body,
        "id=\"new-token\" type=\"text\" value=\"",
        "\"",
    )
    .expect("jeton affiché")
    .to_string();
    let call = |name: &'static str, arguments: serde_json::Value| {
        let app = &app;
        let token = token.clone();
        async move {
            let (_, body) = app
                .api(
                    Method::POST,
                    "/mcp",
                    Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                                 "params": { "name": name, "arguments": arguments } })),
                    Some(&token),
                )
                .await;
            body["result"].clone()
        }
    };

    let result = call(
        "modifier_outils_espace",
        json!({ "space": "ecole", "tools": { "calendar": false } }),
    )
    .await;
    assert_eq!(result["isError"], false, "{result}");
    let page = alice.get("/e/ecole/").await;
    assert_eq!(
        sidebar(&page.body),
        ["dashboard", "notes", "files", "tools"]
    );

    let result = call("lire_outils_espace", json!({ "space": "ecole" })).await;
    let read: serde_json::Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    let calendar = read["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["tool"] == "calendar")
        .unwrap();
    assert_eq!(calendar["shown"], false);

    let refused = call(
        "modifier_outils_espace",
        json!({ "space": "ecole", "tools": { "dashboard": false } }),
    )
    .await;
    assert_eq!(refused["isError"], true);
}
