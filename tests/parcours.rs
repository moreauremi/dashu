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
