//! API, lot « agenda » (SPEC, « API complète et corbeille ») : les
//! événements créés dans dashu (et leur copie dans Google Agenda si
//! l'agenda est relié), et les sources iCal. Mêmes fonctions et mêmes
//! vérifications que le site : un événement est à son auteur ; une source
//! iCal n'accepte qu'une adresse HTTPS publique (sinon un assistant trompé
//! pourrait faire lire à dashu une adresse du réseau local).
//!
//! Un événement supprimé l'est pour de bon (il ne passe pas par la
//! corbeille), comme sur le site.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use super::{ApiError, ApiResult, ApiUser, iso};
use crate::AppState;
use crate::calendar::{events, sync};
use crate::time;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/events", axum::routing::post(create_event))
        .route(
            "/events/{id}",
            get(get_event).patch(update_event).delete(delete_event),
        )
        .route("/calendar/sources", get(list_sources).post(add_source))
        .route("/calendar/sources/{id}", delete(remove_source))
}

// ---------------------------------------------------------------------
// Événements
// ---------------------------------------------------------------------

#[derive(Serialize)]
pub(crate) struct EventOut {
    id: i64,
    title: String,
    location: String,
    start: String,
    end: String,
    all_day: bool,
    space: String,
    /// À copier dans Google Agenda.
    google: bool,
    /// `copied` (dans Google Agenda), `pending` (envoi en attente), `error`
    /// (avec `google_error`), ou vide s'il ne va pas chez Google.
    google_status: &'static str,
    google_error: Option<String>,
}

pub(crate) async fn fetch_event(state: &AppState, user: &ApiUser, id: i64) -> ApiResult<EventOut> {
    let event = events::owned(state, user.id, id)
        .await
        .map_err(|_| ApiError::not_found("événement"))?;
    let google_status = match (
        event.google,
        &event.google_error,
        event.google_dirty > 0 || event.google_event_id.is_none(),
    ) {
        (false, _, _) => "",
        (true, Some(_), _) => "error",
        (true, None, true) => "pending",
        (true, None, false) => "copied",
    };
    Ok(EventOut {
        id,
        title: event.title,
        location: event.location,
        start: iso(event.starts_at),
        end: iso(event.ends_at),
        all_day: event.all_day,
        space: user.slug_of(event.space_id),
        google: event.google,
        google_status,
        google_error: event.google_error,
    })
}

#[derive(Deserialize, Default)]
pub(crate) struct EventBody {
    pub(crate) title: Option<String>,
    /// Le jour, `AAAA-MM-JJ`.
    pub(crate) date: Option<String>,
    /// `HH:MM`, à l'heure de Paris (sauf `all_day`).
    pub(crate) start: Option<String>,
    pub(crate) end: Option<String>,
    pub(crate) all_day: Option<bool>,
    pub(crate) location: Option<String>,
    pub(crate) space: Option<String>,
    /// Aussi dans Google Agenda (si l'agenda est relié).
    pub(crate) google: Option<bool>,
}

/// Le formulaire du site, rempli depuis le JSON : les mêmes vérifications.
fn as_form(
    user: &ApiUser,
    body: &EventBody,
    base: Option<&events::Stored>,
) -> ApiResult<events::EventForm> {
    let space_id = match (body.space.as_deref(), base) {
        (Some(slug), _) => user.space(slug)?.id,
        (None, Some(stored)) => stored.space_id,
        (None, None) => return Err(ApiError::bad_request("space est obligatoire")),
    };
    let all_day = body.all_day.or(base.map(|b| b.all_day)).unwrap_or(false);
    Ok(events::EventForm {
        title: body
            .title
            .clone()
            .or_else(|| base.map(|b| b.title.clone()))
            .unwrap_or_default(),
        date: body
            .date
            .clone()
            .or_else(|| base.map(|b| time::paris_date(b.starts_at).format("%Y-%m-%d").to_string()))
            .unwrap_or_default(),
        start: body
            .start
            .clone()
            .or_else(|| base.map(|b| time::hm(b.starts_at)))
            .unwrap_or_default(),
        end: body
            .end
            .clone()
            .or_else(|| base.map(|b| time::hm(b.ends_at)))
            .unwrap_or_default(),
        all_day: all_day.then(|| "on".to_string()),
        location: body
            .location
            .clone()
            .or_else(|| base.map(|b| b.location.clone()))
            .unwrap_or_default(),
        space_id: space_id.to_string(),
        google: None,
    })
}

/// `POST /api/v1/events` `{"title", "date", "start"?, "end"?, "all_day"?, "location"?, "space", "google"?}`
async fn create_event(
    user: ApiUser,
    State(state): State<AppState>,
    payload: Result<Json<EventBody>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<EventOut>)> {
    let Json(body) = payload?;
    Ok((
        StatusCode::CREATED,
        Json(event_create(&state, &user, body).await?),
    ))
}

