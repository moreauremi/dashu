//! Dossiers (SPEC, « Dossiers ») : ils rangent les fichiers et les notes.
//!
//! * Trois niveaux au plus : Espace / Dossier / Sous-dossier. Le dossier d'un
//!   espace n'est pas une ligne de `folders` : c'est l'espace lui-même, créé,
//!   renommé et supprimé avec lui.
//! * Les dossiers n'existent que dans la base : sur le disque, un fichier
//!   garde son nom aléatoire, quel que soit son dossier (voir `files`).
//! * Un élément et son dossier sont toujours dans le même espace : toute
//!   écriture d'un dossier passe par [`target`], qui le vérifie.
//! * Toutes les requêtes filtrent sur `user_id`, comme les notes.

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

#[derive(Debug, Clone)]
pub struct Folder {
    pub id: i64,
    pub space_id: i64,
    /// Vide pour un dossier du premier niveau (directement dans l'espace).
    pub parent_id: Option<i64>,
    pub name: String,
}

impl Folder {
    /// Vrai pour un dossier du premier niveau : lui seul peut contenir des
    /// sous-dossiers.
    pub fn is_top(&self) -> bool {
        self.parent_id.is_none()
    }
}

/// Tous les dossiers d'un compte, triés par nom. Ils sont peu nombreux : on
/// les lit en une fois plutôt que dossier par dossier.
pub async fn all(db: &SqlitePool, user: i64) -> AppResult<Vec<Folder>> {
    let mut folders: Vec<Folder> = sqlx::query!(
        r#"SELECT id AS "id!", space_id, parent_id, name FROM folders WHERE user_id = ?"#,
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

/// Un dossier du compte, ou rien (y compris s'il est à un autre compte).
pub async fn find(db: &SqlitePool, user: i64, id: i64) -> AppResult<Option<Folder>> {
    Ok(sqlx::query!(
        "SELECT space_id, parent_id, name FROM folders WHERE id = ? AND user_id = ?",
        id,
        user
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
/// pour la racine de l'espace. Un dossier d'un autre compte, ou d'un autre
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
    // Trois niveaux au plus : la boucle s'arrête d'elle-même, la limite n'est
    // qu'un garde-fou.
    while let Some(id) = next
        && out.len() < 3
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
           FROM notes WHERE user_id = ? AND folder_id IS NOT NULL GROUP BY folder_id"#,
        user
    )
    .fetch_all(db)
    .await?
    {
        map.entry(row.folder_id).or_default().notes += row.n;
    }
    for row in sqlx::query!(
        r#"SELECT folder_id AS "folder_id!", count(*) AS "n!: i64"
           FROM files WHERE user_id = ? AND folder_id IS NOT NULL AND expires_at IS NULL GROUP BY folder_id"#,
        user
    )
    .fetch_all(db)
    .await?
    {
        map.entry(row.folder_id).or_default().files += row.n;
    }
    // Un sous-dossier compte dans son dossier parent, avec son contenu. (Un
    // sous-dossier n'a pas d'enfant : l'ordre de la boucle est sans effet.)
    for folder in folders {
        if let Some(parent) = folder.parent_id {
            let inside = map.get(&folder.id).copied().unwrap_or_default();
            let entry = map.entry(parent).or_default();
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
/// un dossier du premier niveau, puis revient là où il a été créé.
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
    // Trois niveaux au plus : pas de dossier dans un sous-dossier.
    if parent.as_ref().is_some_and(|p| !p.is_top()) {
        return Err(AppError::BadRequest(
            "Pas de dossier dans un sous-dossier : trois niveaux au plus.".into(),
        ));
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

    let name = clean_name(&form.name);
    if name.is_empty() {
        ctx.flash_error("Donne un nom au dossier.").await?;
        return Ok(see_other(&back));
    }
    let folders = all(&state.db, user).await?;
    if neighbours(&folders, space.id, parent_id, None)
        .iter()
        .any(|n| same_name(n, &name))
    {
        ctx.flash_error(format!("Un dossier « {name} » existe déjà ici."))
            .await?;
        return Ok(see_other(&back));
    }
    let now = time::now();
    sqlx::query!(
        "INSERT INTO folders (user_id, space_id, parent_id, name, created_at) VALUES (?, ?, ?, ?, ?)",
        user,
        space.id,
        parent_id,
        name,
        now
    )
    .execute(&state.db)
    .await?;
    ctx.flash_ok(format!("Dossier « {name} » créé")).await?;
    Ok(see_other(&back))
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

    let name = clean_name(&form.name);
    if name.is_empty() {
        ctx.flash_error("Donne un nom au dossier.").await?;
        return Ok(see_other(&back));
    }
    let folders = all(&state.db, user).await?;
    if neighbours(&folders, space.id, folder.parent_id, Some(id))
        .iter()
        .any(|n| same_name(n, &name))
    {
        ctx.flash_error(format!("Un dossier « {name} » existe déjà ici."))
            .await?;
        return Ok(see_other(&back));
    }
    sqlx::query!(
        "UPDATE folders SET name = ? WHERE id = ? AND user_id = ?",
        name,
        id,
        user
    )
    .execute(&state.db)
    .await?;
    ctx.flash_ok(format!("Dossier renommé en « {name} »"))
        .await?;
    Ok(see_other(&back))
}

#[derive(Deserialize)]
pub struct DeleteForm {
    /// Sort du contenu : `remonter` (dans le dossier parent) ou `supprimer`.
    /// Obligatoire si le dossier n'est pas vide.
    contenu: Option<String>,
}

/// `POST /folders/{id}/delete` : supprime un dossier. S'il n'est pas vide,
/// son contenu monte dans le dossier parent (ses sous-dossiers montent d'un
/// niveau), ou il est effacé avec lui, fichiers du disque compris.
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

    let mut keys = Vec::new();
    let mut tx = state.db.begin().await?;
    if erase {
        // Les notes et les fichiers du dossier et de ses sous-dossiers ; les
        // sous-dossiers partent avec lui (ON DELETE CASCADE).
        for folder_id in std::iter::once(id).chain(subs.iter().map(|f| f.id)) {
            keys.extend(
                sqlx::query_scalar!(
                    r#"DELETE FROM files WHERE user_id = ? AND folder_id = ? RETURNING storage_key AS "storage_key!""#,
                    user,
                    folder_id
                )
                .fetch_all(&mut *tx)
                .await?,
            );
            sqlx::query!(
                "DELETE FROM notes WHERE user_id = ? AND folder_id = ?",
                user,
                folder_id
            )
            .execute(&mut *tx)
            .await?;
        }
    } else {
        // Le contenu va dans le dossier du dessus (la racine de l'espace pour
        // un dossier du premier niveau), envois en cours compris.
        let parent = folder.parent_id;
        sqlx::query!(
            "UPDATE notes SET folder_id = ? WHERE user_id = ? AND folder_id = ?",
            parent,
            user,
            id
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!(
            "UPDATE files SET folder_id = ? WHERE user_id = ? AND folder_id = ?",
            parent,
            user,
            id
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!(
            "UPDATE uploads SET folder_id = ? WHERE user_id = ? AND folder_id = ?",
            parent,
            user,
            id
        )
        .execute(&mut *tx)
        .await?;
        if !subs.is_empty() {
            // Les sous-dossiers montent d'un niveau, sous un nom libre parmi
            // leurs nouveaux voisins. Le dossier supprimé quitte d'abord son
            // nom : un sous-dossier homonyme peut le reprendre.
            let placeholder = format!("\u{1}{id}");
            sqlx::query!(
                "UPDATE folders SET name = ? WHERE id = ? AND user_id = ?",
                placeholder,
                id,
                user
            )
            .execute(&mut *tx)
            .await?;
            let mut taken = neighbours(&folders, folder.space_id, None, Some(id));
            for sub in &subs {
                let name = free_name(&taken, &sub.name);
                sqlx::query!(
                    "UPDATE folders SET parent_id = NULL, name = ? WHERE id = ? AND user_id = ?",
                    name,
                    sub.id,
                    user
                )
                .execute(&mut *tx)
                .await?;
                taken.push(name);
            }
        }
    }
    sqlx::query!("DELETE FROM folders WHERE id = ? AND user_id = ?", id, user)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    // Le disque après la base : un fichier dont la ligne a disparu n'est plus
    // servi, même si son effacement échoue.
    if let Some(files) = &state.config.files {
        for key in keys {
            crate::files::remove_stored(files, &key).await;
        }
    }

    let message = if inside.is_empty() {
        format!("Dossier « {} » supprimé", folder.name)
    } else if erase {
        format!("Dossier « {} » supprimé, avec son contenu", folder.name)
    } else {
        format!(
            "Dossier « {} » supprimé : son contenu est dans « {} »",
            folder.name,
            label(&space.name, &folders, folder.parent_id)
        )
    };
    ctx.flash_ok(message).await?;
    Ok(see_other(&back))
}

// ---------------------------------------------------------------------
// Fenêtre « Ranger » : tous les espaces et tous les dossiers
// ---------------------------------------------------------------------

/// Une destination de la fenêtre « Ranger ».
pub struct Dest {
    pub space_id: i64,
    pub folder_id: Option<i64>,
    pub name: String,
    /// 0 : un espace ; 1 : un dossier ; 2 : un sous-dossier.
    pub depth: u8,
    pub color: String,
    /// Là où l'élément est déjà.
    pub current: bool,
}

/// Toutes les destinations possibles, dans l'ordre de l'arborescence :
/// chaque espace, puis ses dossiers, chacun suivi de ses sous-dossiers.
/// `here` : l'espace et le dossier actuels de l'élément.
pub fn destinations(
    spaces: &[Space],
    folders: &[Folder],
    here: Option<(i64, Option<i64>)>,
) -> Vec<Dest> {
    let mut out = Vec::new();
    let mut push = |space: &Space, folder: Option<&Folder>, depth: u8| {
        let folder_id = folder.map(|f| f.id);
        out.push(Dest {
            space_id: space.id,
            folder_id,
            name: folder.map_or_else(|| space.name.clone(), |f| f.name.clone()),
            depth,
            color: space.color.clone(),
            current: here == Some((space.id, folder_id)),
        });
    };
    for space in spaces {
        push(space, None, 0);
        for top in folders
            .iter()
            .filter(|f| f.space_id == space.id && f.is_top())
        {
            push(space, Some(top), 1);
            for sub in folders.iter().filter(|f| f.parent_id == Some(top.id)) {
                push(space, Some(sub), 2);
            }
        }
    }
    out
}

#[derive(Template)]
#[template(path = "folders/_destinations.html")]
struct DestinationsFragment {
    title: String,
    /// Adresse des formulaires : déplacer une note, un fichier, ou garder un
    /// fichier de l'onglet Outils.
    action: String,
    /// Page où revenir après le rangement (vide : celle par défaut).
    next: String,
    items: Vec<Dest>,
}

#[derive(Deserialize)]
pub struct DestinationsQuery {
    note: Option<i64>,
    fichier: Option<i64>,
    /// Page où revenir après le rangement.
    next: Option<String>,
}

/// `GET /destinations?note=…` ou `?fichier=…` : le contenu de la fenêtre
/// « Ranger » (htmx), un bouton par espace et par dossier.
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
    let (title, action, here, next) = match (query.note, query.fichier) {
        (Some(id), _) => {
            let note = sqlx::query!(
                "SELECT space_id, folder_id, title FROM notes WHERE id = ? AND user_id = ?",
                id,
                user
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
                Some((note.space_id, note.folder_id)),
                next,
            )
        }
        (None, Some(id)) => {
            let file = sqlx::query!(
                "SELECT space_id, folder_id, name, expires_at FROM files WHERE id = ? AND user_id = ?",
                id,
                user
            )
            .fetch_optional(&state.db)
            .await?
            .ok_or(AppError::NotFound)?;
            // Fichier de l'onglet Outils, pas encore rangé : le garder.
            if file.expires_at.is_some() {
                (
                    format!("Garder « {} » dans", file.name),
                    format!("/files/{id}/keep"),
                    None,
                    next,
                )
            } else {
                (
                    format!("Ranger « {} »", file.name),
                    format!("/files/{id}/move"),
                    Some((file.space_id, file.folder_id)),
                    next,
                )
            }
        }
        (None, None) => return Err(AppError::NotFound),
    };
    let folders = all(&state.db, user).await?;
    Ok(web::html(DestinationsFragment {
        title,
        action,
        next,
        items: destinations(&ctx.spaces, &folders, here),
    }))
}

/// Adresse de la fenêtre « Ranger » d'un élément (`note` ou `fichier`), avec
/// la page où revenir.
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
