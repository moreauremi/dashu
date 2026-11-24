//! Configuration : les variables d'environnement d'abord, puis les réglages
//! de l'instance rangés en base (SPEC, « Installation simple », étape 4).
//!
//! En local, un fichier `.env` à la racine est lu en complément (les vraies
//! variables d'environnement gardent la priorité). En production, c'est
//! `/etc/dashu/dashu.env` (systemd) ou Docker qui les fournit. Voir
//! `.env.example`.
//!
//! Une installation par l'assistant du premier lancement n'a pas besoin de
//! ce fichier : l'adresse publique, la connexion Google et les adresses
//! autorisées viennent de la base (`crate::instance`), et le secret de
//! session d'un fichier `secret` créé à côté de la base. Une variable
//! d'environnement l'emporte toujours, et la valeur ne se change alors pas
//! depuis le site (`FromEnv`).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Context, bail};
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;

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
    /// `None` si `API_TOKEN` n'est pas renseigné : l'API n'accepte alors que
    /// les jetons personnels (Réglages › Connexions).
    pub api: Option<ApiConfig>,
    /// `None` si `FILES_DIR` n'est pas renseigné : la page Fichiers l'indique
    /// et n'accepte aucun envoi.
    pub files: Option<FilesConfig>,
    /// Dossier de travail partagé avec le service de conversion
    /// (`/srv/dashu/tmp`). `None` : pas de compression (bouton absent).
    pub work_dir: Option<std::path::PathBuf>,
    /// Réglages de l'instance donnés par l'environnement : ils ne se changent
    /// pas depuis le site (Réglages › Instance les montre verrouillés).
    pub from_env: FromEnv,
}

/// Quels réglages de l'instance viennent de l'environnement.
#[derive(Debug, Clone, Default)]
pub struct FromEnv {
    /// `BASE_URL`.
    pub base_url: bool,
    /// `GOOGLE_CLIENT_ID` ou `GOOGLE_CLIENT_SECRET`.
    pub google: bool,
    /// `ALLOWED_EMAILS`.
    pub allowed_emails: bool,
}

/// Adresse publique par défaut, sans `BASE_URL` ni réglage en base.
pub const DEFAULT_BASE_URL: &str = "http://localhost:3000";

/// Stockage des fichiers envoyés (v2).
#[derive(Debug, Clone)]
pub struct FilesConfig {
    /// Dossier du contenu des fichiers, par exemple `/srv/dashu/fichiers`.
    pub dir: std::path::PathBuf,
    /// Taille maximale d'un fichier, en octets.
    pub max_upload_bytes: u64,
}

/// Taille maximale d'un fichier par défaut : 2 Go.
pub const DEFAULT_MAX_UPLOAD_MB: u64 = 2048;

/// Accès à l'API JSON (`/api/v1`) par un programme, par exemple un assistant.
#[derive(Clone)]
pub struct ApiConfig {
    /// Jeton attendu dans l'en-tête `Authorization: Bearer …`.
    pub token: String,
    /// Compte au nom duquel agit l'API (doit figurer dans `ALLOWED_EMAILS`).
    pub user_email: String,
}

/// Le jeton n'apparaît jamais dans un `{:?}` (logs, messages d'erreur).
impl std::fmt::Debug for ApiConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiConfig")
            .field("token", &"<masqué>")
            .field("user_email", &self.user_email)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct GoogleConfig {
    pub client_id: String,
    pub client_secret: String,
    /// Adresses de Google pour l'agenda (src/calendar/google.rs). Les vraies
    /// par défaut ; les tests les font pointer vers un faux Google local.
    pub endpoints: GoogleEndpoints,
}

/// Adresses des services de Google utilisés pour l'agenda.
#[derive(Debug, Clone)]
pub struct GoogleEndpoints {
    /// Page de consentement (OAuth 2.0).
    pub auth: String,
    /// Échange du code, et renouvellement du jeton d'accès.
    pub token: String,
    /// Retrait de la permission.
    pub revoke: String,
    /// Adresse e-mail du compte (OpenID Connect « userinfo »).
    pub userinfo: String,
    /// API Google Agenda (v3), sans la barre finale.
    pub calendar: String,
}

