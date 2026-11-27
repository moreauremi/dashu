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

// ---------------------------------------------------------------------
// Lot 3 : la corbeille dans Fichiers
// ---------------------------------------------------------------------

/// Crée une note titrée dans l'espace `slug` ; rend son identifiant.
async fn titled_note(
    client: &mut common::Client,
    slug: &str,
    title: &str,
    folder: Option<i64>,
) -> i64 {
    let path = match folder {
        Some(folder) => format!("/notes?dossier={folder}"),
        None => "/notes".to_string(),
    };
    let created = client.with_header("x-espace", slug).post(&path, &[]).await;
    let id = common::note_id(&created);
    client
        .post(
            &format!("/notes/{id}"),
            &[("title", title), ("body_md", "texte")],
        )
        .await;
    id
}

#[tokio::test]
async fn la_corbeille_est_un_dossier_de_fichiers() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let maths = titled_note(&mut alice, "ecole", "Fiche maths", None).await;
    let devis = titled_note(&mut alice, "taff", "Devis client", None).await;
    for id in [maths, devis] {
        let deleted = alice.post(&format!("/notes/{id}/delete"), &[]).await;
        let page = alice.follow(deleted).await;
        // Le message mène à la corbeille.
        assert!(page.body.contains("Note mise dans la corbeille"));
        assert!(page.body.contains("href=\"/files?dossier=corbeille\""));
    }

    // À la racine de Fichiers, à côté de Temporaire.
    let root = alice.get("/e/perso/files?dossier=racine").await;
    assert!(root.body.contains("Corbeille"));
    assert!(root.body.contains("2 éléments"));
    // Dans un espace : ce qui en vient ; depuis Perso : tout.
    let ecole = alice.get("/e/ecole/files?dossier=corbeille").await;
    assert!(ecole.body.contains("Fiche maths") && !ecole.body.contains("Devis client"));
    let perso = alice.get("/e/perso/files?dossier=corbeille").await;
    assert!(perso.body.contains("Fiche maths") && perso.body.contains("Devis client"));
    // Filtre par sorte.
    let files_only = alice
        .get("/e/perso/files?dossier=corbeille&type=fichier")
        .await;
    assert!(!files_only.body.contains("Fiche maths"));

    // L'ancienne adresse y mène.
    let old = alice.get("/settings/trash").await;
    assert_eq!(old.location(), "/e/perso/files?dossier=corbeille");
}

#[tokio::test]
async fn restaurer_dit_ou_l_element_revient() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let math = folder(&app, &mut alice, "ecole", "Math").await;
    let id = titled_note(&mut alice, "ecole", "Exercice 3", Some(math)).await;
    alice.post(&format!("/notes/{id}/delete"), &[]).await;

    // Son dossier existe encore : il y revient, et le message le dit.
    let restored = alice
        .post(
            &format!("/settings/trash/note/{id}/restore"),
            &[("next", "/files?dossier=corbeille")],
        )
        .await;
    assert_eq!(restored.location(), "/files?dossier=corbeille");
    let page = alice.follow(restored).await;
    assert!(
        page.body.contains("est revenu dans École / Math"),
        "{}",
        page.body
    );
    assert!(page.body.contains(&format!("href=\"/notes/{id}\"")));

    // Son dossier est parti entre-temps : la racine, dite comme telle.
    alice.post(&format!("/notes/{id}/delete"), &[]).await;
    alice.post(&format!("/folders/{math}/delete"), &[]).await;
    let restored = alice
        .post(&format!("/settings/trash/note/{id}/restore"), &[])
        .await;
    let page = alice.follow(restored).await;
    assert!(
        page.body
            .contains("est revenu à la racine de École : son dossier n&#39;existe plus"),
        "{}",
        page.body
    );
}

#[tokio::test]
async fn vider_ne_touche_pas_aux_suppressions_des_autres() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let mut bob = app.logged_in(BOB).await;
    let slug = app.share_space(ALICE, "ecole", BOB).await;
    let mine = titled_note(&mut alice, "ecole", "Brouillon d'Alice", None).await;
    let his = titled_note(&mut bob, &slug, "Notes de Bob", None).await;
    alice.post(&format!("/notes/{mine}/delete"), &[]).await;
    bob.post(&format!("/notes/{his}/delete"), &[]).await;

    // La confirmation dit ce qui reste.
    let page = alice.get("/e/perso/files?dossier=corbeille").await;
    assert!(
        page.body
            .contains("1 élément supprimé par un autre membre reste")
    );
    let emptied = alice
        .post(
            "/settings/trash/empty",
            &[("next", "/files?dossier=corbeille")],
        )
        .await;
    let page = alice.follow(emptied).await;
    assert!(
        page.body
            .contains("Corbeille vidée : 1 élément supprimé définitivement")
    );
    // Ce que Bob a supprimé reste, et il peut encore le restaurer.
    assert!(page.body.contains("Notes de Bob"));
    let restored = bob
        .post(&format!("/settings/trash/note/{his}/restore"), &[])
        .await;
    assert_eq!(restored.status, StatusCode::SEE_OTHER);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM notes WHERE id = ?")
        .bind(mine)
        .fetch_one(&app.state.db)
        .await
        .expect("compte");
    assert_eq!(count, 0, "la note d'Alice est effacée");
}

#[tokio::test]
async fn un_titre_n_est_echappe_qu_une_fois_dans_la_corbeille() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let id = titled_note(&mut alice, "ecole", "R&D", None).await;
    alice.post(&format!("/notes/{id}/delete"), &[]).await;
    let page = alice.get("/e/perso/files?dossier=corbeille").await;
    assert!(
        page.body.contains("<strong>R&#38;D</strong>"),
        "{}",
        page.body
    );
    assert!(!page.body.contains("&#38;amp;"));
    let restored = alice
        .post(&format!("/settings/trash/note/{id}/restore"), &[])
        .await;
    let page = alice.follow(restored).await;
    assert!(page.body.contains("« R&#38;D » est revenu"));
}

