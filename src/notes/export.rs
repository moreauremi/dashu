//! Télécharger une note, en Markdown ou en PDF (bouton « Télécharger » de
//! l'éditeur).
//!
//! * Markdown : le texte tel qu'il est enregistré, sous son titre en titre de
//!   niveau 1, comme l'export de Notion. dashu le sert lui-même.
//! * PDF : la note passe par le service de conversion, comme un fichier `.md`
//!   converti « En PDF » (pandoc et Typst, mise en page façon Notion). dashu
//!   attend le résultat et l'envoie aussitôt, sans le ranger dans les fichiers
//!   de l'espace (voir [`crate::tools::export_pdf`]). Deux options :
//!   la taille du texte (zoom de 80 à 150 %) et les titres gardés avec leur
//!   texte (un titre qui serait coupé de son paragraphe en bas d'une page
//!   passe en haut de la suivante, voir `src/tools/pdf/gabarit.typst`).
//!
//! On télécharge la version enregistrée : la page envoie d'abord les
//! modifications en attente (`static/js/app.js`, « Télécharger une note »).

use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use chrono::Datelike;
use serde::Deserialize;

use crate::AppState;
use crate::auth::session::Ctx;
use crate::files;
use crate::time;
use crate::tools;
use crate::web::{AppError, AppResult};

/// Longueur gardée du titre dans le nom du fichier téléchargé.
const NAME_MAX_CHARS: usize = 120;

/// Tailles du texte proposées pour le PDF, en pourcentage. Le serveur
/// refuse toute autre valeur : elle part telle quelle dans le gabarit Typst.
pub const ZOOMS: [u16; 6] = [80, 90, 100, 110, 125, 150];

/// Options du PDF, telles qu'envoyées par le formulaire de l'éditeur.
#[derive(Deserialize)]
pub struct PdfQuery {
    /// Taille du texte, une valeur de [`ZOOMS`] ; 100 sans précision.
    zoom: Option<u16>,
    /// Case « Garder les titres avec leur texte » : présente si cochée.
    titres: Option<String>,
}

/// Options du PDF, vérifiées.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PdfOptions {
    /// Taille du texte, en pourcentage (une valeur de [`ZOOMS`]).
    pub zoom: u16,
    /// Un titre ne reste pas seul en bas d'une page.
    pub keep_headings: bool,
}

impl Default for PdfOptions {
    fn default() -> Self {
        Self {
            zoom: 100,
            keep_headings: false,
        }
    }
}

impl PdfQuery {
    fn check(self) -> AppResult<PdfOptions> {
        let zoom = self.zoom.unwrap_or(100);
        if !ZOOMS.contains(&zoom) {
            return Err(AppError::BadRequest(
                "Taille du texte inconnue : choisis-en une dans la liste.".into(),
            ));
        }
        Ok(PdfOptions {
            zoom,
            keep_headings: self.titres.is_some(),
        })
    }
}

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
    markdown_download(&state, ctx.user.id, id).await
}

/// La note en Markdown, en téléchargement (aussi pour l'API).
pub(crate) async fn markdown_download(state: &AppState, user: i64, id: i64) -> AppResult<Response> {
    let note = load(state, user, id).await?;
    Ok(download(
        markdown_file(&note.title, &note.body_md).into_bytes(),
        "text/markdown; charset=utf-8",
        &file_name(&note.title, "md"),
    ))
}

/// `GET /notes/{id}/download.pdf?zoom=125&titres=1`. Sans service de
/// conversion (`WORK_DIR` absent), la page n'existe pas et l'éditeur ne
/// propose que le Markdown.
pub async fn pdf(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<PdfQuery>,
) -> AppResult<Response> {
    pdf_download(&state, ctx.user.id, id, query).await
}

/// La note en PDF, par le service de conversion (aussi pour l'API).
pub(crate) async fn pdf_download(
    state: &AppState,
    user: i64,
    id: i64,
    query: PdfQuery,
) -> AppResult<Response> {
    if state.config.work_dir.is_none() {
        return Err(AppError::NotFound);
    }
    let options = query.check()?;
    let note = load(state, user, id).await?;
    let date = time::paris_date(note.updated_at);
    let source = pdf_source(
        &note.title,
        &note.body_md,
        &format!("{} {}", time::day_month(date), date.year()),
        options,
    );
    let bytes = tools::export_pdf(state, source).await?;
    Ok(download(
        bytes,
        "application/pdf",
        &file_name(&note.title, "pdf"),
    ))
}

/// Type d'un fichier Word.
pub const DOCX_MIME: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

