//! Agenda : lecture du flux iCal d'exemple (format Pronote), heure d'été,
//! affichage jour/semaine, comportement quand la synchro échoue.

mod common;

use axum::http::StatusCode;
use common::{ALICE, TestApp};
use dashu::calendar::ical::{self, ParsedEvent};
use dashu::calendar::sync;
use dashu::time;

/// Export iCal au format de Pronote : heures en UTC, lignes repliées, un cours
/// annulé, un jour férié, un événement avec fuseau et un sans.
const FIXTURE: &str = include_str!("fixtures/pronote.ics");

fn fixture() -> Vec<ParsedEvent> {
    ical::parse(FIXTURE).expect("le fichier d'exemple doit être lisible")
}

fn find<'a>(events: &'a [ParsedEvent], uid: &str) -> &'a ParsedEvent {
    events
        .iter()
        .find(|e| e.uid.starts_with(uid))
        .unwrap_or_else(|| panic!("événement {uid} absent"))
}

/// « AAAA-MM-JJ HH:MM » à Paris.
fn paris(ts: i64) -> String {
    time::paris(ts).format("%Y-%m-%d %H:%M").to_string()
}

#[test]
fn lecture_du_fichier_d_exemple() {
    let events = fixture();
    // 15 VEVENT dans le fichier, dont un cours annulé qui est ignoré.
    assert_eq!(events.len(), 14);
    assert!(
        events.iter().all(|e| !e.uid.starts_with("Cours-111")),
        "le cours annulé ne doit pas apparaître"
    );
    // Triés par heure de début.
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0].starts_at <= pair[1].starts_at)
    );

    let reseaux = find(&events, "Cours-101");
    assert_eq!(reseaux.title, "RESEAUX - LEFEVRE M. - SIO2");
    assert_eq!(reseaux.location, "B204");
    assert_eq!(paris(reseaux.starts_at), "2026-11-09 08:00");
    assert_eq!(paris(reseaux.ends_at), "2026-11-09 09:55");
    assert!(!reseaux.all_day);
}

#[test]
fn ligne_repliee_et_virgule_echappee() {
    let events = fixture();
    // Le SUMMARY tient sur deux lignes dans le fichier et contient « \, ».
    let cejm = find(&events, "Cours-106");
    assert_eq!(
        cejm.title,
        "CEJM - Culture économique, juridique et managériale - DURAND P. - SIO2"
    );
    assert_eq!(ical::subject(&cejm.title), "CEJM");
}

#[test]
fn heure_d_ete_et_heure_d_hiver() {
    let events = fixture();
    // Le même cours de 8 h : 06:00 UTC le 23 octobre (été), 07:00 UTC le 2 novembre (hiver).
    let summer = find(&events, "Cours-090");
    let winter = find(&events, "Cours-095");
    assert_eq!(paris(summer.starts_at), "2026-10-23 08:00");
    assert_eq!(paris(winter.starts_at), "2026-11-02 08:00");
    // Une heure d'écart en UTC pour la même heure affichée.
    assert_eq!((winter.starts_at - summer.starts_at) % 86_400, 3600);
    assert_eq!(time::hm(summer.ends_at), "09:55");
    assert_eq!(time::hm(winter.ends_at), "09:55");
}

#[test]
fn fuseau_explicite_heure_flottante_et_journee_entiere() {
    let events = fixture();

    // DTSTART;TZID=Europe/Paris:20261112T090000
    let with_tz = find(&events, "Cours-107");
    assert_eq!(paris(with_tz.starts_at), "2026-11-12 09:00");
    assert_eq!(paris(with_tz.ends_at), "2026-11-12 10:55");

    // DTSTART:20261113T140000 (sans Z ni fuseau) : heure de Paris.
    let floating = find(&events, "Rdv-112");
    assert_eq!(paris(floating.starts_at), "2026-11-13 14:00");

    // DTSTART;VALUE=DATE:20261111 : de minuit à minuit, heure de Paris.
    let holiday = find(&events, "Ferie-11nov");
    assert!(holiday.all_day);
    assert_eq!(paris(holiday.starts_at), "2026-11-11 00:00");
    assert_eq!(paris(holiday.ends_at), "2026-11-12 00:00");
}

#[test]
fn contenu_invalide_refuse() {
    assert!(ical::parse("").is_err());
    assert!(ical::parse("<!doctype html><title>Connexion</title>").is_err());
}

