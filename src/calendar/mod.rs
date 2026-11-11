//! Emploi du temps : vues jour et semaine des événements importés par iCal.

pub mod ical;
pub mod sync;
pub mod view;

use askama::Template;
use axum::extract::{Query, State};
use axum::response::Response;
use chrono::{Datelike, Duration, NaiveDate};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::AppState;
use crate::auth::session::{Ctx, Shell};
use crate::spaces::Space;
use crate::time;
use crate::web::{self, AppResult, local_path, see_other};
use view::{EventRow, Grid};

/// Perso sert de vue d'ensemble : son agenda réunit les agendas de tous les
/// espaces (cours d'École, planning de Taff, et ses propres sources). Les
/// autres espaces n'affichent que leurs propres sources.
pub fn is_overview(space: &Space) -> bool {
    space.slug == "perso"
}

#[derive(Deserialize)]
pub struct PageQuery {
    /// `week` (par défaut) ou `day`.
    view: Option<String>,
    /// Jour affiché, `AAAA-MM-JJ` (par défaut : aujourd'hui).
    date: Option<String>,
}

/// État de synchro d'une source, affiché quand la dernière tentative a échoué.
struct SourceIssue {
    name: String,
    error: String,
    /// « dernière synchro réussie il y a 3 h », ou « jamais synchronisé ».
    last_ok: String,
}

#[derive(Template)]
#[template(path = "calendar/page.html")]
struct CalendarPage {
    shell: Shell,
    title: String,
    is_week: bool,
    grid: Grid,
    prev_href: String,
    next_href: String,
    today_href: String,
    week_href: String,
    day_href: String,
    /// URL de la page courante, pour y revenir après « Rafraîchir ».
    self_href: String,
    has_sources: bool,
    /// « Synchronisé il y a 12 min »
    sync_label: String,
    issues: Vec<SourceIssue>,
}