// ---------------------------------------------------------------------
// Lot 4 : Perso, Notes et Fichiers parlent la même langue
// ---------------------------------------------------------------------

#[tokio::test]
async fn les_widgets_de_perso_parlent_de_tous_les_espaces() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let perso = alice.get("/e/perso/widgets/notes").await;
    assert!(perso.body.contains("Aucune note, dans aucun espace."));
    let ecole = alice.get("/e/ecole/widgets/notes").await;
    assert!(ecole.body.contains("Aucune note dans cet espace."));

    // Les épinglées portent leur espace et tiennent dans le widget.
    let mut ids = Vec::new();
    for title in ["Un", "Deux", "Trois"] {
        let id = titled_note(&mut alice, "ecole", title, None).await;
        alice.post(&format!("/notes/{id}/pin"), &[]).await;
        ids.push(id);
    }
    let widget = alice.get("/e/perso/widgets/notes?w=2&h=1").await;
    assert_eq!(widget.body.matches("<li>").count(), 2, "{}", widget.body);
    assert!(widget.body.contains("class=\"space-of\""));
}

#[tokio::test]
async fn un_widget_ne_mene_pas_a_un_outil_masque() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let taff = app.space_id(ALICE, "taff").await;
    alice
        .post(&format!("/settings/spaces/{taff}/tools/notes"), &[])
        .await;
    let dashboard = alice.get("/e/taff/").await;
    assert!(!dashboard.body.contains("Toutes les notes"));
    assert!(
        alice
            .get("/e/ecole/")
            .await
            .body
            .contains("Toutes les notes")
    );
    let widget = alice.get("/e/taff/widgets/notes").await;
    assert!(!widget.body.contains("Écrire la première"));

    // L'interrupteur remplace aussi la barre du téléphone (hors bande).
    let toggled = alice
        .with_header("hx-request", "true")
        .with_header("x-espace", "taff")
        .post(
            &format!("/settings/spaces/{taff}/tools/notes"),
            &[("shown", "1")],
        )
        .await;
    assert!(
        toggled.body.contains(
            "id=\"tabbar\" class=\"tabbar\" aria-label=\"Raccourcis\" hx-swap-oob=\"true\""
        )
    );
}

#[tokio::test]
async fn les_sections_de_dossier_des_notes_s_ouvrent() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let math = folder(&app, &mut alice, "ecole", "Math").await;
    let id = titled_note(&mut alice, "ecole", "Fiche VLAN", Some(math)).await;
    titled_note(&mut alice, "ecole", "Hors dossier", None).await;
    alice
        .post(&format!("/notes/{id}/tags"), &[("name", "bts")])
        .await;

    let list = alice.get("/e/ecole/notes").await;
    assert!(
        list.body
            .contains(&format!("href=\"/notes?dossier={math}\""))
    );
    let opened = alice.get(&format!("/e/ecole/notes?dossier={math}")).await;
    assert!(opened.body.contains("École / Math"));
    assert!(opened.body.contains("Voir dans Fichiers"));
    assert!(opened.body.contains("Fiche VLAN") && !opened.body.contains("Hors dossier"));
    // Hors de Perso, un dossier d'un autre espace s'ouvre dans le sien.
    let moved = alice.get(&format!("/e/taff/notes?dossier={math}")).await;
    assert_eq!(moved.location(), format!("/e/ecole/notes?dossier={math}"));

    // « #BTS » trouve le tag « bts », comme dans Fichiers.
    let tagged = alice.get("/e/ecole/notes?tag=%23BTS").await;
    assert!(tagged.body.contains("Fiche VLAN"));

    // Le menu d'une ligne épingle sans ouvrir la note, et revient à la liste.
    assert!(list.body.contains(&format!("action=\"/notes/{id}/pin\"")));
    let pinned = alice
        .post(&format!("/notes/{id}/pin"), &[("next", "/notes")])
        .await;
    assert_eq!(pinned.location(), "/notes");
    let page = alice.follow(pinned).await;
    assert!(page.body.contains("Note épinglée"));
}

#[tokio::test]
async fn la_recherche_de_fichiers_trouve_aussi_les_notes() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let math = folder(&app, &mut alice, "ecole", "Math").await;
    titled_note(&mut alice, "ecole", "Fiche VLAN", Some(math)).await;
    titled_note(&mut alice, "taff", "VLAN du client", None).await;
    let found = alice
        .get(&format!("/e/ecole/files?dossier={math}&q=vlan"))
        .await;
    assert!(found.body.contains("Fiche VLAN"));
    assert!(!found.body.contains("VLAN du client"));
    assert!(
        found.body.contains("Trouvé dans École / Math : 1 note"),
        "{}",
        found.body
    );
}

#[tokio::test]
async fn l_indicateur_mene_a_l_espace_de_la_conversion() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let user = app.user_id(ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    sqlx::query(
        "INSERT INTO jobs (user_id, space_id, kind, source_name, source_size, status, created_at, updated_at)
         VALUES (?, ?, 'convert', 'film.mov', 10, 'running', 0, 0)",
    )
    .bind(user)
    .bind(ecole)
    .execute(&app.state.db)
    .await
    .expect("conversion");
    // Depuis Taff, l'indicateur mène aux fichiers d'École, où est son suivi.
    let page = alice.get("/e/taff/notes").await;
    let indicator =
        common::between(&page.body, "id=\"jobs-indicator\"", "</a>").unwrap_or_default();
    assert!(indicator.contains("href=\"/e/ecole/files\""), "{indicator}");
}
