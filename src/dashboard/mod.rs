//! Page d'accueil : en-tête et widgets de l'espace actif.
//!
//! La page ne contient que des emplacements vides. Chaque widget est un
//! fragment chargé à part par htmx (`hx-trigger="load"`) : un widget lent ou
//! en erreur ne retarde pas les autres.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::Response;
use chrono::{NaiveDate, Timelike};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::AppState;
use crate::auth::session::{Ctx, Shell};
use crate::calendar::ical;
use crate::time;
use crate::web::{self, AppError, AppResult};

const TASK_MAX_CHARS: usize = 200;

#[derive(Template)]
#[template(path = "dashboard/page.html")]
struct DashboardPage {
    shell: Shell,
    greeting: &'static str,
    date: String,
    time: String,
}

/// `GET /`
pub async fn page(ctx: Ctx) -> AppResult<Response> {
    let now = time::now();
    let greeting = match time::paris(now).hour() {
        5..=11 => "Bonjour",
        12..=17 => "Bon après-midi",
        _ => "Bonsoir",
    };
    let page = DashboardPage {
        greeting,
        date: time::weekday_day_month(time::paris_date(now)),
        time: time::hm(now),
        shell: ctx.shell("dashboard").await?,
    };
    Ok(web::html(page))
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
}

#[derive(Template)]
#[template(path = "dashboard/_courses.html")]
struct CoursesWidget {
    items: Vec<CourseItem>,
    has_sources: bool,
}

/// `GET /widgets/courses` : les 3 prochains événements de l'espace actif.
pub async fn courses(ctx: Ctx, State(state): State<AppState>) -> AppResult<Response> {
    let now = time::now();
    let today = time::paris_date(now);
    let (user, space) = (ctx.user.id, ctx.active.id);

    let items = sqlx::query!(
        r#"SELECT e.title, e.location, e.starts_at, e.ends_at
           FROM calendar_events e
           JOIN calendar_sources s ON s.id = e.source_id
           WHERE s.user_id = ? AND s.space_id = ? AND e.all_day = 0 AND e.ends_at > ?
           ORDER BY e.starts_at
           LIMIT 3"#,
        user,
        space,
        now
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
    })
    .collect();

    let has_sources = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "n!: i64" FROM calendar_sources WHERE user_id = ? AND space_id = ?"#,
        user,
        space
    )
    .fetch_one(&state.db)
    .await?
        > 0;

    Ok(web::html(CoursesWidget { items, has_sources }))
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

/// `GET /widgets/notes` : les notes épinglées, puis les 5 dernières modifiées.
pub async fn recent_notes(ctx: Ctx, State(state): State<AppState>) -> AppResult<Response> {
    let now = time::now();
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
        } else if recent.len() < 5 {
            recent.push(link);
        }
    }
    Ok(web::html(NotesWidget { pinned, recent }))
}
