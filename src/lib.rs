//! dashu : espace de travail personnel (dashboard, notes Markdown, emploi du temps).
//!
//! Un dossier par module, chacun avec ses routes, ses handlers et ses
//! requêtes SQL. Ce fichier assemble le tout : état partagé et routeur.

pub mod api;
pub mod assets;
pub mod auth;
pub mod calendar;
pub mod commands;
pub mod config;
pub mod dashboard;
pub mod db;
pub mod files;
pub mod folders;
pub mod instance;
pub mod mcp;
pub mod notes;
pub mod oauth;
pub mod restart;
pub mod settings;
pub mod setup;
pub mod spaces;
pub mod tabbar;
pub mod time;
pub mod tools;
pub mod trash;
pub mod web;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{Extensions, HeaderMap, StatusCode, Version, header};
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::{delete, get, post, put};
use sqlx::SqlitePool;
use tower::Layer as _;
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
    /// Sessions des notes ouvertes en direct (voir `notes::live`).
    pub live: Arc<notes::live::Live>,
    /// Un envoi à Google Agenda à la fois : deux envois simultanés créeraient
    /// deux fois le même événement (voir `calendar::google`).
    pub google_push: Arc<tokio::sync::Mutex<()>>,
    /// Vrai dès qu'un compte existe ; avant, tout mène au premier lancement
    /// (`setup`).
    pub installed: Arc<std::sync::atomic::AtomicBool>,
    /// Code d'installation de ce démarrage (voir `setup::new_code`).
    pub setup_code: Arc<str>,
    /// Relance demandée après un changement des réglages de l'instance.
    pub restart: Arc<restart::Restart>,
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
            live: Arc::default(),
            google_push: Arc::default(),
            installed: Arc::default(),
            setup_code: setup::new_code().into(),
            restart: Arc::default(),
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
    // Liens de partage (pages publiques) : leur propre compteur, plus large
    // (20 d'un coup, puis un toutes les 3 s) ; sans quoi ouvrir un lien
    // entamerait les tentatives de connexion.
    let share_config = Arc::new(
        GovernorConfigBuilder::default()
            .period(Duration::from_secs(3))
            .burst_size(20)
            .key_extractor(auth::ratelimit::ClientIp)
            .finish()
            .expect("configuration du limiteur valide"),
    );
    let share_limiter = share_config.limiter().clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(300));
        loop {
            tick.tick().await;
            share_limiter.retain_recent();
        }
    });
    let share_limit =
        || GovernorLayer::new(share_config.clone()).error_handler(files::share::too_many_requests);

    // --- Routes qui passent par la session et la protection CSRF ----------
    let app = Router::new()
        // Premier lancement, tant qu'aucun compte n'existe
        .route("/setup", get(setup::page).post(setup::finish))
        .route("/setup/code", post(setup::check_code).layer(login_limit()))
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
        // Page « Autoriser Claude » (OAuth), avec la session du site
        .merge(oauth::routes_with_session())
        // Espaces
        .route("/space", post(spaces::switch))
        // Dashboard et ses widgets
        .route("/", get(dashboard::page))
        .route("/widgets/courses", get(dashboard::courses))
        .route("/widgets/tasks", get(dashboard::tasks))
        .route("/widgets/notes", get(dashboard::recent_notes))
        .route("/widgets/files", get(dashboard::recent_files))
        .route("/widgets/calendar", get(dashboard::calendar_widget))
        .route("/widgets/settings", get(dashboard::quick_settings))
        .route("/dashboard/grid", get(dashboard::grid))
        .route("/dashboard/layout", post(dashboard::save_layout))
        .route("/tasks", post(dashboard::add_task))
        .route("/tasks/{id}/toggle", post(dashboard::toggle_task))
        .route("/tasks/{id}/move", post(dashboard::move_task))
        .route("/tasks/{id}", delete(dashboard::delete_task))
        // Notes
        .route("/notes", get(notes::list).post(notes::create))
        .route("/notes/new", get(notes::new_page))
        .route("/notes/{id}", get(notes::edit_page).post(notes::save))
        .route("/notes/{id}/live", get(notes::live::socket))
        .route("/notes/{id}/pin", post(notes::toggle_pin))
        .route("/notes/{id}/move", post(notes::move_to_space))
        .route("/notes/{id}/folder", post(notes::set_folder))
        .route("/notes/{id}/delete", post(notes::delete))
        .route("/notes/{id}/download.md", get(notes::export::markdown))
        .route("/notes/{id}/download.pdf", get(notes::export::pdf))
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
        .route("/files/{id}/move", post(files::move_to_space))
        .route("/files/{id}/keep", post(files::keep))
        .route("/files/{id}/rename", post(files::rename))
        .route("/files/{id}/share", post(files::share::create))
        .route(
            "/files/{id}/share/{link}/revoke",
            post(files::share::revoke),
        )
        .route("/files/{id}/tags", post(files::tags::add))
        .route("/files/{id}/tags/{tag_id}", delete(files::tags::remove))
        .route("/folders", post(folders::create))
        .route("/folders/{id}/rename", post(folders::rename))
        .route("/folders/{id}/delete", post(folders::delete))
        .route("/folders/{id}/move", post(folders::move_folder))
        .route("/destinations", get(folders::picker))
        // Outils : compression et conversions (le travail est fait par
        // dashu-worker), onglet Outils, suivi dans la barre latérale
        .route("/files/{id}/compress", post(tools::compress))
        .route("/files/{id}/convert", post(tools::convert))
        .route("/files/jobs", get(tools::jobs_fragment))
        .route("/files/jobs/{id}/replace", post(tools::replace_original))
        .route("/jobs/indicator", get(tools::indicator))
        .route("/tools", get(tools::page))
        // Agenda
        .route("/calendar", get(calendar::page))
        .route("/calendar/refresh", post(calendar::refresh))
        // Événements créés dans dashu, copiés dans Google Agenda si relié.
        .route("/calendar/events/new", get(calendar::events::new_page))
        .route("/calendar/events", post(calendar::events::create))
        .route(
            "/calendar/events/{id}",
            get(calendar::events::edit_page).post(calendar::events::update),
        )
        .route(
            "/calendar/events/{id}/delete",
            post(calendar::events::delete),
        )
        .route(
            "/settings/google-calendar/connect",
            get(calendar::google::connect),
        )
        .route(
            "/settings/google-calendar/disconnect",
            post(calendar::google::disconnect),
        )
        // Barre de commandes « / »
        .route("/cmd", post(commands::run))
        .route("/cmd/catalogue", get(commands::catalogue))
        .route("/cmd/suggest", get(commands::suggest))
        // Réglages
        .route("/settings", get(settings::page))
        .route("/settings/theme", post(settings::set_theme))
        .route("/settings/profile", post(settings::profile::save_profile))
        .route("/settings/avatar", post(settings::profile::upload_avatar))
        .route(
            "/settings/avatar/delete",
            post(settings::profile::delete_avatar),
        )
        .route("/profile/avatar", get(settings::profile::avatar))
        .route("/settings/size", post(settings::profile::set_size))
        .route("/settings/sidebar", post(settings::profile::set_sidebar))
        .route("/settings/password", post(settings::set_password))
        // Réglages › Instance, pour l'administrateur
        .route(
            "/settings/instance/address",
            post(settings::instance::set_address),
        )
        .route(
            "/settings/instance/google",
            post(settings::instance::set_google),
        )
        .route(
            "/settings/instance/google/remove",
            post(settings::instance::remove_google),
        )
        .route(
            "/settings/instance/emails",
            post(settings::instance::add_email),
        )
        .route(
            "/settings/instance/emails/remove",
            post(settings::instance::remove_email),
        )
        .route("/settings/tabs", post(settings::set_tabs))
        .route("/settings/google/unlink", post(settings::unlink_google))
        // Connexions : jetons d'accès du compte
        .route("/settings/tokens", post(settings::connexions::create))
        .route(
            "/settings/tokens/revoke-all",
            post(settings::connexions::revoke_all),
        )
        .route(
            "/settings/tokens/{id}/revoke",
            post(settings::connexions::revoke),
        )
        .route(
            "/settings/grants/{id}/revoke",
            post(settings::connexions::revoke_grant),
        )
        .route("/settings/spaces", post(spaces::create))
        .route("/settings/spaces/{id}", post(spaces::update))
        .route("/settings/spaces/{id}/archive", post(spaces::archive))
        .route("/settings/spaces/{id}/restore", post(spaces::restore))
        .route("/settings/moment", post(settings::set_moment))
        .route("/settings/spaces/{id}/members", get(spaces::members_page))
        .route("/settings/spaces/{id}/invitations", post(spaces::invite))
        .route(
            "/settings/spaces/{id}/invitations/{inv}/cancel",
            post(spaces::cancel_invitation),
        )
        .route(
            "/settings/spaces/{id}/members/{user}/remove",
            post(spaces::remove),
        )
        .route("/settings/spaces/{id}/leave", post(spaces::leave))
        .route(
            "/settings/invitations/{inv}/accept",
            post(spaces::accept_invitation),
        )
        .route(
            "/settings/invitations/{inv}/decline",
            post(spaces::decline_invitation),
        )
        .route(
            "/settings/spaces/{id}/delete",
            get(spaces::delete_page).post(spaces::delete),
        )
        // Corbeille
        .route("/settings/trash", get(trash::page::show))
        .route("/settings/trash/empty", post(trash::page::empty))
        .route(
            "/settings/trash/{kind}/{id}/restore",
            post(trash::page::restore),
        )
        .route(
            "/settings/trash/{kind}/{id}/erase",
            post(trash::page::erase),
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
    let router = Router::new()
        .route("/health", get(health))
        // Liens de partage : publics, sans session ni connexion.
        .route("/s/{token}", get(files::share::page).layer(share_limit()))
        .route(
            "/s/{token}/fichier",
            get(files::share::download).layer(share_limit()),
        )
        .route("/static/{*path}", get(assets::serve))
        .merge(app)
        .layer(from_fn(web::error::error_pages))
        .merge(api::router())
        // Connecteur Claude (MCP) : comme l'API, sans session ni CSRF.
        .merge(mcp::router())
        // OAuth : découverte, enregistrement, jetons (sans session ni cookie).
        .merge(oauth::router())
        // Sans compte, tout mène au premier lancement.
        .layer(from_fn_with_state(state.clone(), setup::guard))
        .layer(from_fn_with_state(
            state.clone(),
            web::headers::security_headers,
        ))
        .layer(CompressionLayer::new().compress_when(DefaultPredicate::new().and(not_a_download)))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    // L'espace dans l'adresse (`/e/<slug>/…`) : le préfixe doit être retiré
    // avant le routage, donc autour du routeur entier (une couche de
    // `Router::layer` ne s'applique qu'après le choix de la route).
    Router::new().fallback_service(from_fn(spaces::url::rewrite).layer(router))
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
