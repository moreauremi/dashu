//! Briques communes à toutes les pages : rendu des gabarits, erreurs, CSRF,
//! en-têtes de sécurité, page /styleguide.

pub mod csrf;
pub mod error;
pub mod headers;
pub mod styleguide;

use askama::Template;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};

pub use error::{AppError, AppResult};

/// Rend un gabarit askama en réponse HTML.
pub fn html<T: Template>(template: T) -> Response {
    match template.render() {
        Ok(body) => Html(body).into_response(),
        Err(error) => AppError::Internal(error.into()).into_response(),
    }
}

/// Vrai si htmx demande un fragment à insérer dans la page (widget, liste,
/// aperçu…), par opposition à une page entière : navigation classique, lien
/// `hx-boost`, ou retour arrière du navigateur.
pub fn is_fragment_request(headers: &HeaderMap) -> bool {
    headers.contains_key("hx-request")
        && !headers.contains_key("hx-boosted")
        && !headers.contains_key("hx-history-restore-request")
}

/// Redirection 303 : après un POST, le navigateur (ou htmx) recharge la page
/// cible en GET.
pub fn see_other(location: &str) -> Response {
    (StatusCode::SEE_OTHER, [(header::LOCATION, location)]).into_response()
}

/// Ne garde une URL de retour que si elle pointe à l'intérieur du site : pas
/// de redirection ouverte vers un autre domaine.
pub fn local_path(path: Option<&str>) -> &str {
    match path {
        Some(p) if p.starts_with('/') && !p.starts_with("//") && !p.contains('\\') => p,
        _ => "/",
    }
}

/// Échappe un texte pour l'insérer dans du HTML écrit à la main (les gabarits
/// askama, eux, échappent tout seuls).
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_de_retour_locale_uniquement() {
        assert_eq!(local_path(Some("/notes?q=a")), "/notes?q=a");
        assert_eq!(local_path(Some("//evil.example")), "/");
        assert_eq!(local_path(Some("https://evil.example")), "/");
        assert_eq!(local_path(Some("/\\evil.example")), "/");
        assert_eq!(local_path(None), "/");
    }

    #[test]
    fn echappement_html() {
        assert_eq!(
            escape(r#"<b a="1">&'"#),
            "&lt;b a=&quot;1&quot;&gt;&amp;&#39;"
        );
    }
}
