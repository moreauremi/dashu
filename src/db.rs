//! Connexion SQLite : mode WAL, clés étrangères, migrations au démarrage.

use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{ConnectOptions, Connection, SqlitePool};
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
        // L'image Docker n'a pas de /tmp : les tris volumineux restent en mémoire.
        .pragma("temp_store", "memory")
        .busy_timeout(Duration::from_secs(5));

    migrate(&options).await?;

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await?;
    session_store(&pool).migrate().await?;

    Ok(pool)
}

/// Applique les migrations sur une connexion à part, clés étrangères
/// désactivées : SQLite l'exige pour reconstruire une table référencée par
/// d'autres (sinon `DROP TABLE` efface en cascade tout ce qui la référence,
/// voir 0007_espaces_libres.sql). Le réglage doit être pris à l'ouverture de
/// la connexion : `PRAGMA foreign_keys` est sans effet dans une transaction,
/// et sqlx en ouvre une par migration.
///
/// Conséquence pour qui écrit une migration : elle ne profite pas des
/// cascades, et doit supprimer elle-même les lignes qui dépendent de celles
/// qu'elle efface. `foreign_key_check` vérifie ensuite qu'aucune référence
/// n'est cassée.
async fn migrate(options: &SqliteConnectOptions) -> anyhow::Result<()> {
    let mut conn = options.clone().foreign_keys(false).connect().await?;
    sqlx::migrate!().run(&mut conn).await?;
    // rowid est NULL pour une table WITHOUT ROWID (note_tags).
    let broken: Vec<(String, Option<i64>)> =
        sqlx::query_as(r#"SELECT "table", rowid FROM pragma_foreign_key_check"#)
            .fetch_all(&mut conn)
            .await?;
    conn.close().await?;
    if let Some((table, _)) = broken.first() {
        anyhow::bail!(
            "références cassées après les migrations : {} ligne(s), dont une dans {table}",
            broken.len()
        );
    }
    Ok(())
}

pub fn session_store(pool: &SqlitePool) -> SqliteStore {
    SqliteStore::new(pool.clone())
        .with_table_name(SESSIONS_TABLE)
        .expect("nom de table de sessions valide")
}
