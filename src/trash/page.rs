//! Page Corbeille, dans Réglages : ce qui a été supprimé, du plus récent au
//! plus ancien, avec qui l'a supprimé, quand et par où. Restaurer, supprimer
//! définitivement (avec confirmation), vider : sur le site seulement.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::Response;
use sqlx::SqlitePool;

use super::{KEEP_DAYS, Kind};
use crate::AppState;
use crate::auth::session::{Ctx, Shell};
use crate::spaces::person_name;
use crate::time;
use crate::web::{self, AppError, AppResult, see_other};

const BACK: &str = "/settings/trash";

/// Une ligne de la page.
pub struct Entry {
    pub kind: Kind,
    pub id: i64,
    pub name: String,
    /// Espace d'origine.
    pub origin: String,
    pub origin_color: String,
    /// « Alice », ou « toi ».
    pub who: String,
    pub via: String,
    pub deleted_at: i64,
    /// Ce qui est parti avec un dossier : « 3 notes, 2 fichiers ».
    pub contents: String,
}

impl Entry {
    pub fn kind_key(&self) -> &'static str {
        self.kind.key()
    }

    pub fn icon(&self) -> &'static str {
        match self.kind {
            Kind::Note => "notes",
            Kind::File => "file",
            Kind::Folder => "folder",
            Kind::Space => "archive",
        }
    }

    pub fn label(&self) -> &'static str {
        match self.kind {
            Kind::Note => "Note",
            Kind::File => "Fichier",
            Kind::Folder => "Dossier",
            Kind::Space => "Espace",
        }
    }
}

/// Tout ce que le compte voit dans la corbeille : les corbeilles de ses
/// espaces (et des espaces partagés qu'il a rejoints), et ses espaces
/// supprimés.
pub async fn entries(db: &SqlitePool, user: i64) -> AppResult<Vec<Entry>> {
    let rows = sqlx::query!(
        r#"SELECT t.kind AS "kind!: String", t.id AS "id!: i64", t.name AS "name!: String",
                  t.origin AS "origin!: String", t.color AS "color!: String",
                  t.deleted_by AS "by: i64", u.email AS "email?: String",
                  u.first_name AS "first_name?: String", u.last_name AS "last_name?: String",
                  t.deleted_via AS "via: String", t.deleted_at AS "deleted_at!: i64",
                  t.op AS "op: i64"
           FROM (
               SELECT 'note' AS kind, n.id, n.title AS name, s.name AS origin, s.color,
                      n.deleted_by, n.deleted_via, n.deleted_at, n.trash_op AS op
                 FROM notes n JOIN trash_access a ON a.trash_id = n.space_id
                 JOIN spaces s ON s.id = a.space_id
                WHERE a.user_id = ?1 AND n.trash_root = 1
               UNION ALL
               SELECT 'fichier', f.id, f.name, s.name, s.color,
                      f.deleted_by, f.deleted_via, f.deleted_at, f.trash_op
                 FROM files f JOIN trash_access a ON a.trash_id = f.space_id
                 JOIN spaces s ON s.id = a.space_id
                WHERE a.user_id = ?1 AND f.trash_root = 1
               UNION ALL
               SELECT 'dossier', d.id, coalesce(d.trash_name, d.name), s.name, s.color,
                      d.deleted_by, d.deleted_via, d.deleted_at, d.trash_op
                 FROM folders d JOIN trash_access a ON a.trash_id = d.space_id
                 JOIN spaces s ON s.id = a.space_id
                WHERE a.user_id = ?1 AND d.trash_root = 1
               UNION ALL
               SELECT 'espace', s.id, s.name, s.name, s.color,
                      s.deleted_by, s.deleted_via, s.deleted_at, NULL
                 FROM spaces s WHERE s.user_id = ?1 AND s.deleted_at IS NOT NULL
           ) t
           LEFT JOIN users u ON u.id = t.deleted_by
           ORDER BY t.deleted_at DESC, t.id DESC"#,
        user
    )
    .fetch_all(db)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(kind) = Kind::from_key(&row.kind) else {
            continue;
        };
        let contents = match (kind, row.op) {
            (Kind::Folder, Some(op)) => folder_contents(db, op).await?,
            _ => String::new(),
        };
        let who = match (row.by, row.email) {
            (Some(by), _) if by == user => "toi".to_string(),
            (_, Some(email)) => person_name(
                row.first_name.as_deref().unwrap_or(""),
                row.last_name.as_deref().unwrap_or(""),
                &email,
            ),
            _ => "un compte supprimé".to_string(),
        };
        out.push(Entry {
            kind,
            id: row.id,
            name: match kind {
                Kind::Note => crate::notes::display_title(&row.name),
                _ => row.name,
            },
            origin: row.origin,
            origin_color: row.color,
            who,
            via: row.via.unwrap_or_else(|| super::VIA_SITE.to_string()),
            deleted_at: row.deleted_at,
            contents,
        });
    }
    Ok(out)
}

