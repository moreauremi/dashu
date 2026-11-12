//! Page d'accueil : en-tête et widgets de l'espace actif, en grille bento.
//!
//! Chaque espace a sa disposition (voir [`layout`]) : quels widgets, dans
//! quel ordre, de quelle taille. On la modifie sur la page même (mode
//! édition, static/js/dashboard.js), qui l'envoie à `POST /dashboard/layout`.
//!
//! La page ne contient que des emplacements vides. Chaque widget est un
//! fragment chargé à part par htmx (`hx-trigger="load"`), qui reçoit sa
//! taille pour n'afficher que ce qui tient : un widget lent ou en erreur ne
//! retarde pas les autres.

pub mod layout;

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{NaiveDate, Timelike};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::AppState;
use crate::auth::session::{Ctx, Shell};
use crate::calendar::{self, ical};
use crate::files;
use crate::time;
use crate::web::{self, AppError, AppResult};
use layout::{Kind, SizeQuery, Widget};

const TASK_MAX_CHARS: usize = 200;

#[derive(Template)]
#[template(path = "dashboard/page.html")]
struct DashboardPage {
    shell: Shell,
    greeting: &'static str,
    date: String,
    time: String,
    widgets: Vec<Widget>,
    /// Widgets qu'on peut ajouter (tous, sauf Fichiers s'il n'est pas activé).
    catalog: Vec<&'static Kind>,
}

#[derive(Template)]
#[template(path = "dashboard/_grid.html")]
struct GridFragment {
    widgets: Vec<Widget>,
}

/// Types de widgets disponibles sur ce serveur.
fn catalog(state: &AppState) -> Vec<&'static Kind> {
    let files = state.config.files.is_some();
    layout::KINDS
        .iter()
        .filter(|k| files || k.key != "files")
        .collect()
}

/// `GET /`
pub async fn page(ctx: Ctx, State(state): State<AppState>) -> AppResult<Response> {
    let now = time::now();
    let greeting = match time::paris(now).hour() {
        5..=11 => "Bonjour",
        12..=17 => "Bon après-midi",
        _ => "Bonsoir",
    };
    let widgets = layout::load(&state.db, ctx.active.id, state.config.files.is_some()).await?;
    let page = DashboardPage {
        widgets,
        catalog: catalog(&state),
        greeting,
        date: time::weekday_day_month(time::paris_date(now)),
        time: time::hm(now),
        shell: ctx.shell("dashboard").await?,
    };
    Ok(web::html(page))
}

/// `GET /dashboard/grid` : la grille seule, rechargée après l'ajout d'un widget.
pub async fn grid(ctx: Ctx, State(state): State<AppState>) -> AppResult<Response> {
    let widgets = layout::load(&state.db, ctx.active.id, state.config.files.is_some()).await?;
    Ok(web::html(GridFragment { widgets }))
}

#[derive(Deserialize)]
pub struct LayoutForm {
    /// « courses:2x2,tasks:2x2 » ; vide : retour à la disposition par défaut.
    #[serde(default)]
    layout: String,
    /// « 1 » : disposition vide voulue (tous les widgets retirés), et non
    /// retour à celle par défaut.
    #[serde(default)]
    empty: Option<String>,
}

