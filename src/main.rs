//! Point d'entrée du binaire dashu.
//!
//!   dashu                        lance le serveur
//!   dashu set-password <email>   crée un compte ou remplace son mot de passe
//!   dashu backup <fichier>       copie à chaud de la base (VACUUM INTO)
//!   dashu quota …                plafonds de stockage (voir src/files/quota.rs)
//!   dashu healthcheck            interroge /health (pour le HEALTHCHECK Docker)
//!   dashu worker                 service de conversion (voir src/tools/worker.rs)
//!   dashu download [--check]     téléchargeur de liens (voir src/tools/telechargeur.rs)
//!   dashu version                version et commit du binaire (lus par dashu-update)
//!   dashu tools list|grant|revoke   accès aux outils réservés (voir src/registry.rs)
//!   dashu demo-export <dossier>  démo statique (feature `demo`, voir src/demo)

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, bail};
use dashu::config::{Config, Env};
use dashu::{
    AppState, auth, build_router, calendar, db, files, instance, registry, restart, setup, tools,
    trash,
};
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
        ["quota", rest @ ..] => quota(rest).await,
        ["healthcheck"] => healthcheck().await,
        ["worker"] => worker().await,
        ["download"] => download().await,
        ["download", "--check"] => {
            // Pour la CI et dashu-telechargeur : le pare-feu du service est-il là ?
            if tools::telechargeur::firewall_active() {
                println!(
                    "pare-feu actif : la machine, le réseau local et Tailscale sont injoignables"
                );
                Ok(())
            } else {
                println!("pare-feu absent : le téléchargeur joindrait le réseau local");
                std::process::exit(1);
            }
        }
        ["tools", "list"] => tools_list().await,
        ["tools", "grant", email, tool] => tools_grant(email, tool, true).await,
        ["tools", "revoke", email, tool] => tools_grant(email, tool, false).await,
        #[cfg(feature = "demo")]
        ["demo-export", out] => dashu::demo::export(std::path::Path::new(out), "/demo").await,
        #[cfg(feature = "demo")]
        ["demo-export", out, "--base", base] => {
            dashu::demo::export(std::path::Path::new(out), base).await
        }
        ["version"] | ["--version"] => {
            // Une seule ligne, « dashu <version> <commit> » : dashu-update la
            // compare à celle de la version à installer.
            println!(
                "dashu {} {}",
                env!("CARGO_PKG_VERSION"),
                env!("DASHU_COMMIT")
            );
            Ok(())
        }
        _ => {
            eprintln!(
                "usage :
  dashu                        lance le serveur
  dashu set-password <email>   crée un compte ou remplace son mot de passe
  dashu backup <fichier>       copie à chaud de la base SQLite
  dashu quota list|show|set|reset  plafonds de stockage des comptes
  dashu tools list             outils réservés et comptes autorisés
  dashu tools grant <email> <outil>    autorise un compte à un outil réservé
  dashu tools revoke <email> <outil>   retire cette autorisation
  dashu healthcheck            vérifie que le serveur répond
  dashu worker                 service de conversion (lit WORK_DIR)
  dashu download [--check]     téléchargeur de liens (lit WORK_DIR)
  dashu version                affiche la version et le commit
  dashu demo-export <dossier> [--base /demo]   démo statique (binaire construit avec --features demo)"
            );
            std::process::exit(2);
        }
    }
}

async fn serve() -> anyhow::Result<()> {
    // Les logs d'abord : la lecture de la configuration peut déjà avertir.
    let env = Env::load();
    init_logging(env.get("LOG_FORMAT").as_deref() == Some("json"));
    let (config, pool) = Config::load(&env).await?;
    let bind_addr = config.bind_addr;
    if let Some(files) = &config.files {
        files::prepare_storage(files);
    }
    let state = AppState::new(config, pool.clone())?;
    if state.config.allowed_emails.is_empty() && setup::installed(&state).await? {
        tracing::warn!("aucune adresse autorisée : personne ne peut se connecter");
    }
    // Sans compte : l'adresse du premier lancement et son code, dans le journal.
    setup::announce(&state).await?;

    // Tâches de fond, toutes les heures : synchro des agendas, purge des
    // sessions expirées et des envois de fichiers abandonnés.
    calendar::sync::spawn_hourly(state.clone());
    // Envois à Google Agenda qui n'ont pas pu partir (Google indisponible).
    calendar::google::spawn_retry(state.clone());
    files::spawn_cleanup(state.clone());
    trash::spawn_purge(state.clone());
    tools::spawn_collector(state.clone());
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
    .with_graceful_shutdown(shutdown_signal(state.restart.clone()))
    .await?;

    pool.close().await;
    if state.restart.asked() {
        // Nouvelle configuration (premier lancement, Réglages › Instance) :
        // dashu se remplace par une nouvelle copie de lui-même.
        return Err(restart::exec());
    }
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

/// Attend Ctrl+C, SIGTERM (`docker stop`) ou une demande de relance, pour
/// finir les requêtes en cours.
async fn shutdown_signal(restart: std::sync::Arc<restart::Restart>) {
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
        () = restart.wait() => {},
    }
}

