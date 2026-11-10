//! Mots de passe : hachage et vérification en Argon2id.
//!
//! Argon2 est volontairement lent et gourmand en mémoire ; les calculs
//! partent donc dans `spawn_blocking` pour ne pas figer le serveur.

use std::sync::LazyLock;

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};

pub const MIN_LENGTH: usize = 12;

/// Hache un mot de passe. Le résultat (« $argon2id$v=19$… ») contient
/// l'algorithme, ses paramètres et un sel aléatoire propre à ce hachage.
pub async fn hash(password: String) -> anyhow::Result<String> {
    tokio::task::spawn_blocking(move || hash_blocking(&password)).await?
}

fn hash_blocking(password: &str) -> anyhow::Result<String> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|e| anyhow::anyhow!("hachage du mot de passe : {e}"))
}

/// Hachage d'un mot de passe que personne ne connaît, calculé une fois.
static DUMMY_HASH: LazyLock<String> =
    LazyLock::new(|| hash_blocking(&crate::web::csrf::random_token()).expect("hachage factice"));

/// Vérifie un mot de passe.
///
/// Si le compte n'existe pas ou n'a pas de mot de passe (`stored` vaut
/// `None`), on fait quand même un calcul Argon2 complet contre un hachage
/// factice : la réponse met le même temps, ce qui empêche de deviner quels
/// emails ont un compte en chronométrant le formulaire.
pub async fn verify(password: String, stored: Option<String>) -> bool {
    tokio::task::spawn_blocking(move || {
        let (hash, known) = match stored.as_deref() {
            Some(hash) => (hash, true),
            None => (DUMMY_HASH.as_str(), false),
        };
        let Ok(parsed) = PasswordHash::new(hash) else {
            return false;
        };
        let matches = Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok();
        matches && known
    })
    .await
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hachage_et_verification() {
        let hashed = hash("correct horse battery".into()).await.unwrap();
        assert!(hashed.starts_with("$argon2id$"));
        assert!(verify("correct horse battery".into(), Some(hashed.clone())).await);
        assert!(!verify("mauvais mot de passe".into(), Some(hashed)).await);
    }

    #[tokio::test]
    async fn deux_hachages_du_meme_mot_de_passe_different() {
        // Sel aléatoire par hachage.
        let a = hash("correct horse battery".into()).await.unwrap();
        let b = hash("correct horse battery".into()).await.unwrap();
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn compte_inconnu_toujours_refuse() {
        assert!(!verify("peu importe".into(), None).await);
    }
}