/// `POST /dashboard/layout` : enregistre la disposition de l'espace actif.
pub async fn save_layout(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<LayoutForm>,
) -> AppResult<Response> {
    let widgets = layout::parse(&form.layout).map_err(AppError::BadRequest)?;
    let keep_empty = form.empty.as_deref() == Some("1");
    let value = (!widgets.is_empty() || keep_empty).then_some(widgets.as_slice());
    layout::save(&state.db, ctx.user.id, ctx.active.id, value).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ---------------------------------------------------------------------
// Widget « Calendrier »
// ---------------------------------------------------------------------

#[derive(Template)]
#[template(path = "dashboard/_calendar.html")]
struct CalendarWidget {
    grid: calendar::view::Grid,
    has_sources: bool,
    overview: bool,
}

/// `GET /widgets/calendar?w=2&h=3` : la grille de la page Agenda, sur
/// aujourd'hui (1 colonne), trois jours (2 colonnes) ou la semaine (au-delà ;
/// aussi sans taille). La hauteur ne change que la part visible : le widget
/// défile et s'ouvre sur l'heure en cours (static/js/dashboard.js).
pub async fn calendar_widget(
    ctx: Ctx,
    State(state): State<AppState>,
    Query(size): Query<SizeQuery>,
) -> AppResult<Response> {
    let columns = size.get().map_or(layout::MAX_SPAN, |(w, _)| w);
    let (grid, has_sources) =
        calendar::widget_grid(&state.db, ctx.user.id, &ctx.active, columns, time::now()).await?;
    Ok(web::html(CalendarWidget {
        grid,
        has_sources,
        overview: calendar::is_overview(&ctx.active),
    }))
}

// ---------------------------------------------------------------------
// Widget « Prochains cours »
// ---------------------------------------------------------------------

struct CourseItem {
    subject: String,
    details: String,
    location: String,
    /// « Aujourd'hui », « Demain », « ven. 13 nov. »
    day: String,
    /// « 08:00 – 09:55 »
    time: String,
    color: u8,
    /// Le cours a commencé et n'est pas fini.
    ongoing: bool,
    /// Depuis Perso, nom et couleur de l'espace d'où vient l'événement ;
    /// vides pour un événement de l'espace actif.
    tag: String,
    tag_color: String,
}

#[derive(Template)]
#[template(path = "dashboard/_courses.html")]
struct CoursesWidget {
    items: Vec<CourseItem>,
    has_sources: bool,
    /// Perso : les agendas de tous les espaces (voir `calendar::is_overview`).
    overview: bool,
}

/// `GET /widgets/courses?w=2&h=2` : les prochains événements de l'espace
/// actif (autant que la taille du widget en montre ; 3 sans taille), ou de
/// tous les espaces depuis Perso. Un événement venu d'un autre espace porte
/// le nom et la couleur de cet espace, comme sur la page Agenda.
pub async fn courses(
    ctx: Ctx,
    State(state): State<AppState>,
    Query(size): Query<SizeQuery>,
) -> AppResult<Response> {
    let now = time::now();
    let today = time::paris_date(now);
    let (user, space) = (ctx.user.id, ctx.active.id);
    let overview = calendar::is_overview(&ctx.active);
    let limit = size.items([1, 4, 7, 10], 3);

    let items = sqlx::query!(
        r#"SELECT e.title, e.location, e.starts_at, e.ends_at,
                  CASE WHEN s.space_id = ?2 THEN '' ELSE sp.name END AS "tag!: String",
                  CASE WHEN s.space_id = ?2 THEN '' ELSE sp.color END AS "tag_color!: String"
           FROM calendar_events e
           JOIN calendar_sources s ON s.id = e.source_id
           JOIN spaces sp ON sp.id = s.space_id
           WHERE s.user_id = ?1 AND (?3 OR s.space_id = ?2) AND e.all_day = 0 AND e.ends_at > ?4
           ORDER BY e.starts_at
           LIMIT ?5"#,
        user,
        space,
        overview,
        now,
        limit
    )
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|e| CourseItem {
        subject: ical::subject(&e.title).to_string(),
        details: ical::details(&e.title),
        location: e.location,
        day: time::relative_day(time::paris_date(e.starts_at), today),
        time: format!("{} – {}", time::hm(e.starts_at), time::hm(e.ends_at)),
        color: ical::subject_color(&e.title),
        ongoing: e.starts_at <= now,
        tag: e.tag,
        tag_color: e.tag_color,
    })
    .collect();

    let has_sources = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "n!: i64" FROM calendar_sources
           WHERE user_id = ?1 AND (?3 OR space_id = ?2)"#,
        user,
        space,
        overview
    )
    .fetch_one(&state.db)
    .await?
        > 0;

    Ok(web::html(CoursesWidget {
        items,
        has_sources,
        overview,
    }))
}

// ---------------------------------------------------------------------
// Widget « Tâches du jour »
// ---------------------------------------------------------------------

struct TaskItem {
    id: i64,
    title: String,
    done: bool,
    /// Pour une tâche en retard : sa date d'origine, « lun. 9 nov. ».
    late_since: Option<String>,
}

#[derive(Template)]
#[template(path = "dashboard/_tasks.html")]
struct TasksWidget {
    tasks: Vec<TaskItem>,
    done_count: usize,
}

/// `GET /widgets/tasks`
pub async fn tasks(ctx: Ctx, State(state): State<AppState>) -> AppResult<Response> {
    tasks_widget(&state.db, &ctx).await
}

