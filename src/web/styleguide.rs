//! `/styleguide` : le design system de dashu sur une page, dans les deux
//! thèmes. Sert de référence quand on ajoute un écran.

use askama::Template;
use axum::response::Response;

use super::AppResult;
use crate::auth::session::{Ctx, Shell};

#[derive(Template)]
#[template(path = "styleguide.html")]
struct StyleguidePage {
    shell: Shell,
    /// Chaque section est rendue une fois par thème.
    themes: [&'static str; 2],
    accents: [(&'static str, &'static str); 3],
    surfaces: [(&'static str, &'static str); 8],
    /// Indices de la palette des matières de l'agenda.
    subject_colors: Vec<u8>,
}

/// `GET /styleguide` (connecté uniquement, comme toute page prenant un `Ctx`).
pub async fn page(ctx: Ctx) -> AppResult<Response> {
    let page = StyleguidePage {
        shell: ctx.shell("styleguide").await?,
        themes: ["dark", "light"],
        accents: [("orange", "Perso"), ("blue", "École"), ("green", "Taff")],
        surfaces: [
            ("bg", "Fond de page"),
            ("surface", "Cartes, barre latérale"),
            ("surface-2", "Champs, survol"),
            ("border", "Bordures"),
            ("text", "Texte"),
            ("text-2", "Texte secondaire"),
            ("text-3", "Légendes"),
            ("accent", "Accent de l'espace"),
        ],
        subject_colors: (0..12).collect(),
    };
    Ok(super::html(page))
}
