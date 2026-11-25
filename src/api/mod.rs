//! API JSON pour un programme (par exemple un assistant personnel).
//!
//! Toutes les routes sont sous `/api/v1` et exigent l'en-tête
//! `Authorization: Bearer <jeton>`. Deux sortes de jetons :
//!
//! * un jeton personnel (`dashu_…`, Réglages › Connexions, voir
//!   `crate::auth::tokens`), qui agit au nom de son compte ;
//! * `API_TOKEN`, s'il est renseigné, qui agit au nom de `API_USER_EMAIL`
//!   (l'accès d'Hermès, inchangé).
//!
//! Pas de session ni de cookie ici, donc pas de protection CSRF à prévoir : un
//! site tiers peut faire envoyer un cookie par le navigateur, pas un en-tête
//! `Authorization`. À l'inverse, un cookie de session valide n'ouvre pas l'API.
//!
//! Ce que l'API permet (SPEC, « API complète et corbeille ») : les tâches,
//! les notes, les fichiers et leurs liens de partage, les dossiers, la
//! corbeille (lister, restaurer), l'agenda, les conversions, les espaces et
//! leurs membres, les réglages. Une suppression part dans la corbeille. Hors
//! de l'API : ce qui protège le compte (mot de passe, connexions, jetons),
//! l'effacement définitif, l'administration.
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
use crate::auth::tokens;
use crate::calendar::ical;
use crate::notes::{self, markdown};
use crate::spaces::{self, Space};
use crate::time;
use crate::web::AppError;

mod agenda;
mod contenu;
mod espaces;
mod outils;

pub(crate) use agenda::{
    EventBody, NewSource, event_create, event_delete, event_update, source_add, source_remove,
    sources,
};
pub(crate) use contenu::{
    FileChanges, FolderChanges, NewUpload, fetch_file, file_delete, file_update, files_list,
    folder_create, folder_delete, folder_update, folders_list, link_create, links_list, stored,
    trash_list, trash_restore, upload_place,
};
pub(crate) use espaces::{
    DashboardBody, NewInvitation, NewSpace, SpaceChanges, SpaceRemoval, ToolsBody, dashboard_read,
    dashboard_write, invitation_accept, invitation_cancel, invitations_list, member_invite,
    member_remove, members_list, settings_read, settings_update, space_archive, space_create,
    space_delete, space_leave, space_unarchive, space_update, spaces_out, tools_read, tools_write,
};
pub(crate) use outils::{fetch_job, formats, jobs, replace_original, start};

const TASK_MAX_CHARS: usize = 200;
/// Une note de plus de 200 000 caractères n'a rien à faire dans dashu.
const BODY_MAX_CHARS: usize = 200_000;
const MAX_TAGS_PER_REQUEST: usize = 20;
/// Plus longue période d'agenda renvoyée en une fois.
const CALENDAR_MAX_DAYS: i64 = 62;

pub fn router() -> Router<AppState> {
    let v1 = Router::new()
        .route("/tasks", get(list_tasks).post(create_task))
        .route(
            "/tasks/{id}",
            get(get_task).patch(update_task).delete(delete_task),
        )
        .route("/notes", get(list_notes).post(create_note))
        .route(
            "/notes/{id}",
            get(get_note).patch(update_note).delete(delete_note),
        )
        .route("/calendar", get(calendar))
        .route("/calendar/refresh", post(refresh_calendar))
        .merge(contenu::routes())
        .merge(outils::routes())
        .merge(agenda::routes())
        .merge(espaces::routes())
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

    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    fn not_found(what: &str) -> Self {
        Self::new(StatusCode::NOT_FOUND, format!("{what} introuvable"))
    }

    pub(crate) fn internal(error: anyhow::Error) -> Self {
        // Le détail part dans les logs, jamais dans la réponse.
        tracing::error!(error = format!("{error:#}"), "erreur interne (API)");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "erreur interne, réessaie dans un instant",
        )
    }
}

impl ApiError {
    /// Message en français, tel qu'il part dans la réponse.
    pub(crate) fn message(&self) -> &str {
        &self.message
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
            AppError::Unavailable(message) => Self::new(StatusCode::SERVICE_UNAVAILABLE, message),
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

pub(crate) type ApiResult<T> = Result<T, ApiError>;

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
    /// Par où passe la requête : le nom du jeton (« Claude Code du Mac »),
    /// de l'application autorisée (« Claude »), ou « Hermès » pour
    /// `API_TOKEN`. C'est la trace qu'affiche la corbeille.
    pub via: String,
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
        let refused = || {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "jeton absent ou invalide (en-tête Authorization: Bearer …)",
            )
        };
        let presented = bearer(parts).ok_or_else(refused)?;

