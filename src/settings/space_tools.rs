//! Réglages › Espaces : les outils affichés dans chaque espace (SPEC,
//! « Outils par espace »). Le registre et l'enregistrement sont dans
//! `crate::registry`.

use askama::Template;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Response;
use serde::Deserialize;

use crate::AppState;
use crate::auth::session::Ctx;
use crate::registry::{self, Shown, Tool};
use crate::web::{self, AppError, AppResult, see_other};

#[derive(Template)]
#[template(path = "settings/space_tools_fragment.html")]
struct Fragment {
    space_id: i64,
    tools: Shown,
    /// Outils affichés dans l'espace actif, s'il vient de changer : la barre
    /// latérale à remplacer.
    nav: Option<Vec<&'static Tool>>,
    /// La barre de raccourcis du téléphone, à remplacer avec la barre
    /// latérale (« Nouvelle note » suit Notes).
    tabs: Vec<&'static crate::tabbar::Tab>,
    base: String,
}

#[derive(Deserialize)]
pub struct ToolForm {
    /// Case cochée : `1` ; décochée, le navigateur n'envoie rien.
    shown: Option<String>,
}

/// Change le réglage d'un outil dans un espace du compte. Aussi pour l'API.
/// `None` si l'outil n'existe pas pour ce compte (inconnu, ou réservé à
/// d'autres) ; une erreur s'il ne se masque pas.
pub(crate) async fn set_tool(
    state: &AppState,
    user_id: i64,
    space_id: i64,
    id: &str,
    shown: bool,
) -> AppResult<Option<&'static Tool>> {
    let current = registry::load(&state.db, user_id, space_id).await?;
    let Some(tool) = current.tool(id) else {
        return Ok(None);
    };
    if !tool.maskable {
        return Err(AppError::BadRequest(format!(
            "{} ne se masque pas : c'est l'accueil de l'espace.",
            tool.name
        )));
    }
    registry::store(&state.db, user_id, space_id, tool, shown).await?;
    Ok(Some(tool))
}

/// `POST /settings/spaces/{id}/tools/{tool}` : affiche (`shown=1`) ou masque
/// un outil dans un espace. Par htmx, la réponse est le bloc de l'espace ;
/// sans, une redirection vers Réglages.
pub async fn set(
    ctx: Ctx,
    State(state): State<AppState>,
    Path((space_id, id)): Path<(i64, String)>,
    headers: HeaderMap,
    axum::Form(form): axum::Form<ToolForm>,
) -> AppResult<Response> {
    // Un espace du compte (à lui ou rejoint) ; sinon, 404.
    let space = ctx.space_by_id(space_id).ok_or(AppError::NotFound)?;
    let shown = form.shown.is_some();
    let tool = set_tool(&state, ctx.user.id, space.id, &id, shown)
        .await?
        .ok_or(AppError::NotFound)?;

    if web::is_fragment_request(&headers) {
        let tools = registry::load(&state.db, ctx.user.id, space.id).await?;
        let nav = (space.id == ctx.active.id).then(|| tools.visible().collect());
        let tabs = crate::tabbar::selected(ctx.user.mobile_tabs.as_deref())
            .into_iter()
            .filter(|tab| registry::tool_of_tab(tab.key).is_none_or(|id| tools.is_shown(id)))
            .collect();
        return Ok(web::html(Fragment {
            space_id: space.id,
            tabs,
            tools,
            nav,
            base: format!("/e/{}", ctx.active.slug),
        }));
    }
    let verb = if shown { "affiché" } else { "masqué" };
    ctx.flash_ok(format!("{} {verb} dans {}", tool.name, space.name))
        .await?;
    Ok(see_other("/settings#espaces"))
}
