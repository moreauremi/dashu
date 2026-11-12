//! dashu : espace de travail personnel (dashboard, notes Markdown, emploi du temps).
//!
//! Un dossier par module, chacun avec ses routes, ses handlers et ses
//! requêtes SQL. Ce fichier assemble le tout : état partagé et routeur.

pub mod api;
pub mod assets;
pub mod auth;
pub mod calendar;
pub mod config;
pub mod dashboard;
pub mod db;
pub mod files;
pub mod notes;
pub mod search;
pub mod settings;
pub mod spaces;
pub mod tabbar;
pub mod time;
pub mod tools;
pub mod web;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{Extensions, HeaderMap, StatusCode, Version, header};
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::{delete, get, post, put};
use sqlx::SqlitePool;
use tower_governor::GovernorLayer;
use tower_governor::governor::GovernorConfigBuilder;
use tower_http::compression::CompressionLayer;
use tower_http::compression::predicate::{DefaultPredicate, Predicate};
use tower_http::trace::TraceLayer;
use tower_sessions::cookie::{Key, SameSite};
use tower_sessions::{Expiry, SessionManagerLayer};

use auth::google::GoogleOidc;
use config::Config;

/// État partagé par tous les handlers. Peu coûteux à cloner (des `Arc`).
#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub config: Arc<Config>,
    /// Client HTTP sortant (téléchargement des flux iCal).
    pub http: reqwest::Client,
    /// `None` si la connexion Google n'est pas configurée.
    pub google: Option<Arc<GoogleOidc>>,
    /// Envois de fichiers dont un morceau est en train d'arriver.
    pub uploads_busy: files::Busy,
}

impl AppState {
    pub fn new(config: Config, db: SqlitePool) -> anyhow::Result<Self> {
        let google = match &config.google {
            Some(google) => Some(GoogleOidc::new(google, &config.base_url)?),
            None => None,
        };
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::limited(3))
            .user_agent(concat!("dashu/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            db,
            config: Arc::new(config),
            http,
            google,
            uploads_busy: Default::default(),
        })
    }
}

