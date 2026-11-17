//! Événements créés dans dashu (SPEC, « Événements et Google Agenda ») :
//! formulaire de création et de modification, suppression. Comme les agendas
//! iCal, un événement est à son auteur, même dans un espace partagé.
//!
//! Un événement coché « Aussi dans Google Agenda » est copié chez Google par
//! `calendar::google` ; ici, on note seulement qu'il y a un changement à
//! envoyer (`google_dirty`) et on lance l'envoi.

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{Duration, NaiveDate, NaiveTime, Timelike};
use serde::Deserialize;

use super::google;
use crate::AppState;
use crate::auth::session::{Ctx, Shell};
use crate::time;
use crate::web::{self, AppError, AppResult, see_other};

const TITLE_MAX_CHARS: usize = 200;
const LOCATION_MAX_CHARS: usize = 200;

#[derive(Template)]
#[template(path = "calendar/event.html")]
struct EventPage {
    shell: Shell,
    /// `None` : nouvel événement.
    id: Option<i64>,
    title: String,
    date: String,
    start: String,
    end: String,
    all_day: bool,
    location: String,
    space_id: i64,
    /// Adresse du compte Google relié, si Google Agenda est connecté.
    google_email: Option<String>,
    /// Google est configuré sur ce serveur (sinon, rien n'en parle).
    google_available: bool,
    google: bool,
    /// État de la copie dans Google : (`ok`, `pending` ou `error`, texte).
    sync: Option<(&'static str, String)>,
    error: Option<String>,
    back: String,
}

#[derive(Deserialize)]
pub struct NewQuery {
    date: Option<String>,
}

#[derive(Deserialize)]
pub struct EventForm {
    title: String,
    date: String,
    #[serde(default)]
    start: String,
    #[serde(default)]
    end: String,
    all_day: Option<String>,
    #[serde(default)]
    location: String,
    space_id: String,
    google: Option<String>,
}

/// Un formulaire valide.
struct Valid {
    title: String,
    location: String,
    starts_at: i64,
    ends_at: i64,
    all_day: bool,
    space_id: i64,
    date: NaiveDate,
}

fn parse_time(text: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(text.trim(), "%H:%M").ok()
}

/// Vérifie le formulaire ; le message d'erreur s'affiche au-dessus.
fn validate(ctx: &Ctx, form: &EventForm) -> Result<Valid, String> {
    let title: String = form.title.trim().chars().take(TITLE_MAX_CHARS).collect();
    if title.is_empty() {
        return Err("Donne un titre à l'événement.".into());
    }
    let location: String = form
        .location
        .trim()
        .chars()
        .take(LOCATION_MAX_CHARS)
        .collect();
    let date = NaiveDate::parse_from_str(form.date.trim(), "%Y-%m-%d")
        .map_err(|_| "Le jour n'est pas une date valide.".to_string())?;
    let space_id: i64 = form
        .space_id
        .trim()
        .parse()
        .map_err(|_| "Choisis un espace.".to_string())?;
    let space = ctx
        .space_by_id(space_id)
        .filter(|s| !s.archived)
        .ok_or("Cet espace n'existe pas ou est archivé.")?;
    let all_day = form.all_day.is_some();
    let (starts_at, ends_at) = if all_day {
        (
            time::day_start(date),
            time::day_start(date + Duration::days(1)),
        )
    } else {
        let start = parse_time(&form.start).ok_or("Donne une heure de début.")?;
        let end = parse_time(&form.end).ok_or("Donne une heure de fin.")?;
        if end <= start {
            return Err("La fin doit venir après le début.".into());
        }
        (
            time::local_to_ts(date.and_time(start), time::PARIS),
            time::local_to_ts(date.and_time(end), time::PARIS),
        )
    };
    Ok(Valid {
        title,
        location,
        starts_at,
        ends_at,
        all_day,
        space_id: space.id,
        date,
    })
}

/// Agenda de l'espace de l'événement, sur sa semaine.
fn calendar_url(ctx: &Ctx, space_id: i64, date: NaiveDate) -> String {
    let path = format!("/calendar?view=week&date={}", date.format("%Y-%m-%d"));
    match ctx.space_by_id(space_id) {
        Some(space) => crate::spaces::url::prefixed(&space.slug, &path),
        None => path,
    }
}

/// « 09:00 » de l'heure ronde qui suit maintenant, et une heure après.
fn default_times(date: NaiveDate, now: i64) -> (String, String) {
    let local = time::paris(now);
    let hour = if date == time::paris_date(now) {
        (local.hour() + 1).min(22)
    } else {
        9
    };
    (format!("{hour:02}:00"), format!("{:02}:00", hour + 1))
}

/// `GET /calendar/events/new?date=2026-11-18`
pub async fn new_page(
    ctx: Ctx,
    State(state): State<AppState>,
    Query(query): Query<NewQuery>,
) -> AppResult<Response> {
    let now = time::now();
    let date = query
        .date
        .as_deref()
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .unwrap_or_else(|| time::paris_date(now));
    let (start, end) = default_times(date, now);
    let linked = google::linked(&state, ctx.user.id).await?;
    let page = EventPage {
        id: None,
        title: String::new(),
        date: date.format("%Y-%m-%d").to_string(),
        start,
        end,
        all_day: false,
        location: String::new(),
        space_id: ctx.active.id,
        google: linked.is_some(),
        google_email: linked.map(|l| l.email),
        google_available: state.config.google.is_some(),
        sync: None,
        error: None,
        back: calendar_url(&ctx, ctx.active.id, date),
        shell: ctx.shell("calendar").await?,
    };
    Ok(web::html(page))
}

/// Le formulaire à nouveau, avec ce qui a été tapé et le message d'erreur.
async fn invalid(
    ctx: Ctx,
    state: &AppState,
    id: Option<i64>,
    form: EventForm,
    error: String,
) -> AppResult<Response> {
    let linked = google::linked(state, ctx.user.id).await?;
    let date = NaiveDate::parse_from_str(form.date.trim(), "%Y-%m-%d")
        .unwrap_or_else(|_| time::paris_date(time::now()));
    let page = EventPage {
        id,
        title: form.title,
        date: form.date,
        start: form.start,
        end: form.end,
        all_day: form.all_day.is_some(),
        location: form.location,
        space_id: form.space_id.parse().unwrap_or(ctx.active.id),
        google: form.google.is_some(),
        google_email: linked.map(|l| l.email),
        google_available: state.config.google.is_some(),
        sync: None,
        error: Some(error),
        back: calendar_url(&ctx, ctx.active.id, date),
        shell: ctx.shell("calendar").await?,
    };
    Ok((StatusCode::UNPROCESSABLE_ENTITY, web::html(page)).into_response())
}

/// `POST /calendar/events`
pub async fn create(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<EventForm>,
) -> AppResult<Response> {
    let valid = match validate(&ctx, &form) {
        Ok(valid) => valid,
        Err(error) => return invalid(ctx, &state, None, form, error).await,
    };
    // Vers Google seulement si un agenda est relié.
    let linked = google::linked(&state, ctx.user.id).await?.is_some();
    let to_google = linked && form.google.is_some();
    let now = time::now();
    sqlx::query!(
        "INSERT INTO events (user_id, space_id, title, location, starts_at, ends_at, all_day,
                             google, google_dirty, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        ctx.user.id,
        valid.space_id,
        valid.title,
        valid.location,
        valid.starts_at,
        valid.ends_at,
        valid.all_day,
        to_google,
        to_google,
        now,
        now
    )
    .execute(&state.db)
    .await?;
    if to_google {
        google::push_soon(&state, ctx.user.id);
    }
    ctx.flash_ok(format!("Événement « {} » ajouté", valid.title))
        .await?;
    Ok(see_other(&calendar_url(&ctx, valid.space_id, valid.date)))
}

/// Un événement tel qu'il est en base.
struct Stored {
    title: String,
    location: String,
    starts_at: i64,
    ends_at: i64,
    all_day: bool,
    space_id: i64,
    google: bool,
    google_event_id: Option<String>,
    google_dirty: i64,
    google_error: Option<String>,
}

/// L'événement de l'utilisateur, ou 404.
async fn owned(state: &AppState, user: i64, id: i64) -> AppResult<Stored> {
    sqlx::query_as!(
        Stored,
        r#"SELECT title, location, starts_at, ends_at, all_day AS "all_day!: bool", space_id,
                  google AS "google!: bool", google_event_id, google_dirty, google_error
           FROM events WHERE id = ? AND user_id = ?"#,
        id,
        user
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)
}

/// `GET /calendar/events/{id}`
pub async fn edit_page(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let event = owned(&state, ctx.user.id, id).await?;
    let date = time::paris_date(event.starts_at);
    let linked = google::linked(&state, ctx.user.id).await?;
    let sync = match (event.google, linked.is_some()) {
        (false, _) => None,
        (true, false) => Some((
            "pending",
            "Pas dans Google Agenda : Google Agenda n'est pas connecté".to_string(),
        )),
        (true, true) => Some(
            match (
                event.google_error,
                event.google_dirty > 0,
                event.google_event_id.is_some(),
            ) {
                (Some(error), _, _) => {
                    ("error", format!("Pas encore dans Google Agenda : {error}"))
                }
                (None, true, _) | (None, false, false) => {
                    ("pending", "Envoi à Google Agenda en cours".to_string())
                }
                (None, false, true) => ("ok", "Dans Google Agenda".to_string()),
            },
        ),
    };
    let page = EventPage {
        id: Some(id),
        title: event.title,
        date: date.format("%Y-%m-%d").to_string(),
        start: time::hm(event.starts_at),
        end: time::hm(event.ends_at),
        all_day: event.all_day,
        location: event.location,
        space_id: event.space_id,
        google: event.google,
        google_email: linked.map(|l| l.email),
        google_available: state.config.google.is_some(),
        sync,
        error: None,
        back: calendar_url(&ctx, event.space_id, date),
        shell: ctx.shell("calendar").await?,
    };
    Ok(web::html(page))
}

/// `POST /calendar/events/{id}`
pub async fn update(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<EventForm>,
) -> AppResult<Response> {
    let stored = owned(&state, ctx.user.id, id).await?;
    let valid = match validate(&ctx, &form) {
        Ok(valid) => valid,
        Err(error) => return invalid(ctx, &state, Some(id), form, error).await,
    };
    // Sans agenda relié, la case n'est pas montrée : on garde le choix d'avant.
    let linked = google::linked(&state, ctx.user.id).await?.is_some();
    let to_google = if linked {
        form.google.is_some()
    } else {
        stored.google
    };
    // Une copie à créer, à modifier ou à retirer chez Google.
    let push = to_google || stored.google_event_id.is_some();
    let now = time::now();
    sqlx::query!(
        "UPDATE events SET space_id = ?, title = ?, location = ?, starts_at = ?, ends_at = ?,
                           all_day = ?, google = ?, google_dirty = google_dirty + ?,
                           google_error = NULL, updated_at = ?
         WHERE id = ? AND user_id = ?",
        valid.space_id,
        valid.title,
        valid.location,
        valid.starts_at,
        valid.ends_at,
        valid.all_day,
        to_google,
        push,
        now,
        id,
        ctx.user.id
    )
    .execute(&state.db)
    .await?;
    if push && linked {
        google::push_soon(&state, ctx.user.id);
    }
    ctx.flash_ok(format!("Événement « {} » enregistré", valid.title))
        .await?;
    Ok(see_other(&calendar_url(&ctx, valid.space_id, valid.date)))
}

/// `POST /calendar/events/{id}/delete`
pub async fn delete(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let event = owned(&state, ctx.user.id, id).await?;
    let mut tx = state.db.begin().await?;
    if let Some(google_id) = &event.google_event_id {
        sqlx::query!(
            "INSERT OR IGNORE INTO google_deletions (user_id, google_event_id) VALUES (?, ?)",
            ctx.user.id,
            google_id
        )
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query!(
        "DELETE FROM events WHERE id = ? AND user_id = ?",
        id,
        ctx.user.id
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    if event.google_event_id.is_some() {
        google::push_soon(&state, ctx.user.id);
    }
    ctx.flash_ok(format!("Événement « {} » supprimé", event.title))
        .await?;
    Ok(see_other(&calendar_url(
        &ctx,
        event.space_id,
        time::paris_date(event.starts_at),
    )))
}
