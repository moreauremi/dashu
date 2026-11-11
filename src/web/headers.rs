//! En-têtes de sécurité ajoutés à toutes les réponses.

use axum::extract::{Request, State};
use axum::http::HeaderValue;
use axum::http::header::{self, HeaderName};
use axum::middleware::Next;
use axum::response::Response;

use crate::AppState;

/// Content-Security-Policy stricte : tout vient de dashu lui-même.
///
///   * `script-src 'self'` : aucun script en ligne, aucun `eval` (htmx est
///     configuré avec `allowEval: false`). Un `<script>` injecté dans une note
///     ne s'exécuterait pas, même si le nettoyage par ammonia échouait.
///   * `style-src 'self'` : aucun attribut `style`, aucune balise `<style>`.
///     L'agenda se place avec des classes (voir build.rs) et CodeMirror pose
///     ses styles via adoptedStyleSheets dans un shadow root.
///   * `img-src … https:` : une note peut afficher une image distante.
///   * `form-action` : seul le départ vers Google est autorisé hors du site.
///   * `frame-ancestors 'none'` : dashu ne peut pas être affiché dans une iframe.
const CSP: &str = "default-src 'self'; \
    script-src 'self'; \
    style-src 'self'; \
    img-src 'self' data: https:; \
    font-src 'self'; \
    connect-src 'self'; \
    form-action 'self' https://accounts.google.com; \
    frame-ancestors 'none'; \
    base-uri 'none'; \
    object-src 'none'";

pub async fn security_headers(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();

    // Une réponse peut poser une CSP plus stricte encore : c'est le cas d'un
    // fichier téléchargé (`sandbox`, voir src/files). On ne la remplace pas.
    if !headers.contains_key(header::CONTENT_SECURITY_POLICY) {
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CSP),
        );
    }
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=(), payment=()"),
    );
    headers.insert(
        HeaderName::from_static("cross-origin-opener-policy"),
        HeaderValue::from_static("same-origin"),
    );
    if state.config.is_https() {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        );
    }
    // Les pages sont personnelles : aucun cache partagé ne doit les garder.
    // Les fichiers statiques posent leur propre Cache-Control (un an).
    if !headers.contains_key(header::CACHE_CONTROL) {
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-cache"),
        );
    }
    response
}