/// « 3 notes, 2 fichiers, 1 sous-dossier » : ce qu'un dossier a emporté.
async fn folder_contents(db: &SqlitePool, op: i64) -> AppResult<String> {
    let counts = sqlx::query!(
        r#"SELECT (SELECT count(*) FROM notes WHERE trash_op = ?1) AS "notes!: i64",
                  (SELECT count(*) FROM files WHERE trash_op = ?1) AS "files!: i64",
                  (SELECT count(*) FROM folders WHERE trash_op = ?1 AND trash_root = 0) AS "subs!: i64""#,
        op
    )
    .fetch_one(db)
    .await?;
    let plural = |n: i64, one: &str, many: &str| match n {
        0 => None,
        1 => Some(format!("1 {one}")),
        n => Some(format!("{n} {many}")),
    };
    let parts: Vec<String> = [
        plural(counts.notes, "note", "notes"),
        plural(counts.files, "fichier", "fichiers"),
        plural(counts.subs, "sous-dossier", "sous-dossiers"),
    ]
    .into_iter()
    .flatten()
    .collect();
    Ok(if parts.is_empty() {
        "vide".to_string()
    } else {
        parts.join(", ")
    })
}

pub struct Row {
    pub entry: Entry,
    /// « il y a 5 min »
    pub when: String,
    /// « effacé dans 29 jours »
    pub left: String,
}

#[derive(Template)]
#[template(path = "settings/trash.html")]
struct TrashPage {
    shell: Shell,
    rows: Vec<Row>,
    keep_days: i64,
}

/// `GET /settings/trash`
pub async fn show(ctx: Ctx, State(state): State<AppState>) -> AppResult<Response> {
    let now = time::now();
    let rows = entries(&state.db, ctx.user.id)
        .await?
        .into_iter()
        .map(|entry| {
            let end = entry.deleted_at + KEEP_DAYS * 24 * 3600;
            let days = ((end - now) / (24 * 3600)).max(0);
            Row {
                when: time::ago(entry.deleted_at, now),
                left: match days {
                    0 => "effacé dans moins d'un jour".to_string(),
                    1 => "effacé dans 1 jour".to_string(),
                    n => format!("effacé dans {n} jours"),
                },
                entry,
            }
        })
        .collect();
    let page = TrashPage {
        rows,
        keep_days: KEEP_DAYS,
        shell: ctx.shell("settings").await?,
    };
    Ok(web::html(page))
}

fn kind_of(key: &str) -> AppResult<Kind> {
    Kind::from_key(key).ok_or(AppError::NotFound)
}

/// `POST /settings/trash/{kind}/{id}/restore`
pub async fn restore(
    ctx: Ctx,
    State(state): State<AppState>,
    Path((kind, id)): Path<(String, i64)>,
) -> AppResult<Response> {
    let kind = kind_of(&kind)?;
    match super::restore(&state.db, ctx.user.id, kind, id).await {
        Ok(name) => {
            ctx.flash_ok(format!("« {name} » est revenu à sa place"))
                .await?
        }
        Err(AppError::BadRequest(message)) => ctx.flash_error(message).await?,
        Err(error) => return Err(error),
    }
    Ok(see_other(BACK))
}

/// `POST /settings/trash/{kind}/{id}/erase` : supprime définitivement.
pub async fn erase(
    ctx: Ctx,
    State(state): State<AppState>,
    Path((kind, id)): Path<(String, i64)>,
) -> AppResult<Response> {
    let kind = kind_of(&kind)?;
    super::erase(&state, ctx.user.id, kind, id).await?;
    ctx.flash_ok("Supprimé définitivement").await?;
    Ok(see_other(BACK))
}

/// `POST /settings/trash/empty` : vide la corbeille.
pub async fn empty(ctx: Ctx, State(state): State<AppState>) -> AppResult<Response> {
    let count = super::empty(&state, ctx.user.id).await?;
    ctx.flash_ok(match count {
        0 => "La corbeille était déjà vide".to_string(),
        1 => "Corbeille vidée : 1 élément supprimé définitivement".to_string(),
        n => format!("Corbeille vidée : {n} éléments supprimés définitivement"),
    })
    .await?;
    Ok(see_other(BACK))
}

/// Nombre d'éléments dans la corbeille, pour la carte de Réglages.
pub async fn count(db: &SqlitePool, user: i64) -> AppResult<usize> {
    Ok(entries(db, user).await?.len())
}
