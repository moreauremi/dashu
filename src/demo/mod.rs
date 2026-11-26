//! Mode démo (feature Cargo `demo`) : dashu.dev/demo.
//!
//! La démo est l'application elle-même, figée en pages statiques. La commande
//! `dashu demo-export <dossier>` :
//!
//!   1. démarre l'application complète dans le processus, sur une base
//!      SQLite jetable, sans réseau ni secret ;
//!   2. la remplit avec les fixtures (`fixtures.json`) en passant par l'API
//!      JSON, la même que celle d'un assistant ; les dates sont calculées à
//!      partir du jour de la construction ;
//!   3. parcourt les pages comme un navigateur connecté au compte démo, et
//!      les écrit dans le dossier, liens réécrits sous `/demo` (`export.rs`).
//!
//! Aucun gabarit n'est copié : les pages sortent des mêmes routes et des
//! mêmes gabarits qu'en production. Dans le navigateur, `static/js/demo.js`
//! garde tout en local : rien n'est envoyé nulle part.
//!
//! Ce module n'existe que dans un binaire compilé avec `--features demo` ;
//! celui du serveur ne le contient pas.

mod export;
mod media;
mod seed;

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, bail};
use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use tower::ServiceExt;

use crate::config::{ApiConfig, Config, FilesConfig, FromEnv};
use crate::{AppState, build_router, db};

/// Adresse du compte démo : `.invalid` ne peut appartenir à personne.
pub const EMAIL_DOMAIN: &str = "demo.invalid";

