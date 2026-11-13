//! Notes Markdown : liste, recherche plein texte, éditeur, tags.
//!
//! Toutes les requêtes filtrent sur `user_id` : l'identifiant d'une note dans
//! l'URL ne suffit jamais à y accéder.

pub mod markdown;

use std::collections::HashMap;

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::AppState;
use crate::auth::session::{Ctx, Shell};
use crate::spaces::Space;
use crate::time;
use crate::web::{self, AppError, AppResult, escape, see_other};

pub(crate) const TITLE_MAX_CHARS: usize = 200;
const TAG_MAX_CHARS: usize = 40;
/// Marqueurs que SQLite place autour des mots trouvés ; remplacés par
/// `<mark>` après échappement du texte (voir `mark_matches`).
const MARK_OPEN: char = '\u{1}';
const MARK_CLOSE: char = '\u{2}';

// ---------------------------------------------------------------------
// Liste et recherche
// ---------------------------------------------------------------------

pub struct NoteItem {
    pub id: i64,
    /// Titre prêt à insérer (échappé, avec `<mark>` en cas de recherche).
    pub title_html: String,
    /// Extrait prêt à insérer (échappé, avec `<mark>` en cas de recherche).
    pub excerpt_html: String,
    pub pinned: bool,
    pub updated: String,
    pub tags: Vec<String>,
    /// Vue d'ensemble de Perso : l'espace d'une note venue d'ailleurs.
    pub space: Option<crate::spaces::SpaceTag>,
    space_id: i64,
}

pub struct TagChip {
    pub name: String,
    pub count: i64,
    pub active: bool,
    pub href: String,
}

#[derive(Template)]
#[template(path = "notes/list.html")]
struct ListPage {
    shell: Shell,
    q: String,
    tag: Option<String>,
    items: Vec<NoteItem>,
    chips: Vec<TagChip>,
    /// Lien qui retire le filtre par tag en gardant la recherche.
    clear_tag_href: String,
    /// Perso : filtre « Tous les espaces » / « Seulement Perso ».
    overview: bool,
    only: bool,
    all_href: String,
    only_href: String,
}

#[derive(Deserialize)]
pub struct ListQuery {
    q: Option<String>,
    tag: Option<String>,
    /// `1` : dans Perso, seulement ses propres notes.
    #[serde(default)]
    seul: u8,
}

/// `GET /notes?q=…&tag=…`
pub async fn list(
    ctx: Ctx,
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> AppResult<Response> {
    let q = query.q.unwrap_or_default().trim().to_string();
    let tag = query
        .tag
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    let only = query.seul == 1;
    // Perso montre les notes de tous les espaces, sauf « Seulement Perso ».
    let (user, space) = (ctx.user.id, ctx.scope(only));
    let now = time::now();

    let mut items = match fts_query(&q) {
        Some(fts) => search_in_space(&state.db, user, space, &fts, tag.as_deref(), now).await?,
        None => recent_in_space(&state.db, user, space, tag.as_deref(), now).await?,
    };

    // Tags de chaque note affichée (une seule requête pour toute la liste).
    let mut tags_by_note: HashMap<i64, Vec<String>> = HashMap::new();
    for row in sqlx::query!(
        r#"SELECT nt.note_id AS "note_id!", t.name AS "name!"
           FROM note_tags nt
           JOIN tags t ON t.id = nt.tag_id
           JOIN notes n ON n.id = nt.note_id
           WHERE n.user_id = ?1 AND (?2 IS NULL AND n.space_id NOT IN (SELECT id FROM spaces WHERE archived_at IS NOT NULL) OR n.space_id = ?2)
           ORDER BY t.name"#,
        user,
        space
    )
    .fetch_all(&state.db)
    .await?
    {
        tags_by_note.entry(row.note_id).or_default().push(row.name);
    }
    for item in &mut items {
        item.tags = tags_by_note.remove(&item.id).unwrap_or_default();
        item.space = ctx.tag_of(item.space_id);
    }

    let chips = sqlx::query!(
        r#"SELECT t.name AS "name!", COUNT(*) AS "count!: i64"
           FROM tags t
           JOIN note_tags nt ON nt.tag_id = t.id
           JOIN notes n ON n.id = nt.note_id
           WHERE t.user_id = ?1 AND (?2 IS NULL AND n.space_id NOT IN (SELECT id FROM spaces WHERE archived_at IS NOT NULL) OR n.space_id = ?2)
           GROUP BY t.id
           ORDER BY t.name"#,
        user,
        space
    )
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|row| {
        let active = tag
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case(&row.name));
        TagChip {
            // Recliquer sur le tag actif retire le filtre.
            href: list_url(&q, if active { None } else { Some(&row.name) }, only),
            name: row.name,
            count: row.count,
            active,
        }
    })
    .collect();

    let page = ListPage {
        shell: ctx.shell("notes").await?,
        clear_tag_href: list_url(&q, None, only),
        overview: ctx.active.is_overview(),
        only,
        all_href: list_url(&q, tag.as_deref(), false),
        only_href: list_url(&q, tag.as_deref(), true),
        q,
        tag,
        items,
        chips,
    };
    Ok(web::html(page))
}

