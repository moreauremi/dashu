//! Protection CSRF.
//!
//! Chaque session porte un jeton aléatoire. Toute requête qui modifie des
//! données (tout sauf GET/HEAD/OPTIONS) doit le renvoyer :
//!   * dans l'en-tête `X-CSRF-Token` : c'est le cas de toutes les requêtes
//!     htmx, grâce à l'attribut `hx-headers` posé sur le conteneur de la page ;
//!   * à défaut, dans un champ `_csrf` du formulaire : pour les formulaires
//!     envoyés sans htmx (connexion, déconnexion) et pour `sendBeacon`.
//!
//! Un site tiers peut faire envoyer le cookie de session par le navigateur,
//! mais il ne peut pas lire ce jeton : sa requête est donc refusée.

use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, header};
use axum::middleware::Next;
use axum::response::Response;
use tower_sessions::Session;

use super::error::{AppError, AppResult};

pub const SESSION_KEY: &str = "csrf";
pub const HEADER: &str = "x-csrf-token";
pub const FIELD: &str = "_csrf";

/// Taille maximale d'un formulaire lu pour y chercher le champ `_csrf`.
const MAX_FORM_BYTES: usize = 2 * 1024 * 1024;

/// Jeton de la session, créé au premier besoin.
pub async fn token(session: &Session) -> AppResult<String> {
    if let Some(token) = session.get::<String>(SESSION_KEY).await? {
        return Ok(token);
    }
    rotate(session).await
}

/// Remplace le jeton (à la connexion).
pub async fn rotate(session: &Session) -> AppResult<String> {
    let token = random_token();
    session.insert(SESSION_KEY, &token).await?;
    Ok(token)
}

/// 256 bits aléatoires, en hexadécimal.
pub fn random_token() -> String {
    use std::fmt::Write as _;
    let bytes: [u8; 32] = rand::random();
    bytes
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// Middleware : refuse (403) toute requête modifiante sans jeton valide.
pub async fn protect(session: Session, request: Request, next: Next) -> AppResult<Response> {
    if matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    ) {
        return Ok(next.run(request).await);
    }

    let Some(expected) = session.get::<String>(SESSION_KEY).await? else {
        return Err(AppError::Forbidden);
    };

    if let Some(sent) = request.headers().get(HEADER) {
        return if constant_time_eq(sent.as_bytes(), expected.as_bytes()) {
            Ok(next.run(request).await)
        } else {
            Err(AppError::Forbidden)
        };
    }

    let is_form = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/x-www-form-urlencoded"));
    if !is_form {
        return Err(AppError::Forbidden);
    }

    // Pas d'en-tête : on lit le corps pour y trouver le champ, puis on le
    // remet en place pour le handler.
    let (parts, body) = request.into_parts();
    let bytes = axum::body::to_bytes(body, MAX_FORM_BYTES)
        .await
        .map_err(|_| AppError::BadRequest("Formulaire trop volumineux.".into()))?;
    let valid = form_urlencoded::parse(&bytes).any(|(key, value)| {
        key == FIELD && constant_time_eq(value.as_bytes(), expected.as_bytes())
    });
    if !valid {
        return Err(AppError::Forbidden);
    }
    Ok(next
        .run(Request::from_parts(parts, Body::from(bytes)))
        .await)
}

/// Comparaison en temps constant : le temps de réponse ne révèle pas combien
/// de caractères du jeton étaient corrects.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparaison_en_temps_constant() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }

    #[test]
    fn jetons_aleatoires() {
        let (a, b) = (random_token(), random_token());
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
    }
}
