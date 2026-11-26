//! Corbeille (SPEC, « API complète et corbeille »).
//!
//! Supprimer une note, un fichier, un dossier ou un espace, depuis le site
//! comme depuis l'API, le met de côté 30 jours au lieu de l'effacer : une
//! erreur, la sienne ou celle d'un assistant, se rattrape.
//!
//! Comment ce qui est dans la corbeille disparaît de partout : une note, un
//! fichier ou un dossier supprimé passe dans l'espace « corbeille » de son
//! espace, un espace caché (`spaces.trash_of`) que la vue `space_access` ne
//! donne à personne. Toutes les requêtes de contenu passent par cette vue
//! (listes, recherche, barre de commandes, Perso, widgets, API, liens de
//! partage). Un espace supprimé reste en place, marqué `deleted_at` : la vue
//! le cache avec tout son contenu (migration 0020).
//!
//! * Un dossier part avec tout ce qu'il contient, sous un même numéro
//!   d'opération (`trash_op`) : le restaurer ramène l'ensemble, sans ramener
//!   ce qui avait été supprimé à part avant.
//! * Dans un espace partagé, la corbeille de l'espace est commune : tout
//!   membre voit ce qui y a été supprimé et peut le restaurer. Un espace
//!   supprimé ne se voit et ne se restaure que par son propriétaire.
//! * Supprimer définitivement et vider la corbeille : sur le site seulement.
//! * Au bout de 30 jours, dashu efface tout seul, fichiers du disque compris.
//! * Les tâches, les événements et les fichiers de Temporaire (onglet
//!   Outils) ne passent pas par la corbeille.

pub mod page;

use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::AppState;
use crate::folders::{self, Folder};
use crate::time;
use crate::web::{AppError, AppResult};

/// Jours avant l'effacement définitif.
pub const KEEP_DAYS: i64 = 30;
/// Ce qui passe par le site (par opposition à un jeton ou une application).
pub const VIA_SITE: &str = "le site";

/// Qui supprime, et par où : la trace montrée par la page Corbeille.
#[derive(Debug, Clone, Copy)]
pub struct Who<'a> {
    pub user: i64,
    /// « le site », ou le nom du jeton ou de l'application.
    pub via: &'a str,
}

impl<'a> Who<'a> {
    pub fn site(user: i64) -> Self {
        Self {
            user,
            via: VIA_SITE,
        }
    }
}

/// L'espace corbeille d'un espace, créé à sa première suppression. Caché :
/// slug hors de portée des noms d'espace, jamais dans `space_access`.
async fn bin_of(tx: &mut Transaction<'_, Sqlite>, space_id: i64) -> sqlx::Result<i64> {
    if let Some(id) = sqlx::query_scalar!(
        r#"SELECT id AS "id!" FROM spaces WHERE trash_of = ?"#,
        space_id
    )
    .fetch_optional(&mut **tx)
    .await?
    {
        return Ok(id);
    }
    Ok(sqlx::query!(
        "INSERT INTO spaces (user_id, slug, name, color, icon, position, trash_of)
         SELECT user_id, '~corbeille-' || id, 'Corbeille', color, icon, 1000000 + id, id
         FROM spaces WHERE id = ?",
        space_id
    )
    .execute(&mut **tx)
    .await?
    .last_insert_rowid())
}

/// Nouveau numéro d'opération.
async fn next_op(tx: &mut Transaction<'_, Sqlite>) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"SELECT 1 + max(coalesce((SELECT max(trash_op) FROM notes), 0),
                          coalesce((SELECT max(trash_op) FROM files), 0),
                          coalesce((SELECT max(trash_op) FROM folders), 0)) AS "n!: i64""#
    )
    .fetch_one(&mut **tx)
    .await
}

// ---------------------------------------------------------------------
// Mettre dans la corbeille
// ---------------------------------------------------------------------