/// `/notes?q=…&tag=…&seul=1` avec les paramètres encodés.
fn list_url(q: &str, tag: Option<&str>, only: bool) -> String {
    let mut params = form_urlencoded::Serializer::new(String::new());
    if !q.is_empty() {
        params.append_pair("q", q);
    }
    if let Some(tag) = tag {
        params.append_pair("tag", tag);
    }
    if only {
        params.append_pair("seul", "1");
    }
    let params = params.finish();
    if params.is_empty() {
        "/notes".into()
    } else {
        format!("/notes?{params}")
    }
}

/// Transforme la saisie de l'utilisateur en requête FTS5.
///
/// On ne garde que des mots (lettres et chiffres), chacun entre guillemets et
/// suivi de `*` : « reseau vl » cherche les notes qui contiennent un mot
/// commençant par « reseau » ET un mot commençant par « vl ». Aucun caractère
/// de la syntaxe FTS5 (guillemets, parenthèses, OR, NEAR…) ne peut donc venir
/// de la saisie.
pub fn fts_query(input: &str) -> Option<String> {
    let words: Vec<String> = input
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(12)
        .map(|w| format!("\"{w}\"*"))
        .collect();
    if words.is_empty() {
        None
    } else {
        Some(words.join(" "))
    }
}

/// Échappe un texte renvoyé par FTS5 puis transforme ses marqueurs en `<mark>`.
pub fn mark_matches(text: &str) -> String {
    escape(text)
        .replace(MARK_OPEN, "<mark>")
        .replace(MARK_CLOSE, "</mark>")
}

fn display_title(title: &str) -> String {
    if title.trim().is_empty() {
        "Sans titre".into()
    } else {
        escape(title)
    }
}

async fn recent_in_space(
    db: &SqlitePool,
    user: i64,
    space: Option<i64>,
    tag: Option<&str>,
    now: i64,
) -> AppResult<Vec<NoteItem>> {
    let rows = sqlx::query!(
        r#"SELECT n.id AS "id!", n.title AS "title!", substr(n.body_md, 1, 600) AS "head!: String",
                  n.pinned AS "pinned!: bool", n.updated_at AS "updated_at!: i64",
                  n.space_id AS "space_id!: i64"
           FROM notes n
           WHERE n.user_id = ?1 AND (?2 IS NULL AND n.space_id NOT IN (SELECT id FROM spaces WHERE archived_at IS NOT NULL) OR n.space_id = ?2)
             AND (?3 IS NULL OR EXISTS (
                   SELECT 1 FROM note_tags nt JOIN tags t ON t.id = nt.tag_id
                   WHERE nt.note_id = n.id AND t.name = ?3))
           ORDER BY n.pinned DESC, n.updated_at DESC
           LIMIT 300"#,
        user,
        space,
        tag
    )
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| NoteItem {
            id: row.id,
            title_html: display_title(&row.title),
            excerpt_html: escape(&markdown::excerpt(&row.head, 160)),
            pinned: row.pinned,
            updated: time::ago(row.updated_at, now),
            tags: Vec::new(),
            space: None,
            space_id: row.space_id,
        })
        .collect())
}

