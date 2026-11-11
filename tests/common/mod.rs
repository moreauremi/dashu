//! Outillage commun aux tests d'intégration.
//!
//! Chaque test démarre une application complète (mêmes routes et mêmes
//! middlewares qu'en production) sur une base SQLite temporaire, et lui parle
//! à travers [`Client`], un mini-navigateur : il garde les cookies et renvoie
//! le jeton CSRF comme le fait htmx.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use dashu::config::{ApiConfig, Config, FilesConfig};
use dashu::{AppState, auth, build_router, db};
use tower::ServiceExt;

pub const ALICE: &str = "alice@example.test";
pub const BOB: &str = "bob@example.test";
/// Mot de passe commun aux comptes de test.
pub const PASSWORD: &str = "correct horse battery staple";
/// Jeton de l'API des tests ; elle agit au nom d'Alice.
pub const API_TOKEN: &str = "test-api-token-0123456789abcdef0123456789";
/// Taille maximale d'un fichier dans les tests : petite, pour tester le refus.
pub const MAX_UPLOAD: u64 = 64 * 1024;

static COUNTER: AtomicU32 = AtomicU32::new(0);

pub struct TestApp {
    pub state: AppState,
    router: Router,
    db_path: PathBuf,
    /// Dossier des fichiers envoyés (FILES_DIR), propre à ce test.
    pub files_dir: PathBuf,
}

impl TestApp {
    /// Application neuve, avec Alice et Bob autorisés et déjà inscrits.
    pub async fn new() -> Self {
        let app = Self::empty().await;
        app.add_user(ALICE).await;
        app.add_user(BOB).await;
        app
    }

    /// Application neuve sans aucun compte.
    pub async fn empty() -> Self {
        Self::with_api(Some(ApiConfig {
            token: API_TOKEN.into(),
            user_email: ALICE.into(),
        }))
        .await
    }

    /// Comme [`TestApp::new`], mais sans API_TOKEN : l'API est désactivée.
    pub async fn without_api() -> Self {
        let app = Self::with_api(None).await;
        app.add_user(ALICE).await;
        app.add_user(BOB).await;
        app
    }

    /// Comme [`TestApp::new`], mais sans FILES_DIR : les fichiers sont désactivés.
    pub async fn without_files() -> Self {
        let app = Self::build(
            Some(ApiConfig {
                token: API_TOKEN.into(),
                user_email: ALICE.into(),
            }),
            false,
        )
        .await;
        app.add_user(ALICE).await;
        app.add_user(BOB).await;
        app
    }

    async fn with_api(api: Option<ApiConfig>) -> Self {
        Self::build(api, true).await
    }

    async fn build(api: Option<ApiConfig>, files: bool) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let db_path =
            std::env::temp_dir().join(format!("dashu-test-{}-{n}.db", std::process::id()));
        remove_db(&db_path);
        let files_dir =
            std::env::temp_dir().join(format!("dashu-test-{}-{n}-fichiers", std::process::id()));
        let _ = std::fs::remove_dir_all(&files_dir);

        let config = Config {
            database_url: format!("sqlite://{}", db_path.display()),
            base_url: "http://localhost:3000".into(),
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            allowed_emails: vec![ALICE.into(), BOB.into()],
            session_secret: "0123456789abcdef".repeat(4),
            google: None,
            log_json: false,
            api,
            files: files.then(|| FilesConfig {
                dir: files_dir.clone(),
                max_upload_bytes: MAX_UPLOAD,
            }),
        };
        let pool = db::connect(&config.database_url)
            .await
            .expect("base de test");
        let state = AppState::new(config, pool).expect("état de test");
        Self {
            router: build_router(state.clone()),
            state,
            db_path,
            files_dir,
        }
    }

    /// Fichiers présents sur le disque, hors envois en cours.
    pub fn stored_files(&self) -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else {
                    out.push(path);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.files_dir, &mut out);
        out
    }

    pub async fn add_user(&self, email: &str) {
        auth::set_password(&self.state.db, email, PASSWORD.into())
            .await
            .expect("création du compte");
    }

    /// Nouveau navigateur, sans cookie, avec sa propre adresse IP.
    pub fn client(&self) -> Client {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        Client {
            router: self.router.clone(),
            cookies: BTreeMap::new(),
            csrf: None,
            ip: format!("203.0.113.{}", n % 250 + 1),
        }
    }

    /// Navigateur déjà connecté.
    pub async fn logged_in(&self, email: &str) -> Client {
        let mut client = self.client();
        let response = client.login(email, PASSWORD).await;
        assert_eq!(
            response.status,
            StatusCode::SEE_OTHER,
            "connexion de {email}"
        );
        client
    }

    /// Identifiant de l'espace `slug` d'un utilisateur.
    pub async fn space_id(&self, email: &str, slug: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT s.id FROM spaces s JOIN users u ON u.id = s.user_id WHERE u.email = ? AND s.slug = ?",
        )
        .bind(email)
        .bind(slug)
        .fetch_one(&self.state.db)
        .await
        .expect("espace")
    }

    pub async fn user_id(&self, email: &str) -> i64 {
        sqlx::query_scalar("SELECT id FROM users WHERE email = ?")
            .bind(email)
            .fetch_one(&self.state.db)
            .await
            .expect("utilisateur")
    }
}

