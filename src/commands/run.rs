//! `POST /cmd` : exécute une ligne de la barre de commandes.
//!
//! Chaque commande appelle le code des boutons existants : les mêmes
//! vérifications s'appliquent (l'élément et l'espace de destination
//! appartiennent à l'utilisateur), et le même message s'affiche ensuite.
//!
//! La réponse ne contient pas de page : l'en-tête `HX-Location` demande à
//! htmx d'ouvrir la page qui suit, comme un lien (même transition, adresse
//! inscrite dans l'historique, espace de l'onglet gardé). Une erreur répond
//! 400 avec son message, que la barre affiche en rouge sans se fermer.

use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::NaiveDate;
use serde::Deserialize;

use super::catalogue::{self, Opened};
use super::{
    Action, Vars, expand, find_by_title, find_space, fold, parse_day, resolve, split_options,
};
use crate::AppState;
use crate::auth::session::{ACTIVE_SPACE, Ctx};
use crate::settings::{self, profile};
use crate::spaces::{self, url};
use crate::tools::{self, Conversion};
use crate::web::{AppError, AppResult, local_path};
use crate::{auth, calendar, dashboard, files, notes, time};

#[derive(Deserialize)]
pub struct RunForm {
    line: String,
    /// Page ouverte (sans l'espace de l'adresse), où l'on revient après une
    /// action qui ne mène pas ailleurs.
    page: Option<String>,
}

fn refuse(message: impl Into<String>) -> AppError {
    AppError::BadRequest(message.into())
}

