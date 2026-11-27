//! Parcours et espaces (SPEC, « Parcours et espaces ») : changer d'espace
//! change vraiment l'onglet d'espace, un espace archivé garde ses fragments
//! et ses écritures, un onglet sur un espace disparu n'écrit nulle part.

mod common;

use axum::http::StatusCode;
use common::{ALICE, BOB, TestApp, hx_location};

/// Tâches d'un espace, par titre.
async fn tasks_of(app: &TestApp, space: i64) -> Vec<String> {
    sqlx::query_scalar("SELECT title FROM tasks WHERE space_id = ? ORDER BY id")
        .bind(space)
        .fetch_all(&app.state.db)
        .await
        .expect("tâches")
}

#[tokio::test]
async fn changer_d_espace_emmene_l_onglet() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;

    // Depuis l'île (requête htmx d'un onglet sur École), vers une page sans
    // préfixe : la réponse porte le nouvel espace, sinon l'en-tête de
    // l'onglet ramènerait École.
    let switched = alice
        .with_header("hx-request", "true")
        .with_header("x-espace", "ecole")
        .post("/space", &[("slug", "taff"), ("next", "/tools")])
        .await;
    let go = hx_location(&switched);
    assert_eq!(go["path"], "/tools");
    assert_eq!(go["headers"]["X-Espace"], "taff");
    // La page ouverte ensuite est bien sur Taff.
    let page = alice
        .with_header("x-espace", "taff")
        .get_boosted("/tools")
        .await;
    assert!(page.body.contains("\"X-Espace\":\"taff\""));

    // Vers une page d'un espace : l'adresse du nouvel espace.
    let switched = alice
        .with_header("hx-request", "true")
        .with_header("x-espace", "taff")
        .post("/space", &[("slug", "ecole"), ("next", "/notes?q=vlan")])
        .await;
    let go = hx_location(&switched);
    assert_eq!(go["path"], "/e/ecole/notes?q=vlan");
    assert_eq!(go["headers"]["X-Espace"], "ecole");
}

#[tokio::test]
async fn changer_d_espace_dans_un_dossier_mene_au_nouvel_espace() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.post("/space", &[("slug", "ecole")]).await;
    let created = alice
        .with_header("x-espace", "ecole")
        .post("/folders", &[("name", "Math")])
        .await;
    assert_eq!(created.status, StatusCode::SEE_OTHER, "{}", created.body);
    let folder: i64 = sqlx::query_scalar("SELECT id FROM folders WHERE name = 'Math'")
        .fetch_one(&app.state.db)
        .await
        .expect("dossier");

    // Le dossier est dans École : en passant sur Taff, on arrive à la racine
    // de Taff, pas de retour dans École.
    let switched = alice
        .post(
            "/space",
            &[
                ("slug", "taff"),
                ("next", &format!("/files?dossier={folder}")),
            ],
        )
        .await;
    assert_eq!(switched.location(), "/e/taff/files");
    let page = alice.follow(switched).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("Fichiers · Taff"));

    // « racine », « temporaire » et la recherche restent.
    let switched = alice
        .post(
            "/space",
            &[
                ("slug", "ecole"),
                ("next", &format!("/files?dossier={folder}&q=td")),
            ],
        )
        .await;
    assert_eq!(switched.location(), "/e/ecole/files?q=td");
    let switched = alice
        .post(
            "/space",
            &[("slug", "taff"), ("next", "/files?dossier=racine")],
        )
        .await;
    assert_eq!(switched.location(), "/e/taff/files?dossier=racine");
}

#[tokio::test]
async fn changer_d_espace_vers_un_outil_masque_mene_a_l_accueil_sans_erreur() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let taff = app.space_id(ALICE, "taff").await;
    alice
        .post(&format!("/settings/spaces/{taff}/tools/notes"), &[])
        .await;

    let switched = alice
        .post("/space", &[("slug", "taff"), ("next", "/notes")])
        .await;
    assert_eq!(switched.location(), "/e/taff/");
    let page = alice.follow(switched).await;
    assert!(page.body.contains("Dashboard · Taff"));
    assert!(!page.body.contains(dashu::registry::HIDDEN));

    // Notes est affiché dans École : la même page.
    let switched = alice
        .post("/space", &[("slug", "ecole"), ("next", "/notes")])
        .await;
    assert_eq!(switched.location(), "/e/ecole/notes");
}

