//! Dossiers (SPEC, « Dossiers ») : ils rangent les fichiers et les notes.
//!
//! * Cinq niveaux de dossiers au plus sous l'espace : Espace / D1 / D2 / D3 /
//!   D4 / D5 (SPEC, « Dossiers : cinq niveaux »). Le dossier
//!   d'un espace n'est pas une ligne de `folders` : c'est l'espace lui-même,
//!   créé, renommé et supprimé avec lui. La limite est vérifiée par le
//!   serveur ([`MAX_DEPTH`]) à chaque création, déplacement et restauration.
//! * Les dossiers n'existent que dans la base : sur le disque, un fichier
//!   garde son nom aléatoire, quel que soit son dossier (voir `files`).
//! * Un élément et son dossier sont toujours dans le même espace : toute
//!   écriture d'un dossier passe par [`target`], qui le vérifie.
//! * L'accès se décide par l'espace (vue `space_access`) : les membres d'un
//!   espace partagé voient et modifient les mêmes dossiers.

use std::collections::HashMap;

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::AppState;
use crate::auth::session::Ctx;
use crate::spaces::Space;
use crate::time;
use crate::web::{self, AppError, AppResult, see_other};

/// Longueur maximale d'un nom de dossier.
pub const NAME_MAX_CHARS: usize = 60;

/// Niveaux de dossiers sous l'espace : cinq (Espace / D1 / D2 / D3 / D4 /
/// D5). La seule valeur de la limite : tout le reste la lit ici.
pub const MAX_DEPTH: usize = 5;

/// Refus d'un dossier créé un niveau trop bas, le même partout (site, API,
/// connecteur, Hermès).
pub fn too_deep(place: &str) -> String {
    format!("Cinq niveaux de dossiers au plus : pas de dossier dans « {place} ».")
}

#[derive(Debug, Clone)]
pub struct Folder {
    pub id: i64,
    pub space_id: i64,
    /// Vide pour un dossier du premier niveau (directement dans l'espace).
    pub parent_id: Option<i64>,
    pub name: String,
}

impl Folder {
    /// Vrai pour un dossier du premier niveau (directement dans l'espace).
    pub fn is_top(&self) -> bool {
        self.parent_id.is_none()
    }
}

/// Niveau d'un dossier sous son espace : 1 pour un dossier du premier niveau,
/// [`MAX_DEPTH`] au plus ; 0 pour la racine de l'espace (`None`).
pub fn depth(folders: &[Folder], id: Option<i64>) -> usize {
    chain(folders, id).len()
}

/// Sous-dossiers d'un dossier à tous les niveaux, du plus proche au plus
/// profond, sans le dossier lui-même.
pub fn descendants(folders: &[Folder], id: i64) -> Vec<i64> {
    let mut out: Vec<i64> = Vec::new();
    let mut level = vec![id];
    // Un niveau à la fois. La limite n'est qu'un garde-fou contre une boucle
    // dans la base, qui ne devrait pas exister.
    for _ in 0..=MAX_DEPTH {
        level = folders
            .iter()
            .filter(|f| {
                f.parent_id.is_some_and(|p| level.contains(&p))
                    && f.id != id
                    && !out.contains(&f.id)
            })
            .map(|f| f.id)
            .collect();
        if level.is_empty() {
            break;
        }
        out.extend(&level);
    }
    out
}

/// Hauteur d'un dossier : 1 sans sous-dossier, 2 si ses sous-dossiers n'en
/// ont pas, et ainsi de suite. Rangé dans un dossier de niveau `n`, son
/// sous-dossier le plus profond arrive au niveau `n + hauteur`.
pub fn height(folders: &[Folder], id: i64) -> usize {
    let mut height = 1;
    let mut level = vec![id];
    for _ in 0..=MAX_DEPTH {
        level = folders
            .iter()
            .filter(|f| f.parent_id.is_some_and(|p| level.contains(&p)) && f.id != id)
            .map(|f| f.id)
            .collect();
        if level.is_empty() {
            break;
        }
        height += 1;
    }
    height
}

/// Tous les dossiers des espaces d'un compte, triés par nom. Ils sont peu nombreux : on
/// les lit en une fois plutôt que dossier par dossier.
pub async fn all(db: &SqlitePool, user: i64) -> AppResult<Vec<Folder>> {
    let mut folders: Vec<Folder> = sqlx::query!(
        r#"SELECT id AS "id!", space_id, parent_id, name FROM folders
           WHERE space_id IN (SELECT space_id FROM space_access WHERE user_id = ?)"#,
        user
    )
    .fetch_all(db)
    .await?
    .into_iter()
    .map(|row| Folder {
        id: row.id,
        space_id: row.space_id,
        parent_id: row.parent_id,
        name: row.name,
    })
    .collect();
    folders.sort_by_cached_key(|f| (f.name.to_lowercase(), f.id));
    Ok(folders)
}

/// Un dossier d'un espace du compte, ou rien (y compris s'il est dans un espace
/// où le compte n'est pas).
pub async fn find(db: &SqlitePool, user: i64, id: i64) -> AppResult<Option<Folder>> {
    Ok(sqlx::query!(
        "SELECT space_id, parent_id, name FROM folders WHERE id = ?2
           AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?1)",
        user,
        id
    )
    .fetch_optional(db)
    .await?
    .map(|row| Folder {
        id,
        space_id: row.space_id,
        parent_id: row.parent_id,
        name: row.name,
    }))
}

