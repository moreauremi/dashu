//! Espaces partagés : notes communes, tags communs, numéro de version.

mod common;

use axum::http::StatusCode;
use common::{ALICE, BOB, Client, TestApp, between};

/// Alice crée une note dans l'espace `slug` et lui donne ce titre et ce texte.
async fn note_in(alice: &mut Client, slug: &str, title: &str, body: &str) -> i64 {
    let created = alice.post(&format!("/e/{slug}/notes"), &[]).await;
    assert_eq!(created.status, StatusCode::SEE_OTHER);
    let id: i64 = created
        .location()
        .rsplit('/')
        .next()
        .unwrap()
        .parse()
        .expect("identifiant de note");
    let saved = alice
        .post(
            &format!("/notes/{id}"),
            &[("title", title), ("body_md", body)],
        )
        .await;
    assert_eq!(saved.status, StatusCode::OK);
    id
}

async fn field<T>(app: &TestApp, sql: &str, id: i64) -> T
where
    T: for<'r> sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite> + Send + Unpin,
{
    sqlx::query_scalar(sql)
        .bind(id)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn le_membre_lit_modifie_epingle_et_deplace() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let mut bob = app.logged_in(BOB).await;
    let id = note_in(&mut alice, "ecole", "Cours de réseau", "Le VLAN isole.").await;
    let secret = note_in(&mut alice, "taff", "Dossier privé", "Rien pour Bob.").await;
    let slug = app.share_space(ALICE, "ecole", BOB).await;

    // La liste de l'espace partagé et le Perso de Bob montrent la note.
    let list = bob.get(&format!("/e/{slug}/notes")).await;
    assert_eq!(list.status, StatusCode::OK);
    assert!(list.body.contains("Cours de réseau"));
    assert!(!list.body.contains("Dossier privé"));
    let perso = bob.get("/notes").await;
    assert!(perso.body.contains("Cours de réseau"));
    // La recherche la trouve aussi.
    let found = bob.get(&format!("/e/{slug}/notes?q=vlan")).await;
    assert!(found.body.contains("Cours de réseau"));

    // Il l'ouvre (depuis son Perso : redirection vers l'espace partagé).
    let jump = bob.get(&format!("/e/perso/notes/{id}")).await;
    assert_eq!(jump.status, StatusCode::SEE_OTHER);
    assert_eq!(jump.location(), format!("/e/{slug}/notes/{id}"));
    let page = bob.get(&format!("/e/{slug}/notes/{id}")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("value=\"Cours de réseau\""));

    // Une note d'un espace non partagé : 404 partout.
    assert_eq!(
        bob.get(&format!("/notes/{secret}")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        bob.post(
            &format!("/notes/{secret}"),
            &[("title", "x"), ("body_md", "y")]
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        bob.post(&format!("/notes/{secret}/pin"), &[]).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        bob.post(&format!("/notes/{secret}/delete"), &[])
            .await
            .status,
        StatusCode::NOT_FOUND
    );

    // Il la modifie : Alice voit le nouveau texte.
    let saved = bob
        .post(
            &format!("/notes/{id}"),
            &[("title", "Cours de réseau"), ("body_md", "Texte de Bob.")],
        )
        .await;
    assert_eq!(saved.status, StatusCode::OK);
    let body: String = field(&app, "SELECT body_md FROM notes WHERE id = ?", id).await;
    assert_eq!(body, "Texte de Bob.");
    // La colonne garde l'auteur.
    let author: i64 = field(&app, "SELECT user_id FROM notes WHERE id = ?", id).await;
    assert_eq!(author, app.user_id(ALICE).await);

    // Il l'épingle.
    let pinned = bob.post(&format!("/notes/{id}/pin"), &[]).await;
    assert_eq!(pinned.status, StatusCode::OK);
    let flag: bool = field(&app, "SELECT pinned FROM notes WHERE id = ?", id).await;
    assert!(flag);

    // Il la tague.
    let tagged = bob
        .post(&format!("/notes/{id}/tags"), &[("name", "Réseau")])
        .await;
    assert!(tagged.body.contains("#réseau"));

    // Il la déplace vers son propre espace : Alice n'y a plus accès.
    let bob_perso = app.space_id(BOB, "perso").await.to_string();
    let moved = bob
        .post(
            &format!("/notes/{id}/move"),
            &[("space_id", bob_perso.as_str())],
        )
        .await;
    assert_eq!(moved.status, StatusCode::SEE_OTHER);
    assert_eq!(
        alice.get(&format!("/notes/{id}")).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn tags_de_meme_nom_communs() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let mut bob = app.logged_in(BOB).await;
    let id = note_in(&mut alice, "ecole", "Cours", "Texte").await;
    let slug = app.share_space(ALICE, "ecole", BOB).await;

    alice
        .post(&format!("/notes/{id}/tags"), &[("name", "bts")])
        .await;
    // Bob pose le même nom : rien de plus sur la note.
    let again = bob
        .post(&format!("/notes/{id}/tags"), &[("name", "BTS")])
        .await;
    assert_eq!(again.body.matches("/tags/").count(), 1, "{}", again.body);
    let on_note: i64 = field(&app, "SELECT COUNT(*) FROM note_tags WHERE note_id = ?", id).await;
    assert_eq!(on_note, 1);

    // Bob voit le tag d'Alice et une seule pastille.
    let list = bob.get(&format!("/e/{slug}/notes")).await;
    assert_eq!(list.body.matches("tag=bts").count(), 1, "{}", list.body);
    let filtered = bob.get(&format!("/e/{slug}/notes?tag=bts")).await;
    assert!(filtered.body.contains("Cours"));
    // Le nom est proposé à Bob en suggestion sur la page de la note.
    let page = bob.get(&format!("/e/{slug}/notes/{id}")).await;
    assert!(page.body.contains("bts"));

    // Deux tags de même nom (un par compte) déjà posés : une seule pastille,
    // un seul tag affiché, et le retirer les retire tous les deux.
    let bob_id = app.user_id(BOB).await;
    sqlx::query("INSERT INTO tags (user_id, name) VALUES (?, 'bts')")
        .bind(bob_id)
        .execute(&app.state.db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO note_tags (note_id, tag_id) SELECT ?, id FROM tags WHERE user_id = ? AND name = 'bts'",
    )
    .bind(id)
    .bind(bob_id)
    .execute(&app.state.db)
    .await
    .unwrap();
    let list = bob.get(&format!("/e/{slug}/notes")).await;
    assert_eq!(list.body.matches("tag=bts").count(), 1);
    let page = bob.get(&format!("/e/{slug}/notes/{id}")).await;
    assert_eq!(page.body.matches("/tags/").count(), 1, "{}", page.body);
    let tag_id: i64 = sqlx::query_scalar("SELECT MIN(id) FROM tags WHERE name = 'bts'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    bob.delete(&format!("/notes/{id}/tags/{tag_id}")).await;
    let left: i64 = field(&app, "SELECT COUNT(*) FROM note_tags WHERE note_id = ?", id).await;
    assert_eq!(left, 0);
    let tags: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tags")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(tags, 0, "les tags orphelins de tous les comptes partent");
}

#[tokio::test]
async fn numero_de_version_contre_les_ecrasements() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let mut bob = app.logged_in(BOB).await;
    let id = note_in(&mut alice, "ecole", "Cours", "Départ").await;
    app.share_space(ALICE, "ecole", BOB).await;
    let url = format!("/notes/{id}");

    // La page porte la version dans le formulaire.
    let page = alice.get(&format!("/e/ecole/notes/{id}")).await;
    let hidden = between(&page.body, "id=\"note-revision\"", ">").expect("champ de version");
    assert!(hidden.contains("value=\""));
    let version: i64 = field(&app, "SELECT revision FROM notes WHERE id = ?", id).await;
    assert!(version >= 1);
    let v = version.to_string();

    // Premier enregistrement avec cette version : passe, version suivante renvoyée.
    let first = alice
        .post(
            &url,
            &[("title", "Cours"), ("body_md", "Alice"), ("revision", &v)],
        )
        .await;
    assert_eq!(first.status, StatusCode::OK);
    let next = (version + 1).to_string();
    assert!(
        first.body.contains(&format!("value=\"{next}\"")) && first.body.contains("hx-swap-oob"),
        "{}",
        first.body
    );

    // Second enregistrement avec la même (ancienne) version : 409, rien d'écrit.
    let second = bob
        .post(
            &url,
            &[("title", "Cours"), ("body_md", "Bob"), ("revision", &v)],
        )
        .await;
    assert_eq!(second.status, StatusCode::CONFLICT);
    assert!(second.body.contains("Modifiée ailleurs"));
    let body: String = field(&app, "SELECT body_md FROM notes WHERE id = ?", id).await;
    assert_eq!(body, "Alice");

    // Avec la version renvoyée : passe.
    let third = bob
        .post(
            &url,
            &[("title", "Cours"), ("body_md", "Bob"), ("revision", &next)],
        )
        .await;
    assert_eq!(third.status, StatusCode::OK);

    // Rien de changé : pas d'incrément, la version actuelle est renvoyée.
    let now: i64 = field(&app, "SELECT revision FROM notes WHERE id = ?", id).await;
    let same = bob
        .post(
            &url,
            &[
                ("title", "Cours"),
                ("body_md", "Bob"),
                ("revision", &now.to_string()),
            ],
        )
        .await;
    assert_eq!(same.status, StatusCode::OK);
    assert!(same.body.contains(&format!("value=\"{now}\"")));
    assert_eq!(
        field::<i64>(&app, "SELECT revision FROM notes WHERE id = ?", id).await,
        now
    );

    // Sans version : comme avant.
    let free = alice
        .post(&url, &[("title", "Cours"), ("body_md", "Libre")])
        .await;
    assert_eq!(free.status, StatusCode::OK);
    let body: String = field(&app, "SELECT body_md FROM notes WHERE id = ?", id).await;
    assert_eq!(body, "Libre");
}
