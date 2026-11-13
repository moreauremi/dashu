//! Aperçu des fichiers (v2, étape 3) : vignettes dans la liste, page
//! d'aperçu, lecture d'une vidéo par morceaux, textes montrés sans jamais
//! être exécutés.

mod common;

use axum::http::StatusCode;
use common::{ALICE, BOB, Client, TestApp};
use dashu::tools::{self, worker};

/// Envoie un fichier et renvoie son identifiant.
async fn upload(client: &mut Client, name: &str, content: &[u8]) -> i64 {
    client.get("/files?dossier=espace").await;
    let sent = client.upload(name, content).await;
    assert_eq!(sent.status, StatusCode::CREATED, "{}", sent.body);
    let page = client.get("/files?dossier=espace").await.body;
    let at = page
        .find(&format!("<span>{name}</span>"))
        .unwrap_or_else(|| panic!("{name} absent de la liste"));
    let before = &page[..at];
    let marker = "class=\"file-name\" href=\"/files/";
    let start = before.rfind(marker).unwrap() + marker.len();
    before[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap()
}

/// Une vraie image PNG en dégradé (petite une fois compressée).
fn gradient_png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbImage::from_fn(width, height, |x, _| {
        image::Rgb([(x * 255 / width) as u8, 120, 200])
    });
    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    png
}

#[tokio::test]
async fn la_vignette_est_faite_a_l_arrivee_puis_affichee() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let id = upload(&mut alice, "plage.png", &gradient_png(600, 400)).await;

    // Pas encore de vignette : l'icône du type.
    let page = alice.get("/files?dossier=espace").await.body;
    assert!(!page.contains(&format!("src=\"/files/{id}/thumb\"")));
    assert!(page.contains("file-thumb-icon"));
    assert_eq!(
        alice.get(&format!("/files/{id}/thumb")).await.status,
        StatusCode::NOT_FOUND
    );

    // Une tâche de vignette attend le service de conversion.
    let job: i64 =
        sqlx::query_scalar("SELECT id FROM jobs WHERE kind = 'thumbnail' AND file_id = ?")
            .bind(id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    let dir = tools::job_dir(&app.work_dir, job);
    std::fs::write(dir.join("started"), "").unwrap();
    let converter = worker::Tools { zscale: false };
    worker::process_job(&dir, &|spec, dir, progress| {
        converter.convert(spec, dir, progress)
    });
    assert!(
        dir.join("done").exists(),
        "{:?}",
        std::fs::read_to_string(dir.join("error"))
    );
    tools::collect(&app.state).await.unwrap();

    let page = alice.get("/files?dossier=espace").await.body;
    assert!(page.contains(&format!("src=\"/files/{id}/thumb\"")));
    let thumb = alice.get(&format!("/files/{id}/thumb")).await;
    assert_eq!(thumb.status, StatusCode::OK);
    assert_eq!(thumb.header("content-type"), "image/jpeg");
    let small = image::load_from_memory(&thumb.bytes).unwrap();
    assert_eq!((small.width(), small.height()), (256, 171));

    // Supprimer le fichier efface aussi sa vignette.
    assert_eq!(app.stored_files().len(), 2);
    alice.post(&format!("/files/{id}/delete"), &[]).await;
    assert!(app.stored_files().is_empty(), "{:?}", app.stored_files());
}

#[tokio::test]
async fn une_photo_s_affiche_en_grand() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let png = gradient_png(300, 200);
    let id = upload(&mut alice, "photo.png", &png).await;

    let page = alice.get(&format!("/files/{id}")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains(&format!("<img src=\"/files/{id}/raw\"")));
    assert!(
        page.body
            .contains(&format!("href=\"/files/{id}/download\""))
    );

    let raw = alice.get(&format!("/files/{id}/raw")).await;
    assert_eq!(raw.status, StatusCode::OK);
    assert_eq!(raw.bytes, png);
    assert_eq!(raw.header("content-type"), "image/png");
    assert!(raw.header("content-disposition").starts_with("inline;"));
    assert_eq!(
        raw.header("content-security-policy"),
        "default-src 'none'; sandbox"
    );
    assert_eq!(raw.header("x-content-type-options"), "nosniff");
}