/// Dossier de destination d'un élément rangé dans l'espace `space` : `None`
/// pour la racine de l'espace. Un dossier d'un espace inaccessible, ou d'un autre
/// espace que celui demandé, répond 404.
pub async fn target(
    db: &SqlitePool,
    user: i64,
    space: i64,
    folder: Option<i64>,
) -> AppResult<Option<i64>> {
    let Some(id) = folder else {
        return Ok(None);
    };
    match find(db, user, id).await? {
        Some(found) if found.space_id == space => Ok(Some(found.id)),
        _ => Err(AppError::NotFound),
    }
}

/// Champ de formulaire `folder_id` : vide (ou absent) pour la racine.
pub fn parse_id(raw: Option<&str>) -> AppResult<Option<i64>> {
    match raw.map(str::trim) {
        None | Some("") => Ok(None),
        Some(text) => text
            .parse()
            .map(Some)
            .map_err(|_| AppError::BadRequest("Dossier inconnu.".into())),
    }
}

/// Chemin d'un dossier, du premier niveau jusqu'à lui.
pub fn chain(folders: &[Folder], id: Option<i64>) -> Vec<&Folder> {
    let mut out = Vec::new();
    let mut next = id;
    // Cinq niveaux de dossiers au plus : la boucle s'arrête d'elle-même, la
    // limite n'est
    // qu'un garde-fou.
    while let Some(id) = next
        && out.len() < MAX_DEPTH
    {
        let Some(folder) = folders.iter().find(|f| f.id == id) else {
            break;
        };
        out.push(folder);
        next = folder.parent_id;
    }
    out.reverse();
    out
}

/// « École / Math / Exercices » : l'espace, puis le chemin du dossier.
pub fn label(space: &str, folders: &[Folder], id: Option<i64>) -> String {
    let mut parts = vec![space];
    parts.extend(chain(folders, id).into_iter().map(|f| f.name.as_str()));
    parts.join(" / ")
}

/// Adresse, dans l'onglet Fichiers, d'un dossier de l'espace `slug` (sa
/// racine sans dossier). Elle nomme l'espace : un dossier s'ouvre toujours
/// dans le sien.
pub fn view_url(slug: &str, folder: Option<i64>) -> String {
    let dossier = folder.map_or_else(|| "espace".to_string(), |id| id.to_string());
    crate::spaces::url::prefixed(slug, &format!("/files?dossier={dossier}"))
}

/// Nettoie un nom saisi : sans caractère de contrôle ni espaces en trop,
/// 60 caractères au plus.
pub fn clean_name(raw: &str) -> String {
    let words: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    words
        .chars()
        .take(NAME_MAX_CHARS)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// Deux noms de dossiers identiques, majuscules (accentuées comprises) mises
/// à part.
fn same_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// `wanted`, ou « wanted (2) », « wanted (3) »… s'il est pris parmi `taken`.
pub fn free_name(taken: &[String], wanted: &str) -> String {
    let free = |name: &str| !taken.iter().any(|t| same_name(t, name));
    if free(wanted) {
        return wanted.to_string();
    }
    (2..)
        .map(|n| {
            let suffix = format!(" ({n})");
            let room = NAME_MAX_CHARS.saturating_sub(suffix.chars().count());
            let base: String = wanted.chars().take(room).collect();
            format!("{}{suffix}", base.trim_end())
        })
        .find(|candidate| free(candidate))
        .unwrap_or_default()
}

/// Noms des voisins d'un dossier : les dossiers de même parent dans l'espace,
/// sans `except` (le dossier qu'on renomme).
pub(crate) fn neighbours(
    folders: &[Folder],
    space: i64,
    parent: Option<i64>,
    except: Option<i64>,
) -> Vec<String> {
    folders
        .iter()
        .filter(|f| f.space_id == space && f.parent_id == parent && Some(f.id) != except)
        .map(|f| f.name.clone())
        .collect()
}

// ---------------------------------------------------------------------
// Contenu d'un dossier
// ---------------------------------------------------------------------

/// Ce que contient un dossier, sous-dossiers compris.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Contents {
    pub folders: i64,
    pub notes: i64,
    pub files: i64,
}

impl Contents {
    pub fn is_empty(&self) -> bool {
        self.folders == 0 && self.notes == 0 && self.files == 0
    }

    /// « 2 dossiers, 3 notes et 1 fichier », ou « Vide ».
    pub fn describe(&self) -> String {
        let plural = |n: i64, one: &str, many: &str| {
            (n > 0).then(|| format!("{n} {}", if n > 1 { many } else { one }))
        };
        let parts: Vec<String> = [
            plural(self.folders, "dossier", "dossiers"),
            plural(self.notes, "note", "notes"),
            plural(self.files, "fichier", "fichiers"),
        ]
        .into_iter()
        .flatten()
        .collect();
        match parts.as_slice() {
            [] => "Vide".into(),
            [one] => one.clone(),
            [rest @ .., last] => format!("{} et {last}", rest.join(", ")),
        }
    }
}