impl TestApp {
    /// Appel de l'API JSON avec un jeton (ou sans, si `token` vaut `None`).
    /// Renvoie le statut et le corps lu comme du JSON (`Null` s'il est vide).
    pub async fn api(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
        token: Option<&str>,
    ) -> (StatusCode, serde_json::Value) {
        self.api_raw(method, path, body.map(|b| b.to_string()), token, &[])
            .await
    }

    /// Variante bas niveau : corps brut et en-têtes supplémentaires.
    pub async fn api_raw(
        &self,
        method: Method,
        path: &str,
        body: Option<String>,
        token: Option<&str>,
        extra_headers: &[(&str, &str)],
    ) -> (StatusCode, serde_json::Value) {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        for (name, value) in extra_headers {
            request = request.header(*name, *value);
        }
        let body = match body {
            Some(body) => {
                request = request.header(header::CONTENT_TYPE, "application/json");
                Body::from(body)
            }
            None => Body::empty(),
        };
        let response = self
            .router
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                panic!(
                    "réponse non JSON ({status}) : {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, json)
    }
}

impl Drop for TestApp {
    fn drop(&mut self) {
        remove_db(&self.db_path);
        let _ = std::fs::remove_dir_all(&self.files_dir);
    }
}

fn remove_db(path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

pub struct Client {
    router: Router,
    cookies: BTreeMap<String, String>,
    /// Jeton CSRF de la session, lu dans la dernière page reçue.
    pub csrf: Option<String>,
    ip: String,
}

pub struct TestResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: String,
    /// Corps brut (pour un fichier téléchargé).
    pub bytes: Vec<u8>,
}

impl TestResponse {
    /// Cible d'une redirection.
    pub fn location(&self) -> &str {
        self.headers
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
    }

    pub fn header(&self, name: &str) -> &str {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
    }
}

impl Client {
    pub async fn get(&mut self, path: &str) -> TestResponse {
        self.send(Method::GET, path, None, CsrfMode::Header, false)
            .await
    }

    /// GET tel que l'envoie htmx pour un fragment (widget, résultats…).
    pub async fn get_fragment(&mut self, path: &str) -> TestResponse {
        self.send(Method::GET, path, None, CsrfMode::Header, true)
            .await
    }

    /// POST d'un formulaire, avec le jeton CSRF dans l'en-tête (comme htmx).
    pub async fn post(&mut self, path: &str, form: &[(&str, &str)]) -> TestResponse {
        self.send(
            Method::POST,
            path,
            Some(SentBody::Form(encode(form))),
            CsrfMode::Header,
            false,
        )
        .await
    }

    /// POST sans aucun jeton CSRF : ce que pourrait envoyer un site tiers.
    pub async fn post_without_csrf(&mut self, path: &str, form: &[(&str, &str)]) -> TestResponse {
        self.send(
            Method::POST,
            path,
            Some(SentBody::Form(encode(form))),
            CsrfMode::None,
            false,
        )
        .await
    }

    /// Envoi d'un fichier comme le fait static/js/files.js : le contenu brut
    /// dans le corps, le nom dans l'adresse, le jeton CSRF dans l'en-tête.
    pub async fn upload(&mut self, name: &str, content: &[u8]) -> TestResponse {
        let path = format!("/files?name={}", encode_component(name));
        self.send(
            Method::POST,
            &path,
            Some(SentBody::Raw(content.to_vec(), true)),
            CsrfMode::Header,
            true,
        )
        .await
    }

    /// Envoi sans taille annoncée : seul le comptage des octets reçus peut
    /// alors arrêter un fichier trop gros.
    pub async fn upload_unannounced(&mut self, name: &str, content: &[u8]) -> TestResponse {
        let path = format!("/files?name={}", encode_component(name));
        self.send(
            Method::POST,
            &path,
            Some(SentBody::Raw(content.to_vec(), false)),
            CsrfMode::Header,
            true,
        )
        .await
    }

    /// Même envoi, sans jeton CSRF.
    pub async fn upload_without_csrf(&mut self, name: &str, content: &[u8]) -> TestResponse {
        let path = format!("/files?name={}", encode_component(name));
        self.send(
            Method::POST,
            &path,
            Some(SentBody::Raw(content.to_vec(), true)),
            CsrfMode::None,
            true,
        )
        .await
    }

