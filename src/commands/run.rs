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
use super::{Action, find_space, fold, parse_day, resolve, split_options};
use crate::AppState;
use crate::auth::session::Ctx;
use crate::spaces::{self, url};
use crate::tools::{self, Conversion};
use crate::web::{AppError, AppResult, local_path};
use crate::{dashboard, files, notes, time};

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
    let tree = catalogue::tree(&ctx, &state, &opened);
    let found = resolve(&tree, &form.line).map_err(AppError::BadRequest)?;
    let (action, arg) = (found.node.action, found.arg.to_string());
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
            let (title, at, _) = split_options(&arg, false);
            let space = option_space(&ctx, &at)?;
            let added =
                dashboard::insert_task(&state.db, ctx.user.id, space.id, &title, today).await?;
            if !added {
                return Err(refuse("Il manque le titre de la tâche."));
            }
            if space.id == ctx.active.id {
                ctx.flash_ok("Tâche ajoutée").await?;
            } else {
                ctx.flash_ok(format!("Tâche ajoutée dans {}", space.name))
                    .await?;
            }
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
    }
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
