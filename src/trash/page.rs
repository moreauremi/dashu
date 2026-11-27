//! La corbeille, dossier « Corbeille » de l'onglet Fichiers (SPEC, « Parcours
//! et espaces ») : ce qui a été supprimé, du plus récent au plus ancien, avec
//! d'où il vient, qui l'a supprimé, quand et par où. Dans un espace, ce qui
//! vient de cet espace ; depuis Perso, tout. Restaurer, supprimer
//! définitivement (avec confirmation), vider : sur le site seulement.
//! L'ancienne adresse, `/settings/trash`, y mène.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::Deserialize;
use sqlx::SqlitePool;

use super::{KEEP_DAYS, Kind};
use crate::AppState;
use crate::auth::session::{Ctx, Shell};
use crate::spaces::person_name;
use crate::time;
use crate::web::{self, AppError, AppResult, local_path, see_other};

/// Le dossier Corbeille, dans l'espace de l'onglet.
pub const PLACE: &str = "/files?dossier=corbeille";

/// Une ligne de la page.
pub struct Entry {
    pub kind: Kind,
    pub id: i64,
    pub name: String,
    /// Espace d'origine (l'espace lui-même pour un espace supprimé).
    pub space_id: i64,
    pub origin: String,
    pub origin_color: String,
    /// Dossier d'origine, s'il en avait un.
    pub folder_id: Option<i64>,
    /// Compte qui l'a supprimé.
    pub by: Option<i64>,
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
                  t.space_id AS "space_id!: i64", t.folder_id AS "folder_id: i64",
                  t.origin AS "origin!: String", t.color AS "color!: String",
                  t.deleted_by AS "by: i64", u.email AS "email?: String",
                  u.first_name AS "first_name?: String", u.last_name AS "last_name?: String",
                  t.deleted_via AS "via: String", t.deleted_at AS "deleted_at!: i64",
                  t.op AS "op: i64"
           FROM (
               SELECT 'note' AS kind, n.id, n.title AS name, s.id AS space_id,
                      n.trash_folder_id AS folder_id, s.name AS origin, s.color,
                      n.deleted_by, n.deleted_via, n.deleted_at, n.trash_op AS op
                 FROM notes n JOIN trash_access a ON a.trash_id = n.space_id
                 JOIN spaces s ON s.id = a.space_id
                WHERE a.user_id = ?1 AND n.trash_root = 1
               UNION ALL
               SELECT 'fichier', f.id, f.name, s.id, f.trash_folder_id, s.name, s.color,
                      f.deleted_by, f.deleted_via, f.deleted_at, f.trash_op
                 FROM files f JOIN trash_access a ON a.trash_id = f.space_id
                 JOIN spaces s ON s.id = a.space_id
                WHERE a.user_id = ?1 AND f.trash_root = 1
               UNION ALL
               SELECT 'dossier', d.id, coalesce(d.trash_name, d.name), s.id, d.trash_folder_id,
                      s.name, s.color, d.deleted_by, d.deleted_via, d.deleted_at, d.trash_op
                 FROM folders d JOIN trash_access a ON a.trash_id = d.space_id
                 JOIN spaces s ON s.id = a.space_id
                WHERE a.user_id = ?1 AND d.trash_root = 1
               UNION ALL
               SELECT 'espace', s.id, s.name, s.id, NULL, s.name, s.color,
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
            // Texte brut : le gabarit échappe lui-même.
            name: match kind {
                Kind::Note => crate::notes::plain_title(&row.name),
                _ => row.name,
            },
            space_id: row.space_id,
            origin: row.origin,
            origin_color: row.color,
            folder_id: row.folder_id,
            by: row.by,
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

/// Ce que la corbeille montre dans l'espace ouvert : tout depuis Perso (la
/// vue d'ensemble), sinon ce qui vient de cet espace.
pub async fn visible(ctx: &Ctx, db: &SqlitePool) -> AppResult<Vec<Entry>> {
    let all = entries(db, ctx.user.id).await?;
    Ok(if ctx.active.is_overview() {
        all
    } else {
        all.into_iter()
            .filter(|e| e.space_id == ctx.active.id)
            .collect()
    })
}

pub struct Row {
    pub entry: Entry,
    /// « École / Math » : d'où vient l'élément.
    pub origin: String,
    /// « il y a 5 min »
    pub when: String,
    /// « effacé dans 29 jours »
    pub left: String,
}

/// Un filtre par sorte d'élément : (clé, libellé, adresse, choisi).
pub struct Filter {
    pub label: &'static str,
    pub href: String,
    pub active: bool,
}

#[derive(Template)]
#[template(path = "files/trash.html")]
struct TrashPage {
    shell: Shell,
    rows: Vec<Row>,
    filters: Vec<Filter>,
    /// Cette page, filtre compris : on y revient après une action.
    here: String,
    /// Éléments que « Vider » effacerait (ceux qu'on a supprimés soi-même),
    /// et ceux que d'autres membres ont supprimés, qui restent.
    mine: usize,
    others: usize,
    keep_days: i64,
}

/// `GET /files?dossier=corbeille[&type=…]` (appelée par `files::list`).
pub async fn show(ctx: Ctx, state: &AppState, kind: Option<&str>) -> AppResult<Response> {
    let now = time::now();
    let kind = kind.and_then(Kind::from_key);
    let all_folders = crate::folders::all(&state.db, ctx.user.id).await?;
    let visible = visible(&ctx, &state.db).await?;
    let mine = visible.iter().filter(|e| e.by == Some(ctx.user.id)).count();
    let others = visible.len() - mine;
    let rows = visible
        .into_iter()
        .filter(|e| kind.is_none_or(|k| e.kind == k))
        .map(|entry| {
            let end = entry.deleted_at + KEEP_DAYS * 24 * 3600;
            let days = ((end - now) / (24 * 3600)).max(0);
            // Le dossier d'origine, s'il existe encore (sinon l'espace).
            let origin = match entry.kind {
                Kind::Space => "Espace".to_string(),
                _ => crate::folders::label(
                    &entry.origin,
                    &all_folders,
                    entry
                        .folder_id
                        .filter(|id| all_folders.iter().any(|f| f.id == *id)),
                ),
            };
            Row {
                origin,
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
    let filter = |key: Option<Kind>, label| Filter {
        label,
        href: match key {
            Some(k) => format!("{PLACE}&type={}", k.key()),
            None => PLACE.to_string(),
        },
        active: key == kind,
    };
    let page = TrashPage {
        rows,
        filters: vec![
            filter(None, "Tout"),
            filter(Some(Kind::Note), "Notes"),
            filter(Some(Kind::File), "Fichiers"),
            filter(Some(Kind::Folder), "Dossiers"),
            filter(Some(Kind::Space), "Espaces"),
        ],
        here: match kind {
            Some(k) => format!("{PLACE}&type={}", k.key()),
            None => PLACE.to_string(),
        },
        mine,
        others,
        keep_days: KEEP_DAYS,
        shell: ctx.shell("files").await?,
    };
    Ok(web::html(page))
}

/// `GET /settings/trash` : l'ancienne adresse, vers le dossier Corbeille de
/// Perso (tout ce qui a été supprimé).
pub async fn old_address(ctx: Ctx) -> AppResult<Response> {
    let slug = ctx
        .spaces
        .iter()
        .find(|s| s.is_overview())
        .unwrap_or(&ctx.active)
        .slug
        .clone();
    Ok(see_other(&crate::spaces::url::prefixed(&slug, PLACE)))
}

fn kind_of(key: &str) -> AppResult<Kind> {
    Kind::from_key(key).ok_or(AppError::NotFound)
}

#[derive(Deserialize, Default)]
pub struct BackForm {
    /// Page où revenir (la corbeille, filtre compris).
    next: Option<String>,
}

fn back(form: &BackForm) -> &str {
    match form.next.as_deref() {
        Some(next) => local_path(Some(next)),
        None => PLACE,
    }
}

/// `POST /settings/trash/{kind}/{id}/restore` : remet l'élément à sa place,
/// et dit où : son dossier, la racine de son espace si le dossier n'existe
/// plus, Perso si son espace n'est plus accessible.
pub async fn restore(
    ctx: Ctx,
    State(state): State<AppState>,
    Path((kind, id)): Path<(String, i64)>,
    axum::Form(form): axum::Form<BackForm>,
) -> AppResult<Response> {
    let kind = kind_of(&kind)?;
    match super::restore(&state.db, ctx.user.id, kind, id).await {
        Ok(done) => {
            let all = crate::folders::all(&state.db, ctx.user.id).await?;
            // L'espace vient peut-être d'être restauré : on relit la liste.
            let spaces = crate::spaces::list(&state.db, ctx.user.id).await?;
            let space = spaces.iter().find(|s| s.id == done.space_id);
            let space_name = space.map_or_else(String::new, |s| s.name.clone());
            let slug = space.map_or_else(|| ctx.active.slug.clone(), |s| s.slug.clone());
            let place = crate::folders::label(&space_name, &all, done.folder_id);
            let mut text = match kind {
                Kind::Space => format!("« {} » est de retour", done.name),
                _ if done.at_root => format!(
                    "« {} » est revenu à la racine de {space_name} : son dossier n'existe plus",
                    done.name
                ),
                _ => format!("« {} » est revenu dans {place}", done.name),
            };
            if space.is_some_and(|s| s.archived) {
                text.push_str(" (espace archivé)");
            }
            match done.revoked_links {
                0 => {}
                1 => text.push_str(". Son lien de partage avait été révoqué"),
                _ => text.push_str(". Ses liens de partage avaient été révoqués"),
            }
            let open = match kind {
                Kind::Note => format!("/notes/{id}"),
                Kind::File => format!("/files/{id}"),
                Kind::Folder => crate::folders::view_url(&slug, Some(id)),
                Kind::Space => crate::spaces::url::prefixed(&slug, "/"),
            };
            ctx.flash_link("ok", &text, &open, "Ouvrir").await?;
        }
        Err(AppError::BadRequest(message)) => ctx.flash_error(message).await?,
        Err(error) => return Err(error),
    }
    Ok(see_other(back(&form)))
}

/// `POST /settings/trash/{kind}/{id}/erase` : supprime définitivement.
pub async fn erase(
    ctx: Ctx,
    State(state): State<AppState>,
    Path((kind, id)): Path<(String, i64)>,
    axum::Form(form): axum::Form<BackForm>,
) -> AppResult<Response> {
    let kind = kind_of(&kind)?;
    super::erase(&state, ctx.user.id, kind, id).await?;
    ctx.flash_ok("Supprimé définitivement").await?;
    Ok(see_other(back(&form)))
}

/// `POST /settings/trash/empty` : vide la corbeille de ce qu'on y a mis
/// soi-même.
pub async fn empty(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<BackForm>,
) -> AppResult<Response> {
    // Ce que montre la corbeille ouverte (l'espace de l'onglet, ou tout
    // depuis Perso), et seulement ce qu'on a supprimé soi-même.
    let mut count = 0;
    for entry in visible(&ctx, &state.db).await? {
        if entry.by == Some(ctx.user.id) {
            super::erase(&state, ctx.user.id, entry.kind, entry.id).await?;
            count += 1;
        }
    }
    ctx.flash_ok(match count {
        0 => "Rien à vider : la corbeille ne contient rien que tu aies supprimé".to_string(),
        1 => "Corbeille vidée : 1 élément supprimé définitivement".to_string(),
        n => format!("Corbeille vidée : {n} éléments supprimés définitivement"),
    })
    .await?;
    Ok(see_other(back(&form)))
}

/// Nombre d'éléments dans la corbeille, pour la carte de Réglages.
pub async fn count(db: &SqlitePool, user: i64) -> AppResult<usize> {
    Ok(entries(db, user).await?.len())
}
