//! Réglages de l'instance rangés en base (SPEC, « Installation simple »,
//! étapes 4 et 5) : adresse publique, connexion Google, adresses autorisées.
//!
//! Ils servent quand l'environnement ne les donne pas : `BASE_URL`,
//! `GOOGLE_CLIENT_ID` et `GOOGLE_CLIENT_SECRET`, `ALLOWED_EMAILS` gardent la
//! priorité (voir `config::Config::load`). Ils sont lus au démarrage : après
//! un changement, dashu se relance (`crate::restart`).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use sqlx::{SqliteConnection, SqliteExecutor, SqlitePool};

use crate::calendar::google::{open_with, seal_with};

pub const BASE_URL: &str = "base_url";
pub const GOOGLE_CLIENT_ID: &str = "google_client_id";
/// Chiffré avec la clé tirée du secret de session, en base64.
pub const GOOGLE_CLIENT_SECRET: &str = "google_client_secret";

/// Ce que la base sait de l'instance.
#[derive(Debug, Default, Clone)]
pub struct Stored {
    pub base_url: Option<String>,
    /// Identifiant et secret (en clair) du client Google.
    pub google: Option<(String, String)>,
    /// En minuscules.
    pub allowed_emails: Vec<String>,
}

/// Lit les réglages de l'instance. S'il y a des comptes mais pas
/// d'administrateur (base d'avant cette étape, compte créé par
/// `dashu set-password`), le plus ancien le devient.
pub async fn load(db: &SqlitePool, session_secret: &str) -> anyhow::Result<Stored> {
    ensure_admin(db).await?;

    let mut stored = Stored::default();
    let (mut client_id, mut sealed) = (None, None);
    let rows = sqlx::query!(r#"SELECT key AS "key!", value FROM instance_settings"#)
        .fetch_all(db)
        .await?;
    for row in rows {
        match row.key.as_str() {
            BASE_URL => stored.base_url = Some(row.value),
            GOOGLE_CLIENT_ID => client_id = Some(row.value),
            GOOGLE_CLIENT_SECRET => sealed = Some(row.value),
            _ => {}
        }
    }
    if let (Some(client_id), Some(sealed)) = (client_id, sealed) {
        let secret = STANDARD
            .decode(sealed)
            .ok()
            .and_then(|bytes| open_with(session_secret, &bytes));
        match secret {
            Some(secret) => stored.google = Some((client_id, secret)),
            None => tracing::warn!(
                "secret Google illisible (le secret de session a changé ?) : connexion Google désactivée, à régler de nouveau dans Réglages › Instance"
            ),
        }
    }

    stored.allowed_emails = sqlx::query_scalar!(
        r#"SELECT email AS "email!" FROM allowed_emails ORDER BY added_at, email"#
    )
    .fetch_all(db)
    .await?
    .into_iter()
    .map(|email| email.to_lowercase())
    .collect();
    Ok(stored)
}

/// Le plus ancien compte devient administrateur s'il n'y en a aucun.
pub async fn ensure_admin(db: impl SqliteExecutor<'_>) -> anyhow::Result<()> {
    sqlx::query!(
        "UPDATE users SET is_admin = 1
         WHERE id = (SELECT MIN(id) FROM users)
           AND NOT EXISTS (SELECT 1 FROM users WHERE is_admin = 1)"
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Vrai s'il existe au moins un compte.
pub async fn has_users(db: impl SqliteExecutor<'_>) -> sqlx::Result<bool> {
    sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM users) AS "any!: bool""#)
        .fetch_one(db)
        .await
}

/// Enregistre (ou remplace) un réglage.
pub async fn set(db: impl SqliteExecutor<'_>, key: &str, value: &str) -> sqlx::Result<()> {
    let now = crate::time::now();
    sqlx::query!(
        "INSERT INTO instance_settings (key, value, updated_at) VALUES (?, ?, ?)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        key,
        value,
        now
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Retire un réglage : la valeur par défaut revient.
pub async fn remove(db: impl SqliteExecutor<'_>, key: &str) -> sqlx::Result<()> {
    sqlx::query!("DELETE FROM instance_settings WHERE key = ?", key)
        .execute(db)
        .await?;
    Ok(())
}

/// Enregistre le client Google ; son secret est chiffré avec la clé tirée du
/// secret de session : une copie de la base ne suffit pas à le lire.
pub async fn set_google(
    db: &mut SqliteConnection,
    session_secret: &str,
    client_id: &str,
    client_secret: &str,
) -> sqlx::Result<()> {
    let sealed = STANDARD.encode(seal_with(session_secret, client_secret));
    set(&mut *db, GOOGLE_CLIENT_ID, client_id).await?;
    set(&mut *db, GOOGLE_CLIENT_SECRET, &sealed).await
}

/// Retire le client Google : plus de connexion avec Google.
pub async fn remove_google(db: &mut SqliteConnection) -> sqlx::Result<()> {
    remove(&mut *db, GOOGLE_CLIENT_ID).await?;
    remove(&mut *db, GOOGLE_CLIENT_SECRET).await
}

/// Autorise une adresse (sans effet si elle l'est déjà).
pub async fn allow(
    db: impl SqliteExecutor<'_>,
    email: &str,
    added_by: Option<i64>,
) -> sqlx::Result<()> {
    let email = email.trim().to_lowercase();
    let now = crate::time::now();
    sqlx::query!(
        "INSERT INTO allowed_emails (email, added_at, added_by) VALUES (?, ?, ?)
         ON CONFLICT (email) DO NOTHING",
        email,
        now,
        added_by
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Retire une adresse autorisée. Les données du compte restent.
pub async fn disallow(db: impl SqliteExecutor<'_>, email: &str) -> sqlx::Result<()> {
    let email = email.trim().to_lowercase();
    sqlx::query!("DELETE FROM allowed_emails WHERE email = ?", email)
        .execute(db)
        .await?;
    Ok(())
}

/// Une adresse simple : quelque chose, une arobase, un domaine avec un point,
/// sans espace ni caractère de contrôle.
pub fn valid_email(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    email.len() <= 254
        && !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains('@')
        && email.chars().all(|c| c.is_ascii_graphic())
}

/// Adresse publique saisie : `http(s)://hôte[:port]`, sans chemin. Rend sa
/// forme canonique (sans barre finale), ou `None` si elle ne convient pas.
pub fn normalize_base_url(input: &str) -> Option<String> {
    let url = reqwest::Url::parse(input.trim()).ok()?;
    let ok = matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some_and(|h| !h.is_empty())
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none();
    ok.then(|| url.origin().ascii_serialization())
}
