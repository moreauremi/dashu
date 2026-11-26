//! Quota de stockage par compte (SPEC v2, « Quota de stockage ») : refus à
//! l'ouverture, octets comptés pendant la réception, revérification dans la
//! transaction, plafond baissé sous l'usage, API, jauge, et commande
//! `dashu quota`.

mod common;

use std::process::Command;

use axum::http::{Method, StatusCode};
use common::{ALICE, API_TOKEN, BOB, TestApp};
use dashu::config::DEFAULT_QUOTA_BYTES;
use serde_json::json;

/// Change le plafond d'un compte comme le fait `dashu quota`, sans lancer
/// le binaire.
async fn set_cap(app: &TestApp, email: &str, size: &str) {
    dashu::files::quota::command(
        &app.state.db,
        DEFAULT_QUOTA_BYTES,
        None,
        &["set", email, size],
    )
    .await
    .expect("plafond changé");
}

/// Octets comptés dans le quota d'un compte.
async fn used(app: &TestApp, email: &str) -> i64 {
    let user = app.user_id(email).await;
    dashu::files::quota::usage(&app.state.db, user, DEFAULT_QUOTA_BYTES)
        .await
        .unwrap()
        .used
}

async fn pending_uploads(app: &TestApp) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM uploads")
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

/// Fichiers partiels encore présents dans `.envois`.
fn partials(app: &TestApp) -> usize {
    std::fs::read_dir(app.files_dir.join(".envois"))
        .map(|dir| dir.count())
        .unwrap_or(0)
}