/// Tâches du jour : celles d'aujourd'hui, celles en retard pas encore
/// faites, et celles cochées aujourd'hui (elles restent visibles, barrées,
/// jusqu'à minuit).
async fn tasks_widget(db: &SqlitePool, ctx: &Ctx) -> AppResult<Response> {
    let today = time::paris_date(time::now());
    let today_iso = today.format("%Y-%m-%d").to_string();
    let midnight = time::day_start(today);

    let tasks: Vec<TaskItem> = sqlx::query!(
        r#"SELECT id AS "id!", title, done AS "done!: bool", due_date
           FROM tasks
           WHERE user_id = ?1 AND space_id = ?2
             AND ((done = 0 AND (due_date IS NULL OR due_date <= ?3))
               OR (done = 1 AND done_at >= ?4))
           ORDER BY done, due_date, created_at"#,
        ctx.user.id,
        ctx.active.id,
        today_iso,
        midnight
    )
    .fetch_all(db)
    .await?
    .into_iter()
    .map(|t| {
        let late_since = t
            .due_date
            .as_deref()
            .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
            .filter(|due| !t.done && *due < today)
            .map(time::date_short);
        TaskItem {
            id: t.id,
            title: t.title,
            done: t.done,
            late_since,
        }
    })
    .collect();

    let done_count = tasks.iter().filter(|t| t.done).count();
    Ok(web::html(TasksWidget { tasks, done_count }))
}

#[derive(Deserialize)]
pub struct TaskForm {
    title: String,
}

/// `POST /tasks` : ajoute une tâche pour aujourd'hui dans l'espace actif.
pub async fn add_task(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<TaskForm>,
) -> AppResult<Response> {
    let title: String = form.title.trim().chars().take(TASK_MAX_CHARS).collect();
    if !title.is_empty() {
        let now = time::now();
        let today = time::paris_date(now).format("%Y-%m-%d").to_string();
        sqlx::query!(
            "INSERT INTO tasks (user_id, space_id, title, due_date, created_at) VALUES (?, ?, ?, ?, ?)",
            ctx.user.id,
            ctx.active.id,
            title,
            today,
            now
        )
        .execute(&state.db)
        .await?;
    }
    tasks_widget(&state.db, &ctx).await
}

/// `POST /tasks/{id}/toggle` : coche ou décoche.
pub async fn toggle_task(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let now = time::now();
    // Dans le SET, `done` désigne encore l'ancienne valeur : une tâche qu'on
    // coche reçoit l'heure courante, une tâche qu'on décoche la perd.
    let changed = sqlx::query!(
        "UPDATE tasks SET done_at = CASE WHEN done THEN NULL ELSE ? END, done = NOT done
         WHERE id = ? AND user_id = ?",
        now,
        id,
        ctx.user.id
    )
    .execute(&state.db)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(AppError::NotFound);
    }
    tasks_widget(&state.db, &ctx).await
}

/// `DELETE /tasks/{id}`
pub async fn delete_task(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let deleted = sqlx::query!(
        "DELETE FROM tasks WHERE id = ? AND user_id = ?",
        id,
        ctx.user.id
    )
    .execute(&state.db)
    .await?
    .rows_affected();
    if deleted == 0 {
        return Err(AppError::NotFound);
    }
    tasks_widget(&state.db, &ctx).await
}

// ---------------------------------------------------------------------
// Widget « Notes récentes »
// ---------------------------------------------------------------------

struct NoteLink {
    id: i64,
    title: String,
    updated: String,
}

#[derive(Template)]
#[template(path = "dashboard/_notes.html")]
struct NotesWidget {
    pinned: Vec<NoteLink>,
    recent: Vec<NoteLink>,
}

/// `GET /widgets/notes?w=2&h=1` : les notes épinglées, puis les dernières
/// modifiées (autant que la taille du widget en montre ; 5 sans taille).
pub async fn recent_notes(
    ctx: Ctx,
    State(state): State<AppState>,
    Query(size): Query<SizeQuery>,
) -> AppResult<Response> {
    let now = time::now();
    let limit = usize::try_from(size.items([2, 6, 10, 14], 5)).unwrap_or(5);
    let rows = sqlx::query!(
        r#"SELECT id AS "id!", title, pinned AS "pinned!: bool", updated_at
           FROM notes
           WHERE user_id = ? AND space_id = ?
           ORDER BY pinned DESC, updated_at DESC
           LIMIT 40"#,
        ctx.user.id,
        ctx.active.id
    )
    .fetch_all(&state.db)
    .await?;

    let (mut pinned, mut recent) = (Vec::new(), Vec::new());
    for row in rows {
        let link = NoteLink {
            id: row.id,
            title: if row.title.trim().is_empty() {
                "Sans titre".into()
            } else {
                row.title
            },
            updated: time::ago(row.updated_at, now),
        };
        if row.pinned {
            pinned.push(link);
        } else if recent.len() < limit {
            recent.push(link);
        }
    }
    Ok(web::html(NotesWidget { pinned, recent }))
}

// ---------------------------------------------------------------------
// Widget « Fichiers récents »
// ---------------------------------------------------------------------