/// Ce que contient chaque dossier d'un compte : ses notes, ses fichiers
/// rangés (pas ceux de l'onglet Outils) et ses sous-dossiers, avec leur
/// contenu.
pub async fn contents(
    db: &SqlitePool,
    user: i64,
    folders: &[Folder],
) -> AppResult<HashMap<i64, Contents>> {
    let mut map: HashMap<i64, Contents> = HashMap::new();
    for row in sqlx::query!(
        r#"SELECT folder_id AS "folder_id!", count(*) AS "n!: i64"
           FROM notes
           WHERE space_id IN (SELECT space_id FROM space_access WHERE user_id = ?) AND folder_id IS NOT NULL GROUP BY folder_id"#,
        user
    )
    .fetch_all(db)
    .await?
    {
        map.entry(row.folder_id).or_default().notes += row.n;
    }
    for row in sqlx::query!(
        r#"SELECT folder_id AS "folder_id!", count(*) AS "n!: i64"
           FROM files
           WHERE space_id IN (SELECT space_id FROM space_access WHERE user_id = ?) AND folder_id IS NOT NULL AND expires_at IS NULL GROUP BY folder_id"#,
        user
    )
    .fetch_all(db)
    .await?
    {
        map.entry(row.folder_id).or_default().files += row.n;
    }
    // Un sous-dossier compte dans chacun des dossiers au-dessus de lui, avec
    // ce qu'il contient directement : chaque dossier compte ainsi tout ce qui
    // est en dessous, à tous les niveaux.
    let direct = map.clone();
    for folder in folders {
        let inside = direct.get(&folder.id).copied().unwrap_or_default();
        for above in chain(folders, folder.parent_id) {
            let entry = map.entry(above.id).or_default();
            entry.folders += 1;
            entry.notes += inside.notes;
            entry.files += inside.files;
        }
    }
    Ok(map)
}

// ---------------------------------------------------------------------
// Créer, renommer, supprimer
// ---------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateForm {
    name: String,
    /// Dossier où créer celui-ci ; vide : directement dans l'espace.
    parent_id: Option<String>,
    /// Espace où le créer, sans dossier parent ; l'espace actif par défaut.
    space_id: Option<String>,
}

/// `POST /folders` : crée un dossier dans un espace, ou un sous-dossier dans
/// un dossier (cinq niveaux de dossiers au plus), puis revient là où il a
/// été créé.
pub async fn create(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<CreateForm>,
) -> AppResult<Response> {
    let user = ctx.user.id;
    let parent = match parse_id(form.parent_id.as_deref())? {
        Some(id) => Some(find(&state.db, user, id).await?.ok_or(AppError::NotFound)?),
        None => None,
    };
    // Cinq niveaux de dossiers au plus : refusé par le serveur, pas seulement
    // caché.
    if let Some(parent) = &parent
        && depth(&all(&state.db, user).await?, Some(parent.id)) >= MAX_DEPTH
    {
        return Err(AppError::BadRequest(too_deep(&parent.name)));
    }
    let space = match &parent {
        Some(parent) => ctx.space_by_id(parent.space_id),
        None => match parse_id(form.space_id.as_deref())? {
            Some(id) => ctx.space_by_id(id),
            None => Some(&ctx.active),
        },
    }
    .ok_or(AppError::NotFound)?;
    let parent_id = parent.as_ref().map(|p| p.id);
    let back = view_url(&space.slug, parent_id);
    match create_in(&state.db, user, space.id, parent_id, &form.name).await? {
        Ok((_, name)) => ctx.flash_ok(format!("Dossier « {name} » créé")).await?,
        Err(refusal) => ctx.flash_error(refusal).await?,
    }
    Ok(see_other(&back))
}

/// Crée un dossier dans un espace (déjà vérifié), ou dans un dossier de cet
/// espace. Rend son identifiant et son nom, ou le refus à montrer (nom vide
/// ou déjà pris, niveau de trop). Aussi pour l'API.
pub(crate) async fn create_in(
    db: &SqlitePool,
    user: i64,
    space: i64,
    parent: Option<i64>,
    raw: &str,
) -> AppResult<Result<(i64, String), String>> {
    let name = clean_name(raw);
    if name.is_empty() {
        return Ok(Err("Donne un nom au dossier.".into()));
    }
    let folders = all(db, user).await?;
    if let Some(parent) = parent {
        match folders
            .iter()
            .find(|f| f.id == parent && f.space_id == space)
        {
            None => return Err(AppError::NotFound),
            Some(p) if depth(&folders, Some(p.id)) >= MAX_DEPTH => {
                return Ok(Err(too_deep(&p.name)));
            }
            Some(_) => {}
        }
    }
    if neighbours(&folders, space, parent, None)
        .iter()
        .any(|n| same_name(n, &name))
    {
        return Ok(Err(format!("Un dossier « {name} » existe déjà ici.")));
    }
    let now = time::now();
    let id = sqlx::query!(
        "INSERT INTO folders (user_id, space_id, parent_id, name, created_at) VALUES (?, ?, ?, ?, ?)",
        user,
        space,
        parent,
        name,
        now
    )
    .execute(db)
    .await?
    .last_insert_rowid();
    Ok(Ok((id, name)))
}

#[derive(Deserialize)]
pub struct RenameForm {
    name: String,
}

/// `POST /folders/{id}/rename`
pub async fn rename(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<RenameForm>,
) -> AppResult<Response> {
    let user = ctx.user.id;
    let folder = find(&state.db, user, id).await?.ok_or(AppError::NotFound)?;
    let space = ctx.space_by_id(folder.space_id).ok_or(AppError::NotFound)?;
    let back = view_url(&space.slug, folder.parent_id);
    match rename_to(&state.db, user, &folder, &form.name).await? {
        Ok(name) => {
            ctx.flash_ok(format!("Dossier renommé en « {name} »"))
                .await?
        }
        Err(refusal) => ctx.flash_error(refusal).await?,
    }
    Ok(see_other(&back))
}