        let (id, via) = if presented.starts_with(tokens::PREFIX) {
            // Jeton personnel. Ceux d'une autorisation OAuth ne valent que
            // pour /mcp (SPEC, « Connecteur Claude », « Chaque jeton à sa place »).
            match tokens::authenticate(state, presented).await? {
                Some(owner) if owner.grant_id.is_none() => (owner.user_id, owner.name),
                _ => return Err(refused()),
            }
        } else {
            let api = state.config.api.as_ref().ok_or_else(refused)?;
            if !same_secret(presented.as_bytes(), api.token.as_bytes()) {
                return Err(refused());
            }
            sqlx::query_scalar!(
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
            })
            .map(|id| (id, "Hermès".to_string()))?
        };

        // La trace : le compte, le jeton et l'action, jamais le contenu.
        tracing::info!(
            user = id,
            via,
            method = %parts.method,
            path = parts.uri.path(),
            "appel d'API"
        );
        Self::load(state, id, via).await
    }
}

impl ApiUser {
    /// Le compte et ses espaces, relus à chaque requête.
    pub(crate) async fn load(state: &AppState, id: i64, via: String) -> ApiResult<Self> {
        let mut spaces = spaces::list(&state.db, id).await?;
        if spaces.is_empty() {
            // Compte créé en ligne de commande et jamais ouvert dans le navigateur.
            spaces::ensure_defaults(&state.db, id).await?;
            spaces = spaces::list(&state.db, id).await?;
        }
        Ok(Self { id, spaces, via })
    }

    /// Qui agit, et par où : pour la corbeille.
    pub(crate) fn who(&self) -> crate::trash::Who<'_> {
        crate::trash::Who {
            user: self.id,
            via: &self.via,
        }
    }
}

/// Jeton de l'en-tête `Authorization: Bearer …`, s'il y en a un.
pub(crate) fn bearer(parts: &Parts) -> Option<&str> {
    parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
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
// Tâches
// ---------------------------------------------------------------------

#[derive(Serialize)]
pub(crate) struct TaskOut {
    id: i64,
    title: String,
    done: bool,
    /// Jour prévu, `AAAA-MM-JJ`, ou `null`.
    due_date: Option<String>,
    space: String,
}

#[derive(Deserialize)]
pub(crate) struct TaskListQuery {
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
    Ok(Json(tasks_list(&state, &user, query).await?))
}

pub(crate) async fn tasks_list(
    state: &AppState,
    user: &ApiUser,
    query: TaskListQuery,
) -> ApiResult<Vec<TaskOut>> {
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
        r#"SELECT t.id AS "id!", t.title, t.done AS "done!: bool", t.due_date, t.space_id
           FROM tasks t
           WHERE t.space_id IN (SELECT space_id FROM space_access WHERE user_id = ?1) AND (?2 IS NULL OR t.space_id = ?2) AND (?3 IS NULL OR t.done = ?3)
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
        space: user.slug_of(t.space_id),
    })
    .collect();
    Ok(tasks)
}

async fn fetch_task(db: &SqlitePool, user: &ApiUser, id: i64) -> ApiResult<TaskOut> {
    let task = sqlx::query!(
        r#"SELECT t.id AS "id!", t.title, t.done AS "done!: bool", t.due_date, t.space_id
           FROM tasks t
           WHERE t.id = ? AND t.space_id IN (SELECT space_id FROM space_access WHERE user_id = ?)"#,
        id,
        user.id
    )
    .fetch_optional(db)
    .await?
    .ok_or_else(|| ApiError::not_found("tâche"))?;
    Ok(TaskOut {
        id: task.id,
        title: task.title,
        done: task.done,
        due_date: task.due_date,
        space: user.slug_of(task.space_id),
    })
}