pub(crate) async fn event_create(
    state: &AppState,
    user: &ApiUser,
    body: EventBody,
) -> ApiResult<EventOut> {
    let form = as_form(user, &body, None)?;
    let valid = events::validate_in(&user.spaces, &form).map_err(ApiError::bad_request)?;
    let id = events::insert(state, user.id, &valid, body.google.unwrap_or(true)).await?;
    fetch_event(state, user, id).await
}

/// `GET /api/v1/events/{id}`
async fn get_event(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<EventOut>> {
    Ok(Json(fetch_event(&state, &user, id).await?))
}

/// `PATCH /api/v1/events/{id}` : les champs donnés changent, les autres restent.
async fn update_event(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    payload: Result<Json<EventBody>, JsonRejection>,
) -> ApiResult<Json<EventOut>> {
    let Json(body) = payload?;
    Ok(Json(event_update(&state, &user, id, body).await?))
}

pub(crate) async fn event_update(
    state: &AppState,
    user: &ApiUser,
    id: i64,
    body: EventBody,
) -> ApiResult<EventOut> {
    let stored = events::owned(state, user.id, id)
        .await
        .map_err(|_| ApiError::not_found("événement"))?;
    let form = as_form(user, &body, Some(&stored))?;
    let valid = events::validate_in(&user.spaces, &form).map_err(ApiError::bad_request)?;
    events::save(state, user.id, id, &valid, body.google).await?;
    fetch_event(state, user, id).await
}

/// `DELETE /api/v1/events/{id}` : supprimé pour de bon, sa copie chez Google aussi.
async fn delete_event(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    event_delete(&state, &user, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn event_delete(state: &AppState, user: &ApiUser, id: i64) -> ApiResult<()> {
    events::remove(state, user.id, id)
        .await
        .map_err(|_| ApiError::not_found("événement"))?;
    tracing::info!(
        user = user.id,
        via = user.via,
        event = id,
        "événement supprimé"
    );
    Ok(())
}

// ---------------------------------------------------------------------
// Sources iCal
// ---------------------------------------------------------------------

#[derive(Serialize)]
pub(crate) struct SourceOut {
    id: i64,
    name: String,
    space: String,
    /// Le domaine seul : le reste de l'adresse porte un jeton d'accès.
    host: String,
    last_sync: Option<String>,
    last_error: Option<String>,
}

/// `GET /api/v1/calendar/sources` : les agendas iCal du compte (l'adresse
/// complète, un secret, n'est jamais redonnée).
async fn list_sources(
    user: ApiUser,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<SourceOut>>> {
    Ok(Json(sources(&state, &user).await?))
}

pub(crate) async fn sources(state: &AppState, user: &ApiUser) -> ApiResult<Vec<SourceOut>> {
    Ok(sqlx::query!(
        r#"SELECT id AS "id!", space_id, name, ical_url, last_fetched_at, last_error
           FROM calendar_sources WHERE user_id = ? ORDER BY name"#,
        user.id
    )
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|s| SourceOut {
        id: s.id,
        name: s.name,
        space: user.slug_of(s.space_id),
        host: sync::mask_url(&s.ical_url),
        last_sync: s.last_fetched_at.map(iso),
        last_error: s.last_error,
    })
    .collect())
}

#[derive(Deserialize)]
pub(crate) struct NewSource {
    pub(crate) name: String,
    /// Le lien iCal (HTTPS, adresse publique ; `webcal://` accepté).
    pub(crate) url: String,
    pub(crate) space: String,
}

/// `POST /api/v1/calendar/sources` `{"name", "url", "space"}` : ajoute un
/// agenda iCal ; sa première synchronisation part aussitôt, en arrière-plan.
async fn add_source(
    user: ApiUser,
    State(state): State<AppState>,
    payload: Result<Json<NewSource>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<SourceOut>)> {
    let Json(source) = payload?;
    Ok((
        StatusCode::CREATED,
        Json(source_add(&state, &user, source).await?),
    ))
}

pub(crate) async fn source_add(
    state: &AppState,
    user: &ApiUser,
    source: NewSource,
) -> ApiResult<SourceOut> {
    let space = user.space(&source.space)?.id;
    let (id, _, url) =
        crate::settings::add_source(&state.db, user.id, space, &source.name, &source.url)
            .await?
            .map_err(ApiError::bad_request)?;
    tracing::info!(
        user = user.id,
        via = user.via,
        source = id,
        "agenda iCal ajouté"
    );
    let background = state.clone();
    tokio::spawn(async move {
        let _ = sync::sync_source(&background, id, &url).await;
    });
    sources(state, user)
        .await?
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| ApiError::not_found("agenda"))
}

/// `DELETE /api/v1/calendar/sources/{id}` : l'agenda et ses événements importés.
async fn remove_source(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    source_remove(&state, &user, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn source_remove(state: &AppState, user: &ApiUser, id: i64) -> ApiResult<()> {
    let deleted = sqlx::query!(
        "DELETE FROM calendar_sources WHERE id = ? AND user_id = ?",
        id,
        user.id
    )
    .execute(&state.db)
    .await?
    .rows_affected();
    if deleted == 0 {
        return Err(ApiError::not_found("agenda"));
    }
    Ok(())
}