/// `dashu set-password <email>` : le mot de passe est lu sur l'entrée
/// standard, pour ne jamais apparaître dans l'historique du shell.
async fn set_password(email: &str) -> anyhow::Result<()> {
    let (config, pool) = Config::load(&Env::load()).await?;
    if !config.is_allowed(email) {
        bail!(
            "{email} n'est pas une adresse autorisée (ALLOWED_EMAILS, ou Réglages › Instance) : ce compte ne pourrait pas se connecter"
        );
    }

    eprint!("Mot de passe pour {email} (12 caractères minimum) : ");
    let mut line = String::new();
    // Lecture bloquante : sans importance, cette commande ne fait que ça.
    std::io::stdin().read_line(&mut line)?;
    let password = line.trim_end_matches(['\r', '\n']).to_string();

    auth::set_password(&pool, email, password).await?;
    // Le premier compte de l'instance en devient l'administrateur.
    instance::ensure_admin(&pool).await?;
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

/// `dashu quota …` : usage et plafond de stockage des comptes. Le seul moyen
/// de changer un plafond : aucune page ni route de l'API ne le permet. Lit
/// `DATABASE_URL`, `FILES_DIR` (place du disque) et `FILES_QUOTA` (valeur
/// par défaut), comme le serveur. Sur le serveur, à lancer sous
/// l'utilisateur dashu : `dashu-quota` le fait (deploy/lxc).
async fn quota(args: &[&str]) -> anyhow::Result<()> {
    let env = Env::load();
    let default = dashu::config::default_quota(&env)?;
    let database_url = env
        .get("DATABASE_URL")
        .unwrap_or_else(|| "sqlite://dashu.db".into());
    let files_dir = env
        .get("FILES_DIR")
        .map(|d| std::path::PathBuf::from(d.trim()));
    let pool = db::connect(&database_url)
        .await
        .context("ouverture de la base")?;
    let result = files::quota::command(&pool, default, files_dir.as_deref(), args).await;
    pool.close().await;
    match result {
        Ok(text) => {
            print!("{text}");
            Ok(())
        }
        Err(error) => {
            eprintln!("{}", error.message);
            std::process::exit(error.code);
        }
    }
}

/// `dashu worker` : le service de conversion. Il ne lit que `WORK_DIR` :
/// il n'a ni la configuration de dashu, ni ses secrets, ni la base.
async fn worker() -> anyhow::Result<()> {
    init_logging(std::env::var("LOG_FORMAT").as_deref() == Ok("json"));
    let dir = std::env::var("WORK_DIR")
        .ok()
        .filter(|d| !d.trim().is_empty())
        .context("WORK_DIR manquant : dossier de travail partagé avec dashu")?;
    tokio::task::spawn_blocking(move || tools::worker::run(std::path::Path::new(dir.trim())))
        .await?
}

/// `dashu download` : le téléchargeur de liens, service à part
/// (`dashu-download`). Comme le service de conversion, il ne lit que
/// `WORK_DIR`, plus le cookie de session des plateformes s'il existe
/// (`DOWNLOAD_COOKIES`).
async fn download() -> anyhow::Result<()> {
    init_logging(std::env::var("LOG_FORMAT").as_deref() == Ok("json"));
    let dir = std::env::var("WORK_DIR")
        .ok()
        .filter(|d| !d.trim().is_empty())
        .context("WORK_DIR manquant : dossier de travail partagé avec dashu")?;
    let cookies = std::env::var("DOWNLOAD_COOKIES")
        .ok()
        .filter(|c| !c.trim().is_empty())
        .map(|c| std::path::PathBuf::from(c.trim()));
    tokio::task::spawn_blocking(move || {
        tools::telechargeur::run(std::path::Path::new(dir.trim()), cookies)
    })
    .await?
}

/// Base de dashu, pour les commandes qui n'ont besoin que d'elle :
/// `DATABASE_URL` (variable ou `.env`), migrations appliquées.
async fn database() -> anyhow::Result<sqlx::SqlitePool> {
    let url = Env::load()
        .get("DATABASE_URL")
        .unwrap_or_else(|| "sqlite://dashu.db".into());
    db::connect(&url)
        .await
        .with_context(|| format!("ouverture de la base {url}"))
}

/// `dashu tools list` : les outils réservés et, pour chacun, les comptes
/// autorisés. Seule la ligne de commande, sur le serveur, gère ces accès.
async fn tools_list() -> anyhow::Result<()> {
    let pool = database().await?;
    for (tool, emails) in registry::grants(&pool).await? {
        if emails.is_empty() {
            println!("{} ({}) : aucun compte autorisé", tool.id, tool.name);
        } else {
            println!("{} ({}) : {}", tool.id, tool.name, emails.join(", "));
        }
    }
    pool.close().await;
    Ok(())
}

/// `dashu tools grant|revoke <email> <outil>`.
async fn tools_grant(email: &str, tool: &str, grant: bool) -> anyhow::Result<()> {
    let pool = database().await?;
    let changed = if grant {
        registry::grant(&pool, email, tool).await
    } else {
        registry::revoke(&pool, email, tool).await
    };
    pool.close().await;
    let message = match (grant, changed?) {
        (true, true) => format!("{email} a maintenant accès à {tool}."),
        (true, false) => format!("{email} avait déjà accès à {tool}."),
        (false, true) => format!("{email} n'a plus accès à {tool}."),
        (false, false) => format!("{email} n'avait pas accès à {tool}."),
    };
    println!("{message}");
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
