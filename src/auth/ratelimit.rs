//! Anti-bruteforce : 5 tentatives de connexion par minute et par adresse IP.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use tower_governor::GovernorError;
use tower_governor::key_extractor::KeyExtractor;

use crate::web::error::ErrorInfo;

/// Nombre de tentatives autorisées d'un coup.
pub const BURST: u32 = 5;
/// Une nouvelle tentative est rendue toutes les 12 s, soit 5 par minute.
pub const REFILL_SECONDS: u64 = 12;

/// Identifie le visiteur par son adresse IP.
///
/// En production dashu est derrière le tunnel Cloudflare : toutes les
/// connexions TCP viennent du conteneur `cloudflared`. La vraie adresse du
/// visiteur est dans l'en-tête `CF-Connecting-IP`, que Cloudflare écrase
/// toujours lui-même (un visiteur ne peut pas le falsifier à travers le
/// tunnel, contrairement à `X-Forwarded-For`). Sans cet en-tête (accès direct,
/// en local) on prend l'adresse de la connexion.
#[derive(Clone, Copy, Debug)]
pub struct ClientIp;

impl KeyExtractor for ClientIp {
    type Key = IpAddr;

    fn extract<T>(&self, request: &Request<T>) -> Result<IpAddr, GovernorError> {
        let from_cloudflare = request
            .headers()
            .get("cf-connecting-ip")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<IpAddr>().ok());
        let from_socket = request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.0.ip());
        // Dernier recours : un seul compteur commun, plutôt que pas de limite.
        Ok(from_cloudflare
            .or(from_socket)
            .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)))
    }
}

/// Réponse 429 : le middleware des pages d'erreur la met en forme.
pub fn too_many_requests(error: GovernorError) -> Response {
    let message = match error {
        GovernorError::TooManyRequests { wait_time, .. } => {
            format!("Trop de tentatives de connexion. Réessaie dans {wait_time} secondes.")
        }
        _ => "Trop de tentatives de connexion. Réessaie dans une minute.".to_string(),
    };
    let status = StatusCode::TOO_MANY_REQUESTS;
    let mut response = (status, message.clone()).into_response();
    response
        .extensions_mut()
        .insert(ErrorInfo { status, message });
    response
}