async fn search_in_space(
    db: &SqlitePool,
    user: i64,
    space: Option<i64>,
    fts: &str,
    tag: Option<&str>,
    now: i64,
) -> AppResult<Vec<NoteItem>> {
    // bm25 : pertinence ; un mot trouvé dans le titre pèse 5 fois plus.
    let rows = sqlx::query!(
        r#"SELECT n.id AS "id!", n.title AS "title!",
                  highlight(notes_fts, 0, char(1), char(2)) AS "title_marked!: String",
                  snippet(notes_fts, 1, char(1), char(2), '…', 24) AS "snippet!: String",
                  n.pinned AS "pinned!: bool", n.updated_at AS "updated_at!: i64",
                  n.space_id AS "space_id!: i64"
           FROM notes_fts
           JOIN notes n ON n.id = notes_fts.rowid
           WHERE notes_fts MATCH ?1 AND n.user_id = ?2 AND (?3 IS NULL AND n.space_id NOT IN (SELECT id FROM spaces WHERE archived_at IS NOT NULL) OR n.space_id = ?3)
             AND (?4 IS NULL OR EXISTS (
                   SELECT 1 FROM note_tags nt JOIN tags t ON t.id = nt.tag_id
                   WHERE nt.note_id = n.id AND t.name = ?4))
           ORDER BY bm25(notes_fts, 5.0, 1.0)
           LIMIT 50"#,
        fts,
        user,
        space,
        tag
    )
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| NoteItem {
            id: row.id,
            title_html: if row.title.trim().is_empty() {
                "Sans titre".into()
            } else {
                mark_matches(&row.title_marked)
            },
            excerpt_html: mark_matches(&markdown::plain(&row.snippet)),
            pinned: row.pinned,
            updated: time::ago(row.updated_at, now),
            tags: Vec::new(),
            space: None,
            space_id: row.space_id,
        })
        .collect())
}

// ---------------------------------------------------------------------
// Création
// ---------------------------------------------------------------------

#[derive(Template)]
#[template(path = "notes/new.html")]
struct NewPage {
    shell: Shell,
}

/// `GET /notes/new` : un GET ne doit rien modifier, la page envoie donc
/// elle-même le POST de création (avec son jeton CSRF) dès son affichage.
pub async fn new_page(ctx: Ctx) -> AppResult<Response> {
    Ok(web::html(NewPage {
        shell: ctx.shell("notes").await?,
    }))
}

/// `POST /notes` : crée une note vide dans l'espace actif et ouvre l'éditeur.
pub async fn create(ctx: Ctx, State(state): State<AppState>) -> AppResult<Response> {
    let id = create_note(&state.db, ctx.user.id, ctx.active.id, "").await?;
    Ok(see_other(&format!("/notes/{id}")))
}

/// Crée une note (titre éventuel) dans un espace de l'utilisateur et rend
/// son identifiant. Aussi appelée par la barre de commandes (`/add note`).
pub(crate) async fn create_note(
    db: &SqlitePool,
    user: i64,
    space: i64,
    title: &str,
) -> AppResult<i64> {
    let title: String = title.trim().chars().take(TITLE_MAX_CHARS).collect();

    // Une note encore vierge dans cet espace ? On la réutilise plutôt que
    // d'empiler des « Sans titre ».
    let blank = sqlx::query_scalar!(
        r#"SELECT id AS "id!" FROM notes WHERE user_id = ? AND space_id = ? AND title = '' AND body_md = '' LIMIT 1"#,
        user,
        space
    )
    .fetch_optional(db)
    .await?;

    let now = time::now();
    let id = match blank {
        Some(id) if title.is_empty() => id,
        Some(id) => {
            sqlx::query!(
                "UPDATE notes SET title = ?, updated_at = ? WHERE id = ? AND user_id = ?",
                title,
                now,
                id,
                user
            )
            .execute(db)
            .await?;
            id
        }
        None => sqlx::query!(
            "INSERT INTO notes (user_id, space_id, title, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
            user,
            space,
            title,
            now,
            now
        )
        .execute(db)
        .await?
        .last_insert_rowid(),
    };
    Ok(id)
}