/// Crée une source iCal pour Alice dans l'espace donné et y range le fichier d'exemple.
async fn import_fixture(app: &TestApp, space: &str) -> i64 {
    let now = time::now();
    let source_id = sqlx::query(
        "INSERT INTO calendar_sources (user_id, space_id, name, ical_url, created_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(app.user_id(ALICE).await)
    .bind(app.space_id(ALICE, space).await)
    .bind("Pronote")
    // Port 1 : rien n'y écoute, une synchro réelle échouera tout de suite.
    .bind("http://127.0.0.1:1/pronote.ics")
    .bind(now)
    .execute(&app.state.db)
    .await
    .unwrap()
    .last_insert_rowid();
    sync::store(&app.state.db, source_id, &fixture(), now)
        .await
        .unwrap();
    source_id
}

#[tokio::test]
async fn vue_semaine() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    import_fixture(&app, "ecole").await;

    // L'agenda est rattaché à l'espace École : rien dans Taff.
    alice.post("/space", &[("slug", "taff")]).await;
    let taff = alice.get("/calendar?date=2026-11-10").await;
    assert_eq!(taff.status, StatusCode::OK);
    assert!(!taff.body.contains("RESEAUX"));
    assert!(taff.body.contains("Aucun agenda dans l"));

    // Perso est la vue d'ensemble : les cours d'École y sont, marqués « École ».
    alice.post("/space", &[("slug", "perso")]).await;
    let perso = alice.get("/calendar?date=2026-11-10").await;
    assert_eq!(perso.status, StatusCode::OK);
    assert!(perso.body.contains("<strong>RESEAUX</strong>"));
    assert!(
        perso
            .body
            .contains("Vue d&#39;ensemble de tous les espaces")
    );
    assert!(
        perso
            .body
            .contains(r#"<b class="ev-space" data-accent="blue">École</b> · 08:00 – 09:55 · B204"#),
        "un cours d'École porte le nom de son espace dans la vue d'ensemble"
    );

    alice.post("/space", &[("slug", "ecole")]).await;
    let week = alice.get("/calendar?date=2026-11-10").await;
    // Dans son propre espace, un cours n'a pas d'étiquette.
    assert!(!week.body.contains("ev-space"));
    assert_eq!(week.status, StatusCode::OK);
    assert!(week.body.contains("Semaine du 9 novembre 2026"));
    // Lundi 8 h : première rangée de la grille (gs0), fin à 9 h 55 (rangée 23).
    assert!(
        week.body.contains("gs0 ge23"),
        "le cours de 8 h doit commencer en haut de la grille"
    );
    assert!(week.body.contains("<strong>RESEAUX</strong>"));
    assert!(week.body.contains("08:00 – 09:55 · B204"));
    assert!(week.body.contains("Armistice 1918"));
    assert!(week.body.contains("Point alternance"));
    assert!(week.body.contains("14:00 – 15:00 · Visio"));
    // Jeudi, deux cours se chevauchent : ils se partagent la colonne.
    assert!(week.body.contains("k2 p0") && week.body.contains("k2 p1"));
    // Semaine de 5 jours : pas d'événement le week-end.
    assert!(week.body.contains("cal-d5"));
    // Aucun style en ligne : la CSP les interdit.
    assert!(!week.body.contains("style=\""));
}

#[tokio::test]
async fn vue_semaine_en_heure_d_ete() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    import_fixture(&app, "perso").await;

    // Vendredi 23 octobre 2026, encore en heure d'été : le cours de 06:00 UTC
    // s'affiche à 8 h, en haut de la grille, comme en hiver.
    let week = alice.get("/calendar?date=2026-10-23").await;
    assert!(week.body.contains("Semaine du 19 octobre 2026"));
    assert!(week.body.contains("08:00 – 09:55 · B204"));
    assert!(week.body.contains("gs0 ge23"));
}

