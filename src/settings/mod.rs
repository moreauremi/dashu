//! Réglages : thème, raccourcis du téléphone, mot de passe, compte Google lié,
//! sources iCal.

use std::collections::HashMap;

use askama::Template;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::Deserialize;

use crate::AppState;
use crate::auth::password;
use crate::auth::session::{Ctx, Shell};
use crate::calendar::sync;
use crate::tabbar::{self, Tab};
use crate::time;
use crate::web::{self, AppError, AppResult, local_path, see_other};

const SOURCE_NAME_MAX_CHARS: usize = 60;

struct SourceRow {
    id: i64,
    name: String,
    /// Domaine seul : le chemin de l'URL porte un jeton d'accès et reste caché.
    masked_url: String,
    space_name: String,
    space_color: String,
    status: String,
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "settings/page.html")]
struct SettingsPage {
    shell: Shell,
    has_password: bool,
    google_linked: bool,
    google_enabled: bool,
    sources: Vec<SourceRow>,
    min_password: usize,
    /// Toutes les entrées possibles de la barre du téléphone, et si elles y sont.
    tab_choices: Vec<(&'static Tab, bool)>,
    tab_max: usize,
}

/// `GET /settings`
pub async fn page(ctx: Ctx, State(state): State<AppState>) -> AppResult<Response> {
    let now = time::now();
    let sources = sqlx::query!(
        r#"SELECT id AS "id!", space_id, name, ical_url, last_fetched_at, last_error
           FROM calendar_sources WHERE user_id = ? ORDER BY name"#,
        ctx.user.id
    )
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|s| {
        let space = ctx.space_by_id(s.space_id);
        SourceRow {
            id: s.id,
            name: s.name,
            masked_url: sync::mask_url(&s.ical_url),
            space_name: space.map(|sp| sp.name.clone()).unwrap_or_default(),
            space_color: space.map(|sp| sp.color.clone()).unwrap_or_default(),
            status: match s.last_fetched_at {
                Some(ts) => format!("Synchronisé {}", time::ago(ts, now)),
                None => "Jamais synchronisé".into(),
            },
            error: s.last_error,
        }
    })
    .collect();

    let current = tabbar::selected(ctx.user.mobile_tabs.as_deref());
    let tab_choices = tabbar::ALL
        .iter()
        .map(|tab| (tab, current.contains(&tab)))
        .collect();

    let page = SettingsPage {
        tab_choices,
        tab_max: tabbar::MAX,
        has_password: ctx.user.has_password,
        google_linked: ctx.user.google_linked,
        google_enabled: state.google.is_some(),
        sources,
        min_password: password::MIN_LENGTH,
        shell: ctx.shell("settings").await?,
    };
    Ok(web::html(page))
}

#[derive(Deserialize)]
pub struct ThemeForm {
    /// `dark`, `light` ou `system`.
    theme: String,
    next: Option<String>,
}

/// `POST /settings/theme` : le thème est enregistré sur le compte, il suit
/// donc l'utilisateur d'un appareil à l'autre.
pub async fn set_theme(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<ThemeForm>,
) -> AppResult<Response> {
    let theme = match form.theme.as_str() {
        "dark" => Some("dark"),
        "light" => Some("light"),
        "system" => None,
        _ => return Err(AppError::BadRequest("Thème inconnu.".into())),
    };
    sqlx::query!(
        "UPDATE users SET theme = ? WHERE id = ?",
        theme,
        ctx.user.id
    )
    .execute(&state.db)
    .await?;
    Ok(see_other(local_path(form.next.as_deref())))
}

/// `POST /settings/tabs` : choix des raccourcis du téléphone. Une case
/// cochée arrive sous la forme `tab_<identifiant>=on`.
pub async fn set_tabs(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<HashMap<String, String>>,
) -> AppResult<Response> {
    let keys: Vec<&str> = form
        .keys()
        .filter_map(|name| name.strip_prefix("tab_"))
        .collect();
    match tabbar::encode(&keys) {
        Ok(value) => {
            sqlx::query!(
                "UPDATE users SET mobile_tabs = ? WHERE id = ?",
                value,
                ctx.user.id
            )
            .execute(&state.db)
            .await?;
            ctx.flash_ok("Raccourcis enregistrés").await?;
        }
        Err(message) => ctx.flash_error(message).await?,
    }
    Ok(see_other("/settings#raccourcis"))
}

#[derive(Deserialize)]
pub struct PasswordForm {
    current: Option<String>,
    new: String,
    confirm: String,
}