// ---------------------------------------------------------------------
// Éditeur
// ---------------------------------------------------------------------

pub struct NoteTag {
    pub id: i64,
    pub name: String,
}

#[derive(Template)]
#[template(path = "notes/edit.html")]
struct EditPage {
    shell: Shell,
    id: i64,
    title: String,
    body_md: String,
    pinned: bool,
    /// Aperçu initial, déjà nettoyé par ammonia.
    preview_html: String,
    saved_label: String,
    tags: Vec<NoteTag>,
    /// Tags existants, proposés à la saisie.
    suggestions: Vec<String>,
    /// Espaces vers lesquels on peut déplacer la note.
    other_spaces: Vec<Space>,
}

/// `GET /notes/{id}`
pub async fn edit_page(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let note = sqlx::query!(
        r#"SELECT space_id, title, body_md, pinned AS "pinned!: bool", updated_at
           FROM notes WHERE id = ? AND user_id = ?"#,
        id,
        ctx.user.id
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;

    // Note ouverte depuis un autre espace (recherche globale, Perso) : elle
    // s'ouvre dans le sien, par l'adresse de cet espace.
    if note.space_id != ctx.active.id {
        let space = ctx.space_by_id(note.space_id).ok_or(AppError::NotFound)?;
        return Ok(see_other(&crate::spaces::url::prefixed(
            &space.slug,
            &format!("/notes/{id}"),
        )));
    }

    let page = EditPage {
        id,
        preview_html: markdown::render(&note.body_md),
        saved_label: format!(
            "Enregistré · modifié {}",
            time::ago(note.updated_at, time::now())
        ),
        title: note.title,
        body_md: note.body_md,
        pinned: note.pinned,
        tags: tags_of(&state.db, ctx.user.id, id).await?,
        suggestions: all_tag_names(&state.db, ctx.user.id).await?,
        other_spaces: ctx
            .spaces
            .iter()
            .filter(|s| s.id != ctx.active.id)
            .cloned()
            .collect(),
        shell: ctx.shell("notes").await?,
    };
    Ok(web::html(page))
}

#[derive(Deserialize)]
pub struct SaveForm {
    title: String,
    body_md: String,
}

/// `POST /notes/{id}` : sauvegarde automatique. Répond par le texte de
/// l'indicateur d'état.
pub async fn save(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<SaveForm>,
) -> AppResult<Response> {
    let title: String = form.title.trim().chars().take(TITLE_MAX_CHARS).collect();
    let body = normalize_newlines(&form.body_md);

    let current = sqlx::query!(
        "SELECT title, body_md FROM notes WHERE id = ? AND user_id = ?",
        id,
        ctx.user.id
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;

    let now = time::now();
    // Rien n'a changé : on ne touche pas à la date de modification.
    if current.title != title || current.body_md != body {
        sqlx::query!(
            "UPDATE notes SET title = ?, body_md = ?, updated_at = ? WHERE id = ? AND user_id = ?",
            title,
            body,
            now,
            id,
            ctx.user.id
        )
        .execute(&state.db)
        .await?;
    }
    Ok(Html(format!("Enregistré à {}", time::hm(now))).into_response())
}

#[derive(Deserialize)]
pub struct PreviewForm {
    body_md: String,
}

/// `POST /notes/preview` : rend le Markdown en cours de frappe.
pub async fn preview(_ctx: Ctx, axum::Form(form): axum::Form<PreviewForm>) -> Html<String> {
    Html(markdown::render(&normalize_newlines(&form.body_md)))
}

/// Un formulaire HTML envoie les retours à la ligne en CRLF : on stocke en LF.
pub(crate) fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n")
}

#[derive(Template)]
#[template(path = "notes/_pin.html")]
struct PinButton {
    id: i64,
    pinned: bool,
}

/// `POST /notes/{id}/pin` : épingle ou désépingle. Répond par le bouton à jour.
pub async fn toggle_pin(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let pinned = toggle_pinned(&state.db, ctx.user.id, id).await?;
    Ok(web::html(PinButton { id, pinned }))
}

/// Épingle ou désépingle une note de l'utilisateur ; rend le nouvel état.
/// Aussi appelée par la barre de commandes (`/pin`).
pub(crate) async fn toggle_pinned(db: &SqlitePool, user: i64, id: i64) -> AppResult<bool> {
    sqlx::query_scalar!(
        r#"UPDATE notes SET pinned = NOT pinned WHERE id = ? AND user_id = ? RETURNING pinned AS "pinned!: bool""#,
        id,
        user
    )
    .fetch_optional(db)
    .await?
    .ok_or(AppError::NotFound)
}

#[derive(Deserialize)]
pub struct MoveForm {
    pub(crate) space_id: i64,
}

/// `POST /notes/{id}/move` : déplace la note vers un autre espace.
pub async fn move_to_space(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<MoveForm>,
) -> AppResult<Response> {
    // L'espace cible doit appartenir à l'utilisateur.
    let target = ctx.space_by_id(form.space_id).ok_or(AppError::NotFound)?;
    let moved = sqlx::query!(
        "UPDATE notes SET space_id = ? WHERE id = ? AND user_id = ?",
        target.id,
        id,
        ctx.user.id
    )
    .execute(&state.db)
    .await?
    .rows_affected();
    if moved == 0 {
        return Err(AppError::NotFound);
    }
    ctx.flash_ok(format!("Note déplacée vers {}", target.name))
        .await?;
    Ok(see_other("/notes"))
}

/// `POST /notes/{id}/delete`
pub async fn delete(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let deleted = sqlx::query!(
        "DELETE FROM notes WHERE id = ? AND user_id = ?",
        id,
        ctx.user.id
    )
    .execute(&state.db)
    .await?
    .rows_affected();
    if deleted == 0 {
        return Err(AppError::NotFound);
    }
    prune_orphan_tags(&state.db, ctx.user.id).await?;
    ctx.flash_ok("Note supprimée").await?;
    Ok(see_other("/notes"))
}

// ---------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------

#[derive(Template)]
#[template(path = "notes/_tags.html")]
struct TagsFragment {
    id: i64,
    tags: Vec<NoteTag>,
    suggestions: Vec<String>,
}

#[derive(Deserialize)]
pub struct TagForm {
    name: String,
}

/// `POST /notes/{id}/tags`
pub async fn add_tag(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<TagForm>,
) -> AppResult<Response> {
    let user = ctx.user.id;
    ensure_owned(&state.db, user, id).await?;
    attach_tag(&state.db, user, id, &form.name).await?;
    tags_fragment(&state.db, user, id).await
}

/// Nom de tag tel qu'il est stocké : sans `#`, en minuscules, 40 caractères au plus.
pub(crate) fn normalize_tag(input: &str) -> String {
    input
        .trim()
        .trim_start_matches('#')
        .trim()
        .to_lowercase()
        .chars()
        .take(TAG_MAX_CHARS)
        .collect()
}

/// Pose un tag sur une note (créé au besoin). La note doit appartenir à
/// `user` : à vérifier avant l'appel. Un nom vide est ignoré.
pub(crate) async fn attach_tag(
    db: &SqlitePool,
    user: i64,
    note_id: i64,
    input: &str,
) -> AppResult<()> {
    let name = normalize_tag(input);
    if name.is_empty() {
        return Ok(());
    }
    sqlx::query!(
        "INSERT OR IGNORE INTO tags (user_id, name) VALUES (?, ?)",
        user,
        name
    )
    .execute(db)
    .await?;
    sqlx::query!(
        "INSERT OR IGNORE INTO note_tags (note_id, tag_id)
             SELECT ?, id FROM tags WHERE user_id = ? AND name = ?",
        note_id,
        user,
        name
    )
    .execute(db)
    .await?;
    Ok(())
}

/// `DELETE /notes/{id}/tags/{tag_id}`
pub async fn remove_tag(
    ctx: Ctx,
    State(state): State<AppState>,
    Path((id, tag_id)): Path<(i64, i64)>,
) -> AppResult<Response> {
    let user = ctx.user.id;
    detach_tag(&state.db, user, id, tag_id).await?;
    tags_fragment(&state.db, user, id).await
}

/// Retire un tag d'une note de l'utilisateur (404 si la note n'est pas à
/// lui). Aussi appelée par la barre de commandes (`/untag`).
pub(crate) async fn detach_tag(
    db: &SqlitePool,
    user: i64,
    note_id: i64,
    tag_id: i64,
) -> AppResult<()> {
    ensure_owned(db, user, note_id).await?;
    sqlx::query!(
        "DELETE FROM note_tags WHERE note_id = ? AND tag_id = ?",
        note_id,
        tag_id
    )
    .execute(db)
    .await?;
    prune_orphan_tags(db, user).await
}

async fn tags_fragment(db: &SqlitePool, user: i64, id: i64) -> AppResult<Response> {
    Ok(web::html(TagsFragment {
        id,
        tags: tags_of(db, user, id).await?,
        suggestions: all_tag_names(db, user).await?,
    }))
}

/// 404 si la note n'existe pas ou appartient à quelqu'un d'autre.
async fn ensure_owned(db: &SqlitePool, user: i64, id: i64) -> AppResult<()> {
    sqlx::query_scalar!(
        r#"SELECT id AS "id!" FROM notes WHERE id = ? AND user_id = ?"#,
        id,
        user
    )
    .fetch_optional(db)
    .await?
    .map(|_| ())
    .ok_or(AppError::NotFound)
}

pub(crate) async fn tags_of(db: &SqlitePool, user: i64, note_id: i64) -> AppResult<Vec<NoteTag>> {
    let tags = sqlx::query_as!(
        NoteTag,
        r#"SELECT t.id AS "id!", t.name AS "name!"
           FROM tags t JOIN note_tags nt ON nt.tag_id = t.id
           WHERE nt.note_id = ? AND t.user_id = ?
           ORDER BY t.name"#,
        note_id,
        user
    )
    .fetch_all(db)
    .await?;
    Ok(tags)
}

pub(crate) async fn all_tag_names(db: &SqlitePool, user: i64) -> AppResult<Vec<String>> {
    Ok(sqlx::query_scalar!(
        "SELECT name FROM tags WHERE user_id = ? ORDER BY name",
        user
    )
    .fetch_all(db)
    .await?)
}

/// Supprime les tags qui ne sont plus portés par aucune note.
async fn prune_orphan_tags(db: &SqlitePool, user: i64) -> AppResult<()> {
    sqlx::query!(
        "DELETE FROM tags WHERE user_id = ? AND NOT EXISTS (SELECT 1 FROM note_tags WHERE tag_id = tags.id)",
        user
    )
    .execute(db)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requete_fts_sans_syntaxe_injectable() {
        assert_eq!(
            fts_query("réseau vlan").as_deref(),
            Some("\"réseau\"* \"vlan\"*")
        );
        // Guillemets, opérateurs et parenthèses de FTS5 sont neutralisés.
        assert_eq!(
            fts_query("\"a\" OR (b) c*").as_deref(),
            Some("\"a\"* \"OR\"* \"b\"* \"c\"*")
        );
        assert_eq!(fts_query("   "), None);
        assert_eq!(fts_query("***"), None);
    }

    #[test]
    fn extrait_de_recherche_echappe() {
        let raw = format!("<b>{MARK_OPEN}mot{MARK_CLOSE}</b>");
        assert_eq!(mark_matches(&raw), "&lt;b&gt;<mark>mot</mark>&lt;/b&gt;");
    }

    #[test]
    fn url_de_liste() {
        assert_eq!(list_url("", None, false), "/notes");
        assert_eq!(
            list_url("a b", Some("école"), false),
            "/notes?q=a+b&tag=%C3%A9cole"
        );
        assert_eq!(list_url("", None, true), "/notes?seul=1");
    }
}
