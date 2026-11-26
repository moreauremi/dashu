//! Démo statique de dashu.dev/demo (src/demo) : l'export produit les pages
//! attendues, sans rien de réel ni de privé dedans.
//!
//! Seulement avec la feature : `cargo test --features demo --test demo`.

#![cfg(feature = "demo")]

use std::path::{Path, PathBuf};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn export_de_la_demo() {
    let out = std::env::temp_dir().join(format!("dashu-test-demo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    dashu::demo::export(&out, "/demo")
        .await
        .expect("export de la démo");

    // Les pages principales de chaque espace.
    for page in [
        "index.html",
        "e/perso/index.html",
        "e/perso/notes/index.html",
        "e/ecole/index.html",
        "e/ecole/notes/index.html",
        "e/ecole/calendar/index.html",
        "e/ecole/files/index.html",
        "e/taff/notes/index.html",
        "e/taff/tools/index.html",
        "e/perso/settings/index.html",
    ] {
        assert!(out.join(page).is_file(), "page absente : {page}");
    }

    let mut files = Vec::new();
    walk(&out, &mut files);
    let mut pages = 0;
    for file in &files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        let name = file.display();
        // Rien qui mène au serveur de Rémi, à son réseau ou à un vrai compte.
        for forbidden in [
            "app.dashu.dev",
            "tailscale",
            "ts.net",
            // Le mot « Cloudflare » seul figure dans un commentaire public de
            // files.js ; ce sont les noms d'hôte de tunnel qui sont interdits.
            "cfargotunnel",
            "trycloudflare",
            "@gmail.com",
            "192.168.",
        ] {
            assert!(
                !text.to_lowercase().contains(forbidden),
                "« {forbidden} » dans {name}"
            );
        }
        if file.extension().is_some_and(|e| e == "html") && text.contains("id=\"app\"") {
            pages += 1;
            assert!(
                text.contains("class=\"demo-banner\""),
                "bandeau absent : {name}"
            );
            assert!(
                text.contains("data-demo-reset"),
                "bouton de remise à zéro absent : {name}"
            );
            assert!(
                !text.contains("data-live-url"),
                "notes en direct dans {name}"
            );
            assert!(!text.contains("/1life"), "1Life dans {name}");
            assert!(
                text.contains("\"X-CSRF-Token\":\"demo\""),
                "jeton CSRF réel dans {name}"
            );
            // Toutes les adresses locales passent par /demo (l'espace devant
            // écarte `data-demo-hx-get`, l'adresse neutralisée exprès).
            for attr in [" href=\"/", " src=\"/", " hx-get=\"/"] {
                for (at, _) in text.match_indices(attr) {
                    let rest = &text[at + attr.len() - 1..];
                    assert!(
                        rest.starts_with("/demo/"),
                        "adresse hors de /demo dans {name} : {}",
                        &rest[..rest.len().min(60)]
                    );
                }
            }
        }
    }
    assert!(pages > 30, "trop peu de pages exportées : {pages}");

    // Des données des fixtures, rendues par les vrais gabarits.
    let notes = std::fs::read_to_string(out.join("e/taff/notes/index.html")).unwrap();
    assert!(
        notes.contains("Menuiserie Valtaine"),
        "notes de Taff sans les clients fictifs"
    );
    let _ = std::fs::remove_dir_all(&out);
}