/// `POST /cmd`
pub async fn run(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<RunForm>,
) -> AppResult<Response> {
    let page = local_path(form.page.as_deref()).to_string();
    let opened = catalogue::opened(&ctx, &state, &page).await?;
    let tree = catalogue::tree(&ctx, &state, &opened).await?;
    let found = resolve(&tree, &form.line).map_err(AppError::BadRequest)?;
    let (action, mut arg) = (found.node.action, found.arg.to_string());
    // #date, #cours : remplacés dans les textes que l'on écrit (titres,
    // noms, tags, recherche).
    let writes = matches!(
        action,
        Action::AddTask
            | Action::AddNote
            | Action::AddSpace
            | Action::AddAgenda
            | Action::Tag
            | Action::Search
    );
    if writes && arg.contains('#') {
        arg = expand(&arg, &Vars::now(&ctx, &state).await?).map_err(refuse)?;
    }
    let here = ctx.active.slug.clone();
    let today = time::paris_date(time::now());

    match action {
        Action::Group | Action::Help | Action::AddFile => {
            Err(refuse("Cette commande se lance depuis la barre."))
        }
        Action::GoPage(path) => go(path, &here),
        Action::GoAgenda => go(&agenda_path(&arg, today)?, &here),
        Action::Search => go(&format!("/notes?{}", query(&[("q", &arg)])), &here),
        Action::Open => {
            let hit = super::hits(&ctx, &state, &arg)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| refuse(format!("Rien ne correspond à « {arg} ».")))?;
            go(&hit.href, &hit.slug)
        }
        Action::Space => {
            let slug = find_space(&ctx.spaces, &arg).map_err(refuse)?.slug.clone();
            let form = spaces::SwitchForm {
                slug: slug.clone(),
                next: Some(page),
            };
            follow(spaces::switch(ctx, axum::Form(form)).await?, &slug)
        }
        Action::AddTask => {
            let (title, at, plus) = split_options(&arg, true);
            let space = option_space(&ctx, &at)?;
            let due = match plus.as_slice() {
                [] => today,
                [word] => parse_day(word, today).ok_or_else(|| {
                    refuse(format!(
                        "Jour inconnu : +{word}. Par exemple : +demain, +lundi, +25/11."
                    ))
                })?,
                _ => return Err(refuse("Un seul jour à la fois.")),
            };
            let added =
                dashboard::insert_task(&state.db, ctx.user.id, space.id, &title, due).await?;
            if !added {
                return Err(refuse("Il manque le titre de la tâche."));
            }
            let mut message = String::from("Tâche ajoutée");
            if space.id != ctx.active.id {
                message.push_str(&format!(" dans {}", space.name));
            }
            if due != today {
                message.push_str(&format!(" pour le {}", time::date_short(due)));
            }
            ctx.flash_ok(message).await?;
            go(&page, &here)
        }
        Action::AddNote => {
            let (title, at, _) = split_options(&arg, false);
            let space = option_space(&ctx, &at)?;
            let id = notes::create_note(&state.db, ctx.user.id, space.id, &title).await?;
            go(&format!("/notes/{id}"), &space.slug)
        }
        Action::Move => {
            let space_id = find_space(&ctx.spaces, &arg).map_err(refuse)?.id;
            let response = match opened {
                Opened::Note { id, .. } => {
                    let form = notes::MoveForm { space_id };
                    notes::move_to_space(ctx, State(state), Path(id), axum::Form(form)).await?
                }
                Opened::File { id, .. } => {
                    let form = files::MoveForm {
                        space_id,
                        next: Some(page),
                    };
                    files::move_to_space(ctx, State(state), Path(id), axum::Form(form)).await?
                }
                Opened::Nothing => return Err(AppError::NotFound),
            };
            follow(response, &here)
        }
        Action::Convert(to) => {
            let pages = (to == Conversion::Pages).then_some(arg);
            convert(ctx, state, &opened, to, pages, page, &here).await
        }
        Action::Compress => {
            convert(ctx, state, &opened, Conversion::Compress, None, page, &here).await
        }
        Action::Done | Action::Undone | Action::DeleteTask => {
            task(ctx, &state, action, &arg, &page).await
        }
        Action::Delete => {
            let response = match opened {
                Opened::Note { id, .. } => notes::delete(ctx, State(state), Path(id)).await?,
                Opened::File { id, temporary, .. } => {
                    let form = files::DeleteForm {
                        next: temporary.then(|| "/tools".to_string()),
                    };
                    files::delete(ctx, State(state), Path(id), axum::Form(form)).await?
                }
                Opened::Nothing => return Err(AppError::NotFound),
            };
            follow(response, &here)
        }
        Action::Tag | Action::Untag | Action::Pin => {
            let Opened::Note { id, .. } = opened else {
                return Err(AppError::NotFound);
            };
            note(ctx, &state, action, id, &arg, &page).await
        }
        Action::Keep => {
            let Opened::File { id, .. } = opened else {
                return Err(AppError::NotFound);
            };
            let space_id = find_space(&ctx.spaces, &arg).map_err(refuse)?.id;
            let form = files::KeepForm { space_id };
            follow(
                files::keep(ctx, State(state), Path(id), axum::Form(form)).await?,
                &here,
            )
        }
        Action::Download => {
            let Opened::File { id, .. } = opened else {
                return Err(AppError::NotFound);
            };
            load(&format!("/files/{id}/download"))
        }
        Action::Theme(theme) => {
            let form = settings::ThemeForm {
                theme: theme.into(),
                next: Some(page),
            };
            follow(
                settings::set_theme(ctx, State(state), axum::Form(form)).await?,
                &here,
            )
        }
        Action::Size(size) => {
            let form = profile::SizeForm {
                size: size.into(),
                next: Some(page),
            };
            follow(
                profile::set_size(ctx, State(state), axum::Form(form)).await?,
                &here,
            )
        }
        Action::Sidebar => {
            let form = profile::SidebarForm {
                floating: if ctx.user.sidebar_floating { "0" } else { "1" }.into(),
                next: Some(page),
            };
            follow(
                profile::set_sidebar(ctx, State(state), axum::Form(form)).await?,
                &here,
            )
        }
        Action::Refresh => {
            let form = calendar::RefreshForm { next: Some(page) };
            follow(
                calendar::refresh(ctx, State(state), axum::Form(form)).await?,
                &here,
            )
        }
        Action::Logout => {
            auth::logout(ctx.session.clone()).await?;
            load("/login")
        }
        Action::AddSpace => add_space(ctx, state, arg, &here).await,
        Action::AddAgenda => add_agenda(ctx, state, &arg, &here).await,
    }
}

