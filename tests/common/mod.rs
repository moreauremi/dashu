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
use dashu::config::{ApiConfig, Config, Env, FilesConfig, FromEnv, GoogleConfig, GoogleEndpoints};
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
    /// Dossier de travail des outils (WORK_DIR), propre à ce test.
    pub work_dir: PathBuf,
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
            None,
        )
        .await;
        app.add_user(ALICE).await;
        app.add_user(BOB).await;
        app
    }

    /// Comme [`TestApp::new`], avec le téléchargeur de liens réservé à Alice
    /// (`DOWNLOADER_EMAIL`).
    pub async fn with_downloader() -> Self {
        let app = Self::build_with(
            Some(ApiConfig {
                token: API_TOKEN.into(),
                user_email: ALICE.into(),
            }),
            true,
            None,
            |config| config.downloader_email = Some(ALICE.into()),
        )
        .await;
        app.add_user(ALICE).await;
        app.add_user(BOB).await;
        app
    }

    async fn with_api(api: Option<ApiConfig>) -> Self {
        Self::build(api, true, None).await
    }

    /// Premier lancement : ni compte, ni adresse autorisée, ni réglage venu de
    /// l'environnement (installation par l'assistant ou l'image Docker).
    pub async fn first_launch() -> Self {
        Self::build_with(None, true, None, |config| {
            config.allowed_emails = Vec::new();
            config.from_env = FromEnv::default();
        })
        .await
    }

    /// La même application après une relance : la configuration est relue
    /// comme au démarrage (`Config::load`), sur la même base, avec ces seules
    /// variables d'environnement en plus de DATABASE_URL et SESSION_SECRET.
    pub async fn restarted(&self, env: &[(&str, &str)]) -> Self {
        let database_url = self.state.config.database_url.clone();
        let secret = self.state.config.session_secret.clone();
        let mut pairs: Vec<(&str, &str)> =
            vec![("DATABASE_URL", &database_url), ("SESSION_SECRET", &secret)];
        pairs.extend_from_slice(env);
        let (config, pool) = Config::load(&Env::from_pairs(pairs))
            .await
            .expect("configuration relue");
        let state = AppState::new(config, pool).expect("état relancé");
        // Les dossiers restent ceux de l'application d'origine, qui les
        // nettoie : ceux-ci ne doivent rien effacer en partant.
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let gone =
            std::env::temp_dir().join(format!("dashu-test-{}-{n}-relance", std::process::id()));
        Self {
            router: build_router(state.clone()),
            state,
            db_path: gone.join("base.db"),
            files_dir: gone.join("fichiers"),
            work_dir: gone.join("travail"),
        }
    }

    /// Comme [`TestApp::new`], avec Google configuré et ses adresses sous
    /// `base` : un faux Google local (Google Agenda, voir
    /// tests/agenda_evenements.rs).
    pub async fn with_google(base: &str) -> Self {
        let app = Self::build(
            Some(ApiConfig {
                token: API_TOKEN.into(),
                user_email: ALICE.into(),
            }),
            true,
            Some(GoogleConfig {
                client_id: "client-de-test".into(),
                client_secret: "secret-de-test".into(),
                endpoints: GoogleEndpoints::at(base),
            }),
        )
        .await;
        app.add_user(ALICE).await;
        app.add_user(BOB).await;
        app
    }

    async fn build(api: Option<ApiConfig>, files: bool, google: Option<GoogleConfig>) -> Self {
        Self::build_with(api, files, google, |_| {}).await
    }

    async fn build_with(
        api: Option<ApiConfig>,
        files: bool,
        google: Option<GoogleConfig>,
        tweak: impl FnOnce(&mut Config),
    ) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let db_path =
            std::env::temp_dir().join(format!("dashu-test-{}-{n}.db", std::process::id()));
        remove_db(&db_path);
        let files_dir =
            std::env::temp_dir().join(format!("dashu-test-{}-{n}-fichiers", std::process::id()));
        let _ = std::fs::remove_dir_all(&files_dir);
        let work_dir =
            std::env::temp_dir().join(format!("dashu-test-{}-{n}-travail", std::process::id()));
        let _ = std::fs::remove_dir_all(&work_dir);

        // Comme le serveur de Rémi : adresse, comptes et Google viennent de
        // l'environnement.
        let from_env = FromEnv {
            base_url: true,
            google: google.is_some(),
            allowed_emails: true,
        };
        let mut config = Config {
            database_url: format!("sqlite://{}", db_path.display()),
            base_url: "http://localhost:3000".into(),
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            allowed_emails: vec![ALICE.into(), BOB.into()],
            session_secret: "0123456789abcdef".repeat(4),
            google,
            log_json: false,
            api,
            files: files.then(|| FilesConfig {
                dir: files_dir.clone(),
                max_upload_bytes: MAX_UPLOAD,
            }),
            work_dir: files.then(|| work_dir.clone()),
            downloader_email: None,
            from_env,
        };
        tweak(&mut config);
        let pool = db::connect(&config.database_url)
            .await
            .expect("base de test");
        let state = AppState::new(config, pool).expect("état de test");
        Self {
            router: build_router(state.clone()),
            state,
            db_path,
            files_dir,
            work_dir,
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
        // Le premier compte devient l'administrateur, comme au démarrage.
        dashu::instance::ensure_admin(&self.state.db)
            .await
            .expect("administrateur");
    }

    /// Instance installée par l'assistant du premier lancement : rien ne
    /// vient de l'environnement, Alice (administratrice) et Bob sont
    /// autorisés en base et inscrits.
    pub async fn by_wizard() -> Self {
        let app = Self::build_with(None, true, None, |config| {
            config.from_env = FromEnv::default();
        })
        .await;
        for email in [ALICE, BOB] {
            dashu::instance::allow(&app.state.db, email, None)
                .await
                .expect("adresse autorisée");
            app.add_user(email).await;
        }
        app
    }

    /// Sert l'application sur un vrai port local (127.0.0.1, port libre), pour
    /// les tests qui ont besoin d'une vraie connexion, comme les WebSockets
    /// des notes en direct. Le serveur s'arrête avec le runtime du test.
    pub async fn serve(&self) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("port local");
        let addr = listener.local_addr().expect("adresse locale");
        let app = self
            .router
            .clone()
            .into_make_service_with_connect_info::<std::net::SocketAddr>();
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serveur de test");
        });
        addr
    }

    /// Nouveau navigateur, sans cookie, avec sa propre adresse IP.
    pub fn client(&self) -> Client {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        Client {
            router: self.router.clone(),
            cookies: BTreeMap::new(),
            csrf: None,
            ip: format!("203.0.113.{}", n % 250 + 1),
            extra: Vec::new(),
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

    /// Rend `member` membre de l'espace `slug` de `owner`, sans passer par
    /// l'invitation (raccourci de test). Renvoie le slug de l'espace chez le
    /// membre : celui du propriétaire, ou suffixé (`ecole-2`) s'il est pris.
    pub async fn share_space(&self, owner: &str, slug: &str, member: &str) -> String {
        let space = self.space_id(owner, slug).await;
        let member_id = self.user_id(member).await;
        let taken: Vec<String> = sqlx::query_scalar(
            "SELECT slug FROM spaces WHERE user_id = ?1
             UNION SELECT slug FROM space_members WHERE user_id = ?1",
        )
        .bind(member_id)
        .fetch_all(&self.state.db)
        .await
        .expect("slugs du membre");
        let mut mine = slug.to_string();
        let mut n = 2;
        while taken.contains(&mine) {
            mine = format!("{slug}-{n}");
            n += 1;
        }
        sqlx::query(
            "INSERT INTO space_members (space_id, user_id, slug, position, joined_at)
             VALUES (?, ?, ?, 100, 0)",
        )
        .bind(space)
        .bind(member_id)
        .bind(&mine)
        .execute(&self.state.db)
        .await
        .expect("membre ajouté");
        mine
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

impl TestApp {
    /// Appel de l'API dont la réponse n'est pas du JSON (un téléchargement),
    /// ou dont le corps envoyé est brut (un morceau de fichier).
    pub async fn api_bytes(
        &self,
        method: Method,
        path: &str,
        body: Option<Vec<u8>>,
        token: Option<&str>,
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let body = match body {
            Some(bytes) => {
                request = request.header(header::CONTENT_TYPE, "application/octet-stream");
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
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec();
        (status, headers, bytes)
    }
}

impl Drop for TestApp {
    fn drop(&mut self) {
        remove_db(&self.db_path);
        let _ = std::fs::remove_dir_all(&self.files_dir);
        let _ = std::fs::remove_dir_all(&self.work_dir);
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
    /// En-têtes ajoutés à la prochaine requête seulement.
    extra: Vec<(String, String)>,
}

impl Client {
    /// En-tête `Cookie` de ce navigateur (session comprise), pour ouvrir une
    /// vraie connexion à côté du routeur de test.
    pub fn cookie_header(&self) -> String {
        self.cookies
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }
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

    /// Ajoute un en-tête à la prochaine requête (par exemple `x-espace`,
    /// l'espace de l'onglet que htmx joint à chaque requête).
    pub fn with_header(&mut self, name: &str, value: &str) -> &mut Self {
        self.extra.push((name.into(), value.into()));
        self
    }

    /// GET avec un en-tête `Range` (lecture d'une vidéo par morceaux).
    pub async fn get_range(&mut self, path: &str, range: &str) -> TestResponse {
        self.extra.push(("range".into(), range.into()));
        self.get(path).await
    }

    /// GET tel que l'envoie htmx pour un fragment (widget, résultats…).
    pub async fn get_fragment(&mut self, path: &str) -> TestResponse {
        self.send(Method::GET, path, None, CsrfMode::Header, true)
            .await
    }

    /// GET tel que l'envoie htmx pour changer de page (lien hx-boost).
    pub async fn get_boosted(&mut self, path: &str) -> TestResponse {
        self.extra.push(("hx-boosted".into(), "true".into()));
        self.get_fragment(path).await
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

    /// Envoi d'un fichier comme static/js/files.js : ouverture de l'envoi,
    /// puis le contenu en un seul morceau. Renvoie la dernière réponse.
    pub async fn upload(&mut self, name: &str, content: &[u8]) -> TestResponse {
        self.upload_in_chunks(name, content, usize::MAX).await
    }

    /// Même envoi, découpé en morceaux de `chunk` octets.
    pub async fn upload_in_chunks(
        &mut self,
        name: &str,
        content: &[u8],
        chunk: usize,
    ) -> TestResponse {
        let start = self.start_upload(name, content.len()).await;
        if start.status != StatusCode::CREATED {
            return start;
        }
        let id = start.body.clone();
        let mut offset: usize = 0;
        loop {
            let end = offset.saturating_add(chunk).min(content.len());
            let response = self
                .put_chunk(&id, offset, &content[offset..end], true)
                .await;
            if response.status != StatusCode::OK || end == content.len() {
                return response;
            }
            offset = end;
        }
    }

    /// `POST /files/uploads` : ouvre un envoi ; le corps de la réponse est
    /// l'identifiant de l'envoi.
    pub async fn start_upload(&mut self, name: &str, size: usize) -> TestResponse {
        let path = format!("/files/uploads?name={}&size={size}", encode_component(name));
        self.send(Method::POST, &path, None, CsrfMode::Header, true)
            .await
    }

    /// Ouverture d'un envoi dans un espace choisi (`&space=…`).
    pub async fn start_upload_in(&mut self, name: &str, size: usize, space: i64) -> TestResponse {
        let path = format!(
            "/files/uploads?name={}&size={size}&space={space}",
            encode_component(name)
        );
        self.send(Method::POST, &path, None, CsrfMode::Header, true)
            .await
    }

    /// Envoi dans un dossier (`&folder=…`, qui donne l'espace), en un seul
    /// morceau, comme depuis un dossier de l'onglet Fichiers.
    pub async fn upload_in_folder(
        &mut self,
        name: &str,
        content: &[u8],
        folder: i64,
    ) -> TestResponse {
        let path = format!(
            "/files/uploads?name={}&size={}&folder={folder}",
            encode_component(name),
            content.len()
        );
        let start = self
            .send(Method::POST, &path, None, CsrfMode::Header, true)
            .await;
        if start.status != StatusCode::CREATED {
            return start;
        }
        let id = start.body.clone();
        self.put_chunk(&id, 0, content, true).await
    }

    /// Envoi vers l'onglet Outils (`&tools=1`), en un seul morceau.
    pub async fn upload_to_tools(&mut self, name: &str, content: &[u8]) -> TestResponse {
        let path = format!(
            "/files/uploads?name={}&size={}&tools=1",
            encode_component(name),
            content.len()
        );
        let start = self
            .send(Method::POST, &path, None, CsrfMode::Header, true)
            .await;
        if start.status != StatusCode::CREATED {
            return start;
        }
        let id = start.body.clone();
        self.put_chunk(&id, 0, content, true).await
    }

    /// Même ouverture, sans jeton CSRF.
    pub async fn start_upload_without_csrf(&mut self, name: &str, size: usize) -> TestResponse {
        let path = format!("/files/uploads?name={}&size={size}", encode_component(name));
        self.send(Method::POST, &path, None, CsrfMode::None, true)
            .await
    }

    /// `PUT /files/uploads/{id}?offset=…` : un morceau. Avec `announce` faux,
    /// la taille n'est pas annoncée (pas de Content-Length).
    pub async fn put_chunk(
        &mut self,
        id: &str,
        offset: usize,
        bytes: &[u8],
        announce: bool,
    ) -> TestResponse {
        let path = format!("/files/uploads/{id}?offset={offset}");
        self.send(
            Method::PUT,
            &path,
            Some(SentBody::Raw(bytes.to_vec(), announce)),
            CsrfMode::Header,
            true,
        )
        .await
    }

    /// `POST` d'un corps brut (une image, par exemple), jeton CSRF en en-tête
    /// comme le fait static/js/app.js.
    pub async fn post_raw(&mut self, path: &str, bytes: &[u8]) -> TestResponse {
        self.send(
            Method::POST,
            path,
            Some(SentBody::Raw(bytes.to_vec(), true)),
            CsrfMode::Header,
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
        for (name, value) in std::mem::take(&mut self.extra) {
            request = request.header(name, value);
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

// ---------------------------------------------------------------------
// Barre de commandes « / »
// ---------------------------------------------------------------------

impl Client {
    /// Ligne validée dans la barre de commandes, depuis cette page (adresse
    /// sans l'espace), comme l'envoie le formulaire htmx de `layout.html`.
    pub async fn command(&mut self, line: &str, page: &str) -> TestResponse {
        self.extra.push(("hx-request".into(), "true".into()));
        self.post("/cmd", &[("line", line), ("page", page)]).await
    }

    /// Les commandes proposées sur cette page (`GET /cmd/catalogue`).
    pub async fn catalogue(&mut self, page: &str) -> serde_json::Value {
        let query = encode(&[("page", page)]);
        let response = self.get_fragment(&format!("/cmd/catalogue?{query}")).await;
        assert_eq!(response.status, StatusCode::OK, "{}", response.body);
        let mut json: serde_json::Value = serde_json::from_str(&response.body).unwrap();
        json["commands"].take()
    }
}

/// La page qu'ouvre une commande réussie : en-tête `HX-Location` (204).
pub fn hx_location(response: &TestResponse) -> serde_json::Value {
    assert_eq!(
        response.status,
        StatusCode::NO_CONTENT,
        "commande refusée : {}",
        response.body
    );
    serde_json::from_str(response.header("hx-location")).expect("en-tête HX-Location")
}

/// Une commande du catalogue, par son chemin (`["add", "task"]`), ou `Null`.
pub fn command_node<'a>(tree: &'a serde_json::Value, path: &[&str]) -> &'a serde_json::Value {
    let mut level = tree;
    let mut node = &serde_json::Value::Null;
    for name in path {
        node = level
            .as_array()
            .and_then(|nodes| nodes.iter().find(|n| n["name"] == *name))
            .unwrap_or(&serde_json::Value::Null);
        level = &node["children"];
    }
    node
}

/// Valeurs proposées pour l'argument d'une commande (`v` de chaque choix).
pub fn arg_values(node: &serde_json::Value) -> Vec<String> {
    node["arg"]["values"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(|v| v["v"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Identifiant de note à la fin d'une redirection `/notes/{id}`.
pub fn note_id(response: &TestResponse) -> i64 {
    response
        .location()
        .trim_start_matches("/notes/")
        .parse()
        .expect("redirection vers /notes/{id}")
}
