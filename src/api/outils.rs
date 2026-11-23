//! API, lot « outils » (SPEC, « API complète et corbeille ») : les formats
//! possibles d'un fichier, convertir, compresser, suivre une conversion,
//! remplacer l'original par le résultat. Le travail est fait par le service
//! de conversion (`dashu-worker`), comme pour le site : mêmes fonctions,
//! mêmes refus. Sans service de conversion (`WORK_DIR` absent), ces routes
//! répondent 404.

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::contenu::fetch_file;
use super::{ApiError, ApiResult, ApiUser, iso};
use crate::AppState;
use crate::tools::{self, Conversion};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/files/{id}/conversions", get(conversions))
        .route("/files/{id}/convert", post(convert))
        .route("/files/{id}/compress", post(compress))
        .route("/jobs", get(list_jobs))
        .route("/jobs/{id}", get(get_job))
        .route("/jobs/{id}/replace", post(replace))
}

fn no_worker() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "conversions désactivées sur ce serveur (WORK_DIR)",
    )
}

fn worker_on(state: &AppState) -> ApiResult<()> {
    match (&state.config.files, &state.config.work_dir) {
        (Some(_), Some(_)) => Ok(()),
        _ => Err(no_worker()),
    }
}

#[derive(Serialize)]
pub(crate) struct FormatOut {
    /// Valeur à donner à `to`.
    to: &'static str,
    label: &'static str,
}

