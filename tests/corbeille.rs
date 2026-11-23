//! Corbeille (SPEC, « API complète et corbeille ») : ce qui est supprimé
//! disparaît de partout, se restaure à sa place, s'efface pour de bon depuis
//! la corbeille seulement, et tout seul au bout de 30 jours.

mod common;

use axum::http::{Method, StatusCode};
use common::{ALICE, API_TOKEN, BOB, Client, TestApp};
use serde_json::json;

async fn id_of(app: &TestApp, sql: &str, name: &str) -> i64 {
    sqlx::query_scalar(sql)
        .bind(name)
        .fetch_one(&app.state.db)
        .await
        .unwrap_or_else(|_| panic!("« {name} » absent"))
}

async fn scalar(app: &TestApp, sql: &str) -> i64 {
    sqlx::query_scalar(sql)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

/// Note titrée, avec un mot à chercher, dans l'espace actif.
async fn note(client: &mut Client, title: &str, body: &str) -> i64 {
    let created = client.post("/notes", &[]).await;
    let id = common::note_id(&created);
    client
        .post(
            &format!("/notes/{id}"),
            &[("title", title), ("body_md", body)],
        )
        .await;
    id
}

async fn upload(app: &TestApp, client: &mut Client, name: &str) -> i64 {
    let sent = client.upload(name, b"%PDF-1.4 contenu").await;
    assert_eq!(sent.status, StatusCode::CREATED, "{}", sent.body);
    id_of(app, "SELECT id FROM files WHERE name = ?", name).await
}

async fn create_folder(app: &TestApp, client: &mut Client, space: i64, name: &str) -> i64 {
    let created = client
        .post(
            "/folders",
            &[("name", name), ("space_id", &space.to_string())],
        )
        .await;
    assert_eq!(created.status, StatusCode::SEE_OTHER, "{}", created.body);
    id_of(
        app,
        "SELECT id FROM folders WHERE name = ? AND deleted_at IS NULL",
        name,
    )
    .await
}

async fn put_in(app: &TestApp, table: &str, id: i64, folder: i64) {
    sqlx::query(&format!("UPDATE {table} SET folder_id = ? WHERE id = ?"))
        .bind(folder)
        .bind(id)
        .execute(&app.state.db)
        .await
        .unwrap();
}

#[tokio::test]
async fn une_note_supprimee_disparait_de_partout_puis_revient_a_sa_place() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.post("/space", &[("slug", "ecole")]).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    let math = create_folder(&app, &mut alice, ecole, "Math").await;
    let id = note(&mut alice, "Théorème", "motintrouvable de Pythagore").await;
    put_in(&app, "notes", id, math).await;
    alice
        .post(&format!("/notes/{id}/tags"), &[("name", "geometrie")])
        .await;

    let deleted = alice.post(&format!("/notes/{id}/delete"), &[]).await;
    assert_eq!(deleted.status, StatusCode::SEE_OTHER);

    // Nulle part : la note, la liste, la recherche, Perso, le dossier, le
    // widget, la barre de commandes, l'API.
    assert_eq!(
        alice.get(&format!("/notes/{id}")).await.status,
        StatusCode::NOT_FOUND
    );
    for page in [
        "/e/ecole/notes",
        "/e/perso/notes",
        "/e/ecole/notes?q=motintrouvable",
        "/e/perso/notes?q=motintrouvable",
        "/e/ecole/notes?tag=geometrie",
        "/e/ecole/",
    ] {
        let body = alice.get(page).await.body;
        assert!(!body.contains("Théorème"), "{page}");
    }
    let folder_page = alice.get(&format!("/e/ecole/files?dossier={math}")).await;
    assert!(!folder_page.body.contains("Théorème"));
    let widget = alice.get_fragment("/widgets/notes").await;
    assert!(!widget.body.contains("Théorème"));
    let suggest = alice
        .get_fragment("/cmd/suggest?q=Th%C3%A9or%C3%A8me")
        .await;
    assert!(!suggest.body.contains("Théorème"));
    let open = alice.command("/open Théorème", "/").await;
    assert!(!open.body.contains(&format!("/notes/{id}")));
    let (_, found) = app
        .api(
            Method::GET,
            "/api/v1/notes?q=motintrouvable",
            None,
            Some(API_TOKEN),
        )
        .await;
    assert_eq!(found, json!([]));
    let (status, _) = app
        .api(
            Method::GET,
            &format!("/api/v1/notes/{id}"),
            None,
            Some(API_TOKEN),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // La corbeille la montre : qui, quand, par où, et d'où elle vient.
    let trash = alice.get("/settings/trash").await;
    assert!(trash.body.contains("Théorème"));
    assert!(trash.body.contains("par toi, depuis le site"));
    assert!(trash.body.contains("École"));
    assert!(
        trash.body.contains("effacé dans 30 jours") || trash.body.contains("effacé dans 29 jours")
    );
    let settings = alice.get("/settings").await;
    assert!(settings.body.contains("Ouvrir la corbeille (1 élément)"));

    // Restaurer : dans son espace, dans son dossier, avec son tag.
    let restored = alice
        .post(&format!("/settings/trash/note/{id}/restore"), &[])
        .await;
    assert_eq!(restored.status, StatusCode::SEE_OTHER);
    let (space, folder): (i64, Option<i64>) =
        sqlx::query_as("SELECT space_id, folder_id FROM notes WHERE id = ?")
            .bind(id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!((space, folder), (ecole, Some(math)));
    assert!(
        alice
            .get("/e/ecole/notes?q=motintrouvable")
            .await
            .body
            .contains("Théorème")
    );
    assert!(
        alice
            .get("/e/ecole/notes?tag=geometrie")
            .await
            .body
            .contains("Théorème")
    );
    assert!(!alice.get("/settings/trash").await.body.contains("Théorème"));
}

#[tokio::test]
async fn sans_son_dossier_un_element_revient_a_la_racine() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let perso = app.space_id(ALICE, "perso").await;
    let folder = create_folder(&app, &mut alice, perso, "Brouillons").await;
    let id = note(&mut alice, "Idée", "texte").await;
    put_in(&app, "notes", id, folder).await;

    alice.post(&format!("/notes/{id}/delete"), &[]).await;
    // Le dossier, vide, part aussi ; restaurer la note la met à la racine.
    alice.post(&format!("/folders/{folder}/delete"), &[]).await;
    alice
        .post(&format!("/settings/trash/note/{id}/restore"), &[])
        .await;
    let place: (i64, Option<i64>) =
        sqlx::query_as("SELECT space_id, folder_id FROM notes WHERE id = ?")
            .bind(id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(place, (perso, None));
}

#[tokio::test]
async fn un_fichier_supprime_perd_ses_liens_de_partage() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let id = upload(&app, &mut alice, "devis.pdf").await;
    let created = alice
        .post(&format!("/files/{id}/share"), &[("duree", "1j")])
        .await;
    let at = created.body.find("/s/").expect("lien affiché") + 3;
    let token = created.body[at..at + 64].to_string();
    let mut stranger = app.client();
    assert_eq!(
        stranger.get(&format!("/s/{token}")).await.status,
        StatusCode::OK
    );

    alice.post(&format!("/files/{id}/delete"), &[]).await;
    assert_eq!(
        stranger.get(&format!("/s/{token}")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        alice.get(&format!("/files/{id}")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        alice.get(&format!("/files/{id}/download")).await.status,
        StatusCode::NOT_FOUND
    );
    assert!(
        !alice
            .get("/files?dossier=espace")
            .await
            .body
            .contains("devis.pdf")
    );
    assert!(!alice.get("/files?q=devis").await.body.contains("devis.pdf"));
    assert!(
        !alice
            .get_fragment("/widgets/files")
            .await
            .body
            .contains("devis.pdf")
    );

    // Restauré, il revient, mais le lien reste révoqué.
    alice
        .post(&format!("/settings/trash/fichier/{id}/restore"), &[])
        .await;
    assert!(
        alice
            .get("/files?dossier=espace")
            .await
            .body
            .contains("devis.pdf")
    );
    assert_eq!(
        stranger.get(&format!("/s/{token}")).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn un_dossier_part_avec_son_contenu_et_revient_avec_lui() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.post("/space", &[("slug", "ecole")]).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    let math = create_folder(&app, &mut alice, ecole, "Math").await;
    alice
        .post(
            "/folders",
            &[("name", "Exercices"), ("parent_id", &math.to_string())],
        )
        .await;
    let sub = id_of(&app, "SELECT id FROM folders WHERE name = ?", "Exercices").await;
    let cours = note(&mut alice, "Cours", "texte").await;
    let exo = note(&mut alice, "Exo", "texte").await;
    let vieux = note(&mut alice, "Vieux", "texte").await;
    let pdf = upload(&app, &mut alice, "td.pdf").await;
    put_in(&app, "notes", cours, math).await;
    put_in(&app, "notes", vieux, math).await;
    put_in(&app, "notes", exo, sub).await;
    put_in(&app, "files", pdf, sub).await;

    // « Vieux » est supprimé seul, avant le dossier.
    alice.post(&format!("/notes/{vieux}/delete"), &[]).await;
    let deleted = alice
        .post(
            &format!("/folders/{math}/delete"),
            &[("contenu", "supprimer")],
        )
        .await;
    assert_eq!(deleted.status, StatusCode::SEE_OTHER, "{}", deleted.body);
    let page = alice.get("/e/ecole/files?dossier=espace").await;
    assert!(!page.body.contains(&format!("dossier={math}\"")));
    assert!(!alice.get("/e/ecole/notes").await.body.contains("Cours"));

    // Une ligne pour le dossier, avec ce qu'il a emporté ; une pour « Vieux ».
    let trash = alice.get("/settings/trash").await;
    assert!(trash.body.contains("Math"));
    assert!(trash.body.contains("2 notes, 1 fichier, 1 sous-dossier"));
    assert!(trash.body.contains("Vieux"));
    assert!(!trash.body.contains(">Cours<"));

    // Un nouveau « Math » est créé entre-temps : le restauré devient « Math (2) ».
    create_folder(&app, &mut alice, ecole, "Math").await;
    let restored = alice
        .post(&format!("/settings/trash/dossier/{math}/restore"), &[])
        .await;
    assert_eq!(restored.status, StatusCode::SEE_OTHER);
    let name: String = sqlx::query_scalar("SELECT name FROM folders WHERE id = ?")
        .bind(math)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(name, "Math (2)");
    for (table, id, folder) in [
        ("notes", cours, math),
        ("notes", exo, sub),
        ("files", pdf, sub),
    ] {
        let place: (i64, Option<i64>) = sqlx::query_as(&format!(
            "SELECT space_id, folder_id FROM {table} WHERE id = ?"
        ))
        .bind(id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
        assert_eq!(place, (ecole, Some(folder)), "{table} {id}");
    }
    // « Vieux », supprimé à part, reste dans la corbeille.
    let trash = alice.get("/settings/trash").await;
    assert!(trash.body.contains("Vieux"));
    assert!(
        !trash
            .body
            .contains(&format!("/settings/trash/dossier/{math}/restore"))
    );
}

#[tokio::test]
async fn un_espace_supprime_ne_se_restaure_que_par_son_proprietaire() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let mut bob = app.logged_in(BOB).await;
    let slug = app.share_space(ALICE, "taff", BOB).await;
    alice.post("/space", &[("slug", "taff")]).await;
    note(&mut alice, "Budget", "motdutaff").await;
    let taff = app.space_id(ALICE, "taff").await;
    assert_eq!(
        bob.get(&format!("/e/{slug}/notes")).await.status,
        StatusCode::OK
    );

    let deleted = alice
        .post(
            &format!("/settings/spaces/{taff}/delete"),
            &[("content", "delete")],
        )
        .await;
    assert_eq!(deleted.status, StatusCode::SEE_OTHER);
    // Plus d'accès pour personne, ni de trace dans l'agenda ou la recherche.
    assert_eq!(
        alice.get("/e/taff/notes").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        bob.get(&format!("/e/{slug}/notes")).await.status,
        StatusCode::NOT_FOUND
    );
    assert!(
        !alice
            .get("/e/perso/notes?q=motdutaff")
            .await
            .body
            .contains("Budget")
    );
    let (_, spaces) = app
        .api(Method::GET, "/api/v1/spaces", None, Some(API_TOKEN))
        .await;
    assert!(!spaces.to_string().contains("\"taff\""));

    // Bob ne le voit pas dans sa corbeille et ne peut pas le restaurer.
    assert!(!bob.get("/settings/trash").await.body.contains("Taff"));
    let refused = bob
        .post(&format!("/settings/trash/espace/{taff}/restore"), &[])
        .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);
    let refused = bob
        .post(&format!("/settings/trash/espace/{taff}/erase"), &[])
        .await;
    assert_eq!(refused.status, StatusCode::NOT_FOUND);

    // Un nouvel espace peut prendre le nom et le slug entre-temps.
    alice
        .post(
            "/settings/spaces",
            &[("name", "Taff"), ("color", "green"), ("icon", "work")],
        )
        .await;
    // Alice le restaure : il revient avec son contenu, sous un nom libre.
    let restored = alice
        .post(&format!("/settings/trash/espace/{taff}/restore"), &[])
        .await;
    assert_eq!(restored.status, StatusCode::SEE_OTHER);
    let (name, back_slug): (String, String) =
        sqlx::query_as("SELECT name, slug FROM spaces WHERE id = ?")
            .bind(taff)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(name, "Taff (2)");
    assert_ne!(back_slug, "taff");
    assert!(
        alice
            .get(&format!("/e/{back_slug}/notes"))
            .await
            .body
            .contains("Budget")
    );
}

#[tokio::test]
async fn la_corbeille_d_un_espace_partage_est_commune() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let mut bob = app.logged_in(BOB).await;
    let slug = app.share_space(ALICE, "ecole", BOB).await;
    bob.post("/space", &[("slug", &slug)]).await;
    let id = note(&mut bob, "Notes de Bob", "texte").await;
    bob.post(&format!("/notes/{id}/delete"), &[]).await;

    // Alice la voit, avec son auteur, et la restaure.
    let trash = alice.get("/settings/trash").await;
    assert!(trash.body.contains("Notes de Bob"));
    assert!(trash.body.contains("par bob@example.test"));
    let restored = alice
        .post(&format!("/settings/trash/note/{id}/restore"), &[])
        .await;
    assert_eq!(restored.status, StatusCode::SEE_OTHER);
    assert!(
        alice
            .get("/e/ecole/notes")
            .await
            .body
            .contains("Notes de Bob")
    );

    // Un élément de la corbeille privée d'Alice : invisible et intouchable pour Bob.
    let secret = note(&mut alice, "Journal d'Alice", "texte").await;
    let moved = sqlx::query("UPDATE notes SET space_id = ? WHERE id = ?")
        .bind(app.space_id(ALICE, "perso").await)
        .bind(secret)
        .execute(&app.state.db)
        .await;
    assert!(moved.is_ok());
    alice.post(&format!("/notes/{secret}/delete"), &[]).await;
    assert!(!bob.get("/settings/trash").await.body.contains("Journal"));
    for action in ["restore", "erase"] {
        let refused = bob
            .post(&format!("/settings/trash/note/{secret}/{action}"), &[])
            .await;
        assert_eq!(refused.status, StatusCode::NOT_FOUND, "{action}");
    }
}

#[tokio::test]
async fn supprimer_definitivement_et_vider_sur_le_site_seulement() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let gone = note(&mut alice, "À effacer", "texte").await;
    let file = upload(&app, &mut alice, "photo.pdf").await;
    let other = note(&mut alice, "Autre", "texte").await;
    for (path, id) in [("notes", gone), ("files", file), ("notes", other)] {
        alice.post(&format!("/{path}/{id}/delete"), &[]).await;
    }
    assert_eq!(app.stored_files().len(), 1);

    let erased = alice
        .post(&format!("/settings/trash/note/{gone}/erase"), &[])
        .await;
    assert_eq!(erased.status, StatusCode::SEE_OTHER);
    assert_eq!(
        scalar(
            &app,
            &format!("SELECT count(*) FROM notes WHERE id = {gone}")
        )
        .await,
        0
    );
    let page = alice.get("/settings/trash").await;
    assert!(page.body.contains("hx-confirm=\"Vider la corbeille ?"));
    let emptied = alice.post("/settings/trash/empty", &[]).await;
    assert!(
        alice
            .follow(emptied)
            .await
            .body
            .contains("Corbeille vidée : 2 éléments supprimés définitivement")
    );
    assert_eq!(scalar(&app, "SELECT count(*) FROM notes").await, 0);
    assert_eq!(scalar(&app, "SELECT count(*) FROM files").await, 0);
    assert!(app.stored_files().is_empty());

    // L'API n'a pas de route pour effacer ou vider.
    for path in ["/api/v1/trash/empty", "/api/v1/trash/note/1/erase"] {
        let (status, _) = app
            .api(Method::POST, path, Some(json!({})), Some(API_TOKEN))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn apres_trente_jours_tout_s_efface() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let recent = note(&mut alice, "Récente", "texte").await;
    let old = note(&mut alice, "Ancienne", "texte").await;
    let file = upload(&app, &mut alice, "vieux.pdf").await;
    for (path, id) in [("notes", recent), ("notes", old), ("files", file)] {
        alice.post(&format!("/{path}/{id}/delete"), &[]).await;
    }
    let taff = app.space_id(ALICE, "taff").await;
    alice
        .post(
            &format!("/settings/spaces/{taff}/delete"),
            &[("content", "delete")],
        )
        .await;
    let long_ago = dashu::time::now() - 31 * 24 * 3600;
    for sql in [
        "UPDATE notes SET deleted_at = ? WHERE title = 'Ancienne'",
        "UPDATE files SET deleted_at = ? WHERE name = 'vieux.pdf'",
        "UPDATE spaces SET deleted_at = ? WHERE deleted_at IS NOT NULL",
    ] {
        sqlx::query(sql)
            .bind(long_ago)
            .execute(&app.state.db)
            .await
            .unwrap();
    }
    assert_eq!(app.stored_files().len(), 1);

    dashu::trash::purge(&app.state).await.unwrap();
    assert_eq!(
        scalar(
            &app,
            &format!("SELECT count(*) FROM notes WHERE id = {old}")
        )
        .await,
        0
    );
    assert_eq!(
        scalar(
            &app,
            &format!("SELECT count(*) FROM notes WHERE id = {recent}")
        )
        .await,
        1
    );
    assert_eq!(scalar(&app, "SELECT count(*) FROM files").await, 0);
    assert!(app.stored_files().is_empty());
    assert_eq!(
        scalar(
            &app,
            &format!("SELECT count(*) FROM spaces WHERE id = {taff}")
        )
        .await,
        0
    );
}