#[tokio::test]
async fn un_fichier_de_temporaire_ramene_a_l_onglet_outils() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice
        .upload_to_tools("photo.png", b"pas vraiment une image")
        .await;
    let id: i64 = sqlx::query_scalar("SELECT id FROM files WHERE expires_at IS NOT NULL")
        .fetch_one(&app.state.db)
        .await
        .expect("fichier de Temporaire");

    let switched = alice
        .post(
            "/space",
            &[("slug", "ecole"), ("next", &format!("/files/{id}"))],
        )
        .await;
    assert_eq!(switched.location(), "/tools");
}

#[tokio::test]
async fn creer_un_espace_y_emmene_l_onglet() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let created = alice
        .with_header("hx-request", "true")
        .with_header("x-espace", "ecole")
        .post(
            "/settings/spaces",
            &[("name", "Sport"), ("color", "teal"), ("icon", "dumbbell")],
        )
        .await;
    let go = hx_location(&created);
    assert_eq!(go["path"], "/settings#espaces");
    assert_eq!(go["headers"]["X-Espace"], "sport");

    // Refusé (nom déjà pris) : retour simple aux réglages, l'onglet reste.
    let refused = alice
        .with_header("hx-request", "true")
        .with_header("x-espace", "ecole")
        .post(
            "/settings/spaces",
            &[("name", "Sport"), ("color", "teal"), ("icon", "dumbbell")],
        )
        .await;
    assert_eq!(refused.status, StatusCode::SEE_OTHER);
    assert_eq!(refused.location(), "/settings#espaces");
}

#[tokio::test]
async fn un_espace_archive_garde_ses_fragments_et_ses_ecritures() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let taff = app.space_id(ALICE, "taff").await;
    let perso = app.space_id(ALICE, "perso").await;
    alice
        .post(&format!("/settings/spaces/{taff}/archive"), &[])
        .await;

    // L'espace archivé s'ouvre par son adresse ; son onglet ajoute une tâche
    // sans dire d'espace : elle va dans Taff, pas dans Perso.
    assert_eq!(alice.get("/e/taff/").await.status, StatusCode::OK);
    let added = alice
        .with_header("x-espace", "taff")
        .post("/tasks", &[("title", "Facture client")])
        .await;
    assert!(added.status.is_success(), "{}", added.body);
    assert_eq!(tasks_of(&app, taff).await, ["Facture client"]);
    assert!(tasks_of(&app, perso).await.is_empty());

    // Son widget la montre, même si la session est passée sur un autre espace.
    alice.post("/space", &[("slug", "ecole")]).await;
    let widget = alice
        .with_header("x-espace", "taff")
        .get_fragment("/widgets/tasks")
        .await;
    assert!(widget.body.contains("Facture client"));
}

#[tokio::test]
async fn un_onglet_sur_un_espace_disparu_n_ecrit_nulle_part() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let perso = app.space_id(ALICE, "perso").await;

    let refused = alice
        .with_header("x-espace", "disparu")
        .post("/tasks", &[("title", "Perdue")])
        .await;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert_eq!(refused.header("hx-redirect"), "/");
    assert!(tasks_of(&app, perso).await.is_empty());
    // Le message attend sur l'accueil.
    let home = alice.get("/").await;
    assert!(home.body.contains("Cet espace n&#39;est plus accessible"));

    // Une page, elle, se rabat sur le dernier espace utilisé, et ses
    // redirections ne prennent pas le préfixe de l'espace disparu.
    let page = alice
        .with_header("x-espace", "disparu")
        .get_fragment("/widgets/tasks")
        .await;
    assert_eq!(page.status, StatusCode::OK);
}

#[tokio::test]
async fn un_membre_ne_voit_pas_le_bouton_restaurer() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let mut bob = app.logged_in(BOB).await;
    let slug = app.share_space(ALICE, "taff", BOB).await;
    let taff = app.space_id(ALICE, "taff").await;
    alice
        .post(&format!("/settings/spaces/{taff}/archive"), &[])
        .await;

    let owner = alice.get("/e/taff/notes").await;
    assert!(
        owner
            .body
            .contains(&format!("/settings/spaces/{taff}/restore"))
    );
    let member = bob.get(&format!("/e/{slug}/notes")).await;
    assert_eq!(member.status, StatusCode::OK);
    assert!(!member.body.contains("/restore"));
    assert!(member.body.contains("archivé par son propriétaire"));

    // Le propriétaire restaure depuis le bandeau : il reste sur la page.
    let restored = alice
        .with_header("x-espace", "taff")
        .post(
            &format!("/settings/spaces/{taff}/restore"),
            &[("next", "/notes")],
        )
        .await;
    assert_eq!(restored.location(), "/e/taff/notes");
}