/// `GET /api/v1/tasks/{id}`
async fn get_task(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<TaskOut>> {
    Ok(Json(fetch_task(&state.db, &user, id).await?))
}

#[derive(Deserialize)]
pub(crate) struct NewTask {
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
    Ok((
        StatusCode::CREATED,
        Json(task_create(&state, &user, task).await?),
    ))
}

pub(crate) async fn task_create(
    state: &AppState,
    user: &ApiUser,
    task: NewTask,
) -> ApiResult<TaskOut> {
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
    fetch_task(&state.db, user, id).await
}

#[derive(Deserialize)]
pub(crate) struct TaskChanges {
    pub(crate) title: Option<String>,
    pub(crate) done: Option<bool>,
    pub(crate) due_date: Option<String>,
    /// Déplace la tâche vers cet espace.
    pub(crate) space: Option<String>,
}

/// `PATCH /api/v1/tasks/{id}` `{"done": true}` (champs facultatifs : title,
/// done, due_date, space)
async fn update_task(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    payload: Result<Json<TaskChanges>, JsonRejection>,
) -> ApiResult<Json<TaskOut>> {
    let Json(changes) = payload?;
    Ok(Json(task_update(&state, &user, id, changes).await?))
}

pub(crate) async fn task_update(
    state: &AppState,
    user: &ApiUser,
    id: i64,
    changes: TaskChanges,
) -> ApiResult<TaskOut> {
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
    // L'espace cible est vérifié avant toute écriture.
    let target = match changes.space.as_deref() {
        Some(slug) => Some(user.space(slug)?.id),
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
         WHERE id = ?5 AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?6)",
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
    if let Some(space) = target {
        crate::dashboard::move_task_to(&state.db, user.id, id, space).await?;
    }
    fetch_task(&state.db, user, id).await
}

/// `DELETE /api/v1/tasks/{id}`
async fn delete_task(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    task_delete(&state, &user, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn task_delete(state: &AppState, user: &ApiUser, id: i64) -> ApiResult<()> {
    let deleted = sqlx::query!(
        "DELETE FROM tasks WHERE id = ? AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?)",
        id,
        user.id
    )
    .execute(&state.db)
    .await?
    .rows_affected();
    if deleted == 0 {
        return Err(ApiError::not_found("tâche"));
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Notes
// ---------------------------------------------------------------------

#[derive(Serialize)]
pub(crate) struct NoteSummary {
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
pub(crate) struct NoteOut {
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
pub(crate) struct NoteListQuery {
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
    Ok(Json(notes_list(&state, &user, query).await?))
}

pub(crate) async fn notes_list(
    state: &AppState,
    user: &ApiUser,
    query: NoteListQuery,
) -> ApiResult<Vec<NoteSummary>> {
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
               WHERE notes_fts MATCH ?1 AND n.space_id IN (SELECT space_id FROM space_access WHERE user_id = ?2) AND (?3 IS NULL OR n.space_id = ?3)
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
               WHERE n.space_id IN (SELECT space_id FROM space_access WHERE user_id = ?1) AND (?2 IS NULL OR n.space_id = ?2)
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
           FROM note_tags nt
           JOIN tags t ON t.id = nt.tag_id
           JOIN notes n ON n.id = nt.note_id
           WHERE n.space_id IN (SELECT space_id FROM space_access WHERE user_id = ?)
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
    Ok(items)
}

pub(crate) async fn fetch_note(db: &SqlitePool, user: &ApiUser, id: i64) -> ApiResult<NoteOut> {
    let note = sqlx::query!(
        r#"SELECT id AS "id!", title, body_md, space_id, pinned AS "pinned!: bool", created_at, updated_at
           FROM notes WHERE id = ? AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?)"#,
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
pub(crate) struct NewNote {
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
    Ok((
        StatusCode::CREATED,
        Json(note_create(&state, &user, note).await?),
    ))
}

pub(crate) async fn note_create(
    state: &AppState,
    user: &ApiUser,
    note: NewNote,
) -> ApiResult<NoteOut> {
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
    fetch_note(&state.db, user, id).await
}

#[derive(Deserialize, Default)]
pub(crate) struct NoteChanges {
    pub(crate) title: Option<String>,
    /// Remplace tout le texte.
    pub(crate) body_md: Option<String>,
    /// Ajoute ce texte à la fin, après une ligne vide.
    pub(crate) append: Option<String>,
    pub(crate) pinned: Option<bool>,
    /// Déplace la note vers cet espace.
    pub(crate) space: Option<String>,
    /// Range la note dans ce dossier de son espace ; `null` : sa racine.
    #[serde(default, deserialize_with = "contenu::some")]
    pub(crate) folder: Option<Option<i64>>,
    #[serde(default)]
    pub(crate) add_tags: Vec<String>,
    #[serde(default)]
    pub(crate) remove_tags: Vec<String>,
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
    Ok(Json(note_update(&state, &user, id, changes).await?))
}

pub(crate) async fn note_update(
    state: &AppState,
    user: &ApiUser,
    id: i64,
    changes: NoteChanges,
) -> ApiResult<NoteOut> {
    if changes.body_md.is_some() && changes.append.is_some() {
        return Err(ApiError::bad_request(
            "body_md et append ne vont pas ensemble : l'un remplace, l'autre ajoute",
        ));
    }
    if changes.add_tags.len() + changes.remove_tags.len() > MAX_TAGS_PER_REQUEST {
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
        "SELECT title, body_md FROM notes WHERE id = ? AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?)",
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
            "UPDATE notes SET title = ?, body_md = ?, updated_at = ?, revision = revision + 1
             WHERE id = ? AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?)",
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
            "UPDATE notes SET pinned = ? WHERE id = ? AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?)",
            pinned,
            id,
            user.id
        )
        .execute(&state.db)
        .await?;
    }
    if let Some(space) = target_space {
        // Un dossier appartient à un espace : une note qui change d'espace va
        // à la racine du nouveau (SPEC, « Dossiers »).
        sqlx::query!(
            "UPDATE notes SET folder_id = CASE WHEN space_id = ?1 THEN folder_id END, space_id = ?1
             WHERE id = ?2 AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?3)",
            space.id,
            id,
            user.id
        )
        .execute(&state.db)
        .await?;
    }
    if let Some(folder) = changes.folder {
        notes::file_in_folder(&state.db, user.id, id, folder).await?;
    }
    for tag in &changes.add_tags {
        notes::attach_tag(&state.db, user.id, id, tag).await?;
    }
    for tag in &changes.remove_tags {
        let name = notes::normalize_tag(tag);
        let found = sqlx::query_scalar!(
            r#"SELECT t.id AS "id!" FROM note_tags nt JOIN tags t ON t.id = nt.tag_id
               WHERE nt.note_id = ? AND t.name = ?"#,
            id,
            name
        )
        .fetch_optional(&state.db)
        .await?;
        if let Some(tag_id) = found {
            notes::detach_tag(&state.db, user.id, id, tag_id).await?;
        }
    }
    fetch_note(&state.db, user, id).await
}

/// `DELETE /api/v1/notes/{id}` : dans la corbeille (30 jours pour la
/// restaurer, depuis le site ou par `POST /api/v1/trash/note/{id}/restore`).
async fn delete_note(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    note_delete(&state, &user, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn note_delete(state: &AppState, user: &ApiUser, id: i64) -> ApiResult<()> {
    crate::trash::note(&state.db, user.who(), id)
        .await
        .map_err(|e| match e {
            AppError::NotFound => ApiError::not_found("note"),
            other => other.into(),
        })
}

// ---------------------------------------------------------------------
// Agenda
// ---------------------------------------------------------------------

#[derive(Serialize)]
pub(crate) struct EventOut {
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
    /// Identifiant d'un événement créé dans dashu (modifiable par
    /// `/api/v1/events/{id}`) ; `null` pour un cours d'un flux iCal.
    event: Option<i64>,
}

#[derive(Deserialize)]
pub(crate) struct CalendarQuery {
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
    Ok(Json(calendar_events(&state, &user, query).await?))
}

pub(crate) async fn calendar_events(
    state: &AppState,
    user: &ApiUser,
    query: CalendarQuery,
) -> ApiResult<Vec<EventOut>> {
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
        r#"SELECT e.title AS "title!: String", e.location AS "location!: String",
                  e.starts_at AS "starts_at!: i64", e.ends_at AS "ends_at!: i64",
                  e.all_day AS "all_day!: bool", e.space_id AS "space_id!: i64",
                  e.event_id AS "event_id?: i64"
           FROM agenda_items e
           WHERE e.user_id = ?1 AND (?2 IS NULL OR e.space_id = ?2)
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
        event: e.event_id,
    })
    .collect();
    Ok(events)
}

#[derive(Serialize)]
pub(crate) struct RefreshOut {
    sources: usize,
    failures: usize,
}

/// `POST /api/v1/calendar/refresh` : resynchronise tout de suite les flux iCal.
async fn refresh_calendar(
    user: ApiUser,
    State(state): State<AppState>,
) -> ApiResult<Json<RefreshOut>> {
    Ok(Json(calendar_refresh(&state, &user).await?))
}

pub(crate) async fn calendar_refresh(state: &AppState, user: &ApiUser) -> ApiResult<RefreshOut> {
    let sources = sqlx::query!(
        r#"SELECT id AS "id!", ical_url FROM calendar_sources WHERE user_id = ?"#,
        user.id
    )
    .fetch_all(&state.db)
    .await?;
    let mut failures = 0;
    for source in &sources {
        if crate::calendar::sync::sync_source(state, source.id, &source.ical_url)
            .await
            .is_err()
        {
            failures += 1;
        }
    }
    Ok(RefreshOut {
        sources: sources.len(),
        failures,
    })
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