#[tokio::test]
async fn un_envoi_qui_depasse_le_plafond_est_refuse_tout_de_suite() {
    let app = TestApp::new().await;
    set_cap(&app, ALICE, "100K").await;
    let mut alice = app.logged_in(ALICE).await;
    alice.get("/files?dossier=espace").await;

    let sent = alice.upload("premier.bin", &[1u8; 60_000]).await;
    assert_eq!(sent.status, StatusCode::CREATED, "{}", sent.body);

    // Taille annoncée au-delà de la place restante : refus à l'ouverture,
    // avant le moindre octet.
    let refused = alice.start_upload("second.bin", 60_000).await;
    assert_eq!(refused.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(
        refused
            .body
            .contains("Espace plein : 58,5 Ko utilisés sur 100 Ko. Supprime des fichiers ou demande à l&#39;administrateur d&#39;augmenter ton plafond."),
        "{}",
        refused.body
    );
    assert_eq!(pending_uploads(&app).await, 0);
    assert_eq!(app.stored_files().len(), 1);
    assert_eq!(used(&app, ALICE).await, 60_000);

    // Le quota est celui d'Alice : Bob envoie sans gêne.
    let mut bob = app.logged_in(BOB).await;
    bob.get("/files?dossier=espace").await;
    let sent = bob.upload("bob.bin", &[2u8; 60_000]).await;
    assert_eq!(sent.status, StatusCode::CREATED, "{}", sent.body);
}

#[tokio::test]
async fn un_envoi_pile_a_la_limite_passe_un_octet_de_plus_non() {
    let app = TestApp::new().await;
    set_cap(&app, ALICE, "100K").await;
    let mut alice = app.logged_in(ALICE).await;
    alice.get("/files?dossier=espace").await;

    let sent = alice.upload("a.bin", &[1u8; 60_000]).await;
    assert_eq!(sent.status, StatusCode::CREATED, "{}", sent.body);
    // Exactement la place restante, en deux morceaux.
    let rest = vec![3u8; 100 * 1024 - 60_000];
    let sent = alice.upload_in_chunks("b.bin", &rest, 20_000).await;
    assert_eq!(sent.status, StatusCode::CREATED, "{}", sent.body);
    assert_eq!(used(&app, ALICE).await, 100 * 1024);

    let refused = alice.upload("c.bin", &[0u8]).await;
    assert_eq!(refused.status, StatusCode::PAYLOAD_TOO_LARGE);
    // Un fichier vide tient toujours, même plein.
    let empty = alice.upload("vide.txt", &[]).await;
    assert_eq!(empty.status, StatusCode::CREATED, "{}", empty.body);
}

#[tokio::test]
async fn les_octets_sont_comptes_pendant_la_reception() {
    let app = TestApp::new().await;
    set_cap(&app, ALICE, "80K").await;
    let mut alice = app.logged_in(ALICE).await;
    alice.get("/files?dossier=espace").await;

    // Deux envois ouverts ensemble : chacun tient seul, pas les deux.
    let first = alice.start_upload("un.bin", 50_000).await.body;
    let second = alice.start_upload("deux.bin", 50_000).await.body;
    let done = alice.put_chunk(&first, 0, &[1u8; 50_000], true).await;
    assert_eq!(done.status, StatusCode::CREATED, "{}", done.body);

    // Le second dépasse en cours de route, sans Content-Length : coupé dès
    // le plafond franchi, fichier partiel et envoi effacés.
    let cut = alice.put_chunk(&second, 0, &[2u8; 50_000], false).await;
    assert_eq!(cut.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", cut.body);
    assert!(cut.body.contains("Espace plein"), "{}", cut.body);
    assert_eq!(pending_uploads(&app).await, 0);
    assert_eq!(partials(&app), 0, "le fichier partiel est supprimé");
    assert_eq!(app.stored_files().len(), 1);
    assert_eq!(used(&app, ALICE).await, 50_000);
    // L'envoi coupé n'existe plus : il ne reprend pas.
    let again = alice.put_chunk(&second, 0, &[2u8; 10], true).await;
    assert_eq!(again.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn deux_envois_en_parallele_ne_depassent_pas_le_plafond() {
    let app = TestApp::new().await;
    set_cap(&app, ALICE, "80K").await;
    let mut one = app.logged_in(ALICE).await;
    let mut two = app.logged_in(ALICE).await;
    one.get("/files?dossier=espace").await;
    two.get("/files?dossier=espace").await;

    // Ouverts tous les deux (l'usage est nul), puis envoyés en même temps.
    let a = one.start_upload("a.bin", 50_000).await.body;
    let b = two.start_upload("b.bin", 50_000).await.body;
    let (ra, rb) = tokio::join!(
        one.put_chunk(&a, 0, &[1u8; 50_000], true),
        two.put_chunk(&b, 0, &[2u8; 50_000], true),
    );
    let mut statuses = [ra.status, rb.status];
    statuses.sort();
    assert_eq!(
        statuses,
        [StatusCode::CREATED, StatusCode::PAYLOAD_TOO_LARGE],
        "{} / {}",
        ra.body,
        rb.body
    );
    assert_eq!(used(&app, ALICE).await, 50_000);
    assert_eq!(app.stored_files().len(), 1);
    assert_eq!(pending_uploads(&app).await, 0);
    assert_eq!(partials(&app), 0);
}

#[tokio::test]
async fn un_plafond_baisse_sous_l_usage_ne_supprime_rien() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.get("/files?dossier=espace").await;
    let sent = alice.upload("gros.bin", &[1u8; 60_000]).await;
    assert_eq!(sent.status, StatusCode::CREATED, "{}", sent.body);
    let tool = alice.upload_to_tools("outil.bin", &[4u8; 1_000]).await;
    assert_eq!(tool.status, StatusCode::CREATED, "{}", tool.body);
    let tool: i64 = tool.header("x-file-id").parse().unwrap();

    set_cap(&app, ALICE, "10K").await;

    // Rien n'est supprimé : le fichier reste listé et se télécharge.
    let page = alice.get("/files?dossier=espace").await.body;
    assert!(page.contains("gros.bin"));
    assert_eq!(app.stored_files().len(), 2);
    // La jauge passe en alerte.
    assert!(page.contains("class=\"quota is-alert\""), "{page}");
    assert!(page.contains("58,5 Ko / 10 Ko"), "{page}");

    // Les nouveaux envois, eux, sont refusés.
    let refused = alice.upload("petit.txt", b"x").await;
    assert_eq!(refused.status, StatusCode::PAYLOAD_TOO_LARGE);
    // Un fichier de l'onglet Outils ne compte pas tant qu'il n'est pas
    // gardé ; le garder le fait entrer dans le quota, donc refusé ici.
    let perso = app.space_id(ALICE, "perso").await;
    let kept = alice
        .post(
            &format!("/files/{tool}/keep"),
            &[("space_id", &perso.to_string())],
        )
        .await;
    assert_eq!(kept.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", kept.body);
    assert_eq!(used(&app, ALICE).await, 60_000);

    // La corbeille compte jusqu'à la suppression définitive.
    let id: i64 = sqlx::query_scalar("SELECT id FROM files WHERE name = 'gros.bin'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    alice.post(&format!("/files/{id}/delete"), &[]).await;
    assert_eq!(used(&app, ALICE).await, 60_000);
    alice
        .post(&format!("/settings/trash/fichier/{id}/erase"), &[])
        .await;
    assert_eq!(used(&app, ALICE).await, 0);
    let sent = alice.upload("petit.txt", b"x").await;
    assert_eq!(sent.status, StatusCode::CREATED, "{}", sent.body);
}

#[tokio::test]
async fn la_jauge_montre_l_usage_sans_rien_pour_le_changer() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let page = alice.get("/files").await.body;
    assert!(
        page.contains("id=\"quota-gauge\" class=\"quota\""),
        "{page}"
    );
    assert!(page.contains("0 o / 30 Go"), "{page}");
    assert!(!page.contains("is-alert"));
    // Aucun formulaire ni bouton du plafond, ni ici ni dans les réglages
    // (Alice est l'administratrice de l'instance).
    assert!(!page.contains("<form class=\"quota") && !page.contains("name=\"quota"));
    for path in ["/settings", "/settings/instance"] {
        let body = alice.get(path).await.body;
        assert!(!body.to_lowercase().contains("quota"), "{path}");
    }
}

#[tokio::test]
async fn l_api_respecte_le_meme_quota() {
    let app = TestApp::new().await;
    set_cap(&app, ALICE, "10K").await;
    let (status, body) = app
        .api(
            Method::POST,
            "/api/v1/files/uploads",
            Some(json!({ "name": "rapport.pdf", "size": 20_000, "space": "perso" })),
            Some(API_TOKEN),
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert!(body.to_string().contains("Espace plein"), "{body}");

    // Annonce sous la place, contenu au-delà : coupé pendant la réception.
    let (status, started) = app
        .api(
            Method::POST,
            "/api/v1/files/uploads",
            Some(json!({ "name": "menteur.pdf", "size": 9_000, "space": "perso" })),
            Some(API_TOKEN),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{started}");
    set_cap(&app, ALICE, "4K").await;
    let id = started["upload"].as_str().unwrap();
    let (status, _, _) = app
        .api_bytes(
            Method::PUT,
            &format!("/api/v1/files/uploads/{id}?offset=0"),
            Some(vec![0u8; 9_000]),
            Some(API_TOKEN),
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(pending_uploads(&app).await, 0);
    assert!(app.stored_files().is_empty());
}

/// Lance `dashu quota …` sur la base de l'application de test, dans un
/// dossier vide (pas de `.env` lu par erreur).
fn dashu_quota(app: &TestApp, args: &[&str]) -> (i32, String, String) {
    let here = app.work_dir.join("cli");
    std::fs::create_dir_all(&here).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dashu"))
        .arg("quota")
        .args(args)
        .current_dir(&here)
        .env("DATABASE_URL", &app.state.config.database_url)
        .env("FILES_QUOTA", "30G")
        .env_remove("FILES_DIR")
        .output()
        .expect("lancement du binaire");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

async fn own_cap(app: &TestApp, email: &str) -> Option<i64> {
    sqlx::query_scalar("SELECT quota_bytes FROM users WHERE email = ?")
        .bind(email)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

async fn journal(app: &TestApp) -> Vec<(String, Option<i64>, Option<i64>)> {
    sqlx::query_as("SELECT email, before_bytes, after_bytes FROM quota_changes ORDER BY id")
        .fetch_all(&app.state.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn quota_set_refuse_les_tailles_invalides_et_les_comptes_inconnus() {
    let app = TestApp::new().await;
    for (size, why) in [
        ("-5G", "positive"),
        ("0G", "nul"),
        ("0", "K, M, G ou T"),
        ("50", "K, M, G ou T"),
        ("douze", "K, M, G ou T"),
        ("50X", "K, M, G ou T"),
    ] {
        let (code, out, err) = dashu_quota(&app, &["set", ALICE, size]);
        assert_eq!(code, 1, "{size} : {out}{err}");
        assert!(err.contains(why), "{size} : {err}");
    }
    let (code, _, err) = dashu_quota(&app, &["set", "personne@example.test", "50G"]);
    assert_eq!(code, 1);
    assert!(err.contains("aucun compte personne@example.test"), "{err}");
    let (code, _, err) = dashu_quota(&app, &["show", "personne@example.test"]);
    assert_eq!(code, 1, "{err}");
    // Rien n'a changé, rien n'est journalisé.
    assert_eq!(own_cap(&app, ALICE).await, None);
    assert!(journal(&app).await.is_empty());

    // Commande mal formée : l'aide, code 2.
    for args in [&[][..], &["set", ALICE][..], &["augmente", ALICE][..]] {
        let (code, _, err) = dashu_quota(&app, args);
        assert_eq!(code, 2, "{args:?}");
        assert!(err.contains("dashu quota set <email> <taille>"), "{err}");
    }
}

#[tokio::test]
async fn quota_set_show_reset_et_list() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    alice.get("/files?dossier=espace").await;
    alice.upload("a.bin", &[1u8; 2048]).await;

    let (code, out, err) = dashu_quota(&app, &["set", "Alice@Example.test", "50G"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("Plafond de alice@example.test : 30 Go (défaut) → 50 Go"),
        "{out}"
    );
    assert_eq!(own_cap(&app, ALICE).await, Some(50 * 1024 * 1024 * 1024));

    let (code, out, _) = dashu_quota(&app, &["show", ALICE]);
    assert_eq!(code, 0);
    assert!(out.contains("Utilisé  : 2 Ko (2048 octets)"), "{out}");
    assert!(out.contains("Plafond  : 50 Go"), "{out}");
    assert!(out.contains("défaut → 50 Go"), "{out}");

    let (code, out, _) = dashu_quota(&app, &["list"]);
    assert_eq!(code, 0);
    assert!(out.contains(ALICE) && out.contains(BOB), "{out}");
    assert!(out.contains("30 Go (défaut)"), "{out}");
    assert!(out.contains("Plafonds attribués : 80 Go au total"), "{out}");
    assert!(out.contains("FILES_DIR absent"), "{out}");

    let (code, out, _) = dashu_quota(&app, &["reset", ALICE]);
    assert_eq!(code, 0);
    assert!(out.contains("50 Go → 30 Go (défaut)"), "{out}");
    assert_eq!(own_cap(&app, ALICE).await, None);

    // Journal : date, compte, avant et après (NULL : valeur par défaut).
    assert_eq!(
        journal(&app).await,
        vec![
            (ALICE.to_string(), None, Some(50 * 1024 * 1024 * 1024)),
            (ALICE.to_string(), Some(50 * 1024 * 1024 * 1024), None),
        ]
    );
}

#[tokio::test]
async fn un_plafond_trop_grand_pour_le_disque_donne_un_avertissement() {
    let app = TestApp::new().await;
    std::fs::create_dir_all(&app.files_dir).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dashu"))
        .args(["quota", "set", ALICE, "900000T"])
        .current_dir(std::env::temp_dir())
        .env("DATABASE_URL", &app.state.config.database_url)
        .env("FILES_DIR", &app.files_dir)
        .output()
        .unwrap();
    let out = String::from_utf8(output.stdout).unwrap();
    assert!(output.status.success(), "{out}");
    // `df` n'existe pas partout (Windows) : l'avertissement n'est vérifié
    // que là où la place du disque se lit.
    if !out.contains("illisible") && cfg!(unix) {
        assert!(out.contains("Attention : les plafonds"), "{out}");
    }
    // Avertir n'empêche rien.
    assert_eq!(own_cap(&app, ALICE).await, Some(900_000 * 1024_i64.pow(4)));
}

/// Garde-fou : le plafond ne s'écrit qu'à un seul endroit, la commande. Aucune
/// route du site, de l'API ou du connecteur ne doit pouvoir le changer.
#[test]
fn le_plafond_ne_s_ecrit_que_dans_la_commande() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs" || e == "html") {
                out.push(path);
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut sources = Vec::new();
    walk(&root.join("src"), &mut sources);
    walk(&root.join("templates"), &mut sources);
    let writers: Vec<String> = sources
        .iter()
        .filter(|path| {
            let text = std::fs::read_to_string(path).unwrap();
            text.contains("SET quota_bytes") || text.contains("quota_bytes =")
        })
        .map(|path| path.strip_prefix(root).unwrap().display().to_string())
        .collect();
    assert_eq!(
        writers.len(),
        1,
        "écritures de users.quota_bytes : {writers:?}"
    );
    assert!(
        writers[0]
            .replace('\\', "/")
            .ends_with("src/files/quota.rs")
    );
}