#[tokio::test]
async fn un_espace_archive_se_supprime_meme_avec_perso_seul_en_service() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    let taff = app.space_id(ALICE, "taff").await;
    for id in [ecole, taff] {
        alice
            .post(&format!("/settings/spaces/{id}/archive"), &[])
            .await;
    }
    let deleted = alice
        .post(
            &format!("/settings/spaces/{ecole}/delete"),
            &[("content", "delete")],
        )
        .await;
    let settings = alice.follow(deleted).await;
    assert!(!settings.body.contains("Il faut garder au moins un espace"));
    assert_eq!(alice.get("/e/ecole/").await.status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------
// Lot 2 : on revient d'où l'on vient
// ---------------------------------------------------------------------

/// Crée le dossier `name` dans l'espace de l'onglet ; rend son identifiant.
async fn folder(app: &TestApp, client: &mut common::Client, slug: &str, name: &str) -> i64 {
    let created = client
        .with_header("x-espace", slug)
        .post("/folders", &[("name", name)])
        .await;
    assert_eq!(created.status, StatusCode::SEE_OTHER, "{}", created.body);
    sqlx::query_scalar("SELECT id FROM folders WHERE name = ? ORDER BY id DESC")
        .bind(name)
        .fetch_one(&app.state.db)
        .await
        .expect("dossier")
}

#[tokio::test]
async fn une_note_ramene_a_la_page_d_ou_on_l_a_ouverte() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.post("/space", &[("slug", "ecole")]).await;
    let math = folder(&app, &mut alice, "ecole", "Math").await;
    let place = format!("/e/ecole/files?dossier={math}");

    // « Nouvelle note » dans le dossier : l'éditeur sait d'où il vient.
    let created = alice
        .with_header("x-espace", "ecole")
        .post(&format!("/notes?dossier={math}"), &[])
        .await;
    let id = common::note_id(&created);
    assert!(
        created.location().contains("depuis="),
        "{}",
        created.location()
    );
    let editor = alice.follow(created).await;
    assert!(editor.body.contains(&format!("href=\"{place}\"")));
    assert!(editor.body.contains("Retour : École / Math"));

    // Ouverte depuis le dashboard : le retour y mène.
    let editor = alice
        .get(&format!("/notes/{id}?depuis=%2Fe%2Fecole%2F"))
        .await;
    assert!(editor.body.contains("Retour : Dashboard"));
    // Une adresse d'un autre site est ignorée : la liste des notes.
    let editor = alice
        .get(&format!("/notes/{id}?depuis=https%3A%2F%2Fexemple.com"))
        .await;
    assert!(editor.body.contains("href=\"/notes\""));

    // Supprimer ramène à la page d'origine.
    let deleted = alice
        .post(&format!("/notes/{id}/delete"), &[("next", &place)])
        .await;
    assert_eq!(deleted.location(), place);
}

#[tokio::test]
async fn perso_ouvre_les_autres_espaces_sur_place() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.post("/space", &[("slug", "ecole")]).await;
    let created = alice.post("/notes", &[]).await;
    let id = common::note_id(&created);
    alice
        .post(
            &format!("/notes/{id}"),
            &[("title", "Cours de réseau"), ("body_md", "VLAN")],
        )
        .await;
    alice.upload("td.pdf", b"%PDF-1.4 td").await;
    let file: i64 = sqlx::query_scalar("SELECT id FROM files WHERE name = 'td.pdf'")
        .fetch_one(&app.state.db)
        .await
        .expect("fichier");

    // Depuis Perso : sur place, avec la pastille de l'espace.
    for path in [
        format!("/e/perso/notes/{id}"),
        format!("/e/perso/files/{file}"),
    ] {
        let page = alice.get(&path).await;
        assert_eq!(page.status, StatusCode::OK, "{path}");
        assert!(page.body.contains("\"X-Espace\":\"perso\""), "{path}");
        assert!(page.body.contains("class=\"space-of\""), "{path}");
    }
    // L'éditeur envoie les images collées dans l'espace de la note.
    let ecole = app.space_id(ALICE, "ecole").await;
    let editor = alice.get(&format!("/e/perso/notes/{id}")).await;
    assert!(
        editor
            .body
            .contains(&format!("data-note-space=\"{ecole}\""))
    );

    // Depuis un autre espace : la note s'ouvre dans le sien.
    let moved = alice.get(&format!("/e/taff/notes/{id}")).await;
    assert_eq!(moved.location(), format!("/e/ecole/notes/{id}"));
    let moved = alice.get(&format!("/e/taff/files/{file}")).await;
    assert_eq!(moved.location(), format!("/e/ecole/files/{file}"));
}