/// `POST /settings/password` : définit ou change le mot de passe.
pub async fn set_password(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<PasswordForm>,
) -> AppResult<Response> {
    let back = || see_other("/settings");

    // Changer un mot de passe existant demande de le connaître : une session
    // laissée ouverte sur un poste ne suffit pas à prendre le compte.
    if ctx.user.has_password {
        let stored =
            sqlx::query_scalar!("SELECT password_hash FROM users WHERE id = ?", ctx.user.id)
                .fetch_one(&state.db)
                .await?;
        if !password::verify(form.current.unwrap_or_default(), stored).await {
            ctx.flash_error("Le mot de passe actuel est incorrect.")
                .await?;
            return Ok(back());
        }
    }
    if form.new.chars().count() < password::MIN_LENGTH {
        ctx.flash_error(format!(
            "Le nouveau mot de passe doit faire au moins {} caractères.",
            password::MIN_LENGTH
        ))
        .await?;
        return Ok(back());
    }
    if form.new != form.confirm {
        ctx.flash_error("Les deux saisies du nouveau mot de passe ne correspondent pas.")
            .await?;
        return Ok(back());
    }

    let hash = password::hash(form.new).await?;
    sqlx::query!(
        "UPDATE users SET password_hash = ? WHERE id = ?",
        hash,
        ctx.user.id
    )
    .execute(&state.db)
    .await?;
    ctx.flash_ok("Mot de passe enregistré").await?;
    Ok(back())
}

/// `POST /settings/google/unlink`
pub async fn unlink_google(ctx: Ctx, State(state): State<AppState>) -> AppResult<Response> {
    // Sans mot de passe, Google est le seul moyen d'entrer : on ne le retire pas.
    if !ctx.user.has_password {
        ctx.flash_error("Définis d'abord un mot de passe, sinon tu ne pourrais plus te connecter.")
            .await?;
        return Ok(see_other("/settings"));
    }
    sqlx::query!(
        "UPDATE users SET google_sub = NULL WHERE id = ?",
        ctx.user.id
    )
    .execute(&state.db)
    .await?;
    ctx.flash_ok("Compte Google délié").await?;
    Ok(see_other("/settings"))
}

#[derive(Deserialize)]
pub struct CalendarForm {
    name: String,
    url: String,
    space_id: i64,
}

/// `POST /settings/calendars` : ajoute une source iCal et la synchronise aussitôt.
pub async fn add_calendar(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<CalendarForm>,
) -> AppResult<Response> {
    let back = || see_other("/settings#agendas");

    let name: String = form
        .name
        .trim()
        .chars()
        .take(SOURCE_NAME_MAX_CHARS)
        .collect();
    if name.is_empty() {
        ctx.flash_error("Donne un nom à cet agenda.").await?;
        return Ok(back());
    }
    let Some(space) = ctx.space_by_id(form.space_id) else {
        return Err(AppError::NotFound);
    };
    let url = match sync::validate_url(&form.url) {
        Ok(url) => url,
        Err(message) => {
            ctx.flash_error(message).await?;
            return Ok(back());
        }
    };

    let now = time::now();
    let id = sqlx::query!(
        "INSERT INTO calendar_sources (user_id, space_id, name, ical_url, created_at) VALUES (?, ?, ?, ?, ?)",
        ctx.user.id,
        space.id,
        name,
        url,
        now
    )
    .execute(&state.db)
    .await?
    .last_insert_rowid();

    match sync::sync_source(&state, id, &url).await {
        Ok(count) => {
            ctx.flash_ok(format!(
                "Agenda « {name} » ajouté : {count} événements importés"
            ))
            .await?
        }
        Err(error) => {
            ctx.flash_error(format!(
                "Agenda « {name} » ajouté, mais la première synchro a échoué ({error}). Nouvel essai dans l'heure."
            ))
            .await?;
        }
    }
    Ok(back())
}

/// `POST /settings/calendars/{id}/delete` : supprime la source et son cache.
pub async fn delete_calendar(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let deleted = sqlx::query!(
        "DELETE FROM calendar_sources WHERE id = ? AND user_id = ?",
        id,
        ctx.user.id
    )
    .execute(&state.db)
    .await?
    .rows_affected();
    if deleted == 0 {
        return Err(AppError::NotFound);
    }
    ctx.flash_ok("Agenda supprimé").await?;
    Ok(see_other("/settings#agendas"))
}