/// Construit la démo dans `out`, pour être servie sous `base` (`/demo`).
pub async fn export(out: &Path, base: &str) -> anyhow::Result<()> {
    if !base.starts_with('/') || base.ends_with('/') {
        bail!("le préfixe doit commencer par « / » sans finir par « / » : {base}");
    }
    let fixtures: seed::Fixtures = serde_json::from_str(include_str!("fixtures.json"))
        .context("lecture de src/demo/fixtures.json")?;
    if !fixtures.user.email.ends_with(EMAIL_DOMAIN) {
        bail!("le compte démo doit avoir une adresse en @{EMAIL_DOMAIN}");
    }

    // Tout ce que l'application écrit va dans un dossier temporaire, effacé à
    // la fin : base, fichiers envoyés, dossier de travail des conversions.
    let scratch = std::env::temp_dir().join(format!("dashu-demo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch)?;
    let result = run(out, base, &fixtures, &scratch).await;
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

async fn run(
    out: &Path,
    base: &str,
    fixtures: &seed::Fixtures,
    scratch: &Path,
) -> anyhow::Result<()> {
    let token = hex(&rand::random::<[u8; 32]>());
    let config = Config {
        database_url: format!("sqlite://{}", scratch.join("demo.db").display()),
        // Jamais joint : le compte démo n'a ni Google ni connecteur. Sert
        // seulement aux adresses affichées (Réglages › Connexions).
        base_url: "https://dashu.dev".into(),
        bind_addr: "127.0.0.1:0".parse()?,
        allowed_emails: vec![fixtures.user.email.clone()],
        session_secret: hex(&rand::random::<[u8; 32]>()),
        google: None,
        log_json: false,
        // L'API sert à charger les fixtures, dans le processus. Elle n'est
        // pas exportée : la démo n'a que des pages.
        api: Some(ApiConfig {
            token: token.clone(),
            user_email: fixtures.user.email.clone(),
        }),
        files: Some(FilesConfig {
            dir: scratch.join("fichiers"),
            max_upload_bytes: 10 * 1024 * 1024,
            default_quota_bytes: crate::config::DEFAULT_QUOTA_BYTES,
        }),
        // Un dossier de travail : l'onglet Outils et ses conversions restent
        // visibles (désactivés dans la page par demo.js).
        work_dir: Some(scratch.join("travail")),
        // Le téléchargeur de liens est montré au compte démo, désactivé de
        // la même façon : il n'existe pas de service pour le faire tourner.
        downloader_email: Some(fixtures.user.email.clone()),
        from_env: FromEnv {
            base_url: true,
            google: true,
            allowed_emails: true,
        },
    };
    let files_dir = scratch.join("fichiers");
    std::fs::create_dir_all(&files_dir)?;
    let work_dir = scratch.join("travail");
    std::fs::create_dir_all(work_dir.join("jobs"))?;

    let pool = db::connect(&config.database_url).await?;
    let state = AppState::new(config, pool)?;
    let router = build_router(state.clone());

    // Service de conversion dans un fil à part : il fabrique les vignettes
    // des images envoyées, comme dashu-worker sur le serveur.
    std::thread::spawn(move || {
        if let Err(error) = crate::tools::worker::run(&work_dir) {
            tracing::warn!(
                error = format!("{error:#}"),
                "service de conversion de la démo"
            );
        }
    });

    let mut browser = Browser::new(router.clone(), Some(token));
    seed::load(&state, &mut browser, fixtures).await?;
    let report = export::crawl(&state, &mut browser, out, base).await?;
    println!(
        "démo exportée dans {} : {} pages, {} fichiers statiques, {} adresses désactivées",
        out.display(),
        report.pages,
        report.assets,
        report.disabled
    );
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Réponse lue en entier.
pub(crate) struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl Reply {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json(&self) -> anyhow::Result<serde_json::Value> {
        serde_json::from_slice(&self.body)
            .with_context(|| format!("réponse non JSON : {}", self.text()))
    }

    fn header(&self, name: header::HeaderName) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }
}

/// Navigateur dans le processus : garde le cookie de session, comme le
/// client des tests d'intégration. Aucune requête ne sort de la machine.
pub(crate) struct Browser {
    router: Router,
    cookies: BTreeMap<String, String>,
    api_token: Option<String>,
}

/// Comment une page est demandée : comme htmx le ferait.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mode {
    /// Navigation (hx-boost) : page entière, sans écran de lancement.
    Page,
    /// Fragment (hx-get) : un morceau de page.
    Fragment,
    /// Fichier (image, PDF, CSS…) : ni session htmx ni en-tête particulier.
    Plain,
}

impl Browser {
    fn new(router: Router, api_token: Option<String>) -> Self {
        Self {
            router,
            cookies: BTreeMap::new(),
            api_token,
        }
    }

    async fn send(&mut self, request: Request<Body>) -> anyhow::Result<Reply> {
        let response = self.router.clone().oneshot(request).await?;
        let status = response.status();
        let headers = response.headers().clone();
        for value in headers.get_all(header::SET_COOKIE) {
            let Ok(value) = value.to_str() else { continue };
            let pair = value.split(';').next().unwrap_or_default();
            if let Some((name, value)) = pair.split_once('=') {
                self.cookies.insert(name.trim().into(), value.trim().into());
            }
        }
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        Ok(Reply {
            status,
            headers,
            body,
        })
    }

    fn builder(&self, method: Method, uri: &str) -> axum::http::request::Builder {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            // Le limiteur de tentatives lit l'adresse du client ici.
            .header("cf-connecting-ip", "192.0.2.10");
        if !self.cookies.is_empty() {
            let cookies: Vec<String> = self
                .cookies
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            builder = builder.header(header::COOKIE, cookies.join("; "));
        }
        builder
    }

    /// GET d'une page du site, connecté.
    pub async fn get(
        &mut self,
        uri: &str,
        mode: Mode,
        space: Option<&str>,
    ) -> anyhow::Result<Reply> {
        let mut builder = self.builder(Method::GET, uri);
        if mode != Mode::Plain {
            builder = builder.header("hx-request", "true");
        }
        if mode == Mode::Page {
            builder = builder.header("hx-boosted", "true");
        }
        if let Some(space) = space {
            builder = builder.header(crate::spaces::url::HEADER, space);
        }
        self.send(builder.body(Body::empty())?).await
    }

    /// Connexion au compte démo par le formulaire, comme un visiteur.
    pub async fn login(&mut self, email: &str, password: &str) -> anyhow::Result<String> {
        let page = self.get("/login", Mode::Plain, None).await?;
        let html = page.text();
        let csrf = between(&html, "name=\"_csrf\" value=\"", "\"")
            .context("jeton CSRF du formulaire de connexion")?
            .to_string();
        let form = form_urlencoded::Serializer::new(String::new())
            .append_pair("_csrf", &csrf)
            .append_pair("email", email)
            .append_pair("password", password)
            .finish();
        let request = self
            .builder(Method::POST, "/login")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(form))?;
        let reply = self.send(request).await?;
        if reply.status != StatusCode::SEE_OTHER || reply.header(header::LOCATION) == Some("/login")
        {
            bail!("connexion du compte démo refusée ({})", reply.status);
        }
        Ok(csrf)
    }

    /// Appel de l'API JSON (`/api/v1…`) avec le jeton de la démo.
    pub async fn api(
        &mut self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> anyhow::Result<serde_json::Value> {
        let reply = self
            .api_raw(
                method.clone(),
                path,
                body.map(|b| b.to_string().into_bytes()),
                "application/json",
            )
            .await?;
        if !reply.status.is_success() {
            bail!("API {method} {path} : {} {}", reply.status, reply.text());
        }
        if reply.body.is_empty() {
            return Ok(serde_json::Value::Null);
        }
        reply.json()
    }

    pub async fn api_raw(
        &mut self,
        method: Method,
        path: &str,
        body: Option<Vec<u8>>,
        content_type: &str,
    ) -> anyhow::Result<Reply> {
        let token = self.api_token.clone().context("pas de jeton d'API")?;
        let request = Request::builder()
            .method(method)
            .uri(format!("/api/v1{path}"))
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, content_type)
            .body(body.map(Body::from).unwrap_or_else(Body::empty))?;
        self.send(request).await
    }
}

/// Texte entre `start` et `end`, la première fois qu'il apparaît.
pub(crate) fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let from = text.find(start)? + start.len();
    let len = text[from..].find(end)?;
    Some(&text[from..from + len])
}