#[tokio::test]
async fn une_action_garde_la_recherche_des_fichiers() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.post("/space", &[("slug", "ecole")]).await;
    alice
        .upload("rapport de stage.pdf", b"%PDF-1.4 rapport")
        .await;
    alice.upload("photo.png", b"pas une image").await;

    let page = alice.get("/e/ecole/files?q=rapport").await;
    assert!(page.body.contains("rapport de stage.pdf"));
    // Le retour des actions de la liste garde la recherche.
    assert!(
        page.body
            .contains("name=\"next\" value=\"/e/ecole/files?dossier=espace&#38;q=rapport\""),
        "{}",
        common::between(&page.body, "name=\"next\"", ">").unwrap_or_default()
    );
}

#[tokio::test]
async fn un_evenement_cree_depuis_perso_y_ramene() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await.to_string();
    let form = [
        ("title", "Partiel"),
        ("date", "2026-11-18"),
        ("start", "14:00"),
        ("end", "15:30"),
        ("location", ""),
        ("space_id", ecole.as_str()),
    ];
    // Depuis Perso : l'agenda de Perso, qui réunit tous les espaces.
    let created = alice
        .with_header("x-espace", "perso")
        .post("/calendar/events", &form)
        .await;
    assert_eq!(
        created.location(),
        "/e/perso/calendar?view=week&date=2026-11-18"
    );
    // Depuis Taff : l'agenda de l'espace de l'événement.
    let created = alice
        .with_header("x-espace", "taff")
        .post("/calendar/events", &form)
        .await;
    assert_eq!(
        created.location(),
        "/e/ecole/calendar?view=week&date=2026-11-18"
    );
    // La page de l'événement suit son espace, sauf depuis Perso.
    let id: i64 = sqlx::query_scalar("SELECT id FROM events ORDER BY id LIMIT 1")
        .fetch_one(&app.state.db)
        .await
        .expect("événement");
    let page = alice.get(&format!("/e/taff/calendar/events/{id}")).await;
    assert_eq!(page.location(), format!("/e/ecole/calendar/events/{id}"));
    let page = alice.get(&format!("/e/perso/calendar/events/{id}")).await;
    assert_eq!(page.status, StatusCode::OK);
}

#[tokio::test]
async fn la_connexion_ramene_a_la_page_demandee() {
    let app = TestApp::new().await;
    app.logged_in(ALICE).await;
    let mut visitor = app.client();
    let asked = visitor.get("/e/ecole/notes?q=vlan").await;
    assert_eq!(asked.location(), "/login");
    let signed = visitor.login(ALICE, common::PASSWORD).await;
    assert_eq!(signed.location(), "/e/ecole/notes?q=vlan");

    // Une seule fois : la connexion suivante ouvre l'accueil.
    let mut again = app.client();
    let signed = again.login(ALICE, common::PASSWORD).await;
    assert_eq!(signed.location(), "/");
}

#[tokio::test]
async fn refuser_une_invitation_ramene_aux_espaces() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let mut bob = app.logged_in(BOB).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    alice
        .post(
            &format!("/settings/spaces/{ecole}/invitations"),
            &[("email", BOB)],
        )
        .await;
    let invitation: i64 = sqlx::query_scalar("SELECT id FROM space_invitations")
        .fetch_one(&app.state.db)
        .await
        .expect("invitation");
    let declined = bob
        .post(&format!("/settings/invitations/{invitation}/decline"), &[])
        .await;
    assert_eq!(declined.location(), "/settings#espaces");
}