    pub async fn delete(&mut self, path: &str) -> TestResponse {
        self.send(Method::DELETE, path, None, CsrfMode::Header, false)
            .await
    }

    /// Suit une redirection 303 et renvoie la page d'arrivée.
    pub async fn follow(&mut self, response: TestResponse) -> TestResponse {
        assert_eq!(
            response.status,
            StatusCode::SEE_OTHER,
            "redirection attendue : {}",
            response.body
        );
        let target = response.location().to_string();
        self.get(&target).await
    }

    /// Connexion par le formulaire, comme un navigateur : on charge la page
    /// pour obtenir le jeton CSRF, puis on l'envoie dans le champ caché.
    pub async fn login(&mut self, email: &str, password: &str) -> TestResponse {
        let page = self.get("/login").await;
        let token = between(&page.body, "name=\"_csrf\" value=\"", "\"")
            .expect("jeton CSRF du formulaire")
            .to_string();
        let form = encode(&[("_csrf", &token), ("email", email), ("password", password)]);
        let response = self
            .send(
                Method::POST,
                "/login",
                Some(SentBody::Form(form)),
                CsrfMode::None,
                false,
            )
            .await;
        if response.status == StatusCode::SEE_OTHER {
            // La connexion change le jeton : on relit celui de la session ouverte.
            self.csrf = None;
            self.get("/styleguide").await;
        }
        response
    }

    /// Valeur brute du cookie de session.
    pub fn session_cookie(&self) -> Option<&String> {
        self.cookies.get("dashu")
    }

    pub fn set_session_cookie(&mut self, value: &str) {
        self.cookies.insert("dashu".into(), value.into());
    }

    async fn send(
        &mut self,
        method: Method,
        path: &str,
        body: Option<SentBody>,
        csrf: CsrfMode,
        fragment: bool,
    ) -> TestResponse {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            // Adresse du visiteur, telle que Cloudflare la transmet.
            .header("cf-connecting-ip", &self.ip);
        if !self.cookies.is_empty() {
            let cookies: Vec<String> = self
                .cookies
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            request = request.header(header::COOKIE, cookies.join("; "));
        }
        if let (CsrfMode::Header, Some(token)) = (csrf, &self.csrf) {
            request = request.header("x-csrf-token", token);
        }
        if fragment {
            request = request.header("hx-request", "true");
        }
        let body = match body {
            Some(SentBody::Form(form)) => {
                request = request.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
                Body::from(form)
            }
            Some(SentBody::Raw(bytes, announce)) => {
                request = request.header(header::CONTENT_TYPE, "application/octet-stream");
                if announce {
                    request = request.header(header::CONTENT_LENGTH, bytes.len());
                }
                Body::from(bytes)
            }
            None => Body::empty(),
        };

        let response = self
            .router
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let (parts, body) = response.into_parts();

        for value in parts.headers.get_all(header::SET_COOKIE) {
            let cookie = value.to_str().unwrap();
            let pair = cookie.split(';').next().unwrap();
            if let Some((name, value)) = pair.split_once('=') {
                if value.is_empty() || cookie.contains("Max-Age=0") {
                    self.cookies.remove(name);
                } else {
                    self.cookies.insert(name.to_string(), value.to_string());
                }
            }
        }

        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        let bytes = bytes.to_vec();
        let body = String::from_utf8_lossy(&bytes).into_owned();
        // Les pages connectées portent le jeton dans l'attribut hx-headers.
        if let Some(token) = between(&body, "X-CSRF-Token&quot;:&quot;", "&quot;")
            .or_else(|| between(&body, "X-CSRF-Token\":\"", "\""))
        {
            self.csrf = Some(token.to_string());
        }
        TestResponse {
            status: parts.status,
            headers: parts.headers,
            body,
            bytes,
        }
    }
}

enum SentBody {
    Form(String),
    /// Contenu brut ; le booléen dit si la taille est annoncée (Content-Length).
    Raw(Vec<u8>, bool),
}

#[derive(Clone, Copy)]
enum CsrfMode {
    Header,
    None,
}

fn encode_component(text: &str) -> String {
    form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

fn encode(form: &[(&str, &str)]) -> String {
    form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form)
        .finish()
}

/// Texte compris entre deux marqueurs.
pub fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let from = text.find(start)? + start.len();
    let len = text[from..].find(end)?;
    Some(&text[from..from + len])
}

/// Identifiant de note à la fin d'une redirection `/notes/{id}`.
pub fn note_id(response: &TestResponse) -> i64 {
    response
        .location()
        .trim_start_matches("/notes/")
        .parse()
        .expect("redirection vers /notes/{id}")
}