/// La note en Word, par le service de conversion (1Life › Audit, « Exporter
/// en Word ») : le texte seul, puisque le document d'audit porte déjà son
/// titre, et le document de référence Word s'il y en a un.
pub(crate) async fn docx_download(
    state: &AppState,
    user: i64,
    id: i64,
    reference: Option<Vec<u8>>,
) -> AppResult<Response> {
    let note = load(state, user, id).await?;
    let mut source = note.body_md;
    if !source.ends_with('\n') {
        source.push('\n');
    }
    let bytes = tools::export_document(state, source, tools::Conversion::Docx, reference).await?;
    Ok(download(bytes, DOCX_MIME, &file_name(&note.title, "docx")))
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

/// Markdown confié au service de conversion pour le PDF : un en-tête YAML,
/// lu par le gabarit (`src/tools/pdf/gabarit.typst`) et le filtre
/// (`images.lua`), puis le texte de la note. L'en-tête porte le titre et la
/// date de modification (en haut de la première page), le zoom s'il n'est
/// pas de 100 % (« 1.25 » : un nombre écrit par le serveur, jamais un texte
/// reçu) et `garder-titres`. Sans rien de tout cela, pas d'en-tête.
pub fn pdf_source(title: &str, body: &str, modified: &str, options: PdfOptions) -> String {
    let title = clean_title(title);
    let title = title.trim();
    let mut fields = String::new();
    if !title.is_empty() {
        fields.push_str(&format!("title: {}\n", yaml_text(title)));
        fields.push_str(&format!(
            "date: {}\n",
            yaml_text(&format!("Modifiée le {modified}"))
        ));
    }
    if options.zoom != 100 {
        let zoom = options.zoom;
        fields.push_str(&format!("zoom: '{}.{:02}'\n", zoom / 100, zoom % 100));
    }
    if options.keep_headings {
        fields.push_str("garder-titres: true\n");
    }
    let mut out = String::new();
    if !fields.is_empty() {
        out.push_str("---\n");
        out.push_str(&fields);
        out.push_str("---\n\n");
    }
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
        let simple = PdfOptions::default();
        let source = pdf_source("Cours", "# Partie 1\n", "17 novembre 2026", simple);
        assert_eq!(
            source,
            "---\ntitle: 'Cours'\ndate: 'Modifiée le 17 novembre 2026'\n---\n\n# Partie 1\n"
        );
        // Sans titre ni option : pas d'en-tête du tout.
        assert_eq!(pdf_source("", "Texte", "1 janvier 2026", simple), "Texte\n");

        // Le zoom, écrit par le serveur, et les titres gardés.
        let options = |zoom, keep_headings| PdfOptions {
            zoom,
            keep_headings,
        };
        assert_eq!(
            pdf_source("", "Texte", "", options(125, true)),
            "---\nzoom: '1.25'\ngarder-titres: true\n---\n\nTexte\n"
        );
        assert_eq!(
            pdf_source("", "Texte", "", options(80, false)),
            "---\nzoom: '0.80'\n---\n\nTexte\n"
        );
        assert!(pdf_source("C", "T", "1 mai", options(150, false)).contains("zoom: '1.50'\n"));
    }

    /// Le filtre du PDF ne laisse passer que les zooms qu'il connaît : il
    /// doit connaître tous ceux du formulaire.
    #[test]
    fn le_filtre_connait_chaque_zoom() {
        let filter = include_str!("../tools/pdf/images.lua");
        for zoom in ZOOMS.into_iter().filter(|z| *z != 100) {
            let value = format!("[\"{}.{:02}\"] = true", zoom / 100, zoom % 100);
            assert!(filter.contains(&value), "{value} absent de images.lua");
        }
    }

    #[test]
    fn options_du_pdf() {
        let query = |zoom: Option<u16>, titres: Option<&str>| PdfQuery {
            zoom,
            titres: titres.map(String::from),
        };
        // Sans rien : 100 %, titres libres.
        assert_eq!(query(None, None).check().unwrap(), PdfOptions::default());
        assert_eq!(
            query(Some(125), Some("1")).check().unwrap(),
            PdfOptions {
                zoom: 125,
                keep_headings: true
            }
        );
        for zoom in ZOOMS {
            assert!(query(Some(zoom), None).check().is_ok(), "{zoom}");
        }
        // Une valeur hors de la liste est refusée.
        for zoom in [0, 79, 101, 1000] {
            assert!(query(Some(zoom), None).check().is_err(), "{zoom}");
        }
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