impl Default for GoogleEndpoints {
    fn default() -> Self {
        Self {
            auth: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token: "https://oauth2.googleapis.com/token".into(),
            revoke: "https://oauth2.googleapis.com/revoke".into(),
            userinfo: "https://openidconnect.googleapis.com/v1/userinfo".into(),
            calendar: "https://www.googleapis.com/calendar/v3".into(),
        }
    }
}

impl GoogleEndpoints {
    /// Toutes les adresses sous une même racine (faux Google des tests).
    pub fn at(base: &str) -> Self {
        let base = base.trim_end_matches('/');
        Self {
            auth: format!("{base}/auth"),
            token: format!("{base}/token"),
            revoke: format!("{base}/revoke"),
            userinfo: format!("{base}/userinfo"),
            calendar: format!("{base}/calendar/v3"),
        }
    }
}

impl Config {
    /// Ouvre la base (migrations comprises) et lit la configuration :
    /// l'environnement, puis les réglages rangés en base.
    pub async fn load(env: &Env) -> anyhow::Result<(Self, SqlitePool)> {
        let database_url = env
            .get("DATABASE_URL")
            .unwrap_or_else(|| "sqlite://dashu.db".into());
        let db = crate::db::connect(&database_url)
            .await
            .context("ouverture de la base SQLite")?;
        let session_secret = match env.get("SESSION_SECRET") {
            Some(secret) => secret,
            None => secret_file(&database_url)?,
        };
        let stored = crate::instance::load(&db, &session_secret).await?;
        let config = Self::build(env, database_url, session_secret, stored)?;
        Ok((config, db))
    }

