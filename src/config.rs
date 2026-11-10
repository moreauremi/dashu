//! Configuration lue dans les variables d'environnement.
//!
//! En local, un fichier `.env` à la racine est lu en complément (les vraies
//! variables d'environnement gardent la priorité). En production c'est Docker
//! Compose qui les fournit. Voir `.env.example`.

use std::collections::HashMap;
use std::net::SocketAddr;

use anyhow::{Context, bail};

#[derive(Debug, Clone)]
pub struct Config {
    /// Par exemple `sqlite:///data/dashu.db`.
    pub database_url: String,
    /// URL publique du site, sans barre finale : `https://dashu.dev`.
    pub base_url: String,
    pub bind_addr: SocketAddr,
    /// Emails autorisés à se connecter, en minuscules.
    pub allowed_emails: Vec<String>,
    /// Sert à signer le cookie de session (64 caractères minimum).
    pub session_secret: String,
    /// `None` si les identifiants Google ne sont pas renseignés : le bouton
    /// « Continuer avec Google » est alors masqué.
    pub google: Option<GoogleConfig>,
    /// Logs en JSON (une ligne par événement) plutôt qu'en texte.
    pub log_json: bool,
}

#[derive(Debug, Clone)]
pub struct GoogleConfig {
    pub client_id: String,
    pub client_secret: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let env = Env::load();

        let session_secret = env.require("SESSION_SECRET")?;
        if session_secret.len() < 64 {
            bail!(
                "SESSION_SECRET doit faire au moins 64 caractères (génère-le avec : openssl rand -hex 32)"
            );
        }

        let base_url = env
            .get("BASE_URL")
            .unwrap_or_else(|| "http://localhost:3000".into())
            .trim_end_matches('/')
            .to_string();

        let allowed_emails: Vec<String> = env
            .get("ALLOWED_EMAILS")
            .unwrap_or_default()
            .split(',')
            .map(|e| e.trim().to_lowercase())
            .filter(|e| !e.is_empty())
            .collect();
        if allowed_emails.is_empty() {
            tracing::warn!("ALLOWED_EMAILS est vide : personne ne peut se connecter");
        }

        let google = match (env.get("GOOGLE_CLIENT_ID"), env.get("GOOGLE_CLIENT_SECRET")) {
            (Some(client_id), Some(client_secret)) => Some(GoogleConfig {
                client_id,
                client_secret,
            }),
            _ => None,
        };

        Ok(Self {
            database_url: env
                .get("DATABASE_URL")
                .unwrap_or_else(|| "sqlite://dashu.db".into()),
            base_url,
            bind_addr: env
                .get("BIND_ADDR")
                .unwrap_or_else(|| "0.0.0.0:3000".into())
                .parse()
                .context("BIND_ADDR invalide (attendu : adresse:port)")?,
            allowed_emails,
            session_secret,
            google,
            log_json: env.get("LOG_FORMAT").as_deref() == Some("json"),
        })
    }

    pub fn is_allowed(&self, email: &str) -> bool {
        let email = email.trim().to_lowercase();
        self.allowed_emails.contains(&email)
    }

    /// Vrai en production : le cookie de session reçoit alors l'attribut `Secure`.
    pub fn is_https(&self) -> bool {
        self.base_url.starts_with("https://")
    }
}

/// Variables d'environnement, complétées par le fichier `.env` s'il existe.
struct Env {
    dotenv: HashMap<String, String>,
}

impl Env {
    fn load() -> Self {
        let mut dotenv = HashMap::new();
        if let Ok(text) = std::fs::read_to_string(".env") {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((key, value)) = line.split_once('=') {
                    let value = value.trim().trim_matches('"').trim_matches('\'');
                    dotenv.insert(key.trim().to_string(), value.to_string());
                }
            }
        }
        Self { dotenv }
    }

    /// Valeur non vide de la variable, ou `None`.
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key)
            .ok()
            .or_else(|| self.dotenv.get(key).cloned())
            .filter(|v| !v.trim().is_empty())
    }

    fn require(&self, key: &str) -> anyhow::Result<String> {
        self.get(key).with_context(|| {
            format!("variable d'environnement {key} manquante (voir .env.example)")
        })
    }
}
