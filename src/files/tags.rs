//! Tags des fichiers (SPEC v2, « Page Fichiers ») : les mêmes tags que les
//! notes (table `tags`), posés et retirés depuis l'aperçu d'un fichier, et
//! qui filtrent la liste de l'onglet Fichiers.

use std::collections::HashMap;

use askama::Template;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::AppState;
use crate::auth::session::Ctx;
use crate::notes::{self, NoteTag};
use crate::web::{self, AppError, AppResult};

#[derive(Template)]
#[template(path = "files/_tags.html")]
pub(crate) struct TagsFragment {
    pub id: i64,
    pub tags: Vec<NoteTag>,
    /// Tags existants, proposés à la saisie.
    pub suggestions: Vec<String>,
}

#[derive(Deserialize)]
pub struct TagForm {
    name: String,
}

/// `POST /files/{id}/tags` : pose un tag (créé au besoin).
pub async fn add(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<TagForm>,
) -> AppResult<Response> {
    let user = ctx.user.id;
    ensure_owned(&state.db, user, id).await?;
    let name = notes::normalize_tag(&form.name);
    if !name.is_empty() {
        sqlx::query!(
            "INSERT OR IGNORE INTO tags (user_id, name) VALUES (?, ?)",
            user,
            name
        )
        .execute(&state.db)
        .await?;
        sqlx::query!(
            "INSERT OR IGNORE INTO file_tags (file_id, tag_id)
                 SELECT ?, id FROM tags WHERE user_id = ? AND name = ?",
            id,
            user,
            name
        )
        .execute(&state.db)
        .await?;
    }
    fragment(&state.db, user, id).await
}

/// `DELETE /files/{id}/tags/{tag_id}`
pub async fn remove(
    ctx: Ctx,
    State(state): State<AppState>,
    Path((id, tag_id)): Path<(i64, i64)>,
) -> AppResult<Response> {
    let user = ctx.user.id;
    ensure_owned(&state.db, user, id).await?;
    sqlx::query!(
        "DELETE FROM file_tags WHERE file_id = ? AND tag_id = ?",
        id,
        tag_id
    )
    .execute(&state.db)
    .await?;
    notes::prune_orphan_tags(&state.db, user).await?;
    fragment(&state.db, user, id).await
}

async fn fragment(db: &SqlitePool, user: i64, id: i64) -> AppResult<Response> {
    Ok(web::html(TagsFragment {
        id,
        tags: tags_of(db, user, id).await?,
        suggestions: notes::all_tag_names(db, user).await?,
    }))
}

/// 404 si le fichier n'existe pas ou appartient à quelqu'un d'autre.
async fn ensure_owned(db: &SqlitePool, user: i64, id: i64) -> AppResult<()> {
    sqlx::query_scalar!(
        r#"SELECT id AS "id!" FROM files WHERE id = ? AND user_id = ?"#,
        id,
        user
    )
    .fetch_optional(db)
    .await?
    .map(|_| ())
    .ok_or(AppError::NotFound)
}

/// Les tags d'un fichier, par nom.
pub(crate) async fn tags_of(db: &SqlitePool, user: i64, file_id: i64) -> AppResult<Vec<NoteTag>> {
    Ok(sqlx::query_as!(
        NoteTag,
        r#"SELECT t.id AS "id!", t.name AS "name!"
           FROM tags t JOIN file_tags ft ON ft.tag_id = t.id
           WHERE ft.file_id = ? AND t.user_id = ?
           ORDER BY t.name"#,
        file_id,
        user
    )
    .fetch_all(db)
    .await?)
}

/// Les tags de tous les fichiers d'un compte, en une requête : fichier ->
/// noms de ses tags.
pub(crate) async fn by_file(db: &SqlitePool, user: i64) -> AppResult<HashMap<i64, Vec<String>>> {
    let mut map: HashMap<i64, Vec<String>> = HashMap::new();
    for row in sqlx::query!(
        r#"SELECT ft.file_id AS "file_id!", t.name AS "name!"
           FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
           WHERE t.user_id = ?
           ORDER BY t.name"#,
        user
    )
    .fetch_all(db)
    .await?
    {
        map.entry(row.file_id).or_default().push(row.name);
    }
    Ok(map)
}