#[tokio::test]
async fn vue_jour_et_navigation() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    import_fixture(&app, "perso").await;

    let day = alice.get("/calendar?view=day&date=2026-11-10").await;
    assert!(day.body.contains("Mardi 10 novembre 2026"));
    assert!(day.body.contains("cal-d1"));
    assert!(day.body.contains("ATELIER PRO"));
    assert!(
        !day.body.contains("CYBERSECURITE"),
        "le cours du lundi n'est pas sur la vue du mardi"
    );
    assert!(day.body.contains("date=2026-11-09"), "lien jour précédent");
    assert!(day.body.contains("date=2026-11-11"));

    let week = alice.get("/calendar?view=week&date=2026-11-10").await;
    assert!(
        week.body.contains("date=2026-11-03"),
        "lien semaine précédente"
    );
    assert!(
        week.body.contains("date=2026-11-17"),
        "lien semaine suivante"
    );

    // Une date illisible retombe sur aujourd'hui au lieu d'une erreur.
    assert_eq!(
        alice.get("/calendar?date=n-importe-quoi").await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn synchro_en_echec_le_cache_reste_affiche() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let source_id = import_fixture(&app, "perso").await;

    // Le serveur du flux est injoignable.
    let result = sync::sync_source(&app.state, source_id, "http://127.0.0.1:1/pronote.ics").await;
    assert!(result.is_err());

    // Les 14 événements de la dernière synchro réussie sont toujours là…
    let cached: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM calendar_events WHERE source_id = ?")
            .bind(source_id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(cached, 14);

    // … et la page les affiche, en signalant l'échec et la date du cache.
    let week = alice.get("/calendar?date=2026-11-10").await;
    assert!(week.body.contains("<strong>RESEAUX</strong>"));
    assert!(week.body.contains("la synchro a échoué"));
    assert!(week.body.contains("dernière synchro réussie"));
    // Le message d'erreur ne contient pas l'URL du flux (elle porte un jeton).
    assert!(!week.body.contains("pronote.ics"));

    // Le bouton « Rafraîchir » retente, échoue de même, et le dit.
    let refreshed = alice
        .post(
            "/calendar/refresh",
            &[("next", "/calendar?date=2026-11-10")],
        )
        .await;
    let page = alice.follow(refreshed).await;
    assert!(page.body.contains("La synchro a échoué"));
    assert!(page.body.contains("<strong>RESEAUX</strong>"));
}

#[tokio::test]
async fn ajout_d_une_source_url_validee() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let space = app.space_id(ALICE, "ecole").await.to_string();

    // Adresses locales ou non chiffrées : refusées avant tout téléchargement.
    for url in [
        "http://campus.example.test/a.ics",
        "https://127.0.0.1/a.ics",
        "https://localhost/a.ics",
        "file:///etc/passwd",
    ] {
        let response = alice
            .post(
                "/settings/calendars",
                &[("name", "Test"), ("url", url), ("space_id", &space)],
            )
            .await;
        assert_eq!(response.status, StatusCode::SEE_OTHER, "{url}");
    }
    let sources: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM calendar_sources")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(sources, 0, "aucune source locale ne doit être enregistrée");
}

#[tokio::test]
async fn widget_prochains_cours() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let source_id = import_fixture(&app, "perso").await;

    // Quatre cours à venir (le fichier d'exemple, lui, est daté) : le widget en montre trois.
    let now = time::now();
    let upcoming: Vec<ParsedEvent> = (0..4)
        .map(|i| ParsedEvent {
            uid: format!("futur-{i}"),
            title: format!("COURS{i} - PROF"),
            location: format!("Salle {i}"),
            starts_at: now + 3600 * (i + 1),
            ends_at: now + 3600 * (i + 1) + 1800,
            all_day: false,
        })
        .collect();
    sync::store(&app.state.db, source_id, &upcoming, now)
        .await
        .unwrap();

    let widget = alice.get_fragment("/widgets/courses").await;
    assert_eq!(widget.status, StatusCode::OK);
    for i in 0..3 {
        assert!(widget.body.contains(&format!("COURS{i}")), "COURS{i}");
        assert!(widget.body.contains(&format!("Salle {i}")));
    }
    assert!(!widget.body.contains("COURS3"), "seulement les 3 prochains");
}

#[tokio::test]
async fn perso_reunit_les_agendas_mais_pas_l_inverse() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let ecole = import_fixture(&app, "ecole").await;
    let perso = import_fixture(&app, "perso").await;

    // Les deux sources contiennent les mêmes cours : Perso les montre deux
    // fois, l'un étiqueté « École », l'autre sans étiquette (il est chez lui).
    let overview = alice.get("/calendar?date=2026-11-10").await;
    assert_eq!(overview.body.matches("<strong>RESEAUX</strong>").count(), 2);
    assert_eq!(
        overview.body.matches(">École</b>").count(),
        overview.body.matches("<strong>").count() / 2
    );

    // École ne voit que sa propre source.
    alice.post("/space", &[("slug", "ecole")]).await;
    let only = alice.get("/calendar?date=2026-11-10").await;
    assert_eq!(only.body.matches("<strong>RESEAUX</strong>").count(), 1);

    // « Rafraîchir » depuis École ne tente que sa source, depuis Perso toutes.
    // Ici les tentatives échouent (rien n'écoute sur le port 1) : l'erreur
    // enregistrée montre quelles sources ont été tentées.
    let failed = |id: i64| {
        let db = app.state.db.clone();
        async move {
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT last_error FROM calendar_sources WHERE id = ?",
            )
            .bind(id)
            .fetch_one(&db)
            .await
            .unwrap()
            .is_some()
        }
    };
    alice
        .post("/calendar/refresh", &[("next", "/calendar")])
        .await;
    assert!(failed(ecole).await);
    assert!(!failed(perso).await, "École ne resynchronise pas Perso");
    alice.post("/space", &[("slug", "perso")]).await;
    alice
        .post("/calendar/refresh", &[("next", "/calendar")])
        .await;
    assert!(failed(perso).await);
}
