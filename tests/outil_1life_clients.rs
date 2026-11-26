//! 1Life › Clients (SPEC, « 1Life — Clients ») : un client est un
//! sous-dossier direct du dossier des clients choisi par le compte, avec ses
//! infos en Markdown et ses journées (des événements de l'Agenda). Un test
//! (au moins) par critère d'acceptation.
//!
//! Dépôt public : tous les noms ci-dessous sont inventés.

mod common;

use axum::http::{Method, StatusCode};
use chrono::{Datelike, Duration, NaiveDate, Weekday};
use common::{ALICE, BOB, Client, TestApp, between};
use serde_json::{Value, json};

const LIST: &str = "/e/taff/1life/clients";

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

/// Crée un dossier dans Fichiers (dans l'espace, ou dans `parent`) ; rend
/// son identifiant.
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

/// Choisit le dossier des clients, comme la fenêtre « Ranger ».
async fn choose(client: &mut Client, space: i64, folder: i64) -> common::TestResponse {
    client
        .post(
            &format!("{LIST}/dossier"),
            &[
                ("space_id", &space.to_string()),
                ("folder_id", &folder.to_string()),
            ],
        )
        .await
}

/// Un compte autorisé, avec « Taff / Clients » comme dossier des clients ;
/// rend (client connecté, espace Taff, dossier des clients).
async fn ready(app: &TestApp, email: &str) -> (Client, i64, i64) {
    let mut client = granted(app, email).await;
    let taff = app.space_id(email, "taff").await;
    let book = folder(app, &mut client, taff, None, "Clients").await;
    let chosen = choose(&mut client, taff, book).await;
    assert_eq!(chosen.status, StatusCode::SEE_OTHER);
    (client, taff, book)
}

/// « Nouveau client » ; rend l'identifiant du dossier créé.
async fn new_client(client: &mut Client, name: &str) -> i64 {
    let created = client.post(LIST, &[("name", name)]).await;
    assert_eq!(created.status, StatusCode::SEE_OTHER, "{}", created.body);
    created
        .location()
        .rsplit('/')
        .next()
        .unwrap()
        .parse()
        .unwrap_or_else(|_| panic!("adresse de la fiche : {}", created.location()))
}

/// Les noms de la liste, dans l'ordre.
fn names(body: &str) -> Vec<String> {
    body.split("<span class=\"client-name\">")
        .skip(1)
        .filter_map(|part| part.split("</span>").next())
        .map(str::to_string)
        .collect()
}

/// Le prochain lundi après aujourd'hui (heure de Paris), et la date au
/// format des formulaires.
fn next_monday() -> NaiveDate {
    let today = dashu::time::paris_date(dashu::time::now());
    let mut day = today + Duration::days(1);
    while day.weekday() != Weekday::Mon {
        day += Duration::days(1);
    }
    day
}