/// `GET /calendar?view=week&date=2026-11-10`
pub async fn page(
    ctx: Ctx,
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> AppResult<Response> {
    let now = time::now();
    let today = time::paris_date(now);
    let date = query
        .date
        .as_deref()
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .unwrap_or(today);
    let is_week = query.view.as_deref() != Some("day");
    let (user, space) = (ctx.user.id, ctx.active.id);
    let overview = is_overview(&ctx.active);

    // Jours demandés à la base : la semaine entière (lundi -> dimanche) ou un seul jour.
    let monday = time::monday_of(date);
    let (first, last) = if is_week {
        (monday, monday + Duration::days(6))
    } else {
        (date, date)
    };
    let events = events_between(&state.db, user, space, overview, first, last).await?;

    let dates: Vec<NaiveDate> = if is_week {
        // Samedi et dimanche ne s'affichent que s'il s'y passe quelque chose.
        let saturday = time::day_start(monday + Duration::days(5));
        let weekend = events.iter().any(|e| e.ends_at > saturday);
        (0..if weekend { 7 } else { 5 })
            .map(|i| monday + Duration::days(i))
            .collect()
    } else {
        vec![date]
    };
    let grid = view::build(&dates, &events, now);

    let step = if is_week { 7 } else { 1 };
    let view_name = if is_week { "week" } else { "day" };
    let href = |view: &str, day: NaiveDate| {
        format!("/calendar?view={view}&date={}", day.format("%Y-%m-%d"))
    };

    let title = if is_week {
        format!("Semaine du {} {}", time::day_month(monday), monday.year())
    } else {
        capitalize(&time::date_long(date))
    };

    let sources = sqlx::query!(
        r#"SELECT name, last_fetched_at, last_error FROM calendar_sources
           WHERE user_id = ?1 AND (?3 OR space_id = ?2) ORDER BY name"#,
        user,
        space,
        overview
    )
    .fetch_all(&state.db)
    .await?;

    let last_sync = sources.iter().filter_map(|s| s.last_fetched_at).min();
    let sync_label = match last_sync {
        Some(ts) => format!("Synchronisé {}", time::ago(ts, now)),
        None if sources.is_empty() => String::new(),
        None => "Jamais synchronisé".into(),
    };
    let sync_label = match (overview, sync_label.is_empty()) {
        (true, true) => "Vue d'ensemble de tous les espaces".to_string(),
        (true, false) => format!("Vue d'ensemble de tous les espaces · {sync_label}"),
        (false, _) => sync_label,
    };
    let issues = sources
        .iter()
        .filter_map(|s| {
            s.last_error.as_ref().map(|error| SourceIssue {
                name: s.name.clone(),
                error: error.clone(),
                last_ok: match s.last_fetched_at {
                    Some(ts) => format!("dernière synchro réussie {}", time::ago(ts, now)),
                    None => "jamais synchronisé".into(),
                },
            })
        })
        .collect();

    let page = CalendarPage {
        title,
        is_week,
        grid,
        prev_href: href(view_name, date - Duration::days(step)),
        next_href: href(view_name, date + Duration::days(step)),
        today_href: href(view_name, today),
        week_href: href("week", date),
        day_href: href("day", date),
        self_href: href(view_name, date),
        has_sources: !sources.is_empty(),
        sync_label,
        issues,
        shell: ctx.shell("calendar").await?,
    };
    Ok(web::html(page))
}

/// Événements qui touchent la période [first, last] (jours inclus) : ceux de
/// l'espace, ou ceux de tous les espaces si `overview`. Un événement venu d'un
/// autre espace porte le nom et la couleur de cet espace.
async fn events_between(
    db: &SqlitePool,
    user: i64,
    space: i64,
    overview: bool,
    first: NaiveDate,
    last: NaiveDate,
) -> AppResult<Vec<EventRow>> {
    let from = time::day_start(first);
    let to = time::day_start(last + Duration::days(1));
    let events = sqlx::query_as!(
        EventRow,
        r#"SELECT e.title, e.location, e.starts_at, e.ends_at, e.all_day AS "all_day!: bool",
                  CASE WHEN s.space_id = ?2 THEN '' ELSE sp.name END AS "tag!: String",
                  CASE WHEN s.space_id = ?2 THEN '' ELSE sp.color END AS "tag_color!: String"
           FROM calendar_events e
           JOIN calendar_sources s ON s.id = e.source_id
           JOIN spaces sp ON sp.id = s.space_id
           WHERE s.user_id = ?1 AND (?3 OR s.space_id = ?2) AND e.ends_at > ?4 AND e.starts_at < ?5
           ORDER BY e.starts_at, e.ends_at"#,
        user,
        space,
        overview,
        from,
        to
    )
    .fetch_all(db)
    .await?;
    Ok(events)
}

#[derive(Deserialize)]
pub struct RefreshForm {
    next: Option<String>,
}

/// `POST /calendar/refresh` : resynchronise tout de suite les sources de l'espace actif
/// (toutes, depuis Perso).
pub async fn refresh(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<RefreshForm>,
) -> AppResult<Response> {
    // Depuis Perso (vue d'ensemble), on resynchronise toutes les sources.
    let overview = is_overview(&ctx.active);
    let sources = sqlx::query!(
        r#"SELECT id AS "id!", ical_url FROM calendar_sources
           WHERE user_id = ?1 AND (?3 OR space_id = ?2)"#,
        ctx.user.id,
        ctx.active.id,
        overview
    )
    .fetch_all(&state.db)
    .await?;

    let mut failures = 0;
    for source in &sources {
        if sync::sync_source(&state, source.id, &source.ical_url)
            .await
            .is_err()
        {
            failures += 1;
        }
    }
    match (sources.len(), failures) {
        (0, _) => {}
        (_, 0) => ctx.flash_ok("Agenda à jour").await?,
        _ => {
            ctx.flash_error("La synchro a échoué : la dernière version connue reste affichée.")
                .await?
        }
    }

    let next = local_path(form.next.as_deref());
    Ok(see_other(if next.starts_with("/calendar") {
        next
    } else {
        "/calendar"
    }))
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}
