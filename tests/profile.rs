//! Profil et affichage : prénom et nom, photo de profil ou initiales,
//! taille de l'interface, barre latérale ancrée ou détachée, et ce que la
//! barre latérale montre en bas.

mod common;

use std::io::Cursor;

use axum::http::StatusCode;
use common::{ALICE, BOB, TestApp};

/// Une image JPEG de `w` × `h` pixels, comme celle que prépare le navigateur.
fn jpeg(w: u32, h: u32) -> Vec<u8> {
    let image = image::RgbImage::from_pixel(w, h, image::Rgb([200, 80, 40]));
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
    out.into_inner()
}

#[tokio::test]
async fn barre_laterale_reglages_en_bas_sans_theme() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let page = alice.get("/").await;
    let foot = common::between(&page.body, "class=\"sidebar-foot\"", "</nav>").unwrap();
    // Réglages, la taille de l'interface et le compte sont en bas ; le thème
    // ne se choisit plus que dans Réglages.
    assert!(foot.contains("href=\"/settings\""));
    assert!(foot.contains("action=\"/settings/size\""));
    assert!(foot.contains("class=\"account-link\""));
    assert!(!page.body.contains("action=\"/settings/theme\""));
    let nav = common::between(
        &page.body,
        "class=\"sidebar-nav\"",
        "class=\"sidebar-foot\"",
    )
    .unwrap();
    assert!(!nav.contains("href=\"/settings\""));
    let settings = alice.get("/settings").await;
    assert!(settings.body.contains("action=\"/settings/theme\""));
}

#[tokio::test]
async fn nom_prenom_et_initiales() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;

    // Sans profil : l'email, et sa première lettre sur un fond tiré au hasard.
    let page = alice.get("/").await;
    assert!(
        page.body
            .contains("<span class=\"account-name\">alice@example.test</span>")
    );
    let color: String = sqlx::query_scalar("SELECT avatar_color FROM users WHERE email = ?")
        .bind(ALICE)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert!(
        dashu::spaces::COLORS.iter().any(|(key, _)| *key == color),
        "{color}"
    );
    assert!(page.body.contains(&format!(
        "<span class=\"avatar\" data-accent=\"{color}\" aria-hidden=\"true\">A</span>"
    )));
    // Le fond ne change plus d'une page à l'autre.
    assert!(
        alice
            .get("/notes")
            .await
            .body
            .contains(&format!("data-accent=\"{color}\" aria-hidden=\"true\">A<"))
    );

    let saved = alice
        .post(
            "/settings/profile",
            &[("first_name", "  rémi "), ("last_name", "Moreau")],
        )
        .await;
    let page = alice.follow(saved).await;
    assert!(page.body.contains("Profil enregistré"));
    assert!(
        page.body
            .contains("<span class=\"account-name\">rémi Moreau</span>")
    );
    assert!(page.body.contains("aria-hidden=\"true\">RM</span>"));
    assert!(page.body.contains("name=\"first_name\" value=\"rémi\""));

    let refused = alice
        .post(
            "/settings/profile",
            &[("first_name", &"x".repeat(41)), ("last_name", "")],
        )
        .await;
    let page = alice.follow(refused).await;
    assert!(page.body.contains("tiennent en 40 caractères"));
    assert!(page.body.contains("account-name\">rémi Moreau<"));
}

#[tokio::test]
async fn photo_de_profil() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;

    // Une photo rectangulaire devient un carré de 256 px.
    let sent = alice.post_raw("/settings/avatar", &jpeg(400, 300)).await;
    assert_eq!(sent.status, StatusCode::NO_CONTENT, "{}", sent.body);
    let page = alice.get("/").await;
    assert!(page.body.contains("<img src=\"/profile/avatar?v=1\""));
    assert!(page.body.contains("Photo de profil changée"));
    let photo = alice.get("/profile/avatar?v=1").await;
    assert_eq!(photo.status, StatusCode::OK);
    assert_eq!(photo.header("content-type"), "image/jpeg");
    assert!(photo.header("cache-control").contains("immutable"));
    let decoded = image::load_from_memory(&photo.bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (256, 256));

    // Autre chose qu'une image : refusé, l'ancienne photo reste.
    let refused = alice
        .post_raw("/settings/avatar", b"<svg onload=alert(1)>")
        .await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    // Une image géante (pas préparée par le navigateur) : refusée aussi.
    let refused = alice.post_raw("/settings/avatar", &jpeg(3000, 10)).await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    assert!(alice.get("/").await.body.contains("avatar?v=1"));

    // Bob a sa propre photo (aucune) : l'adresse sert toujours celle du connecté.
    let mut bob = app.logged_in(BOB).await;
    assert_eq!(
        bob.get("/profile/avatar?v=1").await.status,
        StatusCode::NOT_FOUND
    );

    // Retirer la photo : retour aux initiales, nouvelle adresse.
    let removed = alice.post("/settings/avatar/delete", &[]).await;
    let page = alice.follow(removed).await;
    assert!(page.body.contains("Photo de profil retirée"));
    assert!(!page.body.contains("/profile/avatar"));
    assert_eq!(
        alice.get("/profile/avatar?v=2").await.status,
        StatusCode::NOT_FOUND
    );
    let refused = alice
        .post_without_csrf("/settings/avatar/delete", &[])
        .await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn taille_et_barre_detachee() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let page = alice.get("/").await;
    assert!(!page.body.contains("data-size="));
    assert!(!page.body.contains("data-sidebar="));
    assert!(page.body.contains("Détacher la barre latérale"));

    let changed = alice
        .post("/settings/size", &[("size", "large"), ("next", "/notes")])
        .await;
    assert_eq!(changed.location(), "/notes");
    let page = alice.get("/notes").await;
    assert!(page.body.contains("data-size=\"large\""));
    assert!(page.body.contains("value=\"large\" aria-pressed=\"true\""));
    alice.post("/settings/size", &[("size", "normal")]).await;
    assert!(!alice.get("/").await.body.contains("data-size="));
    assert_eq!(
        alice
            .post("/settings/size", &[("size", "geante")])
            .await
            .status,
        StatusCode::BAD_REQUEST
    );

    alice
        .post("/settings/sidebar", &[("floating", "1"), ("next", "/")])
        .await;
    let page = alice.get("/").await;
    assert!(page.body.contains("data-sidebar=\"floating\""));
    assert!(page.body.contains("class=\"sidebar-edge\""));
    assert!(page.body.contains("Ancrer la barre latérale"));
    alice.post("/settings/sidebar", &[("floating", "0")]).await;
    assert!(!alice.get("/").await.body.contains("data-sidebar="));

    // Réglage par compte : Bob n'est pas touché.
    alice.post("/settings/size", &[("size", "small")]).await;
    let mut bob = app.logged_in(BOB).await;
    assert!(!bob.get("/").await.body.contains("data-size="));
}
