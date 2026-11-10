//! Erreurs applicatives et pages d'erreur.
//!
//! Les handlers renvoient `Result<_, AppError>`. La réponse d'erreur ne porte
//! qu'un marqueur [`ErrorInfo`] ; c'est le middleware [`error_pages`] qui
//! choisit la forme finale, parce que lui seul voit la requête :
//!   * navigation normale ou `hx-boost` -> page d'erreur complète,
//!   * fragment htmx (widget, sauvegarde…) -> une ligne de texte à insérer.

use askama::Template;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Response};

#[derive(Debug)]
pub enum AppError {
    NotFound,
    Forbidden,
    BadRequest(String),
    Internal(anyhow::Error),
}

/// Toute erreur inattendue (SQL, session, réseau…) devient une erreur 500.
impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(error: E) -> Self {
        Self::Internal(error.into())
    }
}

pub type AppResult<T> = Result<T, AppError>;

/// Marqueur posé sur une réponse d'erreur, lu par [`error_pages`].
#[derive(Clone)]
pub struct ErrorInfo {
    pub status: StatusCode,
    pub message: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "Cette page n'existe pas, ou plus.".to_string(),
            ),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "Action refusée : la page a expiré. Recharge-la et réessaie.".to_string(),
            ),
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            Self::Internal(error) => {
                // Le détail part dans les logs, jamais dans la page.
                tracing::error!(error = format!("{error:#}"), "erreur interne");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Une erreur est survenue de notre côté. Réessaie dans un instant.".to_string(),
                )
            }
        };
        let mut response = (status, message.clone()).into_response();
        response
            .extensions_mut()
            .insert(ErrorInfo { status, message });
        response
    }
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorPage<'a> {
    code: u16,
    title: &'a str,
    message: &'a str,
}

/// Middleware : met en forme les réponses d'erreur selon le type de requête.
pub async fn error_pages(request: Request, next: Next) -> Response {
    let fragment = super::is_fragment_request(request.headers());
    let is_get = request.method() == axum::http::Method::GET;
    let response = next.run(request).await;

    let info = match response.extensions().get::<ErrorInfo>().cloned() {
        Some(info) => info,
        // Refus venant d'axum lui-même (identifiant non numérique dans l'URL,
        // formulaire incomplet…) : il répond en texte brut et en anglais.
        None if response.status().is_client_error() && is_plain_text(&response) => {
            // Une adresse comme /notes/abc ne désigne aucune page : 404, pas 400.
            let status = match response.status() {
                StatusCode::BAD_REQUEST if is_get => StatusCode::NOT_FOUND,
                other => other,
            };
            let message = match status {
                StatusCode::NOT_FOUND => "Cette page n'existe pas, ou plus.",
                StatusCode::METHOD_NOT_ALLOWED => "Cette action n'est pas possible ici.",
                StatusCode::PAYLOAD_TOO_LARGE => "Contenu trop volumineux.",
                _ => "La requête n'a pas pu être traitée.",
            };
            ErrorInfo {
                status,
                message: message.to_string(),
            }
        }
        None => return response,
    };

    if fragment {
        let body = format!(
            r#"<p class="frag-error" role="alert">{}</p>"#,
            super::escape(&info.message)
        );
        return (info.status, Html(body)).into_response();
    }

    let title = match info.status {
        StatusCode::NOT_FOUND => "Page introuvable",
        StatusCode::FORBIDDEN => "Action refusée",
        StatusCode::BAD_REQUEST => "Requête invalide",
        StatusCode::TOO_MANY_REQUESTS => "Trop de tentatives",
        _ => "Erreur",
    };
    let page = ErrorPage {
        code: info.status.as_u16(),
        title,
        message: &info.message,
    };
    match page.render() {
        Ok(html) => (info.status, Html(html)).into_response(),
        Err(_) => (info.status, info.message).into_response(),
    }
}

fn is_plain_text(response: &Response) -> bool {
    response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/plain"))
}

/// Route de repli : toute URL inconnue.
pub async fn not_found() -> AppError {
    AppError::NotFound
}
