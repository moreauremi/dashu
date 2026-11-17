//! Télécharger une note, en Markdown ou en PDF (bouton « Télécharger » de
//! l'éditeur).
//!
//! * Markdown : le texte tel qu'il est enregistré, sous son titre en titre de
//!   niveau 1, comme l'export de Notion. dashu le sert lui-même.
//! * PDF : la note passe par le service de conversion, comme un fichier `.md`
//!   converti « En PDF » (pandoc et Typst, mise en page façon Notion). dashu
//!   attend le résultat et l'envoie aussitôt, sans le ranger dans les fichiers
//!   de l'espace (voir [`crate::tools::export_pdf`]).
//!
//! On télécharge la version enregistrée : la page envoie d'abord les
//! modifications en attente (`static/js/app.js`, « Télécharger une note »).

use axum::extract::{Path, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use chrono::Datelike;

use crate::AppState;
use crate::auth::session::Ctx;
use crate::files;
use crate::time;
use crate::tools;
use crate::web::{AppError, AppResult};

/// Longueur gardée du titre dans le nom du fichier téléchargé.
const NAME_MAX_CHARS: usize = 120;

struct Note {
    title: String,
    body_md: String,
    updated_at: i64,
}

/// Une note d'un espace de l'utilisateur, ou 404.
async fn load(state: &AppState, user_id: i64, id: i64) -> AppResult<Note> {
    let note = sqlx::query!(
        "SELECT title, body_md, updated_at FROM notes
         WHERE id = ? AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?)",
        id,
        user_id
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(Note {
        title: note.title,
        body_md: note.body_md,
        updated_at: note.updated_at,
    })
}

/// `GET /notes/{id}/download.md`
pub async fn markdown(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let note = load(&state, ctx.user.id, id).await?;
    Ok(download(
        markdown_file(&note.title, &note.body_md).into_bytes(),
        "text/markdown; charset=utf-8",
        &file_name(&note.title, "md"),
    ))
}

/// `GET /notes/{id}/download.pdf`. Sans service de conversion (`WORK_DIR`
/// absent), la page n'existe pas et l'éditeur ne propose que le Markdown.
pub async fn pdf(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    if state.config.work_dir.is_none() {
        return Err(AppError::NotFound);
    }
    let note = load(&state, ctx.user.id, id).await?;
    let date = time::paris_date(note.updated_at);
    let source = pdf_source(
        &note.title,
        &note.body_md,
        &format!("{} {}", time::day_month(date), date.year()),
    );
    let bytes = tools::export_pdf(&state, source).await?;
    Ok(download(
        bytes,
        "application/pdf",
        &file_name(&note.title, "pdf"),
    ))
}

/// Réponse de téléchargement : jamais affichée comme une page (même en-têtes
/// qu'un fichier téléchargé, voir `files::attachment_response`).
fn download(bytes: Vec<u8>, mime: &'static str, name: &str) -> Response {
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    headers.insert(header::CONTENT_DISPOSITION, files::attachment(name));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(files::FILE_CSP),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}

/// Titre sans caractère de contrôle (un titre se tape sur une ligne, mais
/// rien n'empêche d'en envoyer un autre).
fn clean_title(title: &str) -> String {
    title.chars().filter(|c| !c.is_control()).collect()
}

/// Nom du fichier téléchargé : le titre de la note (« Sans titre » s'il est
/// vide), sans barre oblique (elle couperait le nom) ni point au début (un
/// fichier caché), puis l'extension.
pub fn file_name(title: &str, ext: &str) -> String {
    let base: String = clean_title(title)
        .chars()
        .map(|c| if matches!(c, '/' | '\\') { '-' } else { c })
        .take(NAME_MAX_CHARS)
        .collect();
    let base = base.trim().trim_start_matches('.').trim_start();
    let base = if base.is_empty() { "Sans titre" } else { base };
    format!("{base}.{ext}")
}

/// Contenu du fichier `.md` : le titre en `# Titre`, une ligne vide, puis le
/// texte de la note tel qu'il est enregistré. Finit par un retour à la ligne.
pub fn markdown_file(title: &str, body: &str) -> String {
    let title = clean_title(title);
    let title = title.trim();
    let mut out = String::with_capacity(title.len() + body.len() + 4);
    if !title.is_empty() {
        out.push_str("# ");
        out.push_str(title);
        out.push_str("\n\n");
    }
    out.push_str(body);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Markdown confié au service de conversion pour le PDF : un en-tête YAML
/// (titre et date de modification, que le gabarit met en haut de la première
/// page), puis le texte de la note.
pub fn pdf_source(title: &str, body: &str, modified: &str) -> String {
    let title = clean_title(title);
    let title = title.trim();
    let mut out = String::from("---\n");
    if !title.is_empty() {
        out.push_str(&format!("title: {}\n", yaml_text(title)));
        out.push_str(&format!(
            "date: {}\n",
            yaml_text(&format!("Modifiée le {modified}"))
        ));
    }
    out.push_str("---\n\n");
    out.push_str(body);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Un texte pour l'en-tête YAML, rendu tel quel dans le PDF. pandoc lit ces
/// valeurs comme du Markdown : chaque signe de ponctuation ASCII est donc
/// précédé d'une barre oblique inverse (un titre « *Brouillon* » reste écrit
/// avec ses étoiles). La valeur va entre apostrophes, où YAML ne transforme
/// rien, sauf l'apostrophe, doublée.
fn yaml_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2 + 2);
    out.push('\'');
    for c in text.chars().filter(|c| !c.is_control()) {
        if c.is_ascii_punctuation() {
            out.push('\\');
        }
        if c == '\'' {
            out.push_str("''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nom_du_fichier() {
        assert_eq!(file_name("Cours de réseau", "md"), "Cours de réseau.md");
        assert_eq!(file_name("", "pdf"), "Sans titre.pdf");
        assert_eq!(file_name("   ", "pdf"), "Sans titre.pdf");
        // Une barre oblique ne coupe pas le nom ; pas de fichier caché.
        assert_eq!(file_name("TCP/IP", "md"), "TCP-IP.md");
        assert_eq!(file_name("..\\x", "md"), "-x.md");
        assert_eq!(file_name(".bashrc", "md"), "bashrc.md");
        assert_eq!(file_name("Ligne\nsuivante", "md"), "Lignesuivante.md");
        // Un long titre est coupé, l'extension reste.
        let long = file_name(&"a".repeat(300), "pdf");
        assert_eq!(long.chars().count(), NAME_MAX_CHARS + 4);
        assert!(long.ends_with(".pdf"));
    }

    #[test]
    fn fichier_markdown() {
        assert_eq!(
            markdown_file("Cours", "Texte\n\n- [ ] relire"),
            "# Cours\n\nTexte\n\n- [ ] relire\n"
        );
        assert_eq!(markdown_file("", "Texte\n"), "Texte\n");
        assert_eq!(markdown_file("Vide", ""), "# Vide\n\n");
    }

    #[test]
    fn source_du_pdf() {
        let source = pdf_source("Cours", "# Partie 1\n", "17 novembre 2026");
        assert_eq!(
            source,
            "---\ntitle: 'Cours'\ndate: 'Modifiée le 17 novembre 2026'\n---\n\n# Partie 1\n"
        );
        // Sans titre : pas de titre ni de date en tête du PDF.
        assert_eq!(
            pdf_source("", "Texte", "1 janvier 2026"),
            "---\n---\n\nTexte\n"
        );
    }

    #[test]
    fn titre_lu_tel_quel() {
        // Ponctuation échappée pour Markdown, apostrophe doublée pour YAML,
        // pas de retour à la ligne qui sortirait de l'en-tête.
        assert_eq!(yaml_text("*Brouillon*"), r"'\*Brouillon\*'");
        assert_eq!(yaml_text("L'été : 2 $"), r"'L\''été \: 2 \$'");
        assert_eq!(yaml_text("a\n---\nb"), r"'a\-\-\-b'");
        assert_eq!(yaml_text(r"C:\temp"), r"'C\:\\temp'");
    }
}