/// Construit l'application complète : routes et middlewares.
pub fn build_router(state: AppState) -> Router {
    // --- Sessions : cookie signé, contenu stocké dans SQLite -------------
    let https = state.config.is_https();
    let sessions = SessionManagerLayer::new(db::session_store(&state.db))
        // Le préfixe __Host- oblige le navigateur à n'accepter le cookie
        // qu'en HTTPS, pour ce seul domaine.
        .with_name(if https { "__Host-dashu" } else { "dashu" })
        .with_secure(https)
        .with_http_only(true)
        .with_same_site(SameSite::Lax)
        .with_expiry(Expiry::OnInactivity(
            tower_sessions::cookie::time::Duration::days(auth::session::LOGGED_IN_DAYS),
        ))
        .with_signed(Key::from(state.config.session_secret.as_bytes()));

    // --- Anti-bruteforce sur les routes de connexion ----------------------
    let limiter_config = Arc::new(
        GovernorConfigBuilder::default()
            .period(Duration::from_secs(auth::ratelimit::REFILL_SECONDS))
            .burst_size(auth::ratelimit::BURST)
            .key_extractor(auth::ratelimit::ClientIp)
            .finish()
            .expect("configuration du limiteur valide"),
    );
    // Oublie régulièrement les adresses IP qui ne se sont plus présentées.
    let limiter = limiter_config.limiter().clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(300));
        loop {
            tick.tick().await;
            limiter.retain_recent();
        }
    });
    let login_limit = || {
        GovernorLayer::new(limiter_config.clone()).error_handler(auth::ratelimit::too_many_requests)
    };

    // --- Routes qui passent par la session et la protection CSRF ----------
    let app = Router::new()
        // Authentification
        .route(
            "/login",
            get(auth::login_page).merge(post(auth::login).layer(login_limit())),
        )
        .route(
            "/auth/google",
            get(auth::google::start).layer(login_limit()),
        )
        .route(
            "/auth/google/callback",
            get(auth::google::callback).layer(login_limit()),
        )
        .route("/logout", post(auth::logout))
        // Espaces
        .route("/space", post(spaces::switch))
        // Dashboard et ses widgets
        .route("/", get(dashboard::page))
        .route("/widgets/courses", get(dashboard::courses))
        .route("/widgets/tasks", get(dashboard::tasks))
        .route("/widgets/notes", get(dashboard::recent_notes))
        .route("/tasks", post(dashboard::add_task))
        .route("/tasks/{id}/toggle", post(dashboard::toggle_task))
        .route("/tasks/{id}", delete(dashboard::delete_task))
        // Notes
        .route("/notes", get(notes::list).post(notes::create))
        .route("/notes/new", get(notes::new_page))
        .route("/notes/preview", post(notes::preview))
        .route("/notes/{id}", get(notes::edit_page).post(notes::save))
        .route("/notes/{id}/pin", post(notes::toggle_pin))
        .route("/notes/{id}/move", post(notes::move_to_space))
        .route("/notes/{id}/delete", post(notes::delete))
        .route("/notes/{id}/tags", post(notes::add_tag))
        .route("/notes/{id}/tags/{tag_id}", delete(notes::remove_tag))
        // Fichiers (un envoi lit le corps en continu, sans la limite de 2 Mo)
        .route("/files", get(files::list))
        .route("/files/uploads", post(files::start_upload))
        .route("/files/uploads/{id}", put(files::upload_chunk))
        .route("/files/{id}", get(files::view))
        .route("/files/{id}/download", get(files::download))
        .route("/files/{id}/raw", get(files::raw))
        .route("/files/{id}/thumb", get(files::thumb))
        .route("/files/{id}/preview", get(files::preview))
        .route("/files/{id}/delete", post(files::delete))
        // Outils : compression (le travail est fait par dashu-worker)
        .route("/files/{id}/compress", post(tools::compress))
        .route("/files/jobs", get(tools::jobs_fragment))
        // Agenda
        .route("/calendar", get(calendar::page))
        .route("/calendar/refresh", post(calendar::refresh))
        // Recherche
        .route("/search/palette", get(search::palette))
        // Réglages
        .route("/settings", get(settings::page))
        .route("/settings/theme", post(settings::set_theme))
        .route("/settings/password", post(settings::set_password))
        .route("/settings/tabs", post(settings::set_tabs))
        .route("/settings/google/unlink", post(settings::unlink_google))
        .route("/settings/spaces", post(spaces::create))
        .route("/settings/spaces/{id}", post(spaces::update))
        .route(
            "/settings/spaces/{id}/delete",
            get(spaces::delete_page).post(spaces::delete),
        )
        .route("/settings/calendars", post(settings::add_calendar))
        .route(
            "/settings/calendars/{id}/delete",
            post(settings::delete_calendar),
        )
        // Guide de style
        .route("/styleguide", get(web::styleguide::page))
        .fallback(web::error::not_found)
        .layer(from_fn(web::csrf::protect))
        .layer(sessions)
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024));

    // --- Routes sans session, puis middlewares communs --------------------
    // L'ordre des `.layer` se lit de bas en haut : la dernière ajoutée est la
    // première traversée par la requête.
    // L'API JSON est ajoutée après la couche des pages d'erreur : ses erreurs
    // restent en JSON, au lieu d'être transformées en pages HTML.
    Router::new()
        .route("/health", get(health))
        .route("/static/{*path}", get(assets::serve))
        .merge(app)
        .layer(from_fn(web::error::error_pages))
        .merge(api::router())
        .layer(from_fn_with_state(
            state.clone(),
            web::headers::security_headers,
        ))
        .layer(CompressionLayer::new().compress_when(DefaultPredicate::new().and(not_a_download)))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Un fichier téléchargé n'est pas recompressé : souvent déjà compressé
/// (photo, vidéo, PDF), il coûterait du processeur pour rien sur un vieux
/// serveur, et garderait sa taille annoncée (`Content-Length`).
fn not_a_download(_: StatusCode, _: Version, headers: &HeaderMap, _: &Extensions) -> bool {
    !headers.contains_key(header::CONTENT_DISPOSITION)
}

/// `GET /health` : pour la supervision, sans authentification.
async fn health() -> &'static str {
    "ok"
}