    fn build(
        env: &Env,
        database_url: String,
        session_secret: String,
        stored: crate::instance::Stored,
    ) -> anyhow::Result<Self> {
        if session_secret.len() < 64 {
            bail!(
                "SESSION_SECRET doit faire au moins 64 caractères (génère-le avec : openssl rand -hex 32)"
            );
        }

        let from_env = FromEnv {
            base_url: env.get("BASE_URL").is_some(),
            google: env.get("GOOGLE_CLIENT_ID").is_some()
                || env.get("GOOGLE_CLIENT_SECRET").is_some(),
            allowed_emails: env.get("ALLOWED_EMAILS").is_some(),
        };

        let base_url = env
            .get("BASE_URL")
            .or(stored.base_url)
            .unwrap_or_else(|| DEFAULT_BASE_URL.into())
            .trim_end_matches('/')
            .to_string();

        let allowed_emails: Vec<String> = if from_env.allowed_emails {
            env.get("ALLOWED_EMAILS")
                .unwrap_or_default()
                .split(',')
                .map(|e| e.trim().to_lowercase())
                .filter(|e| !e.is_empty())
                .collect()
        } else {
            stored.allowed_emails
        };

        let google = if from_env.google {
            match (env.get("GOOGLE_CLIENT_ID"), env.get("GOOGLE_CLIENT_SECRET")) {
                (Some(client_id), Some(client_secret)) => Some((client_id, client_secret)),
                _ => None,
            }
        } else {
            stored.google
        }
        .map(|(client_id, client_secret)| GoogleConfig {
            client_id,
            client_secret,
            endpoints: GoogleEndpoints::default(),
        });

        let api = match env.get("API_TOKEN") {
            None => None,
            Some(token) => {
                let token = token.trim().to_string();
                if token.len() < 32 {
                    bail!(
                        "API_TOKEN doit faire au moins 32 caractères (génère-le avec : openssl rand -hex 32)"
                    );
                }
                let user_email = env
                    .require("API_USER_EMAIL")
                    .context("API_TOKEN est renseigné : il faut aussi API_USER_EMAIL")?
                    .trim()
                    .to_lowercase();
                if !allowed_emails.contains(&user_email) {
                    bail!(
                        "API_USER_EMAIL ({user_email}) doit être une adresse autorisée (ALLOWED_EMAILS)"
                    );
                }
                Some(ApiConfig { token, user_email })
            }
        };

        let files = match env.get("FILES_DIR") {
            None => None,
            Some(dir) => {
                let max_mb = match env.get("MAX_UPLOAD_MB") {
                    None => DEFAULT_MAX_UPLOAD_MB,
                    Some(value) => value
                        .trim()
                        .parse::<u64>()
                        .ok()
                        .filter(|mb| *mb > 0)
                        .context("MAX_UPLOAD_MB doit être un nombre entier de Mo, supérieur à 0")?,
                };
                Some(FilesConfig {
                    dir: dir.trim().into(),
                    max_upload_bytes: max_mb * 1024 * 1024,
                })
            }
        };

        Ok(Self {
            database_url,
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
            api,
            // Sans fichiers, rien à compresser.
            work_dir: files
                .as_ref()
                .and(env.get("WORK_DIR"))
                .map(|dir| dir.trim().into()),
            files,
            from_env,
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

/// Secret de session gardé dans un fichier `secret` à côté de la base, créé
/// au premier démarrage quand `SESSION_SECRET` n'est pas donné (image Docker,
/// installation par l'assistant). Lisible par dashu seul ; dashu-backup le
/// sauvegarde avec la base. Le perdre déconnecte tout le monde et rend
/// illisibles les jetons de Google Agenda et le secret Google rangé en base.
fn secret_file(database_url: &str) -> anyhow::Result<String> {
    let options = SqliteConnectOptions::from_str(database_url)
        .with_context(|| format!("DATABASE_URL invalide : {database_url}"))?;
    let path: PathBuf = match options.get_filename().parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join("secret"),
        _ => PathBuf::from("secret"),
    };
    let read = |path: &PathBuf| -> anyhow::Result<String> {
        let secret = std::fs::read_to_string(path)
            .with_context(|| format!("lecture de {}", path.display()))?
            .trim()
            .to_string();
        if secret.len() < 64 {
            bail!(
                "{} : secret trop court (64 caractères au moins)",
                path.display()
            );
        }
        Ok(secret)
    };
    if path.exists() {
        return read(&path);
    }

    let secret = crate::web::csrf::random_token();
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.mode(0o600);
    }
    match open.open(&path) {
        Ok(mut file) => {
            use std::io::Write;
            writeln!(file, "{secret}")
                .and_then(|()| file.sync_all())
                .with_context(|| format!("écriture de {}", path.display()))?;
            tracing::info!(fichier = %path.display(), "secret de session créé");
            Ok(secret)
        }
        // Créé entre-temps par un autre processus : c'est le sien qui compte.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => read(&path),
        Err(e) => Err(e).with_context(|| format!("création de {}", path.display())),
    }
}

/// Variables d'environnement, complétées par le fichier `.env` s'il existe.
pub struct Env {
    dotenv: HashMap<String, String>,
    /// Faux pour les tests : seules les paires données comptent.
    real: bool,
}

impl Env {
    /// Les variables du processus, complétées par `.env`.
    pub fn load() -> Self {
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
        Self { dotenv, real: true }
    }

    /// Seulement ces variables (tests).
    pub fn from_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let dotenv = pairs
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Self {
            dotenv,
            real: false,
        }
    }

    /// Valeur non vide de la variable, ou `None`.
    pub fn get(&self, key: &str) -> Option<String> {
        self.real
            .then(|| std::env::var(key).ok())
            .flatten()
            .or_else(|| self.dotenv.get(key).cloned())
            .filter(|v| !v.trim().is_empty())
    }

    fn require(&self, key: &str) -> anyhow::Result<String> {
        self.get(key).with_context(|| {
            format!("variable d'environnement {key} manquante (voir .env.example)")
        })
    }
}
