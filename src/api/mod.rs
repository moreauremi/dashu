//! API JSON pour un programme (par exemple un assistant personnel).
//!
//! Toutes les routes sont sous `/api/v1` et exigent l'en-tête
//! `Authorization: Bearer <API_TOKEN>`. L'API agit au nom d'un seul compte,
//! `API_USER_EMAIL`. Sans `API_TOKEN`, elle est désactivée et répond 404.
//!
//! Pas de session ni de cookie ici, donc pas de protection CSRF à prévoir : un
//! site tiers peut faire envoyer un cookie par le navigateur, pas un en-tête
//! `Authorization`. À l'inverse, un cookie de session valide n'ouvre pas l'API.
//!
//! Ce que l'API permet : lire et gérer les tâches, créer, lire, chercher et
//! compléter les notes (pas les supprimer), lire l'agenda. Elle ne touche ni
//! au mot de passe, ni aux sources iCal, ni aux réglages.
//!
//! Les erreurs sont toujours du JSON : `{"error": "message en français"}`.

use std::collections::HashMap;

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{FromRequestParts, Path, Query, Request, State};
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use axum::middleware::{Next, from_fn};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Duration, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::SqlitePool;

use crate::AppState;
use crate::calendar::ical;
use crate::notes::{self, markdown};
use crate::spaces::{self, Space};
use crate::time;
use crate::web::AppError;

const TASK_MAX_CHARS: usize = 200;
/// Une note de plus de 200 000 caractères n'a rien à faire dans dashu.
const BODY_MAX_CHARS: usize = 200_000;
const MAX_TAGS_PER_REQUEST: usize = 20;
/// Plus longue période d'agenda renvoyée en une fois.
const CALENDAR_MAX_DAYS: i64 = 62;

pub fn router() -> Router<AppState> {
    let v1 = Router::new()
        .route("/spaces", get(list_spaces))
        .route("/tasks", get(list_tasks).post(create_task))
        .route(
            "/tasks/{id}",
            get(get_task).patch(update_task).delete(delete_task),
        )
        .route("/notes", get(list_notes).post(create_note))
        .route("/notes/{id}", get(get_note).patch(update_note))
        .route("/calendar", get(calendar))
        .route("/calendar/refresh", post(refresh_calendar))
        .fallback(unknown_route)
        .layer(from_fn(json_errors));
    Router::new().nest("/api/v1", v1)
}

// ---------------------------------------------------------------------
// Erreurs
// ---------------------------------------------------------------------

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    fn not_found(what: &str) -> Self {
        Self::new(StatusCode::NOT_FOUND, format!("{what} introuvable"))
    }

    fn internal(error: anyhow::Error) -> Self {
        // Le détail part dans les logs, jamais dans la réponse.
        tracing::error!(error = format!("{error:#}"), "erreur interne (API)");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "erreur interne, réessaie dans un instant",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.status, Json(json!({ "error": self.message }))).into_response();
        if self.status == StatusCode::UNAUTHORIZED {
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                header::HeaderValue::from_static("Bearer"),
            );
        }
        response
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        Self::internal(error.into())
    }
}

impl From<AppError> for ApiError {
    fn from(error: AppError) -> Self {
        match error {
            AppError::NotFound => Self::new(StatusCode::NOT_FOUND, "introuvable"),
            AppError::Forbidden => Self::new(StatusCode::FORBIDDEN, "action refusée"),
            AppError::BadRequest(message) => Self::bad_request(message),
            AppError::TooLarge(message) => Self::new(StatusCode::PAYLOAD_TOO_LARGE, message),
            AppError::Internal(error) => Self::internal(error),
        }
    }
}

impl From<JsonRejection> for ApiError {
    fn from(rejection: JsonRejection) -> Self {
        Self::bad_request(format!(
            "corps JSON invalide (en-tête Content-Type: application/json attendu) : {}",
            rejection.body_text()
        ))
    }
}

impl From<QueryRejection> for ApiError {
    fn from(rejection: QueryRejection) -> Self {
        Self::bad_request(format!("paramètres invalides : {}", rejection.body_text()))
    }
}

type ApiResult<T> = Result<T, ApiError>;

async fn unknown_route() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "route d'API inconnue")
}

