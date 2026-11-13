//! Notes : cycle de vie complet, recherche plein texte, tags, protection XSS.

mod common;

use axum::http::StatusCode;
use common::{ALICE, TestApp, between, note_id};

#[tokio::test]
async fn cycle_de_vie_d_une_note() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;

    // --- Création ---
    let created = alice.post("/notes", &[]).await;
    assert_eq!(created.status, StatusCode::SEE_OTHER);
    let id = note_id(&created);
    let url = format!("/notes/{id}");
    // Tant que la note est vierge, « Nouvelle note » la réutilise.
    assert_eq!(note_id(&alice.post("/notes", &[]).await), id);

    // --- Modification (sauvegarde automatique) ---
    let saved = alice
        .post(
            &url,
            &[
                ("title", "Révisions réseau"),
                (
                    "body_md",
                    "# VLAN\r\n\r\nLe **trunk** transporte plusieurs VLAN.",
                ),
            ],
        )
        .await;
    assert_eq!(saved.status, StatusCode::OK);
    assert!(saved.body.starts_with("Enregistré à "));

    let page = alice.get(&url).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("value=\"Révisions réseau\""));
    // Le texte arrive déjà mis en forme par le serveur, le temps que
    // l'éditeur se charge.
    assert!(page.body.contains("<h1>VLAN</h1>"));
    assert!(page.body.contains("<strong>trunk</strong>"));
    // Les retours à la ligne du formulaire (CRLF) sont stockés en LF.
    let body: String = sqlx::query_scalar("SELECT body_md FROM notes WHERE id = ?")
        .bind(id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(body, "# VLAN\n\nLe **trunk** transporte plusieurs VLAN.");

    // --- Tags ---
    let tags = alice
        .post(&format!("{url}/tags"), &[("name", "  #Réseau ")])
        .await;
    assert!(tags.body.contains("#réseau"));
    alice.post(&format!("{url}/tags"), &[("name", "bts")]).await;
    let list = alice.get("/notes?tag=bts").await;
    assert!(list.body.contains("Révisions réseau"));
    assert!(
        !alice
            .get("/notes?tag=inconnu")
            .await
            .body
            .contains("Révisions réseau")
    );

    // --- Recherche plein texte, avec extrait surligné ---
    let found = alice.get("/notes?q=trunk").await;
    assert!(found.body.contains("Révisions réseau"));
    assert!(found.body.contains("<mark>trunk</mark>"));
    // Préfixe et accents : « reseau » trouve « réseau », « tru » trouve « trunk ».
    assert!(
        alice
            .get("/notes?q=reseau")
            .await
            .body
            .contains("<mark>réseau</mark>")
    );
    assert!(
        alice
            .get("/notes?q=tru")
            .await
            .body
            .contains("<mark>trunk</mark>")
    );
    assert!(
        !alice
            .get("/notes?q=introuvable")
            .await
            .body
            .contains("Révisions réseau")
    );
    // /open, dans la barre de commandes, trouve la même note.
    assert!(
        alice
            .get_fragment("/cmd/suggest?q=trunk")
            .await
            .body
            .contains(&format!(r#""href":"/notes/{id}""#))
    );

    // --- Épinglage ---
    let pin = alice.post(&format!("{url}/pin"), &[]).await;
    assert!(pin.body.contains("aria-pressed=\"true\""));
    assert!(alice.get("/widgets/notes").await.body.contains("Épinglée"));

    // --- Déplacement vers l'espace École ---
    let ecole = app.space_id(ALICE, "ecole").await.to_string();
    let moved = alice
        .post(&format!("{url}/move"), &[("space_id", &ecole)])
        .await;
    let list = alice.follow(moved).await;
    assert!(list.body.contains("Note déplacée vers École<"));
    // Perso montre tous les espaces : la note y reste, marquée École ;
    // « Seulement Perso » ne la montre plus.
    assert!(list.body.contains("Révisions réseau</span>"));
    assert!(
        list.body
            .contains("<span class=\"dot\" aria-hidden=\"true\"></span>École</span>")
    );
    let only = alice.get("/notes?seul=1").await;
    assert!(
        !only.body.contains("Révisions réseau</span>"),
        "la note a quitté l'espace Perso"
    );
    assert!(
        !alice
            .get("/notes?q=trunk&seul=1")
            .await
            .body
            .contains("<mark>")
    );

    // Ouvrir la note bascule sur son espace ; elle y garde ses tags.
    let opened = alice.get(&url).await;
    let page = alice.follow(opened).await;
    assert!(page.body.contains("data-accent=\"blue\""));
    assert!(page.body.contains("#bts"));
    assert!(
        alice
            .get("/notes?q=trunk")
            .await
            .body
            .contains("<mark>trunk</mark>")
    );

    // --- Suppression ---
    let deleted = alice.post(&format!("{url}/delete"), &[]).await;
    assert!(alice.follow(deleted).await.body.contains("Note supprimée<"));
    assert_eq!(alice.get(&url).await.status, StatusCode::NOT_FOUND);
    assert!(!alice.get("/notes?q=trunk").await.body.contains("<mark>"));
    // L'index de recherche et les tags orphelins sont nettoyés.
    let (indexed, tags): (i64, i64) = (
        sqlx::query_scalar("SELECT COUNT(*) FROM notes_fts WHERE notes_fts MATCH 'trunk'")
            .fetch_one(&app.state.db)
            .await
            .unwrap(),
        sqlx::query_scalar("SELECT COUNT(*) FROM tags")
            .fetch_one(&app.state.db)
            .await
            .unwrap(),
    );
    assert_eq!((indexed, tags), (0, 0));
}

#[tokio::test]
async fn une_note_ne_peut_pas_injecter_de_script() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let id = note_id(&alice.post("/notes", &[]).await);
    let url = format!("/notes/{id}");

    let payload = concat!(
        "<script>alert('xss')</script>\n\n",
        "<img src=x onerror=\"alert(1)\">\n\n",
        "[lien](javascript:alert(2))\n\n",
        "<iframe src=\"https://evil.example\"></iframe>\n\n",
        "<a href=\"data:text/html,<script>alert(3)</script>\">data</a>\n\n",
        "```html\n</code></pre><script>alert(4)</script>\n```\n",
    );
    let title = "\"><script>alert('titre')</script>";
    alice
        .post(&url, &[("title", title), ("body_md", payload)])
        .await;
    alice
        .post(
            &format!("{url}/tags"),
            &[("name", "<script>alert('tag')</script>")],
        )
        .await;

    // Partout où la note apparaît : éditeur, liste, recherche, widget.
    let pages = [
        alice.get(&url).await,
        alice.get("/notes").await,
        alice.get("/notes?q=alert").await,
        alice.get("/widgets/notes").await,
    ];
    for (index, page) in pages.iter().enumerate() {
        assert_eq!(page.status, StatusCode::OK, "page {index}");
        // Les seules balises <script> de la page sont celles de dashu (avec src).
        for script in page.body.split("<script").skip(1) {
            assert!(
                script.trim_start().starts_with("src=\"/static/js/"),
                "page {index} : script injecté : {script:.80}"
            );
        }
        for forbidden in [
            "onerror=\"alert",
            "href=\"javascript:",
            "<iframe",
            "href=\"data:",
        ] {
            assert!(
                !page.body.contains(forbidden),
                "page {index} contient {forbidden}"
            );
        }
    }
    // Le contenu n'est pas perdu : l'éditeur le montre comme du texte, échappé.
    assert!(pages[0].body.contains("alert("));
    assert!(!pages[0].body.contains("<script>alert"));

    // /open (barre de commandes) répond en JSON, jamais lu comme du HTML : le
    // navigateur pose le titre comme du texte (textContent, app.js).
    let open = alice.get_fragment("/cmd/suggest?q=alert").await;
    assert_eq!(open.status, StatusCode::OK);
    assert_eq!(open.header("content-type"), "application/json");
    assert_eq!(open.header("x-content-type-options"), "nosniff");
    let json: serde_json::Value = serde_json::from_str(&open.body).unwrap();
    assert!(json["items"][0]["v"].as_str().unwrap().contains("<script>"));
}

/// L'éditeur (frontend/editor.js, ProseMirror) est une seule vue, comme une
/// page Notion : plus d'aperçu à côté. Il s'appuie sur la page que sert le
/// serveur : le formulaire et sa source Markdown, l'hôte `#editor` avec le
/// texte déjà mis en forme, et le sprite d'icônes du menu « / ».
#[tokio::test]
async fn l_editeur_est_une_seule_vue() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let id = note_id(&alice.post("/notes", &[]).await);
    let url = format!("/notes/{id}");

    // Markdown tel que l'écrit l'éditeur : titre, cases à cocher, tableau
    // (colonne centrée, « | » échappé), barré, lien et image.
    let body = concat!(
        "## Objectifs\n\n",
        "- [x] Créer les VLAN\n- [ ] Tester le ping\n\n",
        "| VLAN | Nom |\n| --- | :---: |\n| 10 | Admin \\| IT |\n\n",
        "Un ~~ancien~~ [lien](https://example.com) et ![schéma](/files/3/raw)",
    );
    alice
        .post(&url, &[("title", "Réseau"), ("body_md", body)])
        .await;
    let page = alice.get(&url).await;
    assert_eq!(page.status, StatusCode::OK);

    // Une seule vue : ni panneaux, ni onglets Écrire / Aperçu, ni aperçu
    // rafraîchi à chaque frappe.
    for gone in [
        "note-panes",
        "data-pane-tab",
        "/notes/preview",
        "id=\"preview\"",
    ] {
        assert!(!page.body.contains(gone), "{gone} ne devrait plus exister");
    }
    assert_ne!(
        alice
            .post("/notes/preview", &[("body_md", "# x")])
            .await
            .status,
        StatusCode::OK
    );

    // Ce que lit l'éditeur : le formulaire, la source Markdown, l'hôte.
    assert!(page.body.contains(r#"<form id="note-form""#));
    assert!(
        page.body
            .contains("<textarea name=\"body_md\" hidden>## Objectifs")
    );
    assert!(page.body.contains("data-note-scroll"));
    assert!(
        page.body
            .contains(r#"<div id="editor" class="editor" data-icons="/static/icons."#)
    );

    // En attendant le script, le texte est déjà mis en forme (et nettoyé).
    let html = between(&page.body, r#"<div id="editor""#, "</article>").unwrap();
    assert!(html.contains("<h2>Objectifs</h2>"));
    assert_eq!(html.matches(r#"type="checkbox""#).count(), 2);
    assert!(html.contains("<td>Admin | IT</td>"));
    assert!(html.contains("<del>ancien</del>"));
    assert!(html.contains(r#"href="https://example.com""#));
    assert!(html.contains(r#"src="/files/3/raw""#));

    // Le script de l'éditeur est servi, et n'a besoin ni d'eval ni de
    // Function : la CSP (script-src 'self') les refuserait.
    let script = page
        .body
        .split("<script src=\"")
        .filter_map(|s| s.split('"').next())
        .find(|src| src.starts_with("/static/js/editor."))
        .expect("script de l'éditeur");
    let js = alice.get(script).await;
    assert_eq!(js.status, StatusCode::OK);
    assert!(js.body.contains("ProseMirror"));
    assert!(!js.body.contains("eval(") && !js.body.contains("new Function"));

    // Les icônes du menu « / », de la barre de mise en forme et de celle des
    // tableaux (frontend/editor.js, frontend/editor/ui.js) sont dans le sprite.
    let sprite_url = between(&page.body, r#"data-icons=""#, "\"").unwrap();
    let sprite = alice.get(sprite_url).await;
    assert_eq!(sprite.status, StatusCode::OK);
    for name in [
        "b-text",
        "b-h1",
        "b-h2",
        "b-h3",
        "b-list",
        "b-olist",
        "tasks",
        "b-table",
        "b-quote",
        "b-hr",
        "code",
        "link",
        "image",
        "paperclip",
        "calendar",
        "bold",
        "italic",
        "strike",
        "code-inline",
        "pencil",
        "unlink",
        "row-add",
        "col-add",
        "row-del",
        "col-del",
        "trash",
        "notes",
        "file-blank",
    ] {
        assert!(
            sprite.body.contains(&format!(r#"<symbol id="{name}""#)),
            "icône {name} absente du sprite"
        );
    }
}

#[tokio::test]
async fn la_recherche_resiste_aux_saisies_tordues() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let id = note_id(&alice.post("/notes", &[]).await);
    alice
        .post(
            &format!("/notes/{id}"),
            &[("title", "Titre"), ("body_md", "contenu")],
        )
        .await;

    // Syntaxe FTS5, SQL, caractères spéciaux : jamais d'erreur, jamais d'injection.
    for q in [
        "\"",
        "*",
        "OR",
        "a OR (b",
        "NEAR(a b)",
        "title:x",
        "'; DROP TABLE notes; --",
        "%",
        "🙂",
        "-",
        "a\"b",
    ] {
        let encoded: String = form_urlencoded::byte_serialize(q.as_bytes()).collect();
        let list = alice.get(&format!("/notes?q={encoded}")).await;
        assert_eq!(list.status, StatusCode::OK, "recherche {q:?}");
        let open = alice
            .get_fragment(&format!("/cmd/suggest?q={encoded}"))
            .await;
        assert_eq!(open.status, StatusCode::OK, "/open {q:?}");
    }
    let notes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notes")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(notes, 1);
}

#[tokio::test]
async fn taches_du_jour() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;

    let widget = alice.post("/tasks", &[("title", "  Rendre le TP  ")]).await;
    assert!(widget.body.contains("Rendre le TP"));
    assert!(widget.body.contains("0 sur 1"));
    // Un titre vide n'ajoute rien.
    alice.post("/tasks", &[("title", "   ")]).await;

    let id: i64 = sqlx::query_scalar("SELECT id FROM tasks")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    let checked = alice.post(&format!("/tasks/{id}/toggle"), &[]).await;
    assert!(checked.body.contains("aria-checked=\"true\""));
    assert!(checked.body.contains("1 sur 1"));
    let unchecked = alice.post(&format!("/tasks/{id}/toggle"), &[]).await;
    assert!(unchecked.body.contains("aria-checked=\"false\""));

    let removed = alice.delete(&format!("/tasks/{id}")).await;
    assert!(removed.body.contains("Rien de prévu"));
    assert_eq!(
        alice.delete(&format!("/tasks/{id}")).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn pages_introuvables() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    for path in ["/notes/999999", "/notes/abc", "/nexiste-pas"] {
        let response = alice.get(path).await;
        assert!(response.status.is_client_error(), "{path}");
        assert!(
            response.body.contains("Page introuvable"),
            "{path} : {}",
            response.body
        );
    }
}