/// `GET /api/v1/files/{id}/conversions` : ce que le menu « Convertir »
/// propose pour ce fichier (`compress` compris s'il se compresse).
async fn conversions(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<FormatOut>>> {
    Ok(Json(formats(&state, &user, id).await?))
}

pub(crate) async fn formats(
    state: &AppState,
    user: &ApiUser,
    id: i64,
) -> ApiResult<Vec<FormatOut>> {
    worker_on(state)?;
    let file = fetch_file(&state.db, user, id).await?;
    Ok(tools::conversions_for(&file.mime)
        .into_iter()
        .map(|c| FormatOut {
            to: c.key(),
            label: c.label(),
        })
        .collect())
}

#[derive(Deserialize)]
pub(crate) struct ConvertBody {
    /// Une valeur de `GET /api/v1/files/{id}/conversions`.
    pub(crate) to: String,
    /// Pour `pages` : « 2-4 » ou « 3 ».
    pub(crate) pages: Option<String>,
}

/// `POST /api/v1/files/{id}/convert` `{"to": "pdf", "pages"?: "2-4"}` : met
/// la conversion en file ; le résultat arrive à côté de l'original.
async fn convert(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    payload: Result<Json<ConvertBody>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<JobOut>)> {
    let Json(body) = payload?;
    Ok((
        StatusCode::ACCEPTED,
        Json(start(&state, &user, id, &body.to, body.pages.as_deref()).await?),
    ))
}

/// `POST /api/v1/files/{id}/compress` : une copie plus légère, à côté.
async fn compress(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<(StatusCode, Json<JobOut>)> {
    Ok((
        StatusCode::ACCEPTED,
        Json(start(&state, &user, id, "compress", None).await?),
    ))
}

pub(crate) async fn start(
    state: &AppState,
    user: &ApiUser,
    id: i64,
    to: &str,
    pages: Option<&str>,
) -> ApiResult<JobOut> {
    worker_on(state)?;
    let to = Conversion::from_key(to).ok_or_else(|| {
        ApiError::bad_request(format!(
            "{} (to : une valeur de /files/{{id}}/conversions)",
            tools::IMPOSSIBLE
        ))
    })?;
    let pages = match to {
        Conversion::Pages => Some(
            tools::parse_pages(pages.unwrap_or_default())
                .ok_or_else(|| ApiError::bad_request("pages à extraire : par exemple 2-4"))?,
        ),
        _ => None,
    };
    let job = tools::queue(state, user.id, id, to, pages).await?;
    tracing::info!(
        user = user.id,
        via = user.via,
        file = id,
        to = to.key(),
        "conversion lancée"
    );
    fetch_job(state, user, job).await
}

#[derive(Serialize)]
pub(crate) struct JobOut {
    id: i64,
    /// « Compression », « En PDF »…
    label: &'static str,
    /// Le fichier de départ (tant qu'il existe), et son nom.
    file: Option<i64>,
    source_name: String,
    /// `queued`, `running`, `done` ou `failed`.
    status: String,
    /// 0 à 100.
    progress: i64,
    error: Option<String>,
    /// Le résultat, une fois la conversion terminée.
    result: Option<serde_json::Value>,
    created_at: String,
    updated_at: String,
}

struct JobRow {
    id: i64,
    kind: String,
    target: Option<String>,
    file_id: Option<i64>,
    source_name: String,
    status: String,
    progress: i64,
    error: Option<String>,
    result_file_id: Option<i64>,
    created_at: i64,
    updated_at: i64,
}

async fn job_out(state: &AppState, user: &ApiUser, row: JobRow) -> ApiResult<JobOut> {
    let result = match row.result_file_id {
        Some(file) => match fetch_file(&state.db, user, file).await {
            Ok(file) => Some(serde_json::to_value(file).map_err(|e| ApiError::internal(e.into()))?),
            Err(_) => None,
        },
        None => None,
    };
    Ok(JobOut {
        id: row.id,
        label: tools::job_label(&row.kind, row.target.as_deref()),
        file: row.file_id,
        source_name: row.source_name,
        status: row.status,
        progress: row.progress,
        error: row.error,
        result,
        created_at: iso(row.created_at),
        updated_at: iso(row.updated_at),
    })
}

/// Une tâche du compte (les conversions sont à leur auteur). 404 sinon.
pub(crate) async fn fetch_job(state: &AppState, user: &ApiUser, id: i64) -> ApiResult<JobOut> {
    let row = sqlx::query_as!(
        JobRow,
        r#"SELECT id AS "id!", kind, target, file_id, source_name, status, progress, error,
                  result_file_id, created_at, updated_at
           FROM jobs WHERE id = ? AND user_id = ? AND kind IN ('compress', 'convert')"#,
        id,
        user.id
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::not_found("conversion"))?;
    job_out(state, user, row).await
}

#[derive(Deserialize, Default)]
pub(crate) struct JobsQuery {
    /// `true` : seulement celles en file ou en cours.
    #[serde(default)]
    pub(crate) active: bool,
}

/// `GET /api/v1/jobs?active=true` : les conversions du compte, récentes d'abord.
async fn list_jobs(
    user: ApiUser,
    State(state): State<AppState>,
    query: Result<Query<JobsQuery>, QueryRejection>,
) -> ApiResult<Json<Vec<JobOut>>> {
    let Query(query) = query?;
    Ok(Json(jobs(&state, &user, query).await?))
}

pub(crate) async fn jobs(
    state: &AppState,
    user: &ApiUser,
    query: JobsQuery,
) -> ApiResult<Vec<JobOut>> {
    worker_on(state)?;
    let rows = sqlx::query_as!(
        JobRow,
        r#"SELECT id AS "id!", kind, target, file_id, source_name, status, progress, error,
                  result_file_id, created_at, updated_at
           FROM jobs WHERE user_id = ?1 AND kind IN ('compress', 'convert')
             AND (?2 = 0 OR status IN ('queued', 'running'))
           ORDER BY created_at DESC, id DESC LIMIT 50"#,
        user.id,
        query.active
    )
    .fetch_all(&state.db)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(job_out(state, user, row).await?);
    }
    Ok(out)
}

/// `GET /api/v1/jobs/{id}` : où en est une conversion.
async fn get_job(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<JobOut>> {
    worker_on(&state)?;
    Ok(Json(fetch_job(&state, &user, id).await?))
}

/// `POST /api/v1/jobs/{id}/replace` : le résultat prend la place de
/// l'original, qui part dans la corbeille (ou est effacé s'il était dans
/// Temporaire).
async fn replace(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    Ok(Json(replace_original(&state, &user, id).await?))
}

pub(crate) async fn replace_original(
    state: &AppState,
    user: &ApiUser,
    id: i64,
) -> ApiResult<serde_json::Value> {
    worker_on(state)?;
    let replaced = tools::replace_with_result(state, user.who(), id).await?;
    Ok(json!({
        "file": fetch_file(&state.db, user, replaced.file).await?,
        "original_in_trash": replaced.original_in_trash,
    }))
}
