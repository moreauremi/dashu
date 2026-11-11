//! Point d'entrée du binaire dashu.
//!
//!   dashu                        lance le serveur
//!   dashu set-password <email>   crée un compte ou remplace son mot de passe
//!   dashu backup <fichier>       copie à chaud de la base (VACUUM INTO)
//!   dashu healthcheck            interroge /health (pour le HEALTHCHECK Docker)

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, bail};
use dashu::config::Config;
use dashu::{AppState, auth, build_router, calendar, db, files};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower_sessions::ExpiredDeletion;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();

    match args.as_slice() {
        [] | ["serve"] => serve().await,
        ["set-password", email] => set_password(email).await,
        ["backup", destination] => backup(destination).await,
        ["healthcheck"] => healthcheck().await,
        _ => {
            eprintln!(
                "usage :\n  dashu                        lance le serveur\n  dashu set-password <email>   crée un compte ou remplace son mot de passe\n  dashu backup <fichier>       copie à chaud de la base SQLite\n  dashu healthcheck            vérifie que le serveur répond"
            );
            std::process::exit(2);
        }
    }
}

async fn serve() -> anyhow::Result<()> {
    let config = Config::from_env()?;
    init_logging(config.log_json);

    let pool = db::connect(&config.database_url)
        .await
        .context("ouverture de la base SQLite")?;
    let bind_addr = config.bind_addr;
    if let Some(files) = &config.files {
        files::prepare_storage(files);
    }
    let state = AppState::new(config, pool.clone())?;

    // Tâches de fond : synchro des agendas toutes les heures, purge des
    // sessions expirées toutes les heures.
    calendar::sync::spawn_hourly(state.clone());
    tokio::spawn(db::session_store(&pool).continuously_delete_expired(Duration::from_secs(3600)));

    let app = build_router(state.clone());
    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("écoute sur {bind_addr}"))?;
    tracing::info!(
        addr = %bind_addr,
        base_url = %state.config.base_url,
        google = state.google.is_some(),
        files = state.config.files.is_some(),
        "dashu démarré"
    );

    // `with_connect_info` : le limiteur de tentatives a besoin de l'adresse IP.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    pool.close().await;
    tracing::info!("dashu arrêté proprement");
    Ok(())
}

/// Logs structurés sur la sortie standard (lus par `docker logs`).
/// Niveau réglable avec `RUST_LOG` (par défaut : info).
fn init_logging(json: bool) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,tower_http=info,sqlx=warn".into());
    // Couleurs seulement dans un terminal : sous systemd, les codes couleur
    // font classer chaque ligne en « blob » par journald.
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()));
    if json {
        builder.json().init();
    } else {
        builder.init();
    }
}

/// Attend Ctrl+C ou SIGTERM (`docker stop`) pour finir les requêtes en cours.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}

/// `dashu set-password <email>` : le mot de passe est lu sur l'entrée
/// standard, pour ne jamais apparaître dans l'historique du shell.
async fn set_password(email: &str) -> anyhow::Result<()> {
    let config = Config::from_env()?;
    if !config.is_allowed(email) {
        bail!("{email} n'est pas dans ALLOWED_EMAILS : ce compte ne pourrait pas se connecter");
    }

    eprint!("Mot de passe pour {email} (12 caractères minimum) : ");
    let mut line = String::new();
    // Lecture bloquante : sans importance, cette commande ne fait que ça.
    std::io::stdin().read_line(&mut line)?;
    let password = line.trim_end_matches(['\r', '\n']).to_string();

    let pool = db::connect(&config.database_url).await?;
    auth::set_password(&pool, email, password).await?;
    pool.close().await;
    eprintln!("Mot de passe enregistré pour {email}.");
    Ok(())
}

/// `dashu backup <fichier>` : copie cohérente de la base pendant que le
/// serveur tourne. `VACUUM INTO` écrit un fichier SQLite complet et compacté,
/// sans bloquer les lectures ni les écritures en cours (mode WAL).
async fn backup(destination: &str) -> anyhow::Result<()> {
    use sqlx::sqlite::SqliteConnectOptions;
    use sqlx::{ConnectOptions, Connection};
    use std::str::FromStr;

    if std::path::Path::new(destination).exists() {
        bail!("{destination} existe déjà : une sauvegarde n'écrase jamais un fichier");
    }
    // DATABASE_URL suffit ici : pas besoin du reste de la configuration.
    let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://dashu.db".into());
    let mut source = SqliteConnectOptions::from_str(&database_url)?
        .busy_timeout(Duration::from_secs(30))
        .connect()
        .await
        .context("ouverture de la base à sauvegarder")?;
    sqlx::query("VACUUM INTO ?")
        .bind(destination)
        .execute(&mut source)
        .await
        .context("VACUUM INTO")?;
    source.close().await?;

    // Une sauvegarde qu'on ne sait pas relire ne vaut rien : on la rouvre et
    // on demande à SQLite d'en vérifier la structure.
    let mut copy = SqliteConnectOptions::new()
        .filename(destination)
        .read_only(true)
        .connect()
        .await?;
    let check: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut copy)
        .await?;
    let notes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notes")
        .fetch_one(&mut copy)
        .await?;
    copy.close().await?;
    if check != "ok" {
        bail!("la sauvegarde {destination} est corrompue : {check}");
    }
    let pluriel = if notes > 1 { "s" } else { "" };
    eprintln!("Sauvegarde écrite et vérifiée : {destination} ({notes} note{pluriel}).");
    Ok(())
}

/// `dashu healthcheck` : l'image Docker finale ne contient ni curl ni wget,
/// le binaire sait donc s'interroger lui-même.
async fn healthcheck() -> anyhow::Result<()> {
    let port = std::env::var("BIND_ADDR")
        .ok()
        .and_then(|addr| addr.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()))
        .unwrap_or(3000);

    let check = async {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        stream
            .write_all(b"GET /health HTTP/1.0\r\nHost: localhost\r\n\r\n")
            .await?;
        let mut response = String::new();
        stream.read_to_string(&mut response).await?;
        anyhow::Ok(response)
    };
    let response = tokio::time::timeout(Duration::from_secs(3), check)
        .await
        .context("pas de réponse en 3 secondes")??;

    if response.starts_with("HTTP/1.0 200") || response.starts_with("HTTP/1.1 200") {
        Ok(())
    } else {
        bail!(
            "réponse inattendue : {}",
            response.lines().next().unwrap_or("")
        )
    }
}