struct FileLink {
    id: i64,
    name: String,
    /// « PDF · 2,4 Mo · il y a 3 h »
    meta: String,
    thumb: bool,
    icon: &'static str,
}

#[derive(Template)]
#[template(path = "dashboard/_files.html")]
struct FilesWidget {
    /// Faux si les fichiers ne sont pas activés sur ce serveur.
    enabled: bool,
    items: Vec<FileLink>,
    /// Fichiers de l'espace au total, s'il y en a plus que de lignes.
    total: i64,
}

/// `GET /widgets/files?w=2&h=1` : les derniers fichiers de l'espace actif.
pub async fn recent_files(
    ctx: Ctx,
    State(state): State<AppState>,
    Query(size): Query<SizeQuery>,
) -> AppResult<Response> {
    if state.config.files.is_none() {
        return Ok(web::html(FilesWidget {
            enabled: false,
            items: Vec::new(),
            total: 0,
        }));
    }
    let now = time::now();
    let limit = size.items([2, 5, 9, 13], 5);
    let items = sqlx::query!(
        r#"SELECT id AS "id!", name, mime, size, updated_at, thumb
           FROM files WHERE user_id = ? AND space_id = ?
           ORDER BY updated_at DESC, id DESC
           LIMIT ?"#,
        ctx.user.id,
        ctx.active.id,
        limit
    )
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|row| FileLink {
        id: row.id,
        meta: format!(
            "{} · {} · {}",
            files::kind_label(&row.mime),
            files::human_size(row.size),
            time::ago(row.updated_at, now)
        ),
        thumb: row.thumb == 1,
        icon: files::icon_for(&row.mime),
        name: row.name,
    })
    .collect();
    let total = sqlx::query_scalar!(
        "SELECT count(*) FROM files WHERE user_id = ? AND space_id = ?",
        ctx.user.id,
        ctx.active.id
    )
    .fetch_one(&state.db)
    .await?;
    Ok(web::html(FilesWidget {
        enabled: true,
        items,
        total,
    }))
}

// ---------------------------------------------------------------------
// Widget « Réglages rapides »
// ---------------------------------------------------------------------

#[derive(Template)]
#[template(path = "dashboard/_settings.html")]
struct SettingsWidget {
    /// `dark`, `light`, ou vide pour suivre le système.
    theme: String,
    compact: bool,
    /// « 2 agendas · synchronisés il y a 20 min », vide sans agenda.
    calendars: String,
    /// Agendas dont la dernière synchro a échoué.
    calendar_errors: i64,
    /// « 34 fichiers · 1,2 Go », vide si les fichiers ne sont pas activés.
    storage: String,
}

/// `GET /widgets/settings?w=2&h=1` : thème, état des agendas, place prise
/// par les fichiers (tous espaces confondus : ce sont des réglages du compte).
pub async fn quick_settings(
    ctx: Ctx,
    State(state): State<AppState>,
    Query(size): Query<SizeQuery>,
) -> AppResult<Response> {
    let now = time::now();
    let sources = sqlx::query!(
        r#"SELECT last_fetched_at, last_error IS NOT NULL AS "failed!: bool"
           FROM calendar_sources WHERE user_id = ?"#,
        ctx.user.id
    )
    .fetch_all(&state.db)
    .await?;
    let calendars = match sources.len() {
        0 => String::new(),
        n => {
            let count = if n == 1 {
                "1 agenda".to_string()
            } else {
                format!("{n} agendas")
            };
            match sources.iter().filter_map(|s| s.last_fetched_at).min() {
                Some(ts) => format!("{count} · synchronisés {}", time::ago(ts, now)),
                None => format!("{count} · jamais synchronisés"),
            }
        }
    };
    let calendar_errors = sources.iter().filter(|s| s.failed).count();

    let storage = if state.config.files.is_some() {
        let used = sqlx::query!(
            r#"SELECT count(*) AS "n!: i64", coalesce(sum(size), 0) AS "bytes!: i64"
               FROM files WHERE user_id = ?"#,
            ctx.user.id
        )
        .fetch_one(&state.db)
        .await?;
        let files = if used.n <= 1 {
            format!("{} fichier", used.n)
        } else {
            format!("{} fichiers", used.n)
        };
        format!("{files} · {}", files::human_size(used.bytes))
    } else {
        String::new()
    };

    Ok(web::html(SettingsWidget {
        theme: ctx.user.theme.clone().unwrap_or_default(),
        compact: size.compact(),
        calendars,
        calendar_errors: i64::try_from(calendar_errors).unwrap_or(0),
        storage,
    }))
}