/// `/done`, `/undone`, `/delete task` : une tâche du jour, par son titre.
async fn task(
    ctx: Ctx,
    state: &AppState,
    action: Action,
    arg: &str,
    page: &str,
) -> AppResult<Response> {
    let tasks: Vec<dashboard::DayTask> = dashboard::day_tasks(&state.db, &ctx)
        .await?
        .into_iter()
        .filter(|t| match action {
            Action::Done => !t.done,
            Action::Undone => t.done,
            _ => true,
        })
        .collect();
    let task =
        find_by_title(&tasks, |t| t.title.as_str(), arg, "Aucune tâche du jour").map_err(refuse)?;
    let user = ctx.user.id;
    let message = match action {
        Action::Done => {
            dashboard::set_task_done(&state.db, user, task.id, true).await?;
            "Tâche cochée"
        }
        Action::Undone => {
            dashboard::set_task_done(&state.db, user, task.id, false).await?;
            "Tâche décochée"
        }
        _ => {
            dashboard::remove_task(&state.db, user, task.id).await?;
            "Tâche supprimée"
        }
    };
    ctx.flash_ok(format!("{message} : {}", task.title)).await?;
    go(page, &ctx.active.slug)
}

/// `/tag`, `/untag`, `/pin` : sur la note ouverte, qui se recharge.
async fn note(
    ctx: Ctx,
    state: &AppState,
    action: Action,
    id: i64,
    arg: &str,
    page: &str,
) -> AppResult<Response> {
    let (db, user) = (&state.db, ctx.user.id);
    let message = match action {
        Action::Tag => {
            let name = notes::normalize_tag(arg);
            if name.is_empty() {
                return Err(refuse("Il manque le nom du tag."));
            }
            notes::attach_tag(db, user, id, &name).await?;
            format!("Tag « {name} » ajouté")
        }
        Action::Untag => {
            let tags = notes::tags_of(db, user, id).await?;
            let tag = find_by_title(&tags, |t| t.name.as_str(), arg, "Aucun tag de cette note")
                .map_err(refuse)?;
            notes::detach_tag(db, user, id, tag.id).await?;
            format!("Tag « {} » retiré", tag.name)
        }
        _ => {
            if notes::toggle_pinned(db, user, id).await? {
                "Note épinglée".into()
            } else {
                "Note désépinglée".into()
            }
        }
    };
    ctx.flash_ok(message).await?;
    go(page, &ctx.active.slug)
}

/// `/add space <nom>` : couleur et icône proposées comme dans Réglages. Le
/// nouvel espace devient l'espace actif : on ouvre son dashboard.
async fn add_space(ctx: Ctx, state: AppState, name: String, here: &str) -> AppResult<Response> {
    let (color, icon) = spaces::suggestion(&ctx.spaces);
    let session = ctx.session.clone();
    let before = session.get::<String>(ACTIVE_SPACE).await?;
    let form = spaces::SpaceForm {
        name,
        color: color.into(),
        icon: icon.into(),
        rules: None,
        auto_mp4: None,
        auto_heic_jpeg: None,
    };
    let response = spaces::create(ctx, State(state), axum::Form(form)).await?;
    match session.get::<String>(ACTIVE_SPACE).await? {
        Some(slug) if Some(&slug) != before.as_ref() => go("/", &slug),
        // Refusé (nom pris, trop d'espaces…) : Réglages, avec le message.
        _ => follow(response, here),
    }
}

/// `/add agenda <url> [@espace] [nom]`, comme le formulaire de Réglages.
async fn add_agenda(ctx: Ctx, state: AppState, arg: &str, here: &str) -> AppResult<Response> {
    let (rest, at, _) = split_options(arg, false);
    let mut words = rest.split_whitespace();
    let url = words
        .next()
        .filter(|w| w.contains("://"))
        .ok_or_else(|| refuse("Il manque l'adresse du flux iCal (https://…)."))?
        .to_string();
    let name = words.collect::<Vec<_>>().join(" ");
    let space_id = option_space(&ctx, &at)?.id;
    let form = settings::CalendarForm {
        name: if name.is_empty() {
            "Agenda".into()
        } else {
            name
        },
        url,
        space_id,
    };
    follow(
        settings::add_calendar(ctx, State(state), axum::Form(form)).await?,
        here,
    )
}

/// Fait suivre au navigateur une vraie navigation : un téléchargement, ou la
/// page de connexion (en-tête `HX-Redirect`).
fn load(path: &str) -> AppResult<Response> {
    let value = HeaderValue::from_str(path)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("adresse invalide : {e}")))?;
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert("hx-redirect", value);
    Ok(response)
}

