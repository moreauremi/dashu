//! Fichiers statiques embarqués dans le binaire, servis sous `/static/`.
//!
//! La table `ASSETS` est générée par `build.rs` : chaque fichier y a une URL
//! contenant l'empreinte de son contenu. Les gabarits appellent [`url`] pour
//! obtenir cette URL, et [`serve`] la sert avec un cache d'un an.

use std::path::PathBuf;
use std::sync::LazyLock;

use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

pub struct Asset {
    /// Chemin dans `static/`, par exemple `css/app.css`.
    pub path: &'static str,
    /// URL publique avec empreinte, par exemple `/static/css/app.3fa91c20d4.css`.
    pub url: &'static str,
    pub mime: &'static str,
    pub bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/assets.rs"));

/// Classes de la grille de l'agenda (voir build.rs). Déjà ajoutées à app.css
/// dans le binaire ; ne sert qu'au mode développement ci-dessous.
const GRID_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/grid.css"));

/// Mode développement : si `DASHU_STATIC_DIR` pointe sur le dossier `static/`,
/// les fichiers sont relus sur le disque à chaque requête. On peut ainsi
/// retoucher le CSS sans recompiler. Jamais défini en production.
static DEV_DIR: LazyLock<Option<PathBuf>> =
    LazyLock::new(|| std::env::var_os("DASHU_STATIC_DIR").map(PathBuf::from));

const ONE_YEAR: &str = "public, max-age=31536000, immutable";

/// URL publique (avec empreinte) d'un fichier de `static/`.
pub fn url(path: &str) -> &'static str {
    match ASSETS.iter().find(|a| a.path == path) {
        Some(asset) => asset.url,
        None => {
            tracing::error!(path, "fichier statique inconnu demandé par un gabarit");
            "/static/introuvable"
        }
    }
}

/// Balise SVG d'une icône du sprite (`static/icons.svg`). L'icône est
/// décorative : le texte accessible est porté par le bouton ou le lien parent.
pub fn icon(name: &str) -> String {
    format!(
        r#"<svg class="i" aria-hidden="true" focusable="false"><use href="{}#{name}"/></svg>"#,
        url("icons.svg")
    )
}

/// Nom affiché de l'application. Le projet, le binaire et les commandes
/// gardent leur nom de code, dashu.
pub const APP_NAME: &str = "Hermes Dashboard";

/// Logo Hermes (`static/logo.svg`) en trois morceaux : le H, « ermes » et la
/// barre. La page de connexion les anime séparément (`.logo-anim` dans
/// app.css). Il prend la couleur du texte et porte le nom de l'application.
pub fn logo() -> String {
    let file = url("logo.svg");
    format!(
        r#"<svg class="logo" viewBox="127 54 972 186" role="img" aria-label="{APP_NAME}" focusable="false"><use href="{file}#h"/><use class="logo-ermes" href="{file}#ermes"/><use class="logo-slash" href="{file}#slash"/></svg>"#
    )
}

/// `GET /static/{*path}`
pub async fn serve(Path(path): Path<String>) -> Response {
    let requested = format!("/static/{path}");
    // On accepte l'URL avec empreinte (cache d'un an) et le chemin nu (pas de
    // cache : il ne garantit pas la version).
    let found = ASSETS
        .iter()
        .find(|a| a.url == requested)
        .map(|a| (a, true))
        .or_else(|| ASSETS.iter().find(|a| a.path == path).map(|a| (a, false)));

    let Some((asset, fingerprinted)) = found else {
        return StatusCode::NOT_FOUND.into_response();
    };

    if let Some(dir) = DEV_DIR.as_ref()
        && let Ok(mut bytes) = std::fs::read(dir.join(asset.path))
    {
        if asset.path == "css/app.css" {
            bytes.extend_from_slice(GRID_CSS.as_bytes());
        }
        return (
            [
                (header::CONTENT_TYPE, asset.mime),
                (header::CACHE_CONTROL, "no-store"),
            ],
            bytes,
        )
            .into_response();
    }

    let cache = if fingerprinted { ONE_YEAR } else { "no-cache" };
    (
        [
            (header::CONTENT_TYPE, asset.mime),
            (header::CACHE_CONTROL, cache),
        ],
        asset.bytes,
    )
        .into_response()
}