/// Middleware : les refus produits par axum lui-même (identifiant non
/// numérique dans l'URL…) sont en texte brut ; on les remet en JSON.
async fn json_errors(request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    let is_text = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/plain"));
    if !(response.status().is_client_error() && is_text) {
        return response;
    }
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap_or_default();
    ApiError::new(status, String::from_utf8_lossy(&bytes).into_owned()).into_response()
}

// ---------------------------------------------------------------------
// Authentification
// ---------------------------------------------------------------------

/// Compte au nom duquel agit la requête, et ses espaces.
pub struct ApiUser {
    pub id: i64,
    pub spaces: Vec<Space>,
}

impl ApiUser {
    /// Espace désigné par son identifiant court : `perso`, `ecole` ou `taff`.
    /// « École » (avec accent ou majuscule) est accepté aussi.
    fn space(&self, slug: &str) -> ApiResult<&Space> {
        let wanted = slug.trim().to_lowercase().replace('é', "e");
        self.spaces
            .iter()
            .find(|s| s.slug == wanted)
            .ok_or_else(|| {
                let known: Vec<&str> = self.spaces.iter().map(|s| s.slug.as_str()).collect();
                ApiError::bad_request(format!(
                    "espace inconnu « {slug} » : valeurs possibles {}",
                    known.join(", ")
                ))
            })
    }

    /// Filtre optionnel par espace.
    fn space_filter(&self, slug: Option<&str>) -> ApiResult<Option<i64>> {
        match slug.map(str::trim).filter(|s| !s.is_empty()) {
            Some(slug) => Ok(Some(self.space(slug)?.id)),
            None => Ok(None),
        }
    }

    fn slug_of(&self, space_id: i64) -> String {
        self.spaces
            .iter()
            .find(|s| s.id == space_id)
            .map(|s| s.slug.clone())
            .unwrap_or_default()
    }
}

impl FromRequestParts<AppState> for ApiUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let Some(api) = &state.config.api else {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "API désactivée : renseigne API_TOKEN et API_USER_EMAIL",
            ));
        };

        let presented = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim);
        let valid = presented.is_some_and(|t| same_secret(t.as_bytes(), api.token.as_bytes()));
        if !valid {
            return Err(ApiError::new(
                StatusCode::UNAUTHORIZED,
                "jeton absent ou invalide (en-tête Authorization: Bearer …)",
            ));
        }

        let id = sqlx::query_scalar!(
            r#"SELECT id AS "id!" FROM users WHERE email = ?"#,
            api.user_email
        )
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::CONFLICT,
                "le compte de API_USER_EMAIL n'existe pas encore : crée-le avec `dashu set-password` ou connecte-toi une fois",
            )
        })?;

        let mut spaces = spaces::list(&state.db, id).await?;
        if spaces.is_empty() {
            // Compte créé en ligne de commande et jamais ouvert dans le navigateur.
            spaces::ensure_defaults(&state.db, id).await?;
            spaces = spaces::list(&state.db, id).await?;
        }
        Ok(Self { id, spaces })
    }
}