#[tokio::test]
async fn une_video_se_lit_par_morceaux() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let content: Vec<u8> = (0..1000u32).map(|i| (i % 256) as u8).collect();
    let id = upload(&mut alice, "clip.mp4", &content).await;

    let page = alice.get(&format!("/files/{id}")).await.body;
    assert!(page.contains(&format!(
        "<video controls playsinline preload=\"metadata\" src=\"/files/{id}/raw\""
    )));

    let whole = alice.get(&format!("/files/{id}/raw")).await;
    assert_eq!(whole.status, StatusCode::OK);
    assert_eq!(whole.header("accept-ranges"), "bytes");
    assert_eq!(whole.header("content-type"), "video/mp4");

    let part = alice
        .get_range(&format!("/files/{id}/raw"), "bytes=100-199")
        .await;
    assert_eq!(part.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(part.header("content-range"), "bytes 100-199/1000");
    assert_eq!(part.header("content-length"), "100");
    assert_eq!(part.bytes, content[100..200]);

    let tail = alice
        .get_range(&format!("/files/{id}/raw"), "bytes=990-")
        .await;
    assert_eq!(tail.bytes, content[990..]);

    let beyond = alice
        .get_range(&format!("/files/{id}/raw"), "bytes=5000-")
        .await;
    assert_eq!(beyond.status, StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(beyond.header("content-range"), "bytes */1000");
}

#[tokio::test]
async fn un_svg_ou_un_html_s_affiche_comme_du_texte() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    for (name, content) in [
        ("dessin.svg", "<svg onload=\"alert(1)\"></svg>"),
        ("page.html", "<script>alert(document.cookie)</script>"),
    ] {
        let id = upload(&mut alice, name, content.as_bytes()).await;
        let page = alice.get(&format!("/files/{id}")).await.body;
        assert!(page.contains("<pre class=\"file-preview-text\">"));
        assert!(page.contains("&lt;"), "{page}");
        assert!(!page.contains(content), "jamais tel quel dans la page");
        // Et jamais servi pour être affiché par le navigateur.
        assert_eq!(
            alice.get(&format!("/files/{id}/raw")).await.status,
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test]
async fn un_markdown_est_mis_en_forme_et_nettoye() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let id = upload(
        &mut alice,
        "cours.md",
        b"# Les VLAN\n\nUn **r\xc3\xa9seau** virtuel.\n\n<script>alert(1)</script>\n",
    )
    .await;
    let page = alice.get(&format!("/files/{id}")).await.body;
    assert!(page.contains("<h1"), "{page}");
    assert!(page.contains("<strong>réseau</strong>"));
    assert!(!page.contains("<script>alert(1)"));
}

#[tokio::test]
async fn un_long_texte_est_coupe() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    // 64 Ko au plus dans les tests : sous les 200 Ko de l'aperçu.
    let id = upload(&mut alice, "journal.log", &b"ligne\n".repeat(1000)).await;
    let page = alice.get(&format!("/files/{id}")).await.body;
    assert!(page.contains("ligne\nligne"));
    assert!(!page.contains("200 premiers Ko"));
}

#[tokio::test]
async fn un_pdf_attend_son_apercu() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let id = upload(&mut alice, "cours.pdf", b"%PDF-1.4").await;
    let page = alice.get(&format!("/files/{id}")).await.body;
    assert!(page.contains("Aperçu en préparation"));
    // Le PDF lui-même n'est jamais servi pour être affiché.
    assert_eq!(
        alice.get(&format!("/files/{id}/raw")).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn un_type_inconnu_n_a_pas_d_apercu() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let id = upload(&mut alice, "archive.zip", b"PK").await;
    let page = alice.get(&format!("/files/{id}")).await.body;
    assert!(page.contains("Pas d'aperçu pour ce type de fichier"));
    // Pas de vignette possible : marqué tout de suite.
    let thumb: i64 = sqlx::query_scalar("SELECT thumb FROM files WHERE id = ?")
        .bind(id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(thumb, -1);
}

#[tokio::test]
async fn l_apercu_d_un_autre_compte_est_introuvable() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let id = upload(&mut alice, "photo.png", &gradient_png(50, 50)).await;
    let mut bob = app.logged_in(BOB).await;
    for path in ["", "/raw", "/thumb", "/preview"] {
        assert_eq!(
            bob.get(&format!("/files/{id}{path}")).await.status,
            StatusCode::NOT_FOUND,
            "/files/{id}{path}"
        );
    }
}
