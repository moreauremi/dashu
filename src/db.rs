//! Connexion SQLite : mode WAL, clés étrangères, migrations au démarrage.

use std::str::FromStr;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use tower_sessions_sqlx_store::SqliteStore;

/// Nom de la table des sessions, gérée par tower-sessions.
const SESSIONS_TABLE: &str = "sessions";

pub async fn connect(database_url: &str) -> anyhow::Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str(database_url)?
        .create_if_missing(true)
        // WAL : les lectures ne bloquent pas l'écriture en cours, et inversement.
        .journal_mode(SqliteJournalMode::Wal)
        // NORMAL est sûr en WAL (pas de corruption) et évite un fsync par transaction.
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await?;

    sqlx::migrate!().run(&pool).await?;
    session_store(&pool).migrate().await?;

    Ok(pool)
}

pub fn session_store(pool: &SqlitePool) -> SqliteStore {
    SqliteStore::new(pool.clone())
        .with_table_name(SESSIONS_TABLE)
        .expect("nom de table de sessions valide")
}