/// Lance une conversion du fichier ouvert, comme son menu « Convertir ».
async fn convert(
    ctx: Ctx,
    state: AppState,
    opened: &Opened,
    to: Conversion,
    pages: Option<String>,
    page: String,
    here: &str,
) -> AppResult<Response> {
    let Opened::File { id, .. } = opened else {
        return Err(AppError::NotFound);
    };
    let form = tools::ConvertForm {
        to: to.key().into(),
        pages,
        next: Some(page),
    };
    follow(
        tools::convert(ctx, State(state), Path(*id), axum::Form(form)).await?,
        here,
    )
}

/// L'espace d'une option `@espace` (au plus une), sinon l'espace actif.
fn option_space<'a>(ctx: &'a Ctx, at: &[&str]) -> AppResult<&'a spaces::Space> {
    match at {
        [] => Ok(&ctx.active),
        [word] => find_space(&ctx.spaces, word).map_err(refuse),
        _ => Err(refuse("Un seul espace à la fois.")),
    }
}

/// Adresse de l'agenda pour `/go agenda [jour|semaine] [quand]`. Un jour
/// précis s'ouvre en vue jour, sauf si « semaine » est demandé.
fn agenda_path(arg: &str, today: NaiveDate) -> AppResult<String> {
    let (mut view, mut date) = (None, None);
    for word in arg.split_whitespace() {
        match fold(word).as_str() {
            "jour" | "day" => view = Some("day"),
            "semaine" | "week" => view = Some("week"),
            _ => {
                let day = parse_day(word, today).ok_or_else(|| {
                    refuse(format!(
                        "Jour inconnu : {word}. Par exemple : demain, lundi, 14/11."
                    ))
                })?;
                date = Some(day.format("%Y-%m-%d").to_string());
            }
        }
    }
    let view = view.or(date.as_ref().map(|_| "day"));
    let mut params = Vec::new();
    if let Some(view) = view {
        params.push(("view", view.to_string()));
    }
    if let Some(date) = date {
        params.push(("date", date));
    }
    let params: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
    Ok(if params.is_empty() {
        "/calendar".into()
    } else {
        format!("/calendar?{}", query(&params))
    })
}

/// Paramètres d'adresse encodés : « q=r%C3%A9seau ».
fn query(params: &[(&str, &str)]) -> String {
    form_urlencoded::Serializer::new(String::new())
        .extend_pairs(params)
        .finish()
}

/// Ouvre une page après la commande, comme un lien de l'application. Une
/// page d'un espace prend l'adresse de cet espace (`/e/ecole/notes`), et la
/// requête porte l'espace de l'onglet (en-tête `X-Espace`), comme toute
/// requête htmx. `HX-Boosted` : le serveur répond par une page entière.
fn go(path: &str, slug: &str) -> AppResult<Response> {
    let path = if url::is_scoped(path) {
        url::prefixed(slug, path)
    } else {
        local_path(Some(path)).to_string()
    };
    let location = serde_json::json!({
        "path": path,
        "target": "body",
        "swap": "innerHTML transition:true",
        "headers": { "X-Espace": slug, "HX-Boosted": "true" },
    });
    let value = HeaderValue::from_str(&location.to_string())
        .map_err(|e| AppError::Internal(anyhow::anyhow!("adresse invalide : {e}")))?;
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert("hx-location", value);
    Ok(response)
}

/// Une action existante répond par une redirection (303) : on ouvre sa
/// destination de la même façon.
fn follow(response: Response, slug: &str) -> AppResult<Response> {
    if !response.status().is_redirection() {
        return Ok(response);
    }
    let location = response
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("/")
        .to_string();
    go(&location, slug)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adresse_de_l_agenda() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 13).unwrap();
        let path = |arg| agenda_path(arg, today).unwrap();
        assert_eq!(path(""), "/calendar");
        assert_eq!(path("demain"), "/calendar?view=day&date=2026-11-14");
        assert_eq!(path("semaine lundi"), "/calendar?view=week&date=2026-11-16");
        assert_eq!(path("jour"), "/calendar?view=day");
        assert!(agenda_path("bientot", today).is_err());
        assert_eq!(query(&[("q", "réseau & co")]), "q=r%C3%A9seau+%26+co");
    }
}
