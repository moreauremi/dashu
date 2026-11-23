//! API, lot « espaces » (SPEC, « API complète et corbeille ») : espaces,
//! membres, tableau de bord et réglages. Pour chaque route : le cas normal,
//! un élément d'un autre compte (404), une action de propriétaire tentée par
//! un membre (403), une saisie invalide.

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

/// Alice partage `ecole` avec Bob : rend le numéro du compte de Bob.
async fn share_ecole(app: &TestApp, alice: &str, bob: &str) -> i64 {
    let (status, invitation) = app
        .api(
            Method::POST,
            "/api/v1/spaces/ecole/invitations",
            Some(json!({ "email": BOB })),
            Some(alice),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{invitation}");
    let id = invitation["id"].as_i64().unwrap();
    let (status, joined) = app
        .api(
            Method::POST,
            &format!("/api/v1/invitations/{id}/accept"),
            None,
            Some(bob),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{joined}");
    // Bob a déjà son propre `ecole` : le partagé prend une autre adresse.
    assert_eq!(joined["slug"], "ecole-2");
    assert_eq!(joined["mine"], false);
    let (_, members) = app
        .api(
            Method::GET,
            "/api/v1/spaces/ecole/members",
            None,
            Some(alice),
        )
        .await;
    members["members"][0]["id"].as_i64().unwrap()
}

#[tokio::test]
async fn espaces_creer_modifier_archiver_supprimer() {
    let app = TestApp::new().await;
    let tk = token(&app, ALICE, "Script").await;
    let t = Some(tk.as_str());

    // Créer : sans couleur ni icône, les premières libres.
    let (status, sport) = app
        .api(
            Method::POST,
            "/api/v1/spaces",
            Some(json!({ "name": "Vie de famille" })),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{sport}");
    assert_eq!(sport["slug"], "vie-de-famille");
    assert_eq!(sport["mine"], true);
    assert_eq!(sport["archived"], false);
    assert_ne!(sport["color"], "orange");

    // Refus, sans rien créer.
    for body in [
        json!({ "name": " " }),
        json!({ "name": "perso" }),
        json!({ "name": "x".repeat(25) }),
        json!({ "name": "Ok", "color": "fuchsia" }),
        json!({ "name": "Ok", "icon": "licorne" }),
    ] {
        let (status, refused) = app
            .api(Method::POST, "/api/v1/spaces", Some(body.clone()), t)
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body} {refused}");
    }
    let (_, list) = app.api(Method::GET, "/api/v1/spaces", None, t).await;
    assert_eq!(list.as_array().unwrap().len(), 4);

    // Modifier : le slug reste, les champs absents aussi.
    let (status, changed) = app
        .api(
            Method::PATCH,
            "/api/v1/spaces/vie-de-famille",
            Some(json!({ "name": "Famille", "color": "rose", "auto_mp4": true })),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["slug"], "vie-de-famille");
    assert_eq!(changed["name"], "Famille");
    assert_eq!(changed["color"], "rose");
    assert_eq!(changed["icon"], sport["icon"]);
    assert_eq!(changed["auto_mp4"], true);
    assert_eq!(changed["auto_heic_jpeg"], false);
    // Une règle changée seule ne touche pas à l'autre.
    let (_, rules) = app
        .api(
            Method::PATCH,
            "/api/v1/spaces/vie-de-famille",
            Some(json!({ "auto_heic_jpeg": true })),
            t,
        )
        .await;
    assert_eq!(rules["auto_mp4"], true);
    assert_eq!(rules["auto_heic_jpeg"], true);
    let (status, _) = app
        .api(
            Method::PATCH,
            "/api/v1/spaces/vie-de-famille",
            Some(json!({ "name": "École" })),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Archiver, désarchiver ; Perso ne s'archive pas.
    let (status, archived) = app
        .api(
            Method::POST,
            "/api/v1/spaces/vie-de-famille/archive",
            None,
            t,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{archived}");
    assert_eq!(archived["archived"], true);
    let (status, _) = app
        .api(
            Method::POST,
            "/api/v1/spaces/vie-de-famille/archive",
            None,
            t,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, back) = app
        .api(
            Method::POST,
            "/api/v1/spaces/vie-de-famille/unarchive",
            None,
            t,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{back}");
    assert_eq!(back["archived"], false);
    let (status, _) = app
        .api(Method::POST, "/api/v1/spaces/perso/archive", None, t)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Un espace d'un autre compte, ou qui n'existe pas : 404.
    let bob = token(&app, BOB, "Bob").await;
    for (method, path) in [
        (Method::GET, "/api/v1/spaces/vie-de-famille"),
        (Method::PATCH, "/api/v1/spaces/vie-de-famille"),
        (
            Method::DELETE,
            "/api/v1/spaces/vie-de-famille?content=trash",
        ),
        (Method::POST, "/api/v1/spaces/vie-de-famille/archive"),
        (Method::GET, "/api/v1/spaces/vie-de-famille/dashboard"),
        (Method::GET, "/api/v1/spaces/vie-de-famille/members"),
    ] {
        let body = (method == Method::PATCH).then(|| json!({ "name": "Volé" }));
        let (status, _) = app.api(method.clone(), path, body, Some(&bob)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
    }

    // Supprimer : le choix du contenu est obligatoire ; Perso reste.
    for path in [
        "/api/v1/spaces/vie-de-famille",
        "/api/v1/spaces/vie-de-famille?content=rien",
        "/api/v1/spaces/vie-de-famille?content=trash&transfer_to=taff",
        "/api/v1/spaces/vie-de-famille?transfer_to=vie-de-famille",
        "/api/v1/spaces/vie-de-famille?transfer_to=nulle-part",
        "/api/v1/spaces/perso?content=trash",
    ] {
        let (status, refused) = app.api(Method::DELETE, path, None, t).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path} {refused}");
    }

    // En transférant : la note passe dans l'autre espace.
    let (_, note) = app
        .api(
            Method::POST,
            "/api/v1/notes",
            Some(json!({ "title": "Anniversaire", "space": "vie-de-famille" })),
            t,
        )
        .await;
    let note = note["id"].as_i64().unwrap();
    let (status, _) = app
        .api(
            Method::DELETE,
            "/api/v1/spaces/vie-de-famille?transfer_to=taff",
            None,
            t,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, moved) = app
        .api(Method::GET, &format!("/api/v1/notes/{note}"), None, t)
        .await;
    assert_eq!(moved["space"], "taff");

    // La trace de la corbeille porte le nom du jeton ; l'espace se restaure.
    let (_, trash) = app.api(Method::GET, "/api/v1/trash", None, t).await;
    let entry = trash
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "Famille")
        .expect("dans la corbeille")
        .clone();
    assert_eq!(entry["via"], "Script");
    let (status, restored) = app
        .api(
            Method::POST,
            &format!("/api/v1/trash/espace/{}/restore", entry["id"]),
            None,
            t,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{restored}");

    // Avec son contenu dans la corbeille.
    let (status, _) = app
        .api(Method::DELETE, "/api/v1/spaces/taff?content=trash", None, t)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = app
        .api(Method::GET, &format!("/api/v1/notes/{note}"), None, t)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn huit_espaces_au_plus() {
    let app = TestApp::new().await;
    let tk = token(&app, ALICE, "Script").await;
    let t = Some(tk.as_str());
    for n in 4..=8 {
        let (status, body) = app
            .api(
                Method::POST,
                "/api/v1/spaces",
                Some(json!({ "name": format!("Espace {n}") })),
                t,
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    let (status, refused) = app
        .api(
            Method::POST,
            "/api/v1/spaces",
            Some(json!({ "name": "De trop" })),
            t,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(refused["error"].as_str().unwrap().contains("8 espaces"));
}

#[tokio::test]
async fn membres_inviter_accepter_retirer_quitter() {
    let app = TestApp::new().await;
    let alice = token(&app, ALICE, "Alice").await;
    let bob = token(&app, BOB, "Bob").await;
    let (a, b) = (Some(alice.as_str()), Some(bob.as_str()));

    // Invitations refusées, sans rien créer.
    for (path, email) in [
        ("/api/v1/spaces/ecole/invitations", "inconnu@example.test"),
        ("/api/v1/spaces/ecole/invitations", ALICE),
        ("/api/v1/spaces/ecole/invitations", " "),
        ("/api/v1/spaces/perso/invitations", BOB),
    ] {
        let (status, refused) = app
            .api(Method::POST, path, Some(json!({ "email": email })), a)
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path} {email} {refused}");
    }
    let (_, none) = app.api(Method::GET, "/api/v1/invitations", None, b).await;
    assert_eq!(none, json!([]));

    // Inviter, voir, annuler.
    let (status, invitation) = app
        .api(
            Method::POST,
            "/api/v1/spaces/ecole/invitations",
            Some(json!({ "email": BOB })),
            a,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{invitation}");
    let id = invitation["id"].as_i64().unwrap();
    let (status, _) = app
        .api(
            Method::POST,
            "/api/v1/spaces/ecole/invitations",
            Some(json!({ "email": BOB })),
            a,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "doublon");
    let (_, members) = app
        .api(Method::GET, "/api/v1/spaces/ecole/members", None, a)
        .await;
    assert_eq!(members["invitations"][0]["id"], id);
    assert_eq!(members["invitations"][0]["person"]["email"], BOB);
    let (_, received) = app.api(Method::GET, "/api/v1/invitations", None, b).await;
    assert_eq!(received[0]["id"], id);
    assert_eq!(received[0]["space"], "École");

    // L'invitation d'un autre : 404, pour l'accepter comme pour la refuser.
    for action in ["accept", "decline"] {
        let (status, _) = app
            .api(
                Method::POST,
                &format!("/api/v1/invitations/{id}/{action}"),
                None,
                a,
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{action}");
    }
    let (status, _) = app
        .api(
            Method::DELETE,
            &format!("/api/v1/spaces/ecole/invitations/{id}"),
            None,
            a,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = app
        .api(
            Method::POST,
            &format!("/api/v1/invitations/{id}/accept"),
            None,
            b,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "annulée");

    // Refuser.
    let (_, again) = app
        .api(
            Method::POST,
            "/api/v1/spaces/ecole/invitations",
            Some(json!({ "email": BOB })),
            a,
        )
        .await;
    let (status, _) = app
        .api(
            Method::POST,
            &format!("/api/v1/invitations/{}/decline", again["id"]),
            None,
            b,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Accepter : Bob est membre et voit le contenu.
    let bob_id = share_ecole(&app, &alice, &bob).await;
    let (_, note) = app
        .api(
            Method::POST,
            "/api/v1/notes",
            Some(json!({ "title": "Commune", "space": "ecole" })),
            a,
        )
        .await;
    let note = note["id"].as_i64().unwrap();
    let (status, _) = app
        .api(Method::GET, &format!("/api/v1/notes/{note}"), None, b)
        .await;
    assert_eq!(status, StatusCode::OK);

    // Un membre voit les membres, pas les invitations.
    let (_, pending) = app
        .api(
            Method::POST,
            "/api/v1/spaces/taff/invitations",
            Some(json!({ "email": BOB })),
            a,
        )
        .await;
    assert!(pending["id"].is_i64());
    let (status, seen) = app
        .api(Method::GET, "/api/v1/spaces/ecole-2/members", None, b)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(seen["owner"]["email"], ALICE);
    assert_eq!(seen["invitations"], json!([]));

    // Tout ce qui revient au propriétaire : 403 pour un membre.
    for (method, path, body) in [
        (
            Method::PATCH,
            "/api/v1/spaces/ecole-2".to_string(),
            Some(json!({ "name": "Volé" })),
        ),
        (Method::POST, "/api/v1/spaces/ecole-2/archive".into(), None),
        (
            Method::POST,
            "/api/v1/spaces/ecole-2/unarchive".into(),
            None,
        ),
        (
            Method::DELETE,
            "/api/v1/spaces/ecole-2?content=trash".into(),
            None,
        ),
        // Même avec un choix de contenu invalide : le refus de droit d'abord.
        (Method::DELETE, "/api/v1/spaces/ecole-2".into(), None),
        (
            Method::POST,
            "/api/v1/spaces/ecole-2/invitations".into(),
            Some(json!({ "email": ALICE })),
        ),
        (
            Method::DELETE,
            "/api/v1/spaces/ecole-2/invitations/1".into(),
            None,
        ),
        (
            Method::DELETE,
            format!("/api/v1/spaces/ecole-2/members/{bob_id}"),
            None,
        ),
    ] {
        let (status, refused) = app.api(method.clone(), &path, body, b).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path} {refused}");
    }

    // Le propriétaire ne quitte pas son espace ; il ne retire pas un inconnu.
    let (status, _) = app
        .api(Method::POST, "/api/v1/spaces/ecole/leave", None, a)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = app
        .api(
            Method::DELETE,
            "/api/v1/spaces/ecole/members/99999",
            None,
            a,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Quitter coupe l'accès à la requête suivante.
    let (status, _) = app
        .api(Method::POST, "/api/v1/spaces/ecole-2/leave", None, b)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = app
        .api(Method::GET, &format!("/api/v1/notes/{note}"), None, b)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Retirer aussi.
    let (_, third) = app.api(Method::GET, "/api/v1/invitations", None, b).await;
    let (status, joined) = app
        .api(
            Method::POST,
            &format!("/api/v1/invitations/{}/accept", third[0]["id"]),
            None,
            b,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{joined}");
    let slug = joined["slug"].as_str().unwrap().to_string();
    let (status, _) = app
        .api(
            Method::DELETE,
            &format!("/api/v1/spaces/taff/members/{bob_id}"),
            None,
            a,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = app
        .api(Method::GET, &format!("/api/v1/spaces/{slug}"), None, b)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn tableau_de_bord_d_un_espace() {
    let app = TestApp::new().await;
    let alice = token(&app, ALICE, "Alice").await;
    let bob = token(&app, BOB, "Bob").await;
    let (a, b) = (Some(alice.as_str()), Some(bob.as_str()));

    let (status, default) = app
        .api(Method::GET, "/api/v1/spaces/ecole/dashboard", None, a)
        .await;
    assert_eq!(status, StatusCode::OK, "{default}");
    assert!(
        default["available"]
            .as_array()
            .unwrap()
            .contains(&json!("tasks"))
    );

    let (status, saved) = app
        .api(
            Method::PUT,
            "/api/v1/spaces/ecole/dashboard",
            Some(json!({ "layout": "tasks:2x2,notes:1x3" })),
            a,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["layout"], "tasks:2x2,notes:1x3");
    assert_eq!(saved["widgets"][1]["widget"], "notes");
    assert_eq!(saved["widgets"][1]["h"], 3);
    // Les autres espaces gardent la leur.
    let (_, other) = app
        .api(Method::GET, "/api/v1/spaces/taff/dashboard", None, a)
        .await;
    assert_eq!(other["layout"], default["layout"]);

    // Saisies invalides : rien ne change. Un corps vide n'efface rien.
    for body in [
        json!({ "layout": "meteo:1x1" }),
        json!({ "layout": "notes:5x1" }),
        json!({ "layout": "notes:1x1,notes:2x2" }),
        json!({}),
    ] {
        let (status, refused) = app
            .api(
                Method::PUT,
                "/api/v1/spaces/ecole/dashboard",
                Some(body.clone()),
                a,
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body} {refused}");
    }
    let (_, kept) = app
        .api(Method::GET, "/api/v1/spaces/ecole/dashboard", None, a)
        .await;
    assert_eq!(kept["layout"], "tasks:2x2,notes:1x3");

    // Le tableau de bord d'un espace partagé est commun.
    share_ecole(&app, &alice, &bob).await;
    let (status, shared) = app
        .api(
            Method::PUT,
            "/api/v1/spaces/ecole-2/dashboard",
            Some(json!({ "layout": "calendar:2x2" })),
            b,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{shared}");
    let (_, seen) = app
        .api(Method::GET, "/api/v1/spaces/ecole/dashboard", None, a)
        .await;
    assert_eq!(seen["layout"], "calendar:2x2");

    // Vide : aucun widget. null : la disposition par défaut.
    let (_, empty) = app
        .api(
            Method::PUT,
            "/api/v1/spaces/ecole/dashboard",
            Some(json!({ "layout": "" })),
            a,
        )
        .await;
    assert_eq!(empty["widgets"], json!([]));
    let (_, reset) = app
        .api(
            Method::PUT,
            "/api/v1/spaces/ecole/dashboard",
            Some(json!({ "layout": null })),
            a,
        )
        .await;
    assert_eq!(reset["layout"], default["layout"]);
}

/// Une petite image PNG valide.
fn png() -> Vec<u8> {
    let image = image::RgbImage::from_pixel(8, 8, image::Rgb([200, 80, 40]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}

#[tokio::test]
async fn reglages_du_compte() {
    let app = TestApp::new().await;
    let alice = token(&app, ALICE, "Alice").await;
    let bob = token(&app, BOB, "Bob").await;
    let (a, b) = (Some(alice.as_str()), Some(bob.as_str()));

    let (status, start) = app.api(Method::GET, "/api/v1/settings", None, a).await;
    assert_eq!(status, StatusCode::OK, "{start}");
    assert_eq!(start["email"], ALICE);
    assert_eq!(start["theme"], "system");
    assert_eq!(start["ui_size"], "normal");
    assert_eq!(start["moment_space"], Value::Null);

    let (status, changed) = app
        .api(
            Method::PATCH,
            "/api/v1/settings",
            Some(json!({
                "first_name": "  Alice ", "theme": "dark", "ui_size": "large",
                "sidebar_floating": true, "mobile_tabs": ["notes", "dashboard"],
                "moment_space": "ecole"
            })),
            a,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["first_name"], "Alice");
    assert_eq!(changed["theme"], "dark");
    assert_eq!(changed["ui_size"], "large");
    assert_eq!(changed["sidebar_floating"], true);
    assert_eq!(changed["mobile_tabs"], json!(["dashboard", "notes"]));
    assert_eq!(changed["moment_space"], "ecole");

    // Le site lit les mêmes réglages.
    let mut site = app.logged_in(ALICE).await;
    let page = site.get("/settings").await;
    assert!(page.body.contains("value=\"Alice\""));

    // Les champs absents restent ; null désactive l'espace du moment.
    let (_, partial) = app
        .api(
            Method::PATCH,
            "/api/v1/settings",
            Some(json!({ "last_name": "Martin", "theme": "system", "moment_space": null })),
            a,
        )
        .await;
    assert_eq!(partial["first_name"], "Alice");
    assert_eq!(partial["last_name"], "Martin");
    assert_eq!(partial["theme"], "system");
    assert_eq!(partial["ui_size"], "large");
    assert_eq!(partial["moment_space"], Value::Null);

    // Une saisie invalide ne change rien, même à côté d'un champ valide.
    for body in [
        json!({ "first_name": "Zoé", "theme": "rose" }),
        json!({ "first_name": "Zoé", "ui_size": "enorme" }),
        json!({ "first_name": "x".repeat(41) }),
        json!({ "first_name": "Zoé", "mobile_tabs": [] }),
        json!({ "first_name": "Zoé", "mobile_tabs": ["licorne"] }),
        json!({ "first_name": "Zoé", "mobile_tabs": ["dashboard", "notes", "files", "calendar", "search"] }),
        json!({ "first_name": "Zoé", "moment_space": "nulle-part" }),
        // Ce qui protège le compte n'existe pas ici.
        json!({ "first_name": "Zoé", "password": "nouveau-mot-de-passe" }),
        json!({ "email": "pirate@example.test" }),
    ] {
        let (status, refused) = app
            .api(Method::PATCH, "/api/v1/settings", Some(body.clone()), a)
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body} {refused}");
    }
    let (_, kept) = app.api(Method::GET, "/api/v1/settings", None, a).await;
    assert_eq!(kept["first_name"], "Alice");

    // Un espace archivé ne peut pas être l'espace du moment.
    app.api(Method::POST, "/api/v1/spaces/taff/archive", None, a)
        .await;
    let (status, _) = app
        .api(
            Method::PATCH,
            "/api/v1/settings",
            Some(json!({ "moment_space": "taff" })),
            a,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Chaque compte a ses réglages.
    let (_, other) = app.api(Method::GET, "/api/v1/settings", None, b).await;
    assert_eq!(other["email"], BOB);
    assert_eq!(other["first_name"], "");

    // La photo : relue et réenregistrée en JPEG ; une non-image est refusée.
    let (status, _, _) = app
        .api_bytes(Method::GET, "/api/v1/settings/avatar", None, a)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = app
        .api_bytes(
            Method::PUT,
            "/api/v1/settings/avatar",
            Some(b"pas une image".to_vec()),
            a,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .api_bytes(Method::PUT, "/api/v1/settings/avatar", Some(png()), a)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, headers, jpeg) = app
        .api_bytes(Method::GET, "/api/v1/settings/avatar", None, a)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "image/jpeg");
    assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    let (_, with) = app.api(Method::GET, "/api/v1/settings", None, a).await;
    assert_eq!(with["has_avatar"], true);
    // Celle d'Alice n'est pas celle de Bob.
    let (status, _, _) = app
        .api_bytes(Method::GET, "/api/v1/settings/avatar", None, b)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = app
        .api(Method::DELETE, "/api/v1/settings/avatar", None, a)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, without) = app.api(Method::GET, "/api/v1/settings", None, a).await;
    assert_eq!(without["has_avatar"], false);
}

#[tokio::test]
async fn changer_une_tache_d_espace() {
    let app = TestApp::new().await;
    let alice = token(&app, ALICE, "Alice").await;
    let bob = token(&app, BOB, "Bob").await;
    let (a, b) = (Some(alice.as_str()), Some(bob.as_str()));

    let (_, task) = app
        .api(
            Method::POST,
            "/api/v1/tasks",
            Some(json!({ "title": "Réviser", "space": "ecole", "due_date": "2026-12-01" })),
            a,
        )
        .await;
    let id = task["id"].as_i64().unwrap();
    let path = format!("/api/v1/tasks/{id}");

    // Le reste de la tâche ne bouge pas.
    let (status, moved) = app
        .api(Method::PATCH, &path, Some(json!({ "space": "taff" })), a)
        .await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    assert_eq!(moved["space"], "taff");
    assert_eq!(moved["title"], "Réviser");
    assert_eq!(moved["due_date"], "2026-12-01");

    // Un espace inconnu : refus, et le titre envoyé avec n'est pas écrit.
    let (status, _) = app
        .api(
            Method::PATCH,
            &path,
            Some(json!({ "title": "Changé", "space": "nulle-part" })),
            a,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, kept) = app.api(Method::GET, &path, None, a).await;
    assert_eq!(kept["title"], "Réviser");
    assert_eq!(kept["space"], "taff");

    // La tâche d'un autre compte : 404, même vers un espace à soi.
    let (status, _) = app
        .api(Method::PATCH, &path, Some(json!({ "space": "perso" })), b)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Par le connecteur : outil à part, marqué destructeur.
    let (_, listed) = app
        .api(
            Method::POST,
            "/mcp",
            Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} })),
            a,
        )
        .await;
    let tool = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "deplacer_tache")
        .expect("outil deplacer_tache")
        .clone();
    assert_eq!(tool["annotations"]["destructiveHint"], true);
    let (_, called) = app
        .api(
            Method::POST,
            "/mcp",
            Some(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                         "params": { "name": "deplacer_tache",
                                     "arguments": { "id": id, "space": "perso" } } })),
            a,
        )
        .await;
    assert_eq!(called["result"]["isError"], false, "{called}");
    let (_, after) = app.api(Method::GET, &path, None, a).await;
    assert_eq!(after["space"], "perso");
}

#[tokio::test]
async fn ce_qui_protege_le_compte_n_a_pas_de_route() {
    let app = TestApp::new().await;
    let tk = token(&app, ALICE, "Script").await;
    for (method, path) in [
        (Method::POST, "/api/v1/settings/password"),
        (Method::PATCH, "/api/v1/settings/password"),
        (Method::POST, "/api/v1/settings/tokens"),
        (Method::GET, "/api/v1/settings/tokens"),
        (Method::POST, "/api/v1/settings/google/unlink"),
        (Method::DELETE, "/api/v1/trash"),
    ] {
        let (status, _) = app
            .api(method.clone(), path, Some(json!({})), Some(&tk))
            .await;
        assert!(
            status == StatusCode::NOT_FOUND || status == StatusCode::METHOD_NOT_ALLOWED,
            "{method} {path} {status}"
        );
    }
}

#[tokio::test]
async fn les_outils_mcp_des_espaces() {
    let app = TestApp::new().await;
    let alice = token(&app, ALICE, "Claude").await;
    let bob = token(&app, BOB, "Claude de Bob").await;
    let call = |who: &'static str, name: &'static str, arguments: Value| {
        let app = &app;
        let tk = if who == "alice" {
            alice.clone()
        } else {
            bob.clone()
        };
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

    // Annotations : ce que la spec fait confirmer est « destructeur ».
    let (_, listed) = app
        .api(
            Method::POST,
            "/mcp",
            Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} })),
            Some(&alice),
        )
        .await;
    let tools = listed["result"]["tools"].as_array().unwrap();
    let hint = |name: &str, field: &str| {
        tools
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("{name}"))["annotations"][field]
            .clone()
    };
    for name in [
        "archiver_espace",
        "supprimer_espace",
        "inviter_membre",
        "retirer_membre",
        "quitter_espace",
    ] {
        assert_eq!(hint(name, "destructiveHint"), true, "{name}");
    }
    for name in ["lister_membres", "lire_reglages", "lire_tableau_de_bord"] {
        assert_eq!(hint(name, "readOnlyHint"), true, "{name}");
    }

    let created = ok(call("alice", "creer_espace", json!({ "name": "Sport" })).await);
    assert_eq!(created["slug"], "sport");
    let edited = ok(call(
        "alice",
        "modifier_espace",
        json!({ "space": "sport", "color": "teal", "auto_mp4": true }),
    )
    .await);
    assert_eq!(edited["color"], "teal");
    assert_eq!(edited["auto_mp4"], true);
    assert_eq!(
        ok(call("alice", "archiver_espace", json!({ "space": "sport" })).await)["archived"],
        true
    );
    assert_eq!(
        ok(call("alice", "desarchiver_espace", json!({ "space": "sport" })).await)["archived"],
        false
    );

    let invitation = ok(call(
        "alice",
        "inviter_membre",
        json!({ "space": "sport", "email": BOB }),
    )
    .await);
    let received = ok(call("bob", "lister_invitations", json!({})).await);
    assert_eq!(received[0]["id"], invitation["id"]);
    let joined = ok(call(
        "bob",
        "accepter_invitation",
        json!({ "id": invitation["id"] }),
    )
    .await);
    assert_eq!(joined["mine"], false);

    // Un membre : refus lisible, sans erreur de protocole.
    let refused = call(
        "bob",
        "supprimer_espace",
        json!({ "space": "sport", "content": "trash" }),
    )
    .await;
    assert_eq!(refused["isError"], true);
    let members = ok(call("alice", "lister_membres", json!({ "space": "sport" })).await);
    let bob_id = members["members"][0]["id"].clone();
    ok(call(
        "alice",
        "retirer_membre",
        json!({ "space": "sport", "id": bob_id }),
    )
    .await);

    let board = ok(call(
        "alice",
        "modifier_tableau_de_bord",
        json!({ "space": "sport", "layout": "tasks:2x2" }),
    )
    .await);
    assert_eq!(board["layout"], "tasks:2x2");
    assert_eq!(
        ok(call("alice", "lire_tableau_de_bord", json!({ "space": "sport" })).await)["layout"],
        "tasks:2x2"
    );

    let settings = ok(call(
        "alice",
        "modifier_reglages",
        json!({ "theme": "light", "moment_space": "sport" }),
    )
    .await);
    assert_eq!(settings["theme"], "light");
    assert_eq!(
        ok(call("alice", "lire_reglages", json!({})).await)["moment_space"],
        "sport"
    );
    let password = call("alice", "modifier_reglages", json!({ "password": "x" })).await;
    assert_eq!(password["isError"], true);

    use base64::Engine as _;
    let content = base64::engine::general_purpose::STANDARD.encode(png());
    ok(call(
        "alice",
        "changer_avatar",
        json!({ "content_base64": content }),
    )
    .await);
    assert_eq!(
        ok(call("alice", "lire_reglages", json!({})).await)["has_avatar"],
        true
    );
    ok(call("alice", "retirer_avatar", json!({})).await);

    ok(call(
        "alice",
        "supprimer_espace",
        json!({ "space": "sport", "transfer_to": "perso" }),
    )
    .await);
    let spaces = ok(call("alice", "lister_espaces", json!({})).await);
    assert!(!spaces.to_string().contains("sport"));
}
