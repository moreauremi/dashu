//! Profil et affichage : prénom et nom, photo de profil, taille de
//! l'interface, barre latérale ancrée ou détachée.
//!
//! La photo est recadrée au carré et réduite dans le navigateur
//! (static/js/app.js) avant l'envoi : le serveur ne reçoit qu'une petite
//! image. Il la relit quand même entièrement (limites strictes, contre une
//! image piégée) et la réenregistre en JPEG de 256 px, sans ses métadonnées.

use std::io::Cursor;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{ImageFormat, ImageReader, Limits};
use serde::Deserialize;

use crate::AppState;
use crate::auth::session::Ctx;
use crate::web::{AppError, AppResult, local_path, see_other};

/// Longueur maximale du prénom et du nom.
pub const NAME_MAX_CHARS: usize = 40;

/// Côté de la photo de profil enregistrée, en pixels.
const AVATAR_SIDE: u32 = 256;

/// Retour aux réglages, sur la carte du profil.
const BACK: &str = "/settings#profil";

#[derive(Deserialize)]
pub struct ProfileForm {
    first_name: String,
    last_name: String,
}

/// Nom nettoyé : espaces resserrés, pas de caractère de contrôle.
fn clean(raw: &str) -> Result<String, String> {
    let name = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.chars().count() > NAME_MAX_CHARS || name.chars().any(char::is_control) {
        return Err(format!(
            "Le prénom et le nom tiennent en {NAME_MAX_CHARS} caractères."
        ));
    }
    Ok(name)
}

/// `POST /settings/profile` : prénom et nom.
pub async fn save_profile(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<ProfileForm>,
) -> AppResult<Response> {
    let (first, last) = match (clean(&form.first_name), clean(&form.last_name)) {
        (Ok(first), Ok(last)) => (first, last),
        (Err(message), _) | (_, Err(message)) => {
            ctx.flash_error(message).await?;
            return Ok(see_other(BACK));
        }
    };
    sqlx::query!(
        "UPDATE users SET first_name = ?, last_name = ? WHERE id = ?",
        first,
        last,
        ctx.user.id
    )
    .execute(&state.db)
    .await?;
    ctx.flash_ok("Profil enregistré").await?;
    Ok(see_other(BACK))
}

/// Relit l'image envoyée et la réenregistre en JPEG carré de 256 px.
fn normalize_avatar(bytes: &[u8]) -> Result<Vec<u8>, String> {
    const UNREADABLE: &str = "Cette image n'a pas pu être lue (JPEG, PNG ou WebP).";
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| UNREADABLE.to_string())?;
    if !matches!(
        reader.format(),
        Some(ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP)
    ) {
        return Err(UNREADABLE.into());
    }
    // Le navigateur envoie 256 px ; au-delà de 2048, ce n'est pas lui.
    let mut limits = Limits::default();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(|_| UNREADABLE.to_string())?;
    let square = image.resize_to_fill(AVATAR_SIDE, AVATAR_SIDE, FilterType::Lanczos3);
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 85)
        .encode_image(&square.to_rgb8())
        .map_err(|_| "La photo n'a pas pu être enregistrée.".to_string())?;
    Ok(out)
}

/// `POST /settings/avatar` : la nouvelle photo, en corps brut (image/jpeg
/// préparée par le navigateur). Répond 204 ; la page se recharge ensuite.
pub async fn upload_avatar(
    ctx: Ctx,
    State(state): State<AppState>,
    body: Bytes,
) -> AppResult<Response> {
    if body.is_empty() {
        return Err(AppError::BadRequest("Aucune image reçue.".into()));
    }
    let jpeg = tokio::task::spawn_blocking(move || normalize_avatar(&body))
        .await?
        .map_err(AppError::BadRequest)?;
    sqlx::query!(
        "UPDATE users SET avatar = ?, avatar_version = avatar_version + 1 WHERE id = ?",
        jpeg,
        ctx.user.id
    )
    .execute(&state.db)
    .await?;
    ctx.flash_ok("Photo de profil changée").await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST /settings/avatar/delete` : retour aux initiales.
pub async fn delete_avatar(ctx: Ctx, State(state): State<AppState>) -> AppResult<Response> {
    sqlx::query!(
        "UPDATE users SET avatar = NULL, avatar_version = avatar_version + 1 WHERE id = ?",
        ctx.user.id
    )
    .execute(&state.db)
    .await?;
    ctx.flash_ok("Photo de profil retirée").await?;
    Ok(see_other(BACK))
}

/// `GET /profile/avatar?v=3` : la photo de l'utilisateur connecté. L'adresse
/// change avec la photo (`v`) : elle se garde un an dans le navigateur.
pub async fn avatar(ctx: Ctx, State(state): State<AppState>) -> AppResult<Response> {
    let jpeg = sqlx::query_scalar!("SELECT avatar FROM users WHERE id = ?", ctx.user.id)
        .fetch_one(&state.db)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/jpeg"),
            (
                header::CACHE_CONTROL,
                "private, max-age=31536000, immutable",
            ),
        ],
        jpeg,
    )
        .into_response())
}

#[derive(Deserialize)]
pub struct SizeForm {
    /// `small`, `normal` ou `large`.
    size: String,
    next: Option<String>,
}

/// `POST /settings/size` : taille de l'interface, enregistrée sur le compte.
pub async fn set_size(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<SizeForm>,
) -> AppResult<Response> {
    let size = match form.size.as_str() {
        "small" => Some("small"),
        "large" => Some("large"),
        "normal" => None,
        _ => return Err(AppError::BadRequest("Taille inconnue.".into())),
    };
    sqlx::query!(
        "UPDATE users SET ui_size = ? WHERE id = ?",
        size,
        ctx.user.id
    )
    .execute(&state.db)
    .await?;
    Ok(see_other(local_path(form.next.as_deref())))
}

#[derive(Deserialize)]
pub struct SidebarForm {
    /// `1` : détachée (cachée à gauche) ; `0` : ancrée.
    floating: String,
    next: Option<String>,
}

/// `POST /settings/sidebar` : barre latérale ancrée ou détachée.
pub async fn set_sidebar(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<SidebarForm>,
) -> AppResult<Response> {
    let floating = match form.floating.as_str() {
        "1" => true,
        "0" => false,
        _ => return Err(AppError::BadRequest("Choix inconnu.".into())),
    };
    sqlx::query!(
        "UPDATE users SET sidebar_floating = ? WHERE id = ?",
        floating,
        ctx.user.id
    )
    .execute(&state.db)
    .await?;
    Ok(see_other(local_path(form.next.as_deref())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noms() {
        assert_eq!(clean("  Rémi   ").unwrap(), "Rémi");
        assert_eq!(clean("Jean  Pierre").unwrap(), "Jean Pierre");
        assert!(clean(&"a".repeat(41)).is_err());
        assert!(clean("a\u{0}b").is_err());
    }
}
