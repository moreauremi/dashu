//! Retour depuis la page d'un fichier (demandé le 15 novembre 2026) : le
//! lien ramène à la page d'où l'on a ouvert le fichier (paramètre `depuis`,
//! posé par app.js), avec son nom, et l'origine survit aux actions de la
//! page. Sans origine valable, retour au dossier du fichier, comme avant.

mod common;

use axum::http::StatusCode;
use common::{ALICE, Client, TestApp, between};

/// Ce que `depuis=…` devient dans une adresse.
fn encoded(depuis: &str) -> String {
    form_urlencoded::byte_serialize(depuis.as_bytes()).collect()
}

/// Une adresse telle qu'askama l'écrit dans un attribut.
fn in_html(url: &str) -> String {
    url.replace('&', "&#38;")
}

/// (lien, nom) du retour de la page d'un fichier.
async fn back_of(client: &mut Client, url: &str) -> (String, String) {
    let page = client.get(url).await;
    assert_eq!(page.status, StatusCode::OK, "{url}");
    let link = between(&page.body, "<a class=\"back-link\" href=\"", "\"")
        .expect("lien de retour")
        .to_string();
    let label = between(&page.body, "data-retour>", "</a>")
        .and_then(|a| between(a, "<span>", "</span>"))
        .expect("nom du retour")
        .to_string();
    (link, label)
}

struct Setup {
    app: TestApp,
    alice: Client,
    file: i64,
    reseaux: i64,
    note: i64,
}

/// Un fichier dans École / Réseaux, et une note dans École.
async fn setup() -> Setup {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let ecole = app.space_id(ALICE, "ecole").await;
    alice
        .post(
            "/folders",
            &[("name", "Réseaux"), ("space_id", &ecole.to_string())],
        )
        .await;
    let reseaux: i64 = sqlx::query_scalar("SELECT id FROM folders WHERE name = 'Réseaux'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    alice
        .upload_in_folder("cours-vlan.pdf", b"%PDF vlan", reseaux)
        .await;
    let file: i64 = sqlx::query_scalar("SELECT id FROM files WHERE name = 'cours-vlan.pdf'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    let note = common::note_id(&alice.post("/e/ecole/notes", &[]).await);
    alice
        .post(
            &format!("/notes/{note}"),
            &[("title", "Fiche VLAN"), ("body_md", "voir le PDF")],
        )
        .await;
    Setup {
        app,
        alice,
        file,
        reseaux,
        note,
    }
}

#[tokio::test]
async fn le_retour_ramene_a_la_page_d_origine() {
    let Setup {
        mut alice,
        file,
        reseaux,
        note,
        app: _app,
        ..
    } = setup().await;
    let view = |depuis: &str| format!("/e/ecole/files/{file}?depuis={}", encoded(depuis));

    // Sans origine : le dossier du fichier, comme avant.
    assert_eq!(
        back_of(&mut alice, &format!("/e/ecole/files/{file}")).await,
        (
            format!("/e/ecole/files?dossier={reseaux}"),
            "École / Réseaux".into()
        )
    );

    // Une recherche, un tag, le dossier de l'espace, Temporaire.
    for (depuis, label) in [
        (
            "/e/ecole/files?dossier=espace&q=vlan".to_string(),
            "École · « vlan »".to_string(),
        ),
        (
            format!("/e/ecole/files?dossier={reseaux}&tag=r%C3%A9seau"),
            "École / Réseaux · #réseau".to_string(),
        ),
        ("/e/ecole/files?dossier=espace".into(), "École".into()),
        (
            "/e/ecole/files?dossier=temporaire".into(),
            "Temporaire".into(),
        ),
        // Les autres pages : leur nom.
        ("/e/ecole/".into(), "Dashboard".into()),
        ("/e/ecole/tools".into(), "Outils".into()),
        ("/e/ecole/notes".into(), "Notes".into()),
        (format!("/e/ecole/notes/{note}"), "Fiche VLAN".into()),
    ] {
        assert_eq!(
            back_of(&mut alice, &view(&depuis)).await,
            (in_html(&depuis), label),
            "{depuis}"
        );
    }
}

#[tokio::test]
async fn ouvert_depuis_perso_on_revient_dans_perso() {
    let Setup {
        app: _app,
        mut alice,
        file,
        ..
    } = setup().await;
    // La racine de Perso, avec une recherche dans tous les espaces : le
    // fichier d'École s'ouvre sur place, dans Perso, et le retour y mène.
    let depuis = "/e/perso/files?dossier=racine&q=vlan";
    let path = format!("/e/perso/files/{file}?depuis={}", encoded(depuis));
    assert_eq!(alice.get(&path).await.status, StatusCode::OK);
    let (link, label) = back_of(&mut alice, &path).await;
    assert_eq!(link, in_html(depuis));
    assert_eq!(label, "Fichiers · « vlan »");
}

#[tokio::test]
async fn une_origine_douteuse_est_ignoree() {
    let Setup {
        mut alice,
        file,
        reseaux,
        app: _app,
        ..
    } = setup().await;
    let folder = (
        format!("/e/ecole/files?dossier={reseaux}"),
        "École / Réseaux".to_string(),
    );
    for depuis in [
        "https://exemple.test/piege",
        "//exemple.test/piege",
        "/\\exemple.test",
        // Un espace ou un dossier qui n'existe pas (ou plus).
        "/e/inconnu/files?dossier=espace",
        "/e/ecole/files?dossier=999",
        "/e/ecole/notes/999",
    ] {
        let url = format!("/e/ecole/files/{file}?depuis={}", encoded(depuis));
        assert_eq!(back_of(&mut alice, &url).await, folder, "{depuis}");
    }
    let long = format!("/e/ecole/files?dossier=espace&q={}", "x".repeat(2100));
    let url = format!("/e/ecole/files/{file}?depuis={}", encoded(&long));
    assert_eq!(back_of(&mut alice, &url).await, folder);
}

#[tokio::test]
async fn l_origine_survit_aux_actions_de_la_page() {
    let Setup {
        app,
        mut alice,
        file,
        ..
    } = setup().await;
    let depuis = "/e/ecole/files?dossier=espace&q=vlan";
    let here = format!("/files/{file}?depuis={}", encoded(depuis));
    let page = alice.get(&format!("/e/ecole{here}")).await.body;

    // Renommer (et convertir) : retour sur cette page, origine comprise.
    let rename = between(&page, "action=\"/files/", "</form>").unwrap();
    assert!(
        rename.contains(&format!("name=\"next\" value=\"{here}\"")),
        "{rename}"
    );
    // Ranger : la fenêtre revient aussi ici.
    assert!(page.contains(&format!("next={}", encoded(&here))));
    // Supprimer : retour à la page d'origine.
    assert!(page.contains(&format!("name=\"next\" value=\"{}\"", in_html(depuis))));

    let renamed = alice
        .post(
            &format!("/files/{file}/rename"),
            &[("name", "VLAN"), ("next", &here)],
        )
        .await;
    assert_eq!(renamed.location(), here);
    let (link, label) = back_of(&mut alice, &format!("/e/ecole{here}")).await;
    assert_eq!(
        (link.as_str(), label.as_str()),
        (in_html(depuis).as_str(), "École · « vlan »")
    );

    let deleted = alice
        .post(&format!("/files/{file}/delete"), &[("next", depuis)])
        .await;
    assert_eq!(deleted.location(), depuis);
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM files WHERE deleted_at IS NULL")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(left, 0);
}
