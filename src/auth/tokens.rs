//! Jetons d'accès par compte (SPEC, « Connecteur Claude (MCP) », étape 1).
//!
//! Un jeton permet à un programme (Claude Code, un script…) d'agir au nom
//! d'un compte, avec exactement ses droits sur le site : ses espaces et les
//! espaces partagés qu'il a rejoints. Il se crée, se liste et se révoque dans
//! Réglages › Connexions (`crate::settings::connexions`).
//!
//! * `dashu_` suivi de 32 octets aléatoires en hexadécimal : le préfixe fait
//!   repérer d'un coup d'œil (ou d'un `grep`) un jeton oublié dans un fichier.
//! * La base n'en garde que l'empreinte SHA-256. Un jeton aléatoire de
//!   256 bits n'a pas besoin d'Argon2, qui sert à ralentir la recherche d'un
//!   mot de passe facile à deviner.
//! * Un jeton expiré est refusé. Un compte retiré de `ALLOWED_EMAILS` perd ses
//!   jetons à la requête suivante, comme sa session.
//! * Le compte est relu à chaque requête : rien n'est mis en cache.

use sha2::{Digest, Sha256};

use crate::AppState;
use crate::time;
use crate::web::{AppResult, csrf};

/// Début de tout jeton de dashu.
pub const PREFIX: &str = "dashu_";
/// `last_used_at` n'est réécrit qu'une fois par minute au plus : une rafale
/// d'appels n'écrit pas en base à chaque requête.
const LAST_USED_EVERY: i64 = 60;

/// Nouveau jeton : `dashu_` et 64 caractères hexadécimaux.
pub fn generate() -> String {
    format!("{PREFIX}{}", csrf::random_token())
}

/// Empreinte d'un jeton, telle qu'elle est en base.
pub fn hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

/// Vrai si le texte a la forme d'un jeton de dashu (avant toute requête).
pub fn looks_like_token(text: &str) -> bool {
    text.strip_prefix(PREFIX)
        .is_some_and(|rest| rest.len() == 64 && rest.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Compte au nom duquel agit un jeton valable.
#[derive(Debug, Clone)]
pub struct TokenOwner {
    pub user_id: i64,
    pub token_id: i64,
    /// Autorisation OAuth dont vient le jeton (étape 3), `None` pour un jeton
    /// créé à la main.
    pub grant_id: Option<i64>,
    /// Nom donné au jeton (« Claude Code du Mac ») : la trace de ce qu'il fait.
    pub name: String,
}

/// Le compte d'un jeton présenté, s'il est valable : bonne forme, connu, pas
/// expiré, compte toujours autorisé. `None` dans tous les autres cas, sans
/// dire lequel.
pub async fn authenticate(state: &AppState, presented: &str) -> AppResult<Option<TokenOwner>> {
    if !looks_like_token(presented) {
        return Ok(None);
    }
    let token_hash = hash(presented);
    let now = time::now();
    let Some(row) = sqlx::query!(
        r#"SELECT t.id AS "id!", t.user_id, t.grant_id, t.name, t.last_used_at, u.email
           FROM access_tokens t JOIN users u ON u.id = t.user_id
           WHERE t.token_hash = ? AND t.expires_at > ?"#,
        token_hash,
        now
    )
    .fetch_optional(&state.db)
    .await?
    else {
        return Ok(None);
    };
    if !state.config.is_allowed(&row.email) {
        return Ok(None);
    }
    if row
        .last_used_at
        .is_none_or(|at| now - at >= LAST_USED_EVERY)
    {
        sqlx::query!(
            "UPDATE access_tokens SET last_used_at = ? WHERE id = ?",
            now,
            row.id
        )
        .execute(&state.db)
        .await?;
    }
    Ok(Some(TokenOwner {
        user_id: row.user_id,
        token_id: row.id,
        grant_id: row.grant_id,
        name: row.name,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forme_d_un_jeton() {
        let token = generate();
        assert!(token.starts_with("dashu_"));
        assert_eq!(token.len(), 6 + 64);
        assert!(looks_like_token(&token));
        assert_ne!(generate(), token);
        assert!(!looks_like_token("dashu_abc"));
        assert!(!looks_like_token(&token.replace("dashu_", "dashv_")));
        assert!(!looks_like_token(&format!("{token}0")));
        assert!(!looks_like_token(&format!("dashu_{}", "z".repeat(64))));
    }

    #[test]
    fn empreinte_sha256() {
        assert_eq!(
            hash("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(hash(&generate()).len(), 64);
    }
}
