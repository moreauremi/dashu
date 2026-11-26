//! L'espace dans l'adresse : `/e/<slug>/…` (SPEC, espaces et outils, C).
//!
//! Les pages d'un espace ont une adresse qui le nomme : `/e/ecole/notes`,
//! `/e/ecole/` pour le dashboard. [`rewrite`] retire ce préfixe avant le
//! routage, si bien que les routes ne changent pas, et laisse l'espace demandé
//! sur la requête ([`UrlSpace`]) ; l'extracteur `Ctx` le préfère à celui de la
//! session. Chaque requête htmx d'un onglet porte en plus l'espace de cet
//! onglet (en-tête [`HEADER`], posé par `layout.html`) : deux onglets sur deux
//! espaces ne se gênent plus. Les adresses sans préfixe restent valides (le
//! dernier espace utilisé).

use std::sync::{Arc, Mutex};

use axum::extract::Request;
use axum::http::{HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::web::{AppError, AppResult, local_path};

/// Début des adresses d'un espace.
pub const PREFIX: &str = "/e/";
/// En-tête des requêtes htmx : l'espace de l'onglet.
pub const HEADER: &str = "x-espace";

/// Espace nommé par l'adresse, posé sur la requête par [`rewrite`].
#[derive(Clone, Debug)]
pub struct UrlSpace(pub String);

/// Espace que l'extracteur `Ctx` a reconnu pour la requête, remonté à
/// [`rewrite`] : une redirection prend le préfixe de cet espace-là, jamais
/// celui d'un en-tête d'onglet périmé (espace supprimé, quitté).
#[derive(Clone, Debug, Default)]
pub struct ResolvedSpace(Arc<Mutex<Option<String>>>);

impl ResolvedSpace {
    pub fn set(&self, slug: &str) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = Some(slug.to_string());
        }
    }

    fn get(&self) -> Option<String> {
        self.0.lock().ok().and_then(|slot| slot.clone())
    }
}

/// Ouvre une page dans un espace, depuis une requête htmx : en-tête
/// `HX-Location`, même transition qu'un lien. La page d'un espace prend
/// l'adresse de cet espace (`/e/ecole/notes`), et la requête porte l'espace
/// voulu (en-tête `X-Espace`) : l'onglet change vraiment d'espace, même vers
/// une page sans préfixe (`/tools`). Une redirection ordinaire garderait
/// l'en-tête de l'ancien espace, que `Ctx` préfère à la session.
/// `HX-Boosted` : le serveur répond par une page entière.
pub fn hx_location(path: &str, slug: &str) -> AppResult<Response> {
    let path = if is_scoped(path) {
        prefixed(slug, path)
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

/// Vers `path` dans l'espace `slug` : `HX-Location` pour une requête htmx,
/// redirection 303 (adresse préfixée) sinon.
pub fn go_to(headers: &axum::http::HeaderMap, path: &str, slug: &str) -> AppResult<Response> {
    if headers.contains_key("hx-request") {
        return hx_location(path, slug);
    }
    let path = if is_scoped(path) {
        prefixed(slug, path)
    } else {
        local_path(Some(path)).to_string()
    };
    Ok(crate::web::see_other(&path))
}

/// Un slug possible : minuscules, chiffres et tirets, 40 caractères au plus.
pub fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 40
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Pages d'un espace, dont l'adresse prend le préfixe : celles des outils
/// du registre qui le demandent (dashboard, notes, fichiers, agenda…), avec
/// ou sans paramètres.
pub fn is_scoped(path: &str) -> bool {
    let path = path.split(['?', '#']).next().unwrap_or(path);
    crate::registry::is_scoped_route(path)
}

/// Adresse d'une page dans un espace : « /notes?q=x » -> « /e/ecole/notes?q=x ».
pub fn prefixed(slug: &str, path: &str) -> String {
    format!("{PREFIX}{slug}{path}")
}

/// `/e/ecole/notes` -> (`ecole`, `/notes`) ; `/e/ecole` -> (`ecole`, `/`).
pub fn split(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix(PREFIX)?;
    let (slug, tail) = match rest.find('/') {
        Some(at) => rest.split_at(at),
        None => (rest, "/"),
    };
    valid_slug(slug).then_some((slug, tail))
}

/// Middleware posé devant le routeur (voir `build_router`) :
///   * retire le préfixe `/e/<slug>` et garde l'espace sur la requête ;
///   * une redirection vers une page d'un espace garde l'espace de la requête
///     (adresse ou onglet) ;
///   * un lien sans préfixe suivi par htmx (`hx-boost`) inscrit dans
///     l'historique l'adresse avec l'espace de l'onglet (`HX-Push-Url`).
pub async fn rewrite(mut request: Request, next: Next) -> Response {
    let mut from_url = None;
    if let Some((slug, rest)) = split(request.uri().path()) {
        let target = match request.uri().query() {
            Some(query) => format!("{rest}?{query}"),
            None => rest.to_string(),
        };
        if let Ok(uri) = target.parse::<Uri>() {
            let slug = slug.to_string();
            *request.uri_mut() = uri;
            request.extensions_mut().insert(UrlSpace(slug.clone()));
            from_url = Some(slug);
        }
    }
    let from_tab = request
        .headers()
        .get(HEADER)
        .and_then(|v| v.to_str().ok())
        .filter(|slug| valid_slug(slug))
        .map(str::to_string);
    let boosted_get =
        request.method() == Method::GET && request.headers().contains_key("hx-boosted");
    let asked = request
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_default();
    let had_prefix = from_url.is_some();
    let resolved = ResolvedSpace::default();
    request.extensions_mut().insert(resolved.clone());

    let mut response = next.run(request).await;
    // Une requête qui nomme un espace (adresse ou onglet) garde l'espace
    // reconnu par `Ctx` : l'en-tête d'un onglet peut nommer un espace qui
    // n'existe plus. Sans espace nommé (formulaire classique), l'adresse ne
    // change pas.
    let Some(slug) = from_url
        .or(from_tab)
        .map(|named| resolved.get().unwrap_or(named))
    else {
        return response;
    };
    if response.status().is_redirection() {
        let location = response
            .headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .filter(|l| is_scoped(l))
            .map(|l| prefixed(&slug, l));
        if let Some(value) = location.and_then(|l| HeaderValue::from_str(&l).ok()) {
            response.headers_mut().insert(header::LOCATION, value);
        }
    } else if boosted_get
        && !had_prefix
        && response.status().is_success()
        && is_scoped(&asked)
        && let Ok(value) = HeaderValue::from_str(&prefixed(&slug, &asked))
    {
        response.headers_mut().insert("hx-push-url", value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adresses_d_un_espace() {
        assert_eq!(split("/e/ecole/notes"), Some(("ecole", "/notes")));
        assert_eq!(split("/e/ecole/"), Some(("ecole", "/")));
        assert_eq!(split("/e/ecole"), Some(("ecole", "/")));
        assert_eq!(split("/e/ecole/notes/12"), Some(("ecole", "/notes/12")));
        assert_eq!(split("/e/École/notes"), None);
        assert_eq!(split("/e//notes"), None);
        assert_eq!(split("/notes"), None);
        assert!(is_scoped("/notes?q=vlan") && is_scoped("/"));
        assert!(!is_scoped("/notes/12") && !is_scoped("/settings"));
        assert_eq!(prefixed("ecole", "/"), "/e/ecole/");
        assert!(!valid_slug("a/b") && !valid_slug(&"a".repeat(41)));
    }
}