/// Compare deux secrets en temps constant (pour une longueur donnée) : le
/// temps de réponse ne dit pas combien de caractères étaient justes.
fn same_secret(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ---------------------------------------------------------------------
// Outils
// ---------------------------------------------------------------------

/// Instant Unix en date-heure ISO 8601 à l'heure de Paris : `2026-11-16T08:00:00+01:00`.
fn iso(ts: i64) -> String {
    time::paris(ts).to_rfc3339()
}

fn parse_date(input: &str, field: &str) -> ApiResult<NaiveDate> {
    NaiveDate::parse_from_str(input.trim(), "%Y-%m-%d")
        .map_err(|_| ApiError::bad_request(format!("{field} : date attendue au format AAAA-MM-JJ")))
}

fn clean_title(input: &str, max: usize, field: &str) -> ApiResult<String> {
    let title = input.trim();
    if title.chars().count() > max {
        return Err(ApiError::bad_request(format!(
            "{field} trop long ({max} caractères au plus)"
        )));
    }
    Ok(title.to_string())
}

fn check_body(body: &str) -> ApiResult<()> {
    if body.chars().count() > BODY_MAX_CHARS {
        return Err(ApiError::bad_request(format!(
            "texte trop long ({BODY_MAX_CHARS} caractères au plus)"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Espaces
// ---------------------------------------------------------------------

#[derive(Serialize)]
struct SpaceOut {
    slug: String,
    name: String,
}

/// `GET /api/v1/spaces`
async fn list_spaces(user: ApiUser) -> Json<Vec<SpaceOut>> {
    Json(
        user.spaces
            .into_iter()
            .map(|s| SpaceOut {
                slug: s.slug,
                name: s.name,
            })
            .collect(),
    )
}

// ---------------------------------------------------------------------
// Tâches
// ---------------------------------------------------------------------

#[derive(Serialize)]
struct TaskOut {
    id: i64,
    title: String,
    done: bool,
    /// Jour prévu, `AAAA-MM-JJ`, ou `null`.
    due_date: Option<String>,
    space: String,
}

#[derive(Deserialize)]
struct TaskListQuery {
    space: Option<String>,
    /// `open` (par défaut), `done` ou `all`.
    status: Option<String>,
}

/// `GET /api/v1/tasks?space=ecole&status=open`
async fn list_tasks(
    user: ApiUser,
    State(state): State<AppState>,
    query: Result<Query<TaskListQuery>, QueryRejection>,
) -> ApiResult<Json<Vec<TaskOut>>> {
    let Query(query) = query?;
    let space = user.space_filter(query.space.as_deref())?;
    let done: Option<bool> = match query.status.as_deref().unwrap_or("open") {
        "open" => Some(false),
        "done" => Some(true),
        "all" => None,
        other => {
            return Err(ApiError::bad_request(format!(
                "status inconnu « {other} » : open, done ou all"
            )));
        }
    };

    let tasks = sqlx::query!(
        r#"SELECT t.id AS "id!", t.title, t.done AS "done!: bool", t.due_date, s.slug AS "space!"
           FROM tasks t JOIN spaces s ON s.id = t.space_id
           WHERE t.user_id = ?1 AND (?2 IS NULL OR t.space_id = ?2) AND (?3 IS NULL OR t.done = ?3)
           ORDER BY t.done, t.due_date IS NULL, t.due_date, t.created_at
           LIMIT 500"#,
        user.id,
        space,
        done
    )
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|t| TaskOut {
        id: t.id,
        title: t.title,
        done: t.done,
        due_date: t.due_date,
        space: t.space,
    })
    .collect();
    Ok(Json(tasks))
}

async fn fetch_task(db: &SqlitePool, user: i64, id: i64) -> ApiResult<TaskOut> {
    let task = sqlx::query!(
        r#"SELECT t.id AS "id!", t.title, t.done AS "done!: bool", t.due_date, s.slug AS "space!"
           FROM tasks t JOIN spaces s ON s.id = t.space_id
           WHERE t.id = ? AND t.user_id = ?"#,
        id,
        user
    )
    .fetch_optional(db)
    .await?
    .ok_or_else(|| ApiError::not_found("tâche"))?;
    Ok(TaskOut {
        id: task.id,
        title: task.title,
        done: task.done,
        due_date: task.due_date,
        space: task.space,
    })
}

/// `GET /api/v1/tasks/{id}`
async fn get_task(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<TaskOut>> {
    Ok(Json(fetch_task(&state.db, user.id, id).await?))
}

#[derive(Deserialize)]
struct NewTask {
    title: String,
    space: String,
    /// `AAAA-MM-JJ`. Absent : aujourd'hui, comme depuis le dashboard.
    due_date: Option<String>,
}

/// `POST /api/v1/tasks` `{"title": "…", "space": "ecole", "due_date": "2026-11-20"}`
async fn create_task(
    user: ApiUser,
    State(state): State<AppState>,
    payload: Result<Json<NewTask>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<TaskOut>)> {
    let Json(task) = payload?;
    let title = clean_title(&task.title, TASK_MAX_CHARS, "title")?;
    if title.is_empty() {
        return Err(ApiError::bad_request("title ne peut pas être vide"));
    }
    let space = user.space(&task.space)?.id;
    let now = time::now();
    let due = match task.due_date.as_deref() {
        Some(d) => parse_date(d, "due_date")?,
        None => time::paris_date(now),
    }
    .format("%Y-%m-%d")
    .to_string();

    let id = sqlx::query!(
        "INSERT INTO tasks (user_id, space_id, title, due_date, created_at) VALUES (?, ?, ?, ?, ?)",
        user.id,
        space,
        title,
        due,
        now
    )
    .execute(&state.db)
    .await?
    .last_insert_rowid();
    Ok((
        StatusCode::CREATED,
        Json(fetch_task(&state.db, user.id, id).await?),
    ))
}

#[derive(Deserialize)]
struct TaskChanges {
    title: Option<String>,
    done: Option<bool>,
    due_date: Option<String>,
}

/// `PATCH /api/v1/tasks/{id}` `{"done": true}` (champs facultatifs : title, done, due_date)
async fn update_task(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    payload: Result<Json<TaskChanges>, JsonRejection>,
) -> ApiResult<Json<TaskOut>> {
    let Json(changes) = payload?;
    let title = match changes.title.as_deref() {
        Some(t) => {
            let t = clean_title(t, TASK_MAX_CHARS, "title")?;
            if t.is_empty() {
                return Err(ApiError::bad_request("title ne peut pas être vide"));
            }
            Some(t)
        }
        None => None,
    };
    let due = match changes.due_date.as_deref() {
        Some(d) => Some(parse_date(d, "due_date")?.format("%Y-%m-%d").to_string()),
        None => None,
    };
    let now = time::now();

    // Dans le SET, les colonnes désignent encore les anciennes valeurs : une
    // tâche qu'on coche garde sa date de fin si elle était déjà cochée.
    let changed = sqlx::query!(
        "UPDATE tasks SET
           title = COALESCE(?1, title),
           due_date = COALESCE(?2, due_date),
           done_at = CASE WHEN ?3 IS NULL THEN done_at WHEN ?3 THEN COALESCE(done_at, ?4) ELSE NULL END,
           done = COALESCE(?3, done)
         WHERE id = ?5 AND user_id = ?6",
        title,
        due,
        changes.done,
        now,
        id,
        user.id
    )
    .execute(&state.db)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(ApiError::not_found("tâche"));
    }
    Ok(Json(fetch_task(&state.db, user.id, id).await?))
}

/// `DELETE /api/v1/tasks/{id}`
async fn delete_task(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    let deleted = sqlx::query!(
        "DELETE FROM tasks WHERE id = ? AND user_id = ?",
        id,
        user.id
    )
    .execute(&state.db)
    .await?
    .rows_affected();
    if deleted == 0 {
        return Err(ApiError::not_found("tâche"));
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------
// Notes
// ---------------------------------------------------------------------

#[derive(Serialize)]
struct NoteSummary {
    id: i64,
    title: String,
    space: String,
    pinned: bool,
    tags: Vec<String>,
    /// Début du texte, sans Markdown, ou passage trouvé par la recherche.
    excerpt: String,
    updated_at: String,
}

#[derive(Serialize)]
struct NoteOut {
    id: i64,
    title: String,
    body_md: String,
    space: String,
    pinned: bool,
    tags: Vec<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Deserialize)]
struct NoteListQuery {
    /// Recherche plein texte (titre et contenu, accents ignorés).
    q: Option<String>,
    space: Option<String>,
    tag: Option<String>,
    limit: Option<i64>,
}

/// `GET /api/v1/notes?q=vlan&space=ecole&tag=reseau&limit=20`
async fn list_notes(
    user: ApiUser,
    State(state): State<AppState>,
    query: Result<Query<NoteListQuery>, QueryRejection>,
) -> ApiResult<Json<Vec<NoteSummary>>> {
    let Query(query) = query?;
    let space = user.space_filter(query.space.as_deref())?;
    let tag = query
        .tag
        .as_deref()
        .map(notes::normalize_tag)
        .filter(|t| !t.is_empty());
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let q = query.q.unwrap_or_default();

    let mut items: Vec<NoteSummary> = match notes::fts_query(&q) {
        Some(fts) => sqlx::query!(
            r#"SELECT n.id AS "id!", n.title AS "title!", n.space_id AS "space_id!: i64",
                      n.pinned AS "pinned!: bool", n.updated_at AS "updated_at!: i64",
                      snippet(notes_fts, 1, '', '', '…', 24) AS "excerpt!: String"
               FROM notes_fts
               JOIN notes n ON n.id = notes_fts.rowid
               WHERE notes_fts MATCH ?1 AND n.user_id = ?2 AND (?3 IS NULL OR n.space_id = ?3)
                 AND (?4 IS NULL OR EXISTS (
                       SELECT 1 FROM note_tags nt JOIN tags t ON t.id = nt.tag_id
                       WHERE nt.note_id = n.id AND t.name = ?4))
               ORDER BY bm25(notes_fts, 5.0, 1.0)
               LIMIT ?5"#,
            fts,
            user.id,
            space,
            tag,
            limit
        )
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .map(|n| NoteSummary {
            id: n.id,
            title: n.title,
            space: user.slug_of(n.space_id),
            pinned: n.pinned,
            tags: Vec::new(),
            excerpt: markdown::plain(&n.excerpt),
            updated_at: iso(n.updated_at),
        })
        .collect(),
        None => sqlx::query!(
            r#"SELECT n.id AS "id!", n.title AS "title!", n.space_id AS "space_id!: i64",
                      n.pinned AS "pinned!: bool", n.updated_at AS "updated_at!: i64",
                      substr(n.body_md, 1, 600) AS "head!: String"
               FROM notes n
               WHERE n.user_id = ?1 AND (?2 IS NULL OR n.space_id = ?2)
                 AND (?3 IS NULL OR EXISTS (
                       SELECT 1 FROM note_tags nt JOIN tags t ON t.id = nt.tag_id
                       WHERE nt.note_id = n.id AND t.name = ?3))
               ORDER BY n.pinned DESC, n.updated_at DESC
               LIMIT ?4"#,
            user.id,
            space,
            tag,
            limit
        )
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .map(|n| NoteSummary {
            id: n.id,
            title: n.title,
            space: user.slug_of(n.space_id),
            pinned: n.pinned,
            tags: Vec::new(),
            excerpt: markdown::excerpt(&n.head, 200),
            updated_at: iso(n.updated_at),
        })
        .collect(),
    };

    // Tags de toutes les notes de l'utilisateur, en une requête.
    let mut tags: HashMap<i64, Vec<String>> = HashMap::new();
    for row in sqlx::query!(
        r#"SELECT nt.note_id AS "note_id!", t.name AS "name!"
           FROM note_tags nt JOIN tags t ON t.id = nt.tag_id
           WHERE t.user_id = ?
           ORDER BY t.name"#,
        user.id
    )
    .fetch_all(&state.db)
    .await?
    {
        tags.entry(row.note_id).or_default().push(row.name);
    }
    for item in &mut items {
        item.tags = tags.remove(&item.id).unwrap_or_default();
    }
    Ok(Json(items))
}

async fn fetch_note(db: &SqlitePool, user: &ApiUser, id: i64) -> ApiResult<NoteOut> {
    let note = sqlx::query!(
        r#"SELECT id AS "id!", title, body_md, space_id, pinned AS "pinned!: bool", created_at, updated_at
           FROM notes WHERE id = ? AND user_id = ?"#,
        id,
        user.id
    )
    .fetch_optional(db)
    .await?
    .ok_or_else(|| ApiError::not_found("note"))?;
    let tags = notes::tags_of(db, user.id, id)
        .await?
        .into_iter()
        .map(|t| t.name)
        .collect();
    Ok(NoteOut {
        id: note.id,
        title: note.title,
        body_md: note.body_md,
        space: user.slug_of(note.space_id),
        pinned: note.pinned,
        tags,
        created_at: iso(note.created_at),
        updated_at: iso(note.updated_at),
    })
}

/// `GET /api/v1/notes/{id}` : la note complète, avec son texte Markdown.
async fn get_note(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<NoteOut>> {
    Ok(Json(fetch_note(&state.db, &user, id).await?))
}

#[derive(Deserialize)]
struct NewNote {
    title: String,
    #[serde(default)]
    body_md: String,
    space: String,
    #[serde(default)]
    tags: Vec<String>,
}

/// `POST /api/v1/notes` `{"title": "…", "body_md": "…", "space": "ecole", "tags": ["reseau"]}`
async fn create_note(
    user: ApiUser,
    State(state): State<AppState>,
    payload: Result<Json<NewNote>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<NoteOut>)> {
    let Json(note) = payload?;
    let title = clean_title(&note.title, notes::TITLE_MAX_CHARS, "title")?;
    let body = notes::normalize_newlines(&note.body_md);
    check_body(&body)?;
    if title.is_empty() && body.trim().is_empty() {
        return Err(ApiError::bad_request(
            "une note a besoin d'un titre ou d'un texte",
        ));
    }
    if note.tags.len() > MAX_TAGS_PER_REQUEST {
        return Err(ApiError::bad_request(format!(
            "{MAX_TAGS_PER_REQUEST} tags au plus par requête"
        )));
    }
    let space = user.space(&note.space)?.id;
    let now = time::now();

    let id = sqlx::query!(
        "INSERT INTO notes (user_id, space_id, title, body_md, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
        user.id,
        space,
        title,
        body,
        now,
        now
    )
    .execute(&state.db)
    .await?
    .last_insert_rowid();
    for tag in &note.tags {
        notes::attach_tag(&state.db, user.id, id, tag).await?;
    }
    Ok((
        StatusCode::CREATED,
        Json(fetch_note(&state.db, &user, id).await?),
    ))
}

#[derive(Deserialize)]
struct NoteChanges {
    title: Option<String>,
    /// Remplace tout le texte.
    body_md: Option<String>,
    /// Ajoute ce texte à la fin, après une ligne vide.
    append: Option<String>,
    pinned: Option<bool>,
    /// Déplace la note vers cet espace.
    space: Option<String>,
    #[serde(default)]
    add_tags: Vec<String>,
}

/// `PATCH /api/v1/notes/{id}` (champs facultatifs : title, body_md ou append,
/// pinned, space, add_tags)
async fn update_note(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    payload: Result<Json<NoteChanges>, JsonRejection>,
) -> ApiResult<Json<NoteOut>> {
    let Json(changes) = payload?;
    if changes.body_md.is_some() && changes.append.is_some() {
        return Err(ApiError::bad_request(
            "body_md et append ne vont pas ensemble : l'un remplace, l'autre ajoute",
        ));
    }
    if changes.add_tags.len() > MAX_TAGS_PER_REQUEST {
        return Err(ApiError::bad_request(format!(
            "{MAX_TAGS_PER_REQUEST} tags au plus par requête"
        )));
    }
    // L'espace cible est vérifié avant toute écriture.
    let target_space = match changes.space.as_deref() {
        Some(slug) => Some(user.space(slug)?.clone()),
        None => None,
    };

    let current = sqlx::query!(
        "SELECT title, body_md FROM notes WHERE id = ? AND user_id = ?",
        id,
        user.id
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::not_found("note"))?;

    let title = match changes.title.as_deref() {
        Some(t) => clean_title(t, notes::TITLE_MAX_CHARS, "title")?,
        None => current.title.clone(),
    };
    let body = match (changes.body_md.as_deref(), changes.append.as_deref()) {
        (Some(body), _) => notes::normalize_newlines(body),
        (None, Some(extra)) => {
            let extra = notes::normalize_newlines(extra.trim_end());
            let kept = current.body_md.trim_end();
            if kept.is_empty() {
                extra
            } else {
                format!("{kept}\n\n{extra}")
            }
        }
        (None, None) => current.body_md.clone(),
    };
    check_body(&body)?;

    if title != current.title || body != current.body_md {
        let now = time::now();
        sqlx::query!(
            "UPDATE notes SET title = ?, body_md = ?, updated_at = ? WHERE id = ? AND user_id = ?",
            title,
            body,
            now,
            id,
            user.id
        )
        .execute(&state.db)
        .await?;
    }
    if let Some(pinned) = changes.pinned {
        sqlx::query!(
            "UPDATE notes SET pinned = ? WHERE id = ? AND user_id = ?",
            pinned,
            id,
            user.id
        )
        .execute(&state.db)
        .await?;
    }
    if let Some(space) = target_space {
        sqlx::query!(
            "UPDATE notes SET space_id = ? WHERE id = ? AND user_id = ?",
            space.id,
            id,
            user.id
        )
        .execute(&state.db)
        .await?;
    }
    for tag in &changes.add_tags {
        notes::attach_tag(&state.db, user.id, id, tag).await?;
    }
    Ok(Json(fetch_note(&state.db, &user, id).await?))
}

// ---------------------------------------------------------------------
// Agenda
// ---------------------------------------------------------------------

#[derive(Serialize)]
struct EventOut {
    /// Titre complet, tel que dans le flux iCal.
    title: String,
    /// Matière (début du titre Pronote, avant le premier « - »).
    subject: String,
    /// Reste du titre : professeur, groupe…
    details: String,
    location: String,
    start: String,
    end: String,
    all_day: bool,
    space: String,
}

#[derive(Deserialize)]
struct CalendarQuery {
    /// Premier jour, `AAAA-MM-JJ` (par défaut : aujourd'hui).
    from: Option<String>,
    /// Dernier jour inclus (par défaut : `from` + 6 jours).
    to: Option<String>,
    space: Option<String>,
}

/// `GET /api/v1/calendar?from=2026-11-16&to=2026-11-20&space=ecole`
///
/// `space=perso` donne, comme dans l'application, les agendas de tous les
/// espaces ; chaque événement garde le slug de son espace.
async fn calendar(
    user: ApiUser,
    State(state): State<AppState>,
    query: Result<Query<CalendarQuery>, QueryRejection>,
) -> ApiResult<Json<Vec<EventOut>>> {
    let Query(query) = query?;
    let space = match query.space.as_deref() {
        Some(slug) if user.space(slug)?.is_overview() => None,
        other => user.space_filter(other)?,
    };
    let first = match query.from.as_deref() {
        Some(d) => parse_date(d, "from")?,
        None => time::paris_date(time::now()),
    };
    let last = match query.to.as_deref() {
        Some(d) => parse_date(d, "to")?,
        None => first + Duration::days(6),
    };
    if last < first {
        return Err(ApiError::bad_request("to doit être après from"));
    }
    if (last - first).num_days() >= CALENDAR_MAX_DAYS {
        return Err(ApiError::bad_request(format!(
            "période trop longue ({CALENDAR_MAX_DAYS} jours au plus)"
        )));
    }
    let from = time::day_start(first);
    let to = time::day_start(last + Duration::days(1));

    let events = sqlx::query!(
        r#"SELECT e.title, e.location, e.starts_at, e.ends_at, e.all_day AS "all_day!: bool",
                  c.space_id AS "space_id!: i64"
           FROM calendar_events e
           JOIN calendar_sources c ON c.id = e.source_id
           WHERE c.user_id = ?1 AND (?2 IS NULL OR c.space_id = ?2)
             AND e.ends_at > ?3 AND e.starts_at < ?4
           ORDER BY e.starts_at, e.ends_at
           LIMIT 1000"#,
        user.id,
        space,
        from,
        to
    )
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|e| EventOut {
        subject: ical::subject(&e.title).to_string(),
        details: ical::details(&e.title),
        title: e.title,
        location: e.location,
        start: iso(e.starts_at),
        end: iso(e.ends_at),
        all_day: e.all_day,
        space: user.slug_of(e.space_id),
    })
    .collect();
    Ok(Json(events))
}

#[derive(Serialize)]
struct RefreshOut {
    sources: usize,
    failures: usize,
}

/// `POST /api/v1/calendar/refresh` : resynchronise tout de suite les flux iCal.
async fn refresh_calendar(
    user: ApiUser,
    State(state): State<AppState>,
) -> ApiResult<Json<RefreshOut>> {
    let sources = sqlx::query!(
        r#"SELECT id AS "id!", ical_url FROM calendar_sources WHERE user_id = ?"#,
        user.id
    )
    .fetch_all(&state.db)
    .await?;
    let mut failures = 0;
    for source in &sources {
        if crate::calendar::sync::sync_source(&state, source.id, &source.ical_url)
            .await
            .is_err()
        {
            failures += 1;
        }
    }
    Ok(Json(RefreshOut {
        sources: sources.len(),
        failures,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparaison_de_secrets() {
        assert!(same_secret(b"abcdef", b"abcdef"));
        assert!(!same_secret(b"abcdef", b"abcdeg"));
        assert!(!same_secret(b"abc", b"abcdef"));
        assert!(!same_secret(b"", b"x"));
    }
}