fn iso(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

/// Ajoute une journée depuis la fiche ; rend la réponse.
async fn add_day(
    client: &mut Client,
    id: i64,
    date: NaiveDate,
    extra: &[(&str, &str)],
) -> common::TestResponse {
    let date = iso(date);
    let mut form = vec![
        ("date", date.as_str()),
        ("jours", "1"),
        ("creneau", "journee"),
        ("label", "Paramétrage"),
        ("lieu", "site"),
        ("place", "Lyon"),
    ];
    for (key, value) in extra {
        form.retain(|(k, _)| k != key);
        form.push((key, value));
    }
    client.post(&format!("{LIST}/{id}/journees"), &form).await
}

/// Les événements reliés à un client : (id, titre, lieu, début, fin, google).
async fn days_of(app: &TestApp, client: i64) -> Vec<(i64, String, String, i64, i64, bool)> {
    sqlx::query_as(
        "SELECT e.id, e.title, e.location, e.starts_at, e.ends_at, e.google
           FROM onelife_client_days d JOIN events e ON e.id = d.event_id
          WHERE d.folder_id = ? ORDER BY e.starts_at",
    )
    .bind(client)
    .fetch_all(&app.state.db)
    .await
    .unwrap()
}

fn at(date: NaiveDate, h: u32, m: u32) -> i64 {
    dashu::time::local_to_ts(date.and_hms_opt(h, m, 0).unwrap(), dashu::time::PARIS)
}

#[tokio::test]
async fn sans_dossier_le_module_explique_puis_la_fenetre_le_choisit() {
    let app = TestApp::new().await;
    let mut alice = granted(&app, ALICE).await;
    let taff = app.space_id(ALICE, "taff").await;

    // Rien de choisi : l'état vide explique et ouvre la fenêtre « Ranger ».
    let page = alice.get(LIST).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("Choisir le dossier des clients"));
    assert!(page.body.contains("hx-get=\"/destinations?clients=1\""));
    assert!(!page.body.contains("Nouveau client"));

    // La fenêtre : tous les dossiers des niveaux 1 à 3, les espaces grisés,
    // et l'action du module.
    let a = folder(&app, &mut alice, taff, None, "Clients").await;
    let b = folder(&app, &mut alice, taff, Some(a), "B").await;
    let c = folder(&app, &mut alice, taff, Some(b), "C").await;
    let d = folder(&app, &mut alice, taff, Some(c), "Trop bas").await;
    let picker = alice.get_fragment("/e/taff/destinations?clients=1").await;
    assert_eq!(picker.status, StatusCode::OK);
    assert!(picker.body.contains("Dossier des clients"));
    assert!(
        picker
            .body
            .contains("action=\"/e/taff/1life/clients/dossier\"")
    );
    assert!(
        picker
            .body
            .contains(&format!("name=\"folder_id\" value=\"{c}\""))
    );
    assert!(
        !picker
            .body
            .contains(&format!("name=\"folder_id\" value=\"{d}\""))
    );
    let space_button = between(&picker.body, "class=\"dest dest-d0\"", ">").unwrap();
    assert!(space_button.contains("disabled"), "{space_button}");

    // Un espace seul, ou un dossier trop bas : refusé, rien n'est enregistré.
    let refused = alice
        .post(
            &format!("{LIST}/dossier"),
            &[("space_id", &taff.to_string()), ("folder_id", "")],
        )
        .await;
    assert_eq!(refused.location(), LIST);
    assert!(
        alice
            .follow(refused)
            .await
            .body
            .contains("un espace ne peut pas")
    );
    let refused = choose(&mut alice, taff, d).await;
    assert!(alice.follow(refused).await.body.contains("trop bas"));
    let count = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM onelife_settings")
            .fetch_one(&app.state.db)
            .await
            .unwrap()
    };
    assert_eq!(count().await, 0);

    // Un bon dossier : enregistré, la page montre son chemin.
    let chosen = choose(&mut alice, taff, a).await;
    let page = alice.follow(chosen).await;
    assert!(page.body.contains("Dossier des clients : Taff / Clients"));
    assert!(page.body.contains("Nouveau client"));
    assert!(page.body.contains("<span>Taff / Clients</span>"));
    // La fenêtre le montre comme l'endroit actuel.
    let picker = alice.get_fragment("/e/taff/destinations?clients=1").await;
    let current = between(
        &picker.body,
        &format!("name=\"folder_id\" value=\"{a}\""),
        "</button>",
    )
    .unwrap();
    assert!(current.contains("aria-current=\"true\""));

    // « Changer » le remplace.
    let other = folder(&app, &mut alice, taff, None, "Missions").await;
    choose(&mut alice, taff, other).await;
    assert_eq!(count().await, 1);
    assert!(
        alice
            .get(LIST)
            .await
            .body
            .contains("<span>Taff / Missions</span>")
    );

    // Pour un compte non autorisé, la fenêtre en mode clients n'existe pas.
    let mut bob = app.logged_in(BOB).await;
    assert_eq!(
        bob.get_fragment("/destinations?clients=1").await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn les_clients_sont_les_sous_dossiers_en_direct() {
    let app = TestApp::new().await;
    let (mut alice, taff, book) = ready(&app, ALICE).await;

    // Un dossier créé dans Fichiers est un client ; ses propres
    // sous-dossiers n'en sont pas.
    let atelier = folder(&app, &mut alice, taff, Some(book), "Atelier Test").await;
    folder(&app, &mut alice, taff, Some(atelier), "Sous-dossier").await;
    let exemple = folder(&app, &mut alice, taff, Some(book), "Client Exemple").await;
    assert_eq!(
        names(&alice.get(LIST).await.body),
        ["Atelier Test", "Client Exemple"]
    );

    // Renommé : le module suit.
    alice
        .post(
            &format!("/folders/{exemple}/rename"),
            &[("name", "Client Renommé")],
        )
        .await;
    assert_eq!(
        names(&alice.get(LIST).await.body),
        ["Atelier Test", "Client Renommé"]
    );
    assert!(
        alice
            .get(&format!("{LIST}/{exemple}"))
            .await
            .body
            .contains("<h1>Client Renommé</h1>")
    );

    // Déplacé hors du dossier : il quitte le module, sa fiche répond 404.
    alice
        .post(
            &format!("/folders/{exemple}/move"),
            &[("space_id", &taff.to_string()), ("folder_id", "")],
        )
        .await;
    assert_eq!(names(&alice.get(LIST).await.body), ["Atelier Test"]);
    assert_eq!(
        alice.get(&format!("{LIST}/{exemple}")).await.status,
        StatusCode::NOT_FOUND
    );

    // Mis à la corbeille : pareil.
    alice
        .post(
            &format!("/folders/{atelier}/delete"),
            &[("contenu", "supprimer")],
        )
        .await;
    assert!(names(&alice.get(LIST).await.body).is_empty());
    assert!(
        alice
            .get(LIST)
            .await
            .body
            .contains("Aucun client pour l&#39;instant")
            || alice
                .get(LIST)
                .await
                .body
                .contains("Aucun client pour l'instant")
    );

    // Le dossier des clients lui-même à la corbeille : le module le dit.
    alice
        .post(
            &format!("/folders/{book}/delete"),
            &[("contenu", "supprimer")],
        )
        .await;
    let page = alice.get(LIST).await;
    assert!(
        page.body.contains("n&#39;est plus accessible")
            || page.body.contains("n'est plus accessible")
    );
}

#[tokio::test]
async fn nouveau_client_cree_le_dossier_et_ses_sous_dossiers() {
    let app = TestApp::new().await;
    let (mut alice, _, book) = ready(&app, ALICE).await;

    let created = alice.post(LIST, &[("name", "  Atelier   Test ")]).await;
    assert_eq!(created.status, StatusCode::SEE_OTHER);
    let id: i64 = created
        .location()
        .rsplit('/')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(created.location(), format!("{LIST}/{id}"));
    let fiche = alice.follow(created).await;
    assert!(fiche.body.contains("Client « Atelier Test » créé"));
    assert!(fiche.body.contains("<h1>Atelier Test</h1>"));
    // Le dossier, dans le dossier des clients, et Audit, CR, Fichiers.
    let parent: Option<i64> = sqlx::query_scalar("SELECT parent_id FROM folders WHERE id = ?")
        .bind(id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(parent, Some(book));
    let subs: Vec<String> =
        sqlx::query_scalar("SELECT name FROM folders WHERE parent_id = ? ORDER BY name")
            .bind(id)
            .fetch_all(&app.state.db)
            .await
            .unwrap();
    assert_eq!(subs, ["Audit", "CR", "Fichiers"]);
    // L'encart Fichiers les montre.
    for name in ["Audit", "CR", "Fichiers"] {
        assert!(
            fiche.body.contains(&format!("<span>{name}</span>")),
            "{name}"
        );
    }

    // Un nom déjà pris (majuscules mises à part) ou vide : refusé, avec ce
    // qui a été tapé ; rien n'est créé.
    for (typed, message) in [
        ("atelier test", "Un client « atelier test » existe déjà."),
        ("   ", "Donne un nom au client."),
    ] {
        let refused = alice.post(LIST, &[("name", typed)]).await;
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{typed}");
        assert!(refused.body.contains(message), "{typed}\n{}", refused.body);
        assert!(
            refused.body.contains(&format!("value=\"{typed}\"")),
            "{typed}"
        );
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM folders WHERE parent_id = ?")
        .bind(book)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn la_liste_montre_la_prochaine_journee_et_les_fichiers() {
    let app = TestApp::new().await;
    let (mut alice, _, _) = ready(&app, ALICE).await;
    let atelier = new_client(&mut alice, "Atelier Test").await;
    new_client(&mut alice, "Client Exemple").await;
    new_client(&mut alice, "Équipe Démo").await;

    // Deux fichiers, dont un dans un sous-dossier : comptés tous les deux.
    let audit: i64 =
        sqlx::query_scalar("SELECT id FROM folders WHERE parent_id = ? AND name = 'Audit'")
            .bind(atelier)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(
        alice.upload_in_folder("a.txt", b"a", atelier).await.status,
        StatusCode::CREATED
    );
    assert_eq!(
        alice.upload_in_folder("b.txt", b"b", audit).await.status,
        StatusCode::CREATED
    );
    add_day(&mut alice, atelier, next_monday(), &[]).await;

    let page = alice.get(LIST).await;
    assert_eq!(
        names(&page.body),
        ["Atelier Test", "Client Exemple", "Équipe Démo"]
    );
    let row = between(&page.body, "Atelier Test</span>", "</a>").unwrap();
    assert!(row.contains("2 fichiers"), "{row}");
    let monday = dashu::time::weekday_day_month(next_monday());
    assert!(
        row.contains(&format!("Prochaine journée : {monday}, 9 h – 17 h")),
        "{row}"
    );
    let row = between(&page.body, "Client Exemple</span>", "</a>").unwrap();
    assert!(
        row.contains("Aucune journée prévue") && row.contains("Aucun fichier"),
        "{row}"
    );

    // Recherche par nom, accents et majuscules ignorés.
    for (q, found) in [
        ("equipe", vec!["Équipe Démo"]),
        ("ATELIER", vec!["Atelier Test"]),
        ("ex", vec!["Client Exemple"]),
        ("personne", vec![]),
    ] {
        let page = alice.get(&format!("{LIST}?q={q}")).await;
        assert_eq!(names(&page.body), found, "{q}");
        if found.is_empty() {
            assert!(page.body.contains("Aucun client ne correspond"));
        }
    }
}

#[tokio::test]
async fn les_infos_s_enregistrent_toutes_seules_et_s_affichent_rendues() {
    let app = TestApp::new().await;
    let (mut alice, _, _) = ready(&app, ALICE).await;
    let id = new_client(&mut alice, "Atelier Test").await;
    let infos = format!("{LIST}/{id}/infos");

    // Rien encore ; « Modifier » ouvre la zone de texte dans l'encart.
    let fiche = alice.get(&format!("{LIST}/{id}")).await;
    assert!(
        fiche.body.contains("Rien pour l&#39;instant")
            || fiche.body.contains("Rien pour l'instant")
    );
    assert!(
        fiche
            .body
            .contains(&format!("hx-get=\"{infos}?modifier=1\""))
    );
    let editor = alice.get_fragment(&format!("{infos}?modifier=1")).await;
    assert_eq!(editor.status, StatusCode::OK);
    assert!(
        editor
            .body
            .starts_with("<section class=\"card client-infos\" id=\"client-infos\"")
    );
    assert!(editor.body.contains("<textarea"));
    assert!(editor.body.contains(&format!("hx-post=\"{infos}?auto=1\"")));
    assert!(editor.body.contains("delay:1s"));
    assert!(editor.body.contains("Enregistrement…"));

    // Pendant la frappe : enregistré, « Enregistré à HH:MM ».
    alice.with_header("hx-request", "true");
    let saved = alice
        .post(
            &format!("{infos}?auto=1"),
            &[("body", "Contact : **Camille Exemple**\r\n\n<script>alert(1)</script><img src=x onerror=alert(2)>")],
        )
        .await;
    assert_eq!(saved.status, StatusCode::OK);
    assert!(saved.body.starts_with("Enregistré à "), "{}", saved.body);
    let stored: String =
        sqlx::query_scalar("SELECT body_md FROM onelife_client_infos WHERE folder_id = ?")
            .bind(id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert!(stored.starts_with("Contact : **Camille Exemple**\n\n"));

    // « Terminé » : l'encart rendu ; le script est neutralisé.
    alice.with_header("hx-request", "true");
    let done = alice.post(&infos, &[("body", &stored)]).await;
    assert_eq!(done.status, StatusCode::OK);
    assert!(done.body.contains("<strong>Camille Exemple</strong>"));
    for body in [&done.body, &alice.get(&format!("{LIST}/{id}")).await.body] {
        assert!(body.contains("<strong>Camille Exemple</strong>"));
        assert!(!body.contains("<script>alert(1)"));
        assert!(!body.contains("onerror"));
    }
    // Sans JavaScript, « Terminé » ramène à la fiche.
    let done = alice.post(&infos, &[("body", "Sans script")]).await;
    assert_eq!(done.status, StatusCode::SEE_OTHER);
    assert_eq!(done.location(), format!("{LIST}/{id}"));

    // 20 000 caractères au plus : refusé, rien ne change.
    let long = "x".repeat(20_001);
    alice.with_header("hx-request", "true");
    let refused = alice
        .post(&format!("{infos}?auto=1"), &[("body", &long)])
        .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(refused.body.contains("20 000 caractères au plus"));
    let stored: String =
        sqlx::query_scalar("SELECT body_md FROM onelife_client_infos WHERE folder_id = ?")
            .bind(id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(stored, "Sans script");
}

/// Relie Google Agenda au compte, sans vrai Google : l'envoi échouera, mais
/// l'événement est marqué pour Google.
async fn link_google(app: &TestApp, email: &str) {
    let user = app.user_id(email).await;
    sqlx::query(
        "INSERT INTO google_accounts (user_id, email, refresh_token, connected_at) VALUES (?, ?, ?, 0)",
    )
    .bind(user)
    .bind("agenda@exemple.test")
    .bind(vec![0u8; 16])
    .execute(&app.state.db)
    .await
    .unwrap();
}

#[tokio::test]
async fn une_journee_ajoutee_va_dans_la_fiche_l_agenda_le_gantt_et_google() {
    let app = TestApp::with_google("http://127.0.0.1:9").await;
    let (mut alice, _, _) = ready(&app, ALICE).await;
    let id = new_client(&mut alice, "Atelier Test").await;

    // Le formulaire : la case Google seulement si Google Agenda est relié.
    let form = alice.get(&format!("{LIST}/{id}/journees/new")).await;
    assert_eq!(form.status, StatusCode::OK);
    assert!(!form.body.contains("Envoyer à Google Agenda"));
    link_google(&app, ALICE).await;
    let form = alice.get(&format!("{LIST}/{id}/journees/new")).await;
    assert!(
        form.body
            .contains("Envoyer à Google Agenda (agenda@exemple.test)")
    );

    let monday = next_monday();
    let added = add_day(&mut alice, id, monday, &[("google", "on")]).await;
    assert_eq!(added.status, StatusCode::SEE_OTHER, "{}", added.body);
    let week = iso(monday);
    assert_eq!(added.location(), format!("{LIST}/{id}?semaine={week}"));

    // L'événement : titre, lieu, horaires, espace du dossier, Google.
    let days = days_of(&app, id).await;
    assert_eq!(days.len(), 1);
    let (event, title, location, starts, ends, google) = days[0].clone();
    assert_eq!(title, "Atelier Test — Paramétrage");
    assert_eq!(location, "Sur site · Lyon");
    assert_eq!((starts, ends), (at(monday, 9, 0), at(monday, 17, 0)));
    assert!(google);
    let space: i64 = sqlx::query_scalar("SELECT space_id FROM events WHERE id = ?")
        .bind(event)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(space, app.space_id(ALICE, "taff").await);

    // Dans la fiche : la grille de la semaine, la journée ouvre le formulaire
    // du module.
    let fiche = alice.follow(added).await;
    assert!(fiche.body.contains("Journée du "));
    assert!(fiche.body.contains("class=\"cal cal-d5\""));
    assert!(fiche.body.contains("<strong>Paramétrage</strong>"));
    assert!(
        fiche
            .body
            .contains(&format!("href=\"{LIST}/{id}/journees/{event}\""))
    );
    assert!(!fiche.body.contains(&format!("/calendar/events/{event}")));

    // Dans l'Agenda : l'événement ordinaire, avec son formulaire habituel.
    let agenda = alice
        .get(&format!("/e/taff/calendar?view=week&date={week}"))
        .await;
    assert!(agenda.body.contains("Atelier Test — Paramétrage"));
    assert!(
        agenda
            .body
            .contains(&format!("href=\"/calendar/events/{event}\""))
    );

    // Dans le Gantt de la période qui la contient.
    let gantt = alice.get(&format!("{LIST}?vue=globale&du={week}")).await;
    assert!(gantt.body.contains("<svg class=\"gantt\""));
    assert!(gantt.body.contains("Paramétrage</title>"));
    assert!(gantt.body.contains(&format!("href=\"{LIST}/{id}\"")));
}

#[tokio::test]
async fn demi_journees_et_plusieurs_jours_ouvres() {
    let app = TestApp::new().await;
    let (mut alice, _, _) = ready(&app, ALICE).await;
    let id = new_client(&mut alice, "Atelier Test").await;
    let monday = next_monday();
    let thursday = monday + Duration::days(3);

    // Jeudi, trois jours ouvrés, le matin : jeudi, vendredi, lundi.
    let added = add_day(
        &mut alice,
        id,
        thursday,
        &[("jours", "3"), ("creneau", "matin")],
    )
    .await;
    assert_eq!(added.status, StatusCode::SEE_OTHER, "{}", added.body);
    assert!(
        alice
            .follow(added)
            .await
            .body
            .contains("3 journées ajoutées")
    );
    let days = days_of(&app, id).await;
    let next_monday = monday + Duration::days(7);
    assert_eq!(
        days.iter().map(|d| (d.3, d.4)).collect::<Vec<_>>(),
        [thursday, thursday + Duration::days(1), next_monday]
            .iter()
            .map(|d| (at(*d, 9, 0), at(*d, 12, 30)))
            .collect::<Vec<_>>()
    );
    // L'après-midi : 13 h 30 – 17 h.
    add_day(&mut alice, id, monday, &[("creneau", "apres-midi")]).await;
    let afternoon = days_of(&app, id).await[0].clone();
    assert_eq!(
        (afternoon.3, afternoon.4),
        (at(monday, 13, 30), at(monday, 17, 0))
    );

    // Dans le Gantt : la moitié de la colonne, à gauche le matin, à droite
    // l'après-midi (colonnes de 28 px après 176 px de noms).
    let gantt = alice
        .get(&format!("{LIST}?vue=globale&du={}", iso(monday)))
        .await;
    let col = |i: i64| 176 + i * 28;
    assert!(
        gantt
            .body
            .contains(&format!("x=\"{}\" y=\"50\" width=\"12\"", col(0) + 14)),
        "{}",
        gantt.body
    );
    assert!(
        gantt
            .body
            .contains(&format!("x=\"{}\" y=\"50\" width=\"12\"", col(3) + 2))
    );
    assert!(
        gantt
            .body
            .contains(&format!("x=\"{}\" y=\"50\" width=\"12\"", col(7) + 2))
    );
    assert!(
        !gantt
            .body
            .contains(&format!("x=\"{}\" y=\"50\"", col(5) + 2))
    );

    // Horaires libres, et les refus : premier jour un samedi, trop de jours,
    // fin avant le début.
    add_day(
        &mut alice,
        id,
        monday + Duration::days(1),
        &[("creneau", "libre"), ("start", "10:00"), ("end", "15:30")],
    )
    .await;
    assert_eq!(days_of(&app, id).await.len(), 5);
    for (extra, message) in [
        (vec![("date", iso(monday - Duration::days(2)))], "week-end"),
        (vec![("jours", "21".to_string())], "Entre 1 et 20"),
        (
            vec![
                ("creneau", "libre".to_string()),
                ("start", "15:00".to_string()),
                ("end", "10:00".to_string()),
            ],
            "La fin doit venir après le début.",
        ),
    ] {
        let extra: Vec<(&str, &str)> = extra.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let date = extra
            .iter()
            .find(|(k, _)| *k == "date")
            .map(|(_, v)| NaiveDate::parse_from_str(v, "%Y-%m-%d").unwrap())
            .unwrap_or(monday);
        let refused = add_day(&mut alice, id, date, &extra).await;
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
        assert!(refused.body.contains("value=\"Paramétrage\""), "{message}");
    }
    assert_eq!(days_of(&app, id).await.len(), 5);
}

#[tokio::test]
async fn une_journee_se_modifie_et_se_supprime_depuis_la_fiche() {
    let app = TestApp::new().await;
    let (mut alice, _, _) = ready(&app, ALICE).await;
    let id = new_client(&mut alice, "Atelier Test").await;
    let monday = next_monday();
    add_day(&mut alice, id, monday, &[]).await;
    let event = days_of(&app, id).await[0].0;
    let path = format!("{LIST}/{id}/journees/{event}");

    // Le formulaire du module, rempli.
    let form = alice.get(&path).await;
    assert_eq!(form.status, StatusCode::OK);
    assert!(form.body.contains("Modifier la journée"));
    assert!(form.body.contains("value=\"Paramétrage\""));
    assert!(form.body.contains("value=\"Lyon\""));
    assert!(form.body.contains("value=\"journee\" checked"));
    assert!(form.body.contains(&format!("action=\"{path}/delete\"")));

    // Le client est renommé entre-temps : le titre reprend le nouveau nom.
    alice
        .post(
            &format!("/folders/{id}/rename"),
            &[("name", "Atelier Renommé")],
        )
        .await;
    let saved = alice
        .post(
            &path,
            &[
                ("date", &iso(monday + Duration::days(1))),
                ("creneau", "apres-midi"),
                ("label", "Formation"),
                ("lieu", "distance"),
                ("place", ""),
            ],
        )
        .await;
    assert_eq!(saved.status, StatusCode::SEE_OTHER, "{}", saved.body);
    let day = days_of(&app, id).await[0].clone();
    assert_eq!(day.0, event);
    assert_eq!(day.1, "Atelier Renommé — Formation");
    assert_eq!(day.2, "À distance");
    let tuesday = monday + Duration::days(1);
    assert_eq!((day.3, day.4), (at(tuesday, 13, 30), at(tuesday, 17, 0)));
    let form = alice.get(&path).await;
    assert!(form.body.contains("value=\"apres-midi\" checked"));
    assert!(form.body.contains("value=\"distance\" checked"));

    // Supprimer : l'événement et son lien partent.
    let deleted = alice.post(&format!("{path}/delete"), &[]).await;
    assert_eq!(deleted.status, StatusCode::SEE_OTHER);
    assert!(days_of(&app, id).await.is_empty());
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM events")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(left, 0);
    assert_eq!(alice.get(&path).await.status, StatusCode::NOT_FOUND);

    // Un événement ordinaire de l'Agenda n'est pas une journée du client.
    let created = alice
        .post(
            "/calendar/events",
            &[
                ("title", "Autre chose"),
                ("date", &iso(monday)),
                ("start", "09:00"),
                ("end", "10:00"),
                ("location", ""),
                ("space_id", &app.space_id(ALICE, "taff").await.to_string()),
            ],
        )
        .await;
    assert_eq!(created.status, StatusCode::SEE_OTHER);
    let other: i64 = sqlx::query_scalar("SELECT id FROM events")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(
        alice
            .get(&format!("{LIST}/{id}/journees/{other}"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

/// Relie à un client une journée posée directement en base.
async fn insert_day(app: &TestApp, email: &str, client: i64, starts_at: i64, ends_at: i64) -> i64 {
    let user = app.user_id(email).await;
    let space = app.space_id(email, "taff").await;
    let event: i64 = sqlx::query_scalar(
        "INSERT INTO events (user_id, space_id, title, starts_at, ends_at, created_at, updated_at)
         VALUES (?, ?, 'Journée', ?, ?, 0, 0) RETURNING id",
    )
    .bind(user)
    .bind(space)
    .bind(starts_at)
    .bind(ends_at)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO onelife_client_days (event_id, folder_id, label, created_at) VALUES (?, ?, '', 0)",
    )
    .bind(event)
    .bind(client)
    .execute(&app.state.db)
    .await
    .unwrap();
    event
}

#[tokio::test]
async fn la_prochaine_journee_donne_la_date_les_horaires_et_le_delai() {
    let app = TestApp::new().await;
    let (mut alice, _, _) = ready(&app, ALICE).await;
    let id = new_client(&mut alice, "Atelier Test").await;
    let fiche = format!("{LIST}/{id}");
    assert!(
        alice
            .get(&fiche)
            .await
            .body
            .contains("Aucune journée prévue")
    );

    let now = dashu::time::now();
    let today = dashu::time::paris_date(now);
    // Une journée finie ne compte pas ; dans 7 jours, puis demain, puis une
    // journée en cours : chaque fois la plus proche.
    insert_day(&app, ALICE, id, now - 7200, now - 3600).await;
    let in_seven = today + Duration::days(7);
    insert_day(&app, ALICE, id, at(in_seven, 9, 0), at(in_seven, 17, 0)).await;
    let label = |date: NaiveDate| dashu::time::weekday_day_month(date);
    let body = alice.get(&fiche).await.body;
    assert!(
        body.contains(&format!(
            "Prochaine journée : <strong>{}, 9 h – 17 h (dans 7 jours)</strong>",
            label(in_seven)
        )),
        "{}",
        between(&body, "client-next", "</p>").unwrap_or_default()
    );
    let tomorrow = today + Duration::days(1);
    insert_day(&app, ALICE, id, at(tomorrow, 13, 30), at(tomorrow, 17, 0)).await;
    assert!(alice.get(&fiche).await.body.contains(&format!(
        "Prochaine journée : <strong>{}, 13 h 30 – 17 h (demain)</strong>",
        label(tomorrow)
    )));
    insert_day(&app, ALICE, id, now - 60, now + 3600).await;
    assert!(
        alice
            .get(&fiche)
            .await
            .body
            .contains("(aujourd&#39;hui)</strong>")
            || alice
                .get(&fiche)
                .await
                .body
                .contains("(aujourd'hui)</strong>")
    );
}

#[tokio::test]
async fn le_gantt_navigue_et_filtre_sans_style_en_ligne() {
    let app = TestApp::new().await;
    let (mut alice, _, _) = ready(&app, ALICE).await;
    let busy = new_client(&mut alice, "Atelier Test").await;
    new_client(&mut alice, "Client Exemple").await;
    let monday = dashu::time::monday_of(dashu::time::paris_date(dashu::time::now()));
    insert_day(
        &app,
        ALICE,
        busy,
        at(monday + Duration::days(8), 9, 0),
        at(monday + Duration::days(8), 17, 0),
    )
    .await;

    // 4 semaines à partir du lundi de cette semaine : 28 colonnes, un trait
    // sur aujourd'hui, les deux clients, un lien vers chaque fiche.
    let page = alice.get(&format!("{LIST}?vue=globale")).await;
    assert_eq!(page.status, StatusCode::OK);
    let svg = between(&page.body, "<svg class=\"gantt\"", "</svg>").unwrap();
    assert!(svg.contains(&format!("width=\"{}\"", 176 + 28 * 28)));
    assert!(svg.contains("class=\"gantt-today\""));
    assert!(svg.contains("class=\"gantt-weekend\""));
    assert!(svg.contains(&format!("href=\"{LIST}/{busy}\"")));
    assert!(svg.contains("Client Exemple"));
    assert!(svg.contains("class=\"gantt-bar\""));
    assert!(!page.body.contains("style="));
    assert!(!svg.contains("<script"));
    let du = |d: NaiveDate| format!("du={}", iso(d));
    assert!(
        page.body.contains(&format!(
            "href=\"{LIST}?vue=globale&amp;{}\"",
            du(monday - Duration::days(28))
        )) || page.body.contains(&format!(
            "href=\"{LIST}?vue=globale&#38;{}\"",
            du(monday - Duration::days(28))
        ))
    );

    // 3 mois : 91 colonnes ; précédent et suivant avancent de 13 semaines.
    let page = alice.get(&format!("{LIST}?vue=globale&duree=3m")).await;
    let svg = between(&page.body, "<svg class=\"gantt\"", "</svg>").unwrap();
    assert!(svg.contains(&format!("width=\"{}\"", 176 + 91 * 28)));
    assert!(page.body.contains(&du(monday + Duration::days(91))));

    // Seulement les clients qui ont des journées.
    let page = alice.get(&format!("{LIST}?vue=globale&avec=1")).await;
    let svg = between(&page.body, "<svg class=\"gantt\"", "</svg>").unwrap();
    assert!(svg.contains("Atelier Test"));
    assert!(!svg.contains("Client Exemple"));
    assert!(page.body.contains("aria-pressed=\"true\""));

    // Une période sans journée, filtre posé : on le dit.
    let later = monday + Duration::days(28 * 6);
    let page = alice
        .get(&format!("{LIST}?vue=globale&avec=1&{}", du(later)))
        .await;
    assert!(page.body.contains("Aucune journée sur cette période"));
    // « Aujourd'hui » ramène à la période en cours.
    assert!(
        page.body
            .contains(&format!("href=\"{LIST}?vue=globale&amp;avec=1\""))
            || page
                .body
                .contains(&format!("href=\"{LIST}?vue=globale&#38;avec=1\""))
    );
}

#[tokio::test]
async fn l_encart_fichiers_reste_dans_le_dossier_du_client() {
    let app = TestApp::new().await;
    let (mut alice, taff, book) = ready(&app, ALICE).await;
    let id = new_client(&mut alice, "Atelier Test").await;
    let audit: i64 =
        sqlx::query_scalar("SELECT id FROM folders WHERE parent_id = ? AND name = 'Audit'")
            .bind(id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    let sent = alice
        .upload_in_folder("rapport.txt", b"contenu", audit)
        .await;
    assert_eq!(sent.status, StatusCode::CREATED);
    let file = sent.header("x-file-id").to_string();
    // Un fichier hors du client, dans le dossier des clients.
    alice.upload_in_folder("ailleurs.txt", b"x", book).await;
    let _ = taff;

    // Le dossier Audit, dans l'encart : fil d'Ariane depuis le client, le
    // fichier ouvert avec un retour à la fiche ; la zone de dépôt envoie ici.
    let fiche = alice.get(&format!("{LIST}/{id}?dossier={audit}")).await;
    assert_eq!(fiche.status, StatusCode::OK);
    let crumbs = between(&fiche.body, "aria-label=\"Emplacement\"", "</nav>").unwrap();
    assert!(crumbs.contains(&format!("href=\"{LIST}/{id}\">Atelier Test</a>")));
    assert!(crumbs.contains("aria-current=\"page\">Audit</span>"));
    let back = format!("{LIST}/{id}?dossier={audit}")
        .replace('/', "%2F")
        .replace('?', "%3F")
        .replace('=', "%3D");
    assert!(
        fiche
            .body
            .contains(&format!("href=\"/files/{file}?depuis={back}\"")),
        "{}",
        fiche.body
    );
    assert!(fiche.body.contains(&format!("data-folder=\"{audit}\"")));
    assert!(fiche.body.contains("data-upload-input"));
    assert!(fiche.body.contains("id=\"file-results\""));
    assert!(!fiche.body.contains("ailleurs.txt"));
    // L'aperçu du fichier ramène à la fiche.
    let view = alice.get(&format!("/files/{file}?depuis={back}")).await;
    assert!(
        view.body
            .contains(&format!("href=\"{LIST}/{id}?dossier={audit}\""))
            || view
                .body
                .contains(&format!("href=\"{LIST}/{id}?dossier&#61;{audit}\"")),
        "{}",
        view.body
    );

    // Un dossier hors du client (le dossier des clients) : l'encart reste
    // sur le client.
    let fiche = alice.get(&format!("{LIST}/{id}?dossier={book}")).await;
    assert_eq!(fiche.status, StatusCode::OK);
    assert!(!fiche.body.contains("ailleurs.txt"));
    assert!(fiche.body.contains("<span>Audit</span>"));
    assert!(fiche.body.contains(&format!("data-folder=\"{id}\"")));
    // « Ouvrir dans Fichiers » : le même dossier dans l'onglet Fichiers.
    assert!(
        fiche
            .body
            .contains(&format!("href=\"/e/taff/files?dossier={id}\""))
    );
}

#[tokio::test]
async fn un_autre_compte_autorise_ne_voit_rien_du_premier() {
    let app = TestApp::new().await;
    let (mut alice, taff, book) = ready(&app, ALICE).await;
    let id = new_client(&mut alice, "Atelier Test").await;
    let monday = next_monday();
    add_day(&mut alice, id, monday, &[]).await;
    let event = days_of(&app, id).await[0].0;
    alice.with_header("hx-request", "true");
    alice
        .post(
            &format!("{LIST}/{id}/infos?auto=1"),
            &[("body", "Secret d'Alice")],
        )
        .await;

    // Bob, autorisé : il ne peut pas choisir le dossier d'Alice, et ses
    // adresses répondent 404.
    let mut bob = granted(&app, BOB).await;
    assert_eq!(
        choose(&mut bob, taff, book).await.status,
        StatusCode::NOT_FOUND
    );
    let page = bob.get(LIST).await;
    assert!(page.body.contains("Choisir le dossier des clients"));
    assert!(!page.body.contains("Atelier Test"));
    for path in [
        format!("{LIST}/{id}"),
        format!("{LIST}/{id}/infos"),
        format!("{LIST}/{id}/journees/new"),
        format!("{LIST}/{id}/journees/{event}"),
    ] {
        assert_eq!(bob.get(&path).await.status, StatusCode::NOT_FOUND, "{path}");
    }
    for path in [
        format!("{LIST}/{id}/infos"),
        format!("{LIST}/{id}/journees"),
        format!("{LIST}/{id}/journees/{event}"),
        format!("{LIST}/{id}/journees/{event}/delete"),
    ] {
        let response = bob
            .post(
                &path,
                &[
                    ("body", "Volé"),
                    ("date", "2030-01-07"),
                    ("creneau", "journee"),
                ],
            )
            .await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{path}");
    }
    assert_eq!(days_of(&app, id).await.len(), 1);

    // Un espace partagé : Bob choisit le même dossier ; il voit les mêmes
    // clients et les mêmes infos, mais pas les journées d'Alice.
    app.share_space(ALICE, "taff", BOB).await;
    let shared: i64 = sqlx::query_scalar("SELECT space_id FROM folders WHERE id = ?")
        .bind(book)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    let chosen = choose(&mut bob, shared, book).await;
    assert_eq!(chosen.status, StatusCode::SEE_OTHER);
    let fiche = bob.get(&format!("{LIST}/{id}")).await;
    assert_eq!(fiche.status, StatusCode::OK, "{}", fiche.body);
    assert!(fiche.body.contains("Secret d&#39;Alice") || fiche.body.contains("Secret d'Alice"));
    assert!(fiche.body.contains("Aucune journée prévue"));
    assert_eq!(
        bob.get(&format!("{LIST}/{id}/journees/{event}"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

/// Toutes les adresses du module (pages), avec et sans l'espace.
fn pages(id: i64, event: i64) -> Vec<String> {
    let mut out = Vec::new();
    for base in ["", "/e/taff"] {
        for rest in [
            "/1life/clients".to_string(),
            "/1life/clients?vue=globale".to_string(),
            "/1life/clients/dossier".to_string(),
            format!("/1life/clients/{id}"),
            format!("/1life/clients/{id}?dossier={id}"),
            format!("/1life/clients/{id}/infos?modifier=1"),
            format!("/1life/clients/{id}/journees/new"),
            format!("/1life/clients/{id}/journees/{event}"),
            "/1life/clients/abc".to_string(),
            "/1life/clients/abc/journees/xyz".to_string(),
        ] {
            out.push(format!("{base}{rest}"));
        }
    }
    out
}

/// Les formulaires du module, avec et sans l'espace.
fn forms(id: i64, event: i64) -> Vec<String> {
    let mut out = Vec::new();
    for base in ["", "/e/taff"] {
        for rest in [
            "/1life/clients".to_string(),
            "/1life/clients/dossier".to_string(),
            format!("/1life/clients/{id}/infos"),
            format!("/1life/clients/{id}/infos?auto=1"),
            format!("/1life/clients/{id}/journees"),
            format!("/1life/clients/{id}/journees/{event}"),
            format!("/1life/clients/{id}/journees/{event}/delete"),
            "/1life/clients/abc/infos".to_string(),
        ] {
            out.push(format!("{base}{rest}"));
        }
    }
    out
}

#[tokio::test]
async fn sans_autorisation_le_module_repond_404_partout() {
    let app = TestApp::new().await;
    let (mut alice, taff, book) = ready(&app, ALICE).await;
    let id = new_client(&mut alice, "Atelier Test").await;
    add_day(&mut alice, id, next_monday(), &[]).await;
    let event = days_of(&app, id).await[0].0;

    // Bob, connecté mais non autorisé : la page d'une adresse qui n'existe
    // pas, même pour un formulaire mal formé.
    let mut bob = app.logged_in(BOB).await;
    let unknown = bob.get("/nulle-part").await;
    for page in pages(id, event) {
        let response = bob.get(&page).await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{page}");
        assert_eq!(response.body, unknown.body, "{page}");
    }
    for form in forms(id, event) {
        let response = bob
            .post(
                &form,
                &[
                    ("name", "Intrus"),
                    ("space_id", &taff.to_string()),
                    ("folder_id", &book.to_string()),
                ],
            )
            .await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{form}");
        assert_eq!(response.body, unknown.body, "{form}");
        let raw = bob.post_raw(&form, b"{\"name\": 1}").await;
        assert_eq!(raw.status, StatusCode::NOT_FOUND, "{form} brut");
    }
    assert_eq!(
        bob.delete(&format!("/1life/clients/{id}")).await.status,
        StatusCode::NOT_FOUND
    );

    // Sans session : 404 sur les pages ; un formulaire répond comme sur une
    // adresse qui n'existe pas.
    let mut anonymous = app.client();
    for page in pages(id, event) {
        assert_eq!(
            anonymous.get(&page).await.status,
            StatusCode::NOT_FOUND,
            "{page}"
        );
    }
    let unknown_post = anonymous.post("/nulle-part", &[("name", "x")]).await;
    for form in forms(id, event) {
        let response = anonymous.post(&form, &[("name", "x")]).await;
        assert_eq!(response.status, unknown_post.status, "{form}");
        assert_eq!(response.body, unknown_post.body, "{form}");
    }

    // Rien n'a bougé.
    let settings: i64 = sqlx::query_scalar("SELECT count(*) FROM onelife_settings")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(settings, 1);
    assert_eq!(days_of(&app, id).await.len(), 1);

    // Autorisée, mais 1Life masqué dans École : retour à l'accueil d'École,
    // comme les autres outils ; rien n'est écrit par un formulaire.
    let back = alice.get(&format!("/e/ecole/1life/clients/{id}")).await;
    assert_eq!(back.status, StatusCode::SEE_OTHER);
    assert_eq!(back.location(), "/e/ecole/");
    let back = alice
        .post("/e/ecole/1life/clients", &[("name", "Pas ici")])
        .await;
    assert_eq!(back.status, StatusCode::SEE_OTHER);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM folders WHERE parent_id = ?")
        .bind(book)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
    // Un numéro illisible, pour elle : 404, pas 400.
    assert_eq!(
        alice.get(&format!("{LIST}/abc")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        alice.get(&format!("{LIST}/{id}/journees/abc")).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn un_client_en_corbeille_disparait_puis_revient_avec_ses_infos_et_ses_journees() {
    let app = TestApp::new().await;
    let (mut alice, _, _) = ready(&app, ALICE).await;
    let id = new_client(&mut alice, "Atelier Test").await;
    let monday = next_monday();
    add_day(&mut alice, id, monday, &[]).await;
    alice.with_header("hx-request", "true");
    alice
        .post(
            &format!("{LIST}/{id}/infos?auto=1"),
            &[("body", "À garder")],
        )
        .await;

    let deleted = alice
        .post(
            &format!("/folders/{id}/delete"),
            &[("contenu", "supprimer")],
        )
        .await;
    assert_eq!(deleted.status, StatusCode::SEE_OTHER);
    assert!(names(&alice.get(LIST).await.body).is_empty());
    assert_eq!(
        alice.get(&format!("{LIST}/{id}")).await.status,
        StatusCode::NOT_FOUND
    );
    let gantt = alice
        .get(&format!("{LIST}?vue=globale&du={}", iso(monday)))
        .await;
    assert!(!gantt.body.contains("Atelier Test"));
    // La journée reste dans l'Agenda (SPEC, décision 8).
    let agenda = alice
        .get(&format!("/e/taff/calendar?view=week&date={}", iso(monday)))
        .await;
    assert!(agenda.body.contains("Atelier Test — Paramétrage"));

    let restored = alice
        .post(&format!("/settings/trash/dossier/{id}/restore"), &[])
        .await;
    assert_eq!(restored.status, StatusCode::SEE_OTHER);
    assert_eq!(names(&alice.get(LIST).await.body), ["Atelier Test"]);
    let fiche = alice
        .get(&format!("{LIST}/{id}?semaine={}", iso(monday)))
        .await;
    assert_eq!(fiche.status, StatusCode::OK);
    assert!(fiche.body.contains("À garder"));
    assert!(fiche.body.contains("<strong>Paramétrage</strong>"));
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
async fn cr_et_audit_restent_bientot_et_clients_reste_hors_de_l_api() {
    let app = TestApp::new().await;
    let (mut alice, _, _) = ready(&app, ALICE).await;
    new_client(&mut alice, "Atelier Test").await;

    let home = alice.get("/e/taff/1life").await;
    assert_eq!(home.status, StatusCode::OK);
    let card = between(&home.body, "href=\"/e/taff/1life/clients\"", "</a>").unwrap();
    assert!(card.contains("Clients") && !card.contains("Bientôt"));
    assert_eq!(home.body.matches("Bientôt").count(), 2);
    for module in ["cr", "audit"] {
        let page = alice.get(&format!("/e/taff/1life/{module}")).await;
        assert!(page.body.contains("Module à venir"), "{module}");
    }

    // Ni dans l'API…
    let tk = token(&mut alice).await;
    for path in [
        "/api/v1/1life",
        "/api/v1/1life/clients",
        "/api/v1/clients",
        "/api/v1/onelife/clients",
        "/api/v1/1life/clients/dossier",
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
            .all(|t| !t.contains("client") && !t.contains("1life") && !t.contains("journee")),
        "{tools:?}"
    );
    // … ni dans la barre de commandes (seul `/go 1life` y mène) …
    alice.with_header("x-espace", "taff");
    let catalogue: Value = alice.catalogue("/").await;
    assert!(!catalogue.to_string().contains("/1life/clients"));
    // … ni dans les commandes d'Hermès.
    let hermes = std::fs::read_to_string("deploy/hermes/dashu").unwrap();
    assert!(!hermes.contains("1life") && !hermes.contains("onelife"));
}
