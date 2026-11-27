//! Les actions d'une ligne de l'onglet Fichiers (demandé le 15 novembre
//! 2026) : un seul bouton « … » par dossier, note ou fichier, qui ouvre un
//! menu dont chaque choix a son icône et son texte. Les choix mènent aux
//! mêmes actions qu'avant.

mod common;

use common::{ALICE, TestApp, between};

async fn id_of(app: &TestApp, table: &str, column: &str, name: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT id FROM {table} WHERE {column} = ?"))
        .bind(name)
        .fetch_one(&app.state.db)
        .await
        .unwrap_or_else(|_| panic!("« {name} » absent de {table}"))
}

/// La ligne d'un élément de la liste, de son nom à la fin de la ligne.
fn row<'a>(page: &'a str, name: &str) -> &'a str {
    let list = &page[page.find("class=\"file-list\"").expect("liste")..];
    between(list, &format!("<span>{name}</span>"), "</li>")
        .unwrap_or_else(|| panic!("« {name} » absent de la liste"))
}

/// Un choix de menu : son icône, puis son texte.
fn choice(icon: &str, label: &str) -> String {
    format!("#{icon}\"/></svg>{label}<")
}

/// Une ligne n'a plus qu'un bouton : « … ».
fn only_more_button(row: &str, menu: &str) {
    assert_eq!(row.matches("class=\"icon-btn\"").count(), 1, "{row}");
    assert!(row.contains(&format!("popovertarget=\"{menu}\"")), "{row}");
    assert!(row.contains("#more\"/></svg>"), "{row}");
}

#[tokio::test]
async fn un_menu_par_ligne_avec_icone_et_texte() {
    let app = TestApp::new().await;
    let mut alice = app.logged_in(ALICE).await;
    let perso = app.space_id(ALICE, "perso").await.to_string();
    for name in ["Vide", "Cours"] {
        alice
            .post("/folders", &[("name", name), ("space_id", &perso)])
            .await;
    }
    let (vide, cours) = (
        id_of(&app, "folders", "name", "Vide").await,
        id_of(&app, "folders", "name", "Cours").await,
    );
    alice.upload_in_folder("td.pdf", b"%PDF td", cours).await;
    alice.upload("rapport.pdf", b"%PDF rapport").await;
    let note = common::note_id(&alice.post("/notes", &[]).await);
    alice
        .post(
            &format!("/notes/{note}"),
            &[("title", "Idées"), ("body_md", "contenu")],
        )
        .await;
    let file = id_of(&app, "files", "name", "rapport.pdf").await;
    let page = alice.get("/files?dossier=espace").await.body;

    // Un fichier : télécharger, renommer, ranger, convertir, supprimer.
    let line = row(&page, "rapport.pdf");
    only_more_button(line, &format!("pm-actions-file-{file}"));
    let menu = between(
        line,
        &format!("id=\"pm-actions-file-{file}\""),
        &format!("id=\"pm-rename-file-{file}\""),
    )
    .unwrap();
    for (icon, label) in [
        ("download", "Télécharger"),
        ("pencil", "Renommer"),
        ("move", "Ranger ailleurs"),
        ("convert", "Convertir…"),
        ("trash", "Supprimer"),
    ] {
        assert!(menu.contains(&choice(icon, label)), "{label} : {menu}");
    }
    assert!(menu.contains(&format!("href=\"/files/{file}/download\"")));
    assert!(menu.contains(&format!("popovertarget=\"pm-rename-file-{file}\"")));
    assert!(menu.contains(&format!("hx-get=\"/destinations?fichier={file}")));
    assert!(menu.contains(&format!("popovertarget=\"pm-convert-{file}\"")));
    assert!(menu.contains(&format!("action=\"/files/{file}/delete\"")));
    assert!(menu.contains("class=\"pop-danger\""));
    // Renommer et convertir ont toujours leur fenêtre, dans la ligne.
    assert!(line.contains(&format!("id=\"pm-rename-file-{file}\"")));
    assert!(line.contains(&format!("id=\"pm-convert-{file}\"")));

    // Un dossier vide : renommer, déplacer, supprimer après confirmation.
    let line = row(&page, "Vide");
    only_more_button(line, &format!("pm-actions-folder-{vide}"));
    let menu = between(
        line,
        &format!("id=\"pm-actions-folder-{vide}\""),
        &format!("id=\"pm-rename-{vide}\""),
    )
    .unwrap();
    for (icon, label) in [
        ("pencil", "Renommer"),
        ("move", "Déplacer"),
        ("trash", "Supprimer"),
    ] {
        assert!(menu.contains(&choice(icon, label)), "{label} : {menu}");
    }
    assert!(menu.contains(&format!("action=\"/folders/{vide}/delete\"")));
    assert!(menu.contains("hx-confirm=\"Mettre le dossier vide « Vide » à la corbeille ?\""));
    // Un dossier plein : « Supprimer… » ouvre l'avertissement.
    let line = row(&page, "Cours");
    only_more_button(line, &format!("pm-actions-folder-{cours}"));
    assert!(line.contains(&choice("trash", "Supprimer…")));
    assert!(line.contains(&format!("popovertarget=\"pd-delete-{cours}\"")));
    assert!(line.contains(&format!("id=\"pd-delete-{cours}\"")));

    // Une note : la ranger ailleurs.
    let line = row(&page, "Idées");
    only_more_button(line, &format!("pm-actions-note-{note}"));
    assert!(line.contains(&choice("move", "Ranger ailleurs")));
    assert!(line.contains(&format!("hx-get=\"/destinations?note={note}")));

    // Dans Temporaire, le fichier se garde dans un dossier.
    alice.upload_to_tools("scan.pdf", b"%PDF scan").await;
    let scan = id_of(&app, "files", "name", "scan.pdf").await;
    let page = alice.get("/files?dossier=temporaire").await.body;
    let line = row(&page, "scan.pdf");
    only_more_button(line, &format!("pm-actions-file-{scan}"));
    assert!(line.contains(&choice("move", "Garder dans un dossier")));
}