/// Renomme un dossier accessible. Rend le nouveau nom, ou le refus à
/// montrer (nom vide ou déjà pris par un voisin). Aussi pour l'API.
pub(crate) async fn rename_to(
    db: &SqlitePool,
    user: i64,
    folder: &Folder,
    raw: &str,
) -> AppResult<Result<String, String>> {
    let name = clean_name(raw);
    if name.is_empty() {
        return Ok(Err("Donne un nom au dossier.".into()));
    }
    let folders = all(db, user).await?;
    if neighbours(&folders, folder.space_id, folder.parent_id, Some(folder.id))
        .iter()
        .any(|n| same_name(n, &name))
    {
        return Ok(Err(format!("Un dossier « {name} » existe déjà ici.")));
    }
    sqlx::query!(
        "UPDATE folders SET name = ?2 WHERE id = ?3
           AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?1)",
        user,
        name,
        folder.id
    )
    .execute(db)
    .await?;
    Ok(Ok(name))
}

#[derive(Deserialize)]
pub struct DeleteForm {
    /// Sort du contenu : `remonter` (dans le dossier parent) ou `supprimer`.
    /// Obligatoire si le dossier n'est pas vide.
    contenu: Option<String>,
}

/// `POST /folders/{id}/delete` : met un dossier dans la corbeille. S'il
/// n'est pas vide, son contenu monte d'abord dans le dossier parent (ses
/// sous-dossiers montent d'un niveau), ou part dans la corbeille avec lui.
pub async fn delete(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<DeleteForm>,
) -> AppResult<Response> {
    let user = ctx.user.id;
    let folder = find(&state.db, user, id).await?.ok_or(AppError::NotFound)?;
    let space = ctx.space_by_id(folder.space_id).ok_or(AppError::NotFound)?;
    let back = view_url(&space.slug, folder.parent_id);
    let folders = all(&state.db, user).await?;
    let inside = contents(&state.db, user, &folders)
        .await?
        .get(&id)
        .copied()
        .unwrap_or_default();
    let erase = match form.contenu.as_deref() {
        Some("supprimer") => true,
        Some("remonter") => false,
        _ if inside.is_empty() => false,
        _ => {
            return Err(AppError::BadRequest(
                "Ce dossier n'est pas vide : choisis de transférer son contenu ou de tout supprimer."
                    .into(),
            ));
        }
    };
    let subs: Vec<&Folder> = folders.iter().filter(|f| f.parent_id == Some(id)).collect();

    let mut tx = state.db.begin().await?;
    if !erase {
        // Le contenu va dans le dossier du dessus (la racine de l'espace pour
        // un dossier du premier niveau), envois en cours compris.
        let parent = folder.parent_id;
        sqlx::query!(
            "UPDATE notes SET folder_id = ? WHERE folder_id = ?",
            parent,
            id
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!(
            "UPDATE files SET folder_id = ? WHERE folder_id = ?",
            parent,
            id
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!(
            "UPDATE uploads SET folder_id = ? WHERE folder_id = ?",
            parent,
            id
        )
        .execute(&mut *tx)
        .await?;
        if !subs.is_empty() {
            // Les sous-dossiers montent d'un niveau, sous un nom libre parmi
            // leurs nouveaux voisins. Le dossier supprimé quitte d'abord son
            // nom : un sous-dossier homonyme peut le reprendre.
            let placeholder = format!("\u{1}{id}");
            sqlx::query!("UPDATE folders SET name = ? WHERE id = ?", placeholder, id)
                .execute(&mut *tx)
                .await?;
            let mut taken = neighbours(&folders, folder.space_id, parent, Some(id));
            for sub in &subs {
                let name = free_name(&taken, &sub.name);
                sqlx::query!(
                    "UPDATE folders SET parent_id = ?, name = ? WHERE id = ?",
                    parent,
                    name,
                    sub.id
                )
                .execute(&mut *tx)
                .await?;
                taken.push(name);
            }
        }
    }
    tx.commit().await?;
    // Le dossier (et, s'il fallait tout supprimer, son contenu) part dans la
    // corbeille : 30 jours pour le restaurer.
    crate::trash::folder(&state.db, crate::trash::Who::site(user), &folder).await?;

    let message = if inside.is_empty() {
        format!("Dossier « {} » mis dans la corbeille", folder.name)
    } else if erase {
        format!(
            "Dossier « {} » mis dans la corbeille, avec son contenu",
            folder.name
        )
    } else {
        format!(
            "Dossier « {} » mis dans la corbeille : son contenu est dans « {} »",
            folder.name,
            label(&space.name, &folders, folder.parent_id)
        )
    };
    ctx.flash_link(
        "ok",
        &message,
        crate::trash::page::PLACE,
        "Ouvrir la corbeille",
    )
    .await?;
    Ok(see_other(&back))
}

#[derive(Deserialize)]
pub struct MoveForm {
    space_id: i64,
    /// Dossier où le mettre ; vide : la racine de l'espace.
    folder_id: Option<String>,
    /// Page où revenir ; le nouvel emplacement sans précision.
    next: Option<String>,
}

/// `POST /folders/{id}/move` : déplace un dossier, avec ses notes, ses
/// fichiers et ses sous-dossiers, à la racine d'un espace ou dans un autre
/// dossier. Cinq niveaux de dossiers au plus : le dossier et ses sous-dossiers doivent
/// tenir sous la destination. Un refus (niveau de trop, homonyme, le dossier
/// dans lui-même ou dans un de ses sous-dossiers) est dit, et rien ne bouge.
pub async fn move_folder(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<MoveForm>,
) -> AppResult<Response> {
    let user = ctx.user.id;
    // L'espace de destination doit être accessible.
    let target = ctx.space_by_id(form.space_id).ok_or(AppError::NotFound)?;
    let parent_id = parse_id(form.folder_id.as_deref())?;
    let back = form
        .next
        .as_deref()
        .map(|n| web::local_path(Some(n)).to_string())
        .unwrap_or_else(|| view_url(&target.slug, parent_id));
    match move_to(&state.db, user, id, target, parent_id).await? {
        Moved::Same => {}
        Moved::Done(message) => ctx.flash_ok(message).await?,
        Moved::Refused(message) => ctx.flash_error(message).await?,
    }
    Ok(see_other(&back))
}

/// Ce qu'a donné le déplacement d'un dossier.
pub(crate) enum Moved {
    /// Il y était déjà : rien n'a bougé.
    Same,
    /// Déplacé ; le message à montrer.
    Done(String),
    /// Refusé (niveau de trop, homonyme, dans lui-même ou dans un de ses
    /// sous-dossiers) : rien n'a bougé.
    Refused(String),
}

/// Déplace un dossier accessible, avec ses notes, ses fichiers et ses
/// sous-dossiers, à la racine d'un espace accessible (`target`) ou dans un
/// dossier de cet espace. 404 si le dossier ou le dossier de destination
/// n'est pas accessible. Aussi pour l'API.
pub(crate) async fn move_to(
    db: &SqlitePool,
    user: i64,
    id: i64,
    target: &Space,
    parent_id: Option<i64>,
) -> AppResult<Moved> {
    let folders = all(db, user).await?;
    let folder = folders
        .iter()
        .find(|f| f.id == id)
        .ok_or(AppError::NotFound)?;
    // Le dossier de destination doit être dans l'espace choisi.
    let parent = match parent_id {
        Some(pid) => Some(
            folders
                .iter()
                .find(|f| f.id == pid && f.space_id == target.id)
                .ok_or(AppError::NotFound)?,
        ),
        None => None,
    };
    // Ses sous-dossiers, à tous les niveaux : ils le suivent.
    let subs = descendants(&folders, id);

    if folder.space_id == target.id && folder.parent_id == parent_id {
        return Ok(Moved::Same);
    }
    let refusal = match parent {
        Some(p) if p.id == id => Some("Un dossier ne va pas dans lui-même.".to_string()),
        Some(p) if subs.contains(&p.id) => {
            Some("Un dossier ne va pas dans un de ses sous-dossiers.".to_string())
        }
        // Le dossier arrive un niveau sous la destination, son sous-dossier
        // le plus profond `hauteur` niveaux sous elle.
        Some(p) if depth(&folders, Some(p.id)) + height(&folders, id) > MAX_DEPTH => Some(format!(
            "Cinq niveaux de dossiers au plus : « {} » et ses sous-dossiers ne tiennent pas dans « {} ».",
            folder.name,
            label(&target.name, &folders, parent_id)
        )),
        _ if neighbours(&folders, target.id, parent_id, Some(id))
            .iter()
            .any(|n| same_name(n, &folder.name)) =>
        {
            Some(format!(
                "Un dossier « {} » existe déjà dans « {} ».",
                folder.name,
                label(&target.name, &folders, parent_id)
            ))
        }
        _ => None,
    };
    if let Some(message) = refusal {
        return Ok(Moved::Refused(message));
    }

    let mut tx = db.begin().await?;
    sqlx::query!(
        "UPDATE folders SET space_id = ?, parent_id = ? WHERE id = ?",
        target.id,
        parent_id,
        id
    )
    .execute(&mut *tx)
    .await?;
    // Un autre espace : tout son contenu le suit, sous-dossiers, envois et
    // conversions en cours compris (une copie convertie arrive ainsi à côté
    // de son original).
    if target.id != folder.space_id {
        for folder_id in std::iter::once(id).chain(subs.iter().copied()) {
            sqlx::query!(
                "UPDATE folders SET space_id = ? WHERE id = ?",
                target.id,
                folder_id
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query!(
                "UPDATE notes SET space_id = ? WHERE folder_id = ?",
                target.id,
                folder_id
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query!(
                "UPDATE jobs SET space_id = ?1
                   WHERE file_id IN (SELECT id FROM files WHERE folder_id = ?2)",
                target.id,
                folder_id
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query!(
                "UPDATE files SET space_id = ? WHERE folder_id = ?",
                target.id,
                folder_id
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query!(
                "UPDATE uploads SET space_id = ? WHERE folder_id = ?",
                target.id,
                folder_id
            )
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    // Les images que ses notes affichent, rangées ailleurs dans l'ancien
    // espace, les suivent (SPEC, « Parcours et espaces »).
    let mut carried = 0;
    if target.id != folder.space_id {
        for folder_id in std::iter::once(id).chain(subs.iter().copied()) {
            for note in sqlx::query_scalar!(
                r#"SELECT id AS "id!" FROM notes WHERE folder_id = ?"#,
                folder_id
            )
            .fetch_all(db)
            .await?
            {
                carried += crate::notes::carry_images(
                    db,
                    user,
                    note,
                    folder.space_id,
                    target.id,
                    Some(folder_id),
                )
                .await?;
            }
        }
    }
    let mut message = format!(
        "Dossier « {} » déplacé dans « {} »",
        folder.name,
        label(&target.name, &folders, parent_id)
    );
    match carried {
        0 => {}
        1 => message.push_str(" · 1 image de ses notes le suit"),
        n => message.push_str(&format!(" · {n} images de ses notes le suivent")),
    }
    Ok(Moved::Done(message))
}

// ---------------------------------------------------------------------
// Fenêtre « Ranger » : tous les espaces et tous les dossiers
// ---------------------------------------------------------------------

/// Une destination de la fenêtre « Ranger ».
pub struct Dest {
    pub space_id: i64,
    pub folder_id: Option<i64>,
    pub name: String,
    /// 0 : un espace ; 1 : un dossier du premier niveau ; jusqu'à
    /// [`MAX_DEPTH`] pour le plus profond.
    pub depth: usize,
    pub color: String,
    /// Là où l'élément est déjà.
    pub current: bool,
    /// Pas une destination possible (un espace, pour le dossier des clients
    /// de 1Life) : montré, mais grisé.
    pub disabled: bool,
    /// Un espace partagé avec d'autres comptes (marqué « partagé »).
    pub shared: bool,
}

/// Ce que la fenêtre « Ranger » propose pour un élément.
pub struct Reach {
    /// L'espace et le dossier où l'élément est déjà (grisés).
    pub here: Option<(i64, Option<i64>)>,
    /// Niveau le plus profond où il peut aller : [`MAX_DEPTH`] pour une note
    /// ou un fichier ; pour un dossier, `MAX_DEPTH` moins sa hauteur (0 : les
    /// espaces seulement). Cinq niveaux de dossiers au plus.
    pub depth: usize,
    /// Le dossier qu'on déplace : ni lui ni ses sous-dossiers ne sont des
    /// destinations.
    pub moving: Option<i64>,
    /// Les espaces eux-mêmes se choisissent (faux pour le dossier des clients
    /// de 1Life : un espace n'est pas un dossier).
    pub spaces: bool,
}

/// Toutes les destinations possibles, dans l'ordre de l'arborescence :
/// chaque espace, puis ses dossiers, chacun suivi de ses sous-dossiers, à
/// tous les niveaux permis.
pub fn destinations(spaces: &[Space], folders: &[Folder], reach: &Reach) -> Vec<Dest> {
    let mut out = Vec::new();
    for space in spaces {
        out.push(Dest {
            space_id: space.id,
            folder_id: None,
            name: space.name.clone(),
            depth: 0,
            color: space.color.clone(),
            current: reach.here == Some((space.id, None)),
            disabled: !reach.spaces,
            shared: false,
        });
        branch(&mut out, space, folders, reach, None, 1);
    }
    out
}

/// Les dossiers de `parent` dans `space`, chacun suivi des siens, jusqu'au
/// niveau `reach.depth`. Le dossier qu'on déplace n'y est pas, ni rien en
/// dessous de lui.
fn branch(
    out: &mut Vec<Dest>,
    space: &Space,
    folders: &[Folder],
    reach: &Reach,
    parent: Option<i64>,
    depth: usize,
) {
    if depth > reach.depth.min(MAX_DEPTH) {
        return;
    }
    for folder in folders
        .iter()
        .filter(|f| f.space_id == space.id && f.parent_id == parent && Some(f.id) != reach.moving)
    {
        out.push(Dest {
            space_id: space.id,
            folder_id: Some(folder.id),
            name: folder.name.clone(),
            depth,
            color: space.color.clone(),
            current: reach.here == Some((space.id, Some(folder.id))),
            disabled: false,
            shared: false,
        });
        branch(out, space, folders, reach, Some(folder.id), depth + 1);
    }
}

#[derive(Template)]
#[template(path = "folders/_destinations.html")]
struct DestinationsFragment {
    title: String,
    /// Adresse des formulaires : déplacer une note, un fichier ou un dossier,
    /// ou garder un fichier de l'onglet Outils.
    action: String,
    /// Page où revenir après le rangement (vide : celle par défaut).
    next: String,
    items: Vec<Dest>,
    /// L'élément est dans un espace partagé : le sortir le retire aux autres
    /// membres. Le nom de l'espace, pour l'avertissement.
    leaving_shared: Option<String>,
}

#[derive(Deserialize)]
pub struct DestinationsQuery {
    note: Option<i64>,
    fichier: Option<i64>,
    dossier: Option<i64>,
    /// `1` : choisir le dossier des clients de 1Life.
    clients: Option<String>,
    /// `1` : choisir le dossier des modèles d'audit de 1Life.
    modeles: Option<String>,
    /// Page où revenir après le rangement.
    next: Option<String>,
}

/// `GET /destinations?note=…`, `?fichier=…` ou `?dossier=…` : le contenu de
/// la fenêtre « Ranger » (htmx), un bouton par espace et par dossier où
/// l'élément peut aller. `?clients=1` : le dossier des clients de 1Life ;
/// `?modeles=1` : son dossier des modèles d'audit (404 pour un compte qui
/// n'y est pas autorisé).
pub async fn picker(
    ctx: Ctx,
    State(state): State<AppState>,
    Query(query): Query<DestinationsQuery>,
) -> AppResult<Response> {
    let user = ctx.user.id;
    let next = query
        .next
        .as_deref()
        .map(|n| web::local_path(Some(n)).to_string())
        .unwrap_or_default();
    let folders = all(&state.db, user).await?;
    if query.clients.as_deref() == Some("1") {
        if ctx.tools.tool(crate::onelife::TOOL).is_none() {
            return Err(AppError::NotFound);
        }
        // Les dossiers des niveaux 1 à 3 : un client y a la place de ses
        // sous-dossiers. Le choix actuel est marqué « Ici ».
        let chosen = crate::onelife::clients::chosen(&state.db, user).await?;
        let here = chosen
            .and_then(|id| folders.iter().find(|f| f.id == id))
            .map(|f| (f.space_id, Some(f.id)));
        return Ok(web::html(DestinationsFragment {
            title: "Dossier des clients".into(),
            action: format!("/e/{}/1life/clients/dossier", ctx.active.slug),
            next: String::new(),
            leaving_shared: None,
            items: destinations(
                &ctx.spaces,
                &folders,
                &Reach {
                    here,
                    depth: crate::onelife::clients::FOLDER_MAX_DEPTH,
                    moving: None,
                    spaces: false,
                },
            ),
        }));
    }
    if query.modeles.as_deref() == Some("1") {
        if ctx.tools.tool(crate::onelife::TOOL).is_none() {
            return Err(AppError::NotFound);
        }
        // Un dossier à tout niveau ; les espaces sont grisés.
        let chosen = crate::onelife::settings::chosen_folder(&state.db, user).await?;
        let here = chosen
            .and_then(|id| folders.iter().find(|f| f.id == id))
            .map(|f| (f.space_id, Some(f.id)));
        return Ok(web::html(DestinationsFragment {
            title: "Dossier des modèles d'audit".into(),
            action: format!("/e/{}/1life/reglages/modeles", ctx.active.slug),
            next: String::new(),
            leaving_shared: None,
            items: destinations(
                &ctx.spaces,
                &folders,
                &Reach {
                    here,
                    depth: MAX_DEPTH,
                    moving: None,
                    spaces: false,
                },
            ),
        }));
    }
    let (title, action, reach, next) = match (query.note, query.fichier, query.dossier) {
        (Some(id), _, _) => {
            let note = sqlx::query!(
                "SELECT space_id, folder_id, title FROM notes WHERE id = ?2
                   AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?1)",
                user,
                id
            )
            .fetch_optional(&state.db)
            .await?
            .ok_or(AppError::NotFound)?;
            let title = if note.title.trim().is_empty() {
                "Ranger la note".to_string()
            } else {
                format!("Ranger « {} »", note.title)
            };
            // Retour à la note par défaut : elle s'ouvre dans son nouvel espace.
            let next = if next.is_empty() {
                format!("/notes/{id}")
            } else {
                next
            };
            (
                title,
                format!("/notes/{id}/move"),
                Reach {
                    here: Some((note.space_id, note.folder_id)),
                    depth: MAX_DEPTH,
                    moving: None,
                    spaces: true,
                },
                next,
            )
        }
        (None, Some(id), _) => {
            let file = sqlx::query!(
                "SELECT space_id, folder_id, name, expires_at FROM files WHERE id = ?2
                   AND ((expires_at IS NULL AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?1))
                   OR (expires_at IS NOT NULL AND user_id = ?1))",
                user,
                id
            )
            .fetch_optional(&state.db)
            .await?
            .ok_or(AppError::NotFound)?;
            // Fichier de l'onglet Outils, pas encore rangé : le garder.
            if file.expires_at.is_some() {
                (
                    format!("Garder « {} » dans", file.name),
                    format!("/files/{id}/keep"),
                    Reach {
                        here: None,
                        depth: MAX_DEPTH,
                        moving: None,
                        spaces: true,
                    },
                    next,
                )
            } else {
                (
                    format!("Ranger « {} »", file.name),
                    format!("/files/{id}/move"),
                    Reach {
                        here: Some((file.space_id, file.folder_id)),
                        depth: MAX_DEPTH,
                        moving: None,
                        spaces: true,
                    },
                    next,
                )
            }
        }
        (None, None, Some(id)) => {
            let folder = folders
                .iter()
                .find(|f| f.id == id)
                .ok_or(AppError::NotFound)?;
            // Cinq niveaux de dossiers au plus : le dossier et ses sous-dossiers doivent
            // tenir sous la destination.
            let reach_depth = MAX_DEPTH.saturating_sub(height(&folders, id));
            (
                format!("Déplacer le dossier « {} »", folder.name),
                format!("/folders/{id}/move"),
                Reach {
                    here: Some((folder.space_id, folder.parent_id)),
                    depth: reach_depth,
                    moving: Some(id),
                    spaces: true,
                },
                next,
            )
        }
        (None, None, None) => return Err(AppError::NotFound),
    };
    // Les espaces partagés (rejoints, ou à soi avec des membres) : marqués,
    // et un avertissement quand l'élément en vient.
    let shared = shared_spaces(&ctx, &state.db).await?;
    let mut items = destinations(&ctx.spaces, &folders, &reach);
    for item in items.iter_mut().filter(|d| d.depth == 0) {
        item.shared = shared.contains(&item.space_id);
    }
    let leaving_shared = reach
        .here
        .map(|(space, _)| space)
        .filter(|space| shared.contains(space))
        .and_then(|space| ctx.space_by_id(space))
        .map(|space| space.name.clone());
    Ok(web::html(DestinationsFragment {
        title,
        action,
        next,
        items,
        leaving_shared,
    }))
}

/// Les espaces du compte partagés avec d'autres : ceux qu'il a rejoints, et
/// les siens qui ont des membres.
async fn shared_spaces(ctx: &Ctx, db: &sqlx::SqlitePool) -> AppResult<Vec<i64>> {
    let with_members: Vec<i64> = sqlx::query_scalar!(
        r#"SELECT DISTINCT space_id AS "space_id!" FROM space_members
           WHERE space_id IN (SELECT space_id FROM space_access WHERE user_id = ?)"#,
        ctx.user.id
    )
    .fetch_all(db)
    .await?;
    Ok(ctx
        .spaces
        .iter()
        .filter(|s| !s.mine || with_members.contains(&s.id))
        .map(|s| s.id)
        .collect())
}

/// Adresse de la fenêtre « Ranger » d'un élément (`note`, `fichier` ou
/// `dossier`), avec la page où revenir.
pub fn picker_url(kind: &str, id: i64, next: Option<&str>) -> String {
    let mut params = form_urlencoded::Serializer::new(String::new());
    params.append_pair(kind, &id.to_string());
    if let Some(next) = next {
        params.append_pair("next", next);
    }
    format!("/destinations?{}", params.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(id: i64, parent: Option<i64>, name: &str) -> Folder {
        Folder {
            id,
            space_id: 1,
            parent_id: parent,
            name: name.into(),
        }
    }

    #[test]
    fn noms_nettoyes() {
        assert_eq!(clean_name("  Math \t  sup\u{7} "), "Math sup");
        assert_eq!(clean_name("\n"), "");
        assert_eq!(clean_name(&"é".repeat(80)).chars().count(), NAME_MAX_CHARS);
    }

    #[test]
    fn noms_libres() {
        let taken = vec!["Math".to_string(), "math (2)".to_string()];
        assert_eq!(free_name(&taken, "Français"), "Français");
        assert_eq!(free_name(&taken, "MATH"), "MATH (3)");
        assert_eq!(free_name(&[String::from("École")], "école"), "école (2)");
        let long = "a".repeat(NAME_MAX_CHARS);
        let renamed = free_name(std::slice::from_ref(&long), &long);
        assert!(renamed.ends_with(" (2)") && renamed.chars().count() == NAME_MAX_CHARS);
    }

    #[test]
    fn chemins_et_libelles() {
        let folders = vec![folder(1, None, "Math"), folder(2, Some(1), "Exercices")];
        assert_eq!(label("École", &folders, None), "École");
        assert_eq!(label("École", &folders, Some(1)), "École / Math");
        assert_eq!(
            label("École", &folders, Some(2)),
            "École / Math / Exercices"
        );
        assert_eq!(chain(&folders, Some(99)).len(), 0);
        assert_eq!(view_url("ecole", Some(2)), "/e/ecole/files?dossier=2");
        assert_eq!(view_url("ecole", None), "/e/ecole/files?dossier=espace");
    }

    #[test]
    fn niveaux_hauteurs_et_descendants() {
        // Math / Analyse / Chapitre / Exercices, et Algèbre à côté d'Analyse.
        let folders = vec![
            folder(1, None, "Math"),
            folder(2, Some(1), "Analyse"),
            folder(3, Some(2), "Chapitre"),
            folder(4, Some(3), "Exercices"),
            folder(5, Some(1), "Algèbre"),
        ];
        assert_eq!(depth(&folders, None), 0);
        assert_eq!(depth(&folders, Some(1)), 1);
        assert_eq!(depth(&folders, Some(4)), 4);
        assert!(depth(&folders, Some(4)) < MAX_DEPTH);
        assert_eq!(height(&folders, 1), 4);
        assert_eq!(height(&folders, 2), 3);
        assert_eq!(height(&folders, 4), 1);
        assert_eq!(descendants(&folders, 1), vec![2, 5, 3, 4]);
        assert_eq!(descendants(&folders, 4), Vec::<i64>::new());
        assert_eq!(
            label("École", &folders, Some(4)),
            "École / Math / Analyse / Chapitre / Exercices"
        );
    }

    #[test]
    fn une_boucle_dans_la_base_ne_bloque_rien() {
        // Ne devrait pas exister ; les parcours s'arrêtent quand même.
        let folders = vec![folder(1, Some(2), "A"), folder(2, Some(1), "B")];
        assert!(depth(&folders, Some(1)) <= MAX_DEPTH);
        assert!(height(&folders, 1) <= MAX_DEPTH + 2);
        assert_eq!(descendants(&folders, 1), vec![2]);
    }

    #[test]
    fn contenu_en_francais() {
        assert_eq!(Contents::default().describe(), "Vide");
        let one = Contents {
            folders: 0,
            notes: 1,
            files: 0,
        };
        assert_eq!(one.describe(), "1 note");
        let all = Contents {
            folders: 2,
            notes: 3,
            files: 1,
        };
        assert_eq!(all.describe(), "2 dossiers, 3 notes et 1 fichier");
    }

    #[test]
    fn adresse_de_la_fenetre() {
        assert_eq!(
            picker_url("fichier", 4, Some("/files?dossier=2")),
            "/destinations?fichier=4&next=%2Ffiles%3Fdossier%3D2"
        );
    }
}