/// Met une note dans la corbeille. 404 si elle n'est pas accessible.
pub async fn note(db: &SqlitePool, who: Who<'_>, id: i64) -> AppResult<()> {
    let space = sqlx::query_scalar!(
        "SELECT space_id FROM notes WHERE id = ? AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?)",
        id,
        who.user
    )
    .fetch_optional(db)
    .await?
    .ok_or(AppError::NotFound)?;
    let now = time::now();
    let mut tx = db.begin().await?;
    let bin = bin_of(&mut tx, space).await?;
    let op = next_op(&mut tx).await?;
    sqlx::query!(
        "UPDATE notes SET space_id = ?, trash_folder_id = folder_id, folder_id = NULL,
           deleted_at = ?, deleted_by = ?, deleted_via = ?, trash_op = ?, trash_root = 1
         WHERE id = ?",
        bin,
        now,
        who.user,
        who.via,
        op,
        id
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    tracing::info!(
        user = who.user,
        via = who.via,
        note = id,
        "note mise dans la corbeille"
    );
    Ok(())
}

/// Met un fichier rangé dans la corbeille ; ses liens de partage sont
/// révoqués (ils ne reviennent pas avec lui). 404 s'il n'est pas accessible
/// ou s'il est encore dans Temporaire.
pub async fn file(db: &SqlitePool, who: Who<'_>, id: i64) -> AppResult<()> {
    let space = sqlx::query_scalar!(
        "SELECT space_id FROM files WHERE id = ? AND expires_at IS NULL
           AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?)",
        id,
        who.user
    )
    .fetch_optional(db)
    .await?
    .ok_or(AppError::NotFound)?;
    let now = time::now();
    let mut tx = db.begin().await?;
    let bin = bin_of(&mut tx, space).await?;
    let op = next_op(&mut tx).await?;
    sqlx::query!(
        "UPDATE files SET space_id = ?, trash_folder_id = folder_id, folder_id = NULL,
           deleted_at = ?, deleted_by = ?, deleted_via = ?, trash_op = ?, trash_root = 1
         WHERE id = ?",
        bin,
        now,
        who.user,
        who.via,
        op,
        id
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE share_links SET revoked_at = ? WHERE file_id = ? AND revoked_at IS NULL",
        now,
        id
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    tracing::info!(
        user = who.user,
        via = who.via,
        file = id,
        "fichier mis dans la corbeille"
    );
    Ok(())
}

/// Met un dossier dans la corbeille avec tout ce qu'il contient :
/// sous-dossiers, notes, fichiers. Ses liens de partage sont révoqués.
pub async fn folder(db: &SqlitePool, who: Who<'_>, folder: &Folder) -> AppResult<()> {
    let now = time::now();
    let mut tx = db.begin().await?;
    let bin = bin_of(&mut tx, folder.space_id).await?;
    let op = next_op(&mut tx).await?;
    // Le dossier lui-même : à la racine de la corbeille, son parent noté, son
    // nom mis de côté (un autre « Math » peut déjà y être).
    sqlx::query!(
        "UPDATE folders SET space_id = ?, trash_folder_id = parent_id, parent_id = NULL,
           trash_name = ?, name = char(1) || id,
           deleted_at = ?, deleted_by = ?, deleted_via = ?, trash_op = ?, trash_root = 1
         WHERE id = ?",
        bin,
        folder.name,
        now,
        who.user,
        who.via,
        op,
        folder.id
    )
    .execute(&mut *tx)
    .await?;
    // Ses sous-dossiers, à tous les niveaux (cinq niveaux de dossiers au plus),
    // le suivent. `UNION` plutôt que `UNION ALL` : une boucle dans la base,
    // qui ne devrait pas exister, ne ferait pas tourner la requête sans fin.
    sqlx::query!(
        "UPDATE folders SET space_id = ?1, deleted_at = ?2, deleted_by = ?3, deleted_via = ?4,
           trash_op = ?5, trash_root = 0
         WHERE id IN (
           WITH RECURSIVE below(id) AS (
             SELECT id FROM folders WHERE parent_id = ?6
             UNION SELECT f.id FROM folders f JOIN below b ON f.parent_id = b.id
           )
           SELECT id FROM below
         )",
        bin,
        now,
        who.user,
        who.via,
        op,
        folder.id
    )
    .execute(&mut *tx)
    .await?;
    // Ce qu'ils contiennent : rangé dans le même dossier, à l'abri.
    sqlx::query!(
        "UPDATE notes SET space_id = ?1, deleted_at = ?2, deleted_by = ?3, deleted_via = ?4,
           trash_op = ?5, trash_root = 0
         WHERE folder_id IN (SELECT id FROM folders WHERE trash_op = ?5)",
        bin,
        now,
        who.user,
        who.via,
        op
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE files SET space_id = ?1, deleted_at = ?2, deleted_by = ?3, deleted_via = ?4,
           trash_op = ?5, trash_root = 0
         WHERE folder_id IN (SELECT id FROM folders WHERE trash_op = ?5) AND expires_at IS NULL",
        bin,
        now,
        who.user,
        who.via,
        op
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE share_links SET revoked_at = ?1
         WHERE revoked_at IS NULL AND file_id IN (SELECT id FROM files WHERE trash_op = ?2)",
        now,
        op
    )
    .execute(&mut *tx)
    .await?;
    // Un envoi en cours vers ce dossier arrivera à la racine de l'espace.
    sqlx::query!(
        "UPDATE uploads SET folder_id = NULL WHERE folder_id IN (SELECT id FROM folders WHERE trash_op = ?)",
        op
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    tracing::info!(
        user = who.user,
        via = who.via,
        folder = folder.id,
        "dossier mis dans la corbeille"
    );
    Ok(())
}

/// Met un espace dans la corbeille, avec tout son contenu. Les membres s'en
/// vont d'abord (comme s'ils étaient retirés : leurs agendas rentrent chez
/// eux) ; ils ne reviennent pas si l'espace est restauré. Le slug est libéré
/// pour un nouvel espace, et repris à la restauration s'il est encore libre.
pub async fn space(db: &SqlitePool, who: Who<'_>, space_id: i64) -> AppResult<()> {
    let members = sqlx::query_scalar!(
        r#"SELECT user_id AS "user_id!: i64" FROM space_members WHERE space_id = ?"#,
        space_id
    )
    .fetch_all(db)
    .await?;
    for member in members {
        crate::spaces::remove_member(db, space_id, member).await?;
    }
    let now = time::now();
    let mut tx = db.begin().await?;
    sqlx::query!("DELETE FROM space_invitations WHERE space_id = ?", space_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query!(
        "UPDATE share_links SET revoked_at = ?
         WHERE revoked_at IS NULL AND file_id IN (SELECT id FROM files WHERE space_id = ?)",
        now,
        space_id
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE users SET moment_space = NULL WHERE moment_space = ?",
        space_id
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE spaces SET deleted_at = ?, deleted_by = ?, deleted_via = ?, slug = slug || '~' || id
         WHERE id = ? AND deleted_at IS NULL",
        now,
        who.user,
        who.via,
        space_id
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    tracing::info!(
        user = who.user,
        via = who.via,
        space = space_id,
        "espace mis dans la corbeille"
    );
    Ok(())
}

// ---------------------------------------------------------------------
// Restaurer
// ---------------------------------------------------------------------

/// Sorte d'élément de la corbeille.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Note,
    File,
    Folder,
    Space,
}

impl Kind {
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "note" => Some(Self::Note),
            "fichier" => Some(Self::File),
            "dossier" => Some(Self::Folder),
            "espace" => Some(Self::Space),
            _ => None,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::File => "fichier",
            Self::Folder => "dossier",
            Self::Space => "espace",
        }
    }
}

/// Où revient un élément restauré : son espace d'origine, et son dossier
/// s'il existe encore (sinon la racine).
struct Origin {
    space_id: i64,
    folder_id: Option<i64>,
}

/// L'espace d'origine d'un élément de la corbeille accessible au compte, et
/// son dossier s'il est encore là, dans cet espace, hors de la corbeille.
async fn origin(db: &SqlitePool, user: i64, bin: i64, folder: Option<i64>) -> AppResult<Origin> {
    let space_id = sqlx::query_scalar!(
        r#"SELECT space_id AS "space_id!: i64" FROM trash_access WHERE user_id = ? AND trash_id = ?"#,
        user,
        bin
    )
    .fetch_optional(db)
    .await?
    .ok_or(AppError::NotFound)?;
    let folder_id = match folder {
        Some(folder) => sqlx::query_scalar!(
            r#"SELECT id AS "id!" FROM folders WHERE id = ? AND space_id = ? AND deleted_at IS NULL"#,
            folder,
            space_id
        )
        .fetch_optional(db)
        .await?,
        None => None,
    };
    Ok(Origin {
        space_id,
        folder_id,
    })
}

/// Restaure un élément de la corbeille à sa place, ou à la racine de son
/// espace si son dossier n'existe plus. Renvoie son nom, pour le message.
pub async fn restore(db: &SqlitePool, user: i64, kind: Kind, id: i64) -> AppResult<String> {
    match kind {
        Kind::Note => {
            let row = sqlx::query!(
                r#"SELECT space_id, trash_folder_id, title FROM notes WHERE id = ? AND trash_root = 1"#,
                id
            )
            .fetch_optional(db)
            .await?
            .ok_or(AppError::NotFound)?;
            let to = origin(db, user, row.space_id, row.trash_folder_id).await?;
            sqlx::query!(
                "UPDATE notes SET space_id = ?, folder_id = ?, deleted_at = NULL, deleted_by = NULL,
                   deleted_via = NULL, trash_op = NULL, trash_root = 0, trash_folder_id = NULL
                 WHERE id = ?",
                to.space_id,
                to.folder_id,
                id
            )
            .execute(db)
            .await?;
            Ok(crate::notes::display_title(&row.title))
        }
        Kind::File => {
            let row = sqlx::query!(
                r#"SELECT space_id, trash_folder_id, name FROM files WHERE id = ? AND trash_root = 1"#,
                id
            )
            .fetch_optional(db)
            .await?
            .ok_or(AppError::NotFound)?;
            let to = origin(db, user, row.space_id, row.trash_folder_id).await?;
            sqlx::query!(
                "UPDATE files SET space_id = ?, folder_id = ?, deleted_at = NULL, deleted_by = NULL,
                   deleted_via = NULL, trash_op = NULL, trash_root = 0, trash_folder_id = NULL
                 WHERE id = ?",
                to.space_id,
                to.folder_id,
                id
            )
            .execute(db)
            .await?;
            Ok(row.name)
        }
        Kind::Folder => restore_folder(db, user, id).await,
        Kind::Space => restore_space(db, user, id).await,
    }
}

async fn restore_folder(db: &SqlitePool, user: i64, id: i64) -> AppResult<String> {
    let row = sqlx::query!(
        r#"SELECT space_id, trash_folder_id, coalesce(trash_name, name) AS "name!: String",
                  trash_op AS "op!: i64"
           FROM folders WHERE id = ? AND trash_root = 1 AND trash_op IS NOT NULL"#,
        id
    )
    .fetch_optional(db)
    .await?
    .ok_or(AppError::NotFound)?;
    let mut to = origin(db, user, row.space_id, row.trash_folder_id).await?;
    // Cinq niveaux de dossiers au plus : s'il ne tient plus dans son ancien dossier avec
    // ses sous-dossiers (ce dossier a pu descendre entre-temps), il revient à
    // la racine de l'espace. Sa hauteur se lit dans la corbeille, où ses
    // sous-dossiers ont gardé leur parent.
    // Garde-fou contre une boucle dans la base : on ne descend pas plus bas
    // que ce que la règle permet.
    let limit = folders::MAX_DEPTH as i64;
    let height = sqlx::query_scalar!(
        r#"WITH RECURSIVE below(id, level) AS (
             SELECT ?1, 1
             UNION SELECT f.id, b.level + 1 FROM folders f JOIN below b ON f.parent_id = b.id
               WHERE b.level <= ?2
           )
           SELECT max(level) AS "height!: i64" FROM below"#,
        id,
        limit
    )
    .fetch_one(db)
    .await?;
    let all = folders::all(db, user).await?;
    let height = usize::try_from(height).unwrap_or(usize::MAX);
    if to.folder_id.is_some()
        && folders::depth(&all, to.folder_id).saturating_add(height) > folders::MAX_DEPTH
    {
        to.folder_id = None;
    }
    // Un voisin a pris son nom entre-temps : « Math (2) ».
    let taken = folders::neighbours(&all, to.space_id, to.folder_id, None);
    let name = folders::free_name(&taken, &row.name);

    let mut tx = db.begin().await?;
    sqlx::query!(
        "UPDATE folders SET space_id = ?, parent_id = ?, name = ?, trash_name = NULL WHERE id = ?",
        to.space_id,
        to.folder_id,
        name,
        id
    )
    .execute(&mut *tx)
    .await?;
    for statement in [
        "UPDATE folders SET space_id = ?1, deleted_at = NULL, deleted_by = NULL, deleted_via = NULL,
           trash_op = NULL, trash_root = 0, trash_folder_id = NULL WHERE trash_op = ?2",
        "UPDATE notes SET space_id = ?1, deleted_at = NULL, deleted_by = NULL, deleted_via = NULL,
           trash_op = NULL, trash_root = 0, trash_folder_id = NULL WHERE trash_op = ?2",
        "UPDATE files SET space_id = ?1, deleted_at = NULL, deleted_by = NULL, deleted_via = NULL,
           trash_op = NULL, trash_root = 0, trash_folder_id = NULL WHERE trash_op = ?2",
    ] {
        sqlx::query(statement)
            .bind(to.space_id)
            .bind(row.op)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(name)
}

async fn restore_space(db: &SqlitePool, user: i64, id: i64) -> AppResult<String> {
    let row = sqlx::query!(
        r#"SELECT name, slug FROM spaces WHERE id = ? AND user_id = ? AND deleted_at IS NOT NULL"#,
        id,
        user
    )
    .fetch_optional(db)
    .await?
    .ok_or(AppError::NotFound)?;
    let spaces = crate::spaces::list(db, user).await?;
    if spaces.len() >= crate::spaces::MAX_SPACES {
        return Err(AppError::BadRequest(format!(
            "{} espaces au plus : supprimes-en un avant de restaurer « {} ».",
            crate::spaces::MAX_SPACES,
            row.name
        )));
    }
    // Son ancien slug s'il est libre, sinon un nouveau tiré de son nom.
    let wanted = row
        .slug
        .rsplit_once('~')
        .map(|(slug, _)| slug.to_string())
        .unwrap_or(row.slug.clone());
    let taken: Vec<String> = sqlx::query_scalar!(
        r#"SELECT slug AS "slug!" FROM spaces WHERE user_id = ?1
           UNION SELECT slug FROM space_members WHERE user_id = ?1"#,
        user
    )
    .fetch_all(db)
    .await?;
    let slug = if taken.contains(&wanted) {
        let taken: Vec<&str> = taken.iter().map(String::as_str).collect();
        crate::spaces::new_slug(&row.name, &taken)
    } else {
        wanted
    };
    // Un nom repris entre-temps : « École (2) ».
    let name = if spaces
        .iter()
        .any(|s| s.name.to_lowercase() == row.name.to_lowercase())
    {
        format!("{} (2)", row.name)
    } else {
        row.name.clone()
    };
    sqlx::query!(
        "UPDATE spaces SET deleted_at = NULL, deleted_by = NULL, deleted_via = NULL, slug = ?, name = ?
         WHERE id = ?",
        slug,
        name,
        id
    )
    .execute(db)
    .await?;
    Ok(name)
}

// ---------------------------------------------------------------------
// Effacer pour de bon
// ---------------------------------------------------------------------

/// Efface un élément de la corbeille, fichiers du disque compris. Le compte
/// doit y avoir accès (sa corbeille, celle d'un espace partagé, ou un espace
/// à lui).
pub async fn erase(state: &AppState, user: i64, kind: Kind, id: i64) -> AppResult<()> {
    let db = &state.db;
    let keys = match kind {
        Kind::Note => {
            let deleted = sqlx::query!(
                "DELETE FROM notes WHERE id = ? AND trash_root = 1
                   AND space_id IN (SELECT trash_id FROM trash_access WHERE user_id = ?)",
                id,
                user
            )
            .execute(db)
            .await?
            .rows_affected();
            if deleted == 0 {
                return Err(AppError::NotFound);
            }
            crate::notes::prune_orphan_tags(db, user).await?;
            Vec::new()
        }
        Kind::File => sqlx::query_scalar!(
            r#"DELETE FROM files WHERE id = ? AND trash_root = 1
                 AND space_id IN (SELECT trash_id FROM trash_access WHERE user_id = ?)
               RETURNING storage_key AS "storage_key!""#,
            id,
            user
        )
        .fetch_optional(db)
        .await?
        .map(|key| vec![key])
        .ok_or(AppError::NotFound)?,
        Kind::Folder => {
            let op = sqlx::query_scalar!(
                r#"SELECT trash_op AS "op!: i64" FROM folders WHERE id = ? AND trash_root = 1
                     AND trash_op IS NOT NULL
                     AND space_id IN (SELECT trash_id FROM trash_access WHERE user_id = ?)"#,
                id,
                user
            )
            .fetch_optional(db)
            .await?
            .ok_or(AppError::NotFound)?;
            erase_op(db, op).await?
        }
        Kind::Space => {
            sqlx::query_scalar!(
                r#"SELECT id AS "id!" FROM spaces WHERE id = ? AND user_id = ? AND deleted_at IS NOT NULL"#,
                id,
                user
            )
            .fetch_optional(db)
            .await?
            .ok_or(AppError::NotFound)?;
            erase_space(db, id).await?
        }
    };
    remove_from_disk(state, keys).await;
    tracing::info!(user, kind = kind.key(), id, "effacé de la corbeille");
    Ok(())
}

/// Efface tout ce qu'une opération a mis dans la corbeille (un dossier et
/// son contenu). Renvoie les clés des fichiers à effacer du disque.
async fn erase_op(db: &SqlitePool, op: i64) -> AppResult<Vec<String>> {
    let mut tx = db.begin().await?;
    let keys = sqlx::query_scalar!(
        r#"DELETE FROM files WHERE trash_op = ? RETURNING storage_key AS "storage_key!""#,
        op
    )
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query!("DELETE FROM notes WHERE trash_op = ?", op)
        .execute(&mut *tx)
        .await?;
    // Les sous-dossiers d'abord, puis le dossier mis dans la corbeille.
    sqlx::query!(
        "DELETE FROM folders WHERE trash_op = ? AND parent_id IS NOT NULL",
        op
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!("DELETE FROM folders WHERE trash_op = ?", op)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    crate::notes::prune_orphan_tags(db, 0).await?;
    Ok(keys)
}

/// Efface un espace et tout son contenu (sa corbeille comprise). Les notes,
/// tâches, agendas et fichiers sont supprimés un par un (les déclencheurs de
/// la recherche plein texte suivent), puis l'espace : la cascade emporte le
/// reste (étiquettes des notes, événements, envois en cours, conversions).
pub(crate) async fn erase_space(db: &SqlitePool, id: i64) -> AppResult<Vec<String>> {
    let mut tx = db.begin().await?;
    let mut keys = Vec::new();
    // L'espace et sa corbeille.
    let ids = sqlx::query_scalar!(
        r#"SELECT id AS "id!" FROM spaces WHERE id = ?1 OR trash_of = ?1"#,
        id
    )
    .fetch_all(&mut *tx)
    .await?;
    for space in ids {
        keys.extend(
            sqlx::query_scalar!("SELECT storage_key FROM files WHERE space_id = ?", space)
                .fetch_all(&mut *tx)
                .await?,
        );
        sqlx::query!("DELETE FROM notes WHERE space_id = ?", space)
            .execute(&mut *tx)
            .await?;
        sqlx::query!("DELETE FROM tasks WHERE space_id = ?", space)
            .execute(&mut *tx)
            .await?;
        sqlx::query!("DELETE FROM calendar_sources WHERE space_id = ?", space)
            .execute(&mut *tx)
            .await?;
        // Les copies dans Google Agenda partent avec eux (au prochain envoi
        // à Google, voir src/calendar/google.rs).
        sqlx::query!(
            "INSERT OR IGNORE INTO google_deletions (user_id, google_event_id)
             SELECT user_id, google_event_id FROM events
             WHERE space_id = ? AND google_event_id IS NOT NULL",
            space
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!("DELETE FROM events WHERE space_id = ?", space)
            .execute(&mut *tx)
            .await?;
        sqlx::query!("DELETE FROM files WHERE space_id = ?", space)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query!("DELETE FROM spaces WHERE id = ?", id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    crate::notes::prune_orphan_tags(db, 0).await?;
    Ok(keys)
}

/// Le disque après la base : un fichier dont la ligne a disparu n'est plus
/// servi, même si son effacement échoue.
async fn remove_from_disk(state: &AppState, keys: Vec<String>) {
    if let Some(config) = &state.config.files {
        for key in keys {
            crate::files::remove_stored(config, &key).await;
        }
    }
}

/// « Vider la corbeille » : efface tout ce que le compte y voit.
pub async fn empty(state: &AppState, user: i64) -> AppResult<usize> {
    let entries = page::entries(&state.db, user).await?;
    let count = entries.len();
    for entry in entries {
        erase(state, user, entry.kind, entry.id).await?;
    }
    Ok(count)
}

/// Efface ce qui est dans la corbeille depuis plus de 30 jours.
pub async fn purge(state: &AppState) -> AppResult<()> {
    let limit = time::now() - KEEP_DAYS * 24 * 3600;
    let db = &state.db;
    let mut keys = sqlx::query_scalar!(
        r#"DELETE FROM files WHERE deleted_at < ? AND trash_root = 1
           RETURNING storage_key AS "storage_key!""#,
        limit
    )
    .fetch_all(db)
    .await?;
    sqlx::query!(
        "DELETE FROM notes WHERE deleted_at < ? AND trash_root = 1",
        limit
    )
    .execute(db)
    .await?;
    let ops = sqlx::query_scalar!(
        r#"SELECT trash_op AS "op!: i64" FROM folders
           WHERE deleted_at < ? AND trash_root = 1 AND trash_op IS NOT NULL"#,
        limit
    )
    .fetch_all(db)
    .await?;
    for op in ops {
        keys.extend(erase_op(db, op).await?);
    }
    let spaces = sqlx::query_scalar!(
        r#"SELECT id AS "id!" FROM spaces WHERE deleted_at < ?"#,
        limit
    )
    .fetch_all(db)
    .await?;
    for space in spaces {
        keys.extend(erase_space(db, space).await?);
    }
    crate::notes::prune_orphan_tags(db, 0).await?;
    let count = keys.len();
    remove_from_disk(state, keys).await;
    if count > 0 {
        tracing::info!(files = count, "corbeille : effacement après 30 jours");
    }
    Ok(())
}

/// L'effacement des plus de 30 jours : au démarrage, puis toutes les heures.
pub fn spawn_purge(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
        loop {
            tick.tick().await;
            if let Err(error) = purge(&state).await {
                tracing::warn!(error = ?error, "effacement de la corbeille");
            }
        }
    });
}
