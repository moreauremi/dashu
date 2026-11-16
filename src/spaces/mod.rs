//! Espaces. Tout contenu (notes, tâches, agendas, fichiers) appartient à un
//! espace ; l'espace actif est gardé en session et filtre toutes les pages.
//!
//! Un compte naît avec Perso, École et Taff. On peut ensuite en créer, en
//! modifier (nom, couleur, icône) et en supprimer depuis les réglages. Le
//! slug, identifiant court utilisé par l'API et la session, ne change pas
//! quand on renomme un espace.

use askama::Template;
use axum::extract::{Path, State};
use axum::response::Response;
use serde::Deserialize;
use sqlx::SqlitePool;

pub mod url;

use crate::AppState;
use crate::auth::session::{ACTIVE_SPACE, Ctx, Shell};
use crate::files;
use crate::web::{self, AppError, AppResult, local_path, see_other};

#[derive(Debug, Clone)]
pub struct Space {
    pub id: i64,
    /// Identifiant court : `perso`, `ecole`, `taff`, ou tiré du nom à la
    /// création (`sport`). Gardé quand l'espace est renommé.
    pub slug: String,
    pub name: String,
    /// Nom d'un accent du design system (voir [`COLORS`]), appliqué par
    /// l'attribut `data-accent` du conteneur de la page.
    pub color: String,
    /// Nom d'une icône du sprite (voir [`ICONS`]).
    pub icon: String,
    /// Archivé : caché du sélecteur et des listes, restaurable.
    pub archived: bool,
    /// Règle : une vidéo qui arrive est convertie en MP4.
    pub auto_mp4: bool,
    /// Règle : une photo HEIC qui arrive est doublée d'un JPEG.
    pub auto_heic_jpeg: bool,
    /// Compte propriétaire de l'espace (`spaces.user_id`).
    pub owner_id: i64,
    /// Vrai si le compte connecté en est le propriétaire ; faux pour un
    /// espace partagé qu'il a rejoint (voir SPEC, « Espaces partagés »).
    pub mine: bool,
}

/// Slug de Perso, l'espace « vue d'ensemble » : son agenda (page Agenda,
/// widget « Prochains cours », API), ses tâches, notes et fichiers réunissent
/// ceux de tous les espaces (sauf filtre « Seulement Perso »). Il ne se
/// supprime pas ; renommé, il garde ce rôle (le slug ne change pas).
pub const OVERVIEW: &str = "perso";

/// Pastille d'un élément d'un autre espace, dans la vue d'ensemble de Perso :
/// le point de couleur et le nom de son espace.
#[derive(Debug, Clone)]
pub struct SpaceTag {
    pub name: String,
    pub color: String,
}

impl Space {
    /// Icône du sprite associée à l'espace.
    pub fn icon(&self) -> &str {
        if ICONS.iter().any(|(key, _)| *key == self.icon) {
            &self.icon
        } else {
            "home"
        }
    }

    /// Vrai pour Perso (voir [`OVERVIEW`]).
    pub fn is_overview(&self) -> bool {
        self.slug == OVERVIEW
    }
}

/// Accents proposés pour un espace : (clé, nom affiché). Chacun a sa couleur
/// dans app.css (`--orange`…), qui passe 4,5:1 sur les surfaces des deux thèmes.
pub const COLORS: [(&str, &str); 6] = [
    ("orange", "Orange"),
    ("blue", "Bleu"),
    ("green", "Vert"),
    ("violet", "Violet"),
    ("rose", "Rose"),
    ("teal", "Bleu-vert"),
];

/// Icônes proposées pour un espace : (clé dans le sprite, nom affiché).
pub const ICONS: [(&str, &str); 12] = [
    ("home", "Maison"),
    ("building", "Entreprise"),
    ("work", "Travail"),
    ("school", "École"),
    ("book", "Livre"),
    ("code", "Code"),
    ("users", "Groupe"),
    ("heart", "Cœur"),
    ("dumbbell", "Sport"),
    ("music", "Musique"),
    ("plane", "Voyage"),
    ("folder", "Dossier"),
];

/// Au-delà, l'île du haut de l'écran ne tient plus sur un téléphone.
pub const MAX_SPACES: usize = 8;

/// Le nom s'affiche dans l'île : il reste court.
pub const NAME_MAX_CHARS: usize = 24;

/// Retour aux réglages, sur la carte des espaces.
const BACK: &str = "/settings#espaces";

/// (slug, nom, couleur, icône) des espaces d'un nouveau compte, dans l'ordre.
const DEFAULTS: [(&str, &str, &str, &str); 3] = [
    ("perso", "Perso", "orange", "home"),
    ("ecole", "École", "blue", "school"),
    ("taff", "Taff", "green", "work"),
];

/// Crée les espaces d'un compte qui n'en a aucun (première connexion). Un
/// compte qui en a déjà garde les siens : un espace supprimé ne revient pas.
pub async fn ensure_defaults(db: &SqlitePool, user_id: i64) -> AppResult<()> {
    let count = sqlx::query_scalar!("SELECT count(*) FROM spaces WHERE user_id = ?", user_id)
        .fetch_one(db)
        .await?;
    if count > 0 {
        return Ok(());
    }
    for (position, (slug, name, color, icon)) in (0_i64..).zip(DEFAULTS) {
        // OR IGNORE : deux premières connexions simultanées ne se gênent pas.
        sqlx::query!(
            "INSERT OR IGNORE INTO spaces (user_id, slug, name, color, icon, position) VALUES (?, ?, ?, ?, ?, ?)",
            user_id,
            slug,
            name,
            color,
            icon,
            position
        )
        .execute(db)
        .await?;
    }
    Ok(())
}

/// Espaces auxquels un compte a accès, dans l'ordre de son île : les siens,
/// puis les espaces partagés qu'il a rejoints, sous le slug et à la place
/// qu'ils ont chez lui (`space_members`).
pub async fn list(db: &SqlitePool, user_id: i64) -> AppResult<Vec<Space>> {
    let spaces = sqlx::query_as!(
        Space,
        r#"SELECT id AS "id!", slug AS "slug!", name AS "name!", color AS "color!",
                  icon AS "icon!", archived AS "archived!: bool",
                  auto_mp4 AS "auto_mp4!: bool", auto_heic_jpeg AS "auto_heic_jpeg!: bool",
                  owner_id AS "owner_id!: i64", mine AS "mine!: bool"
           FROM (
               SELECT s.id, s.slug, s.name, s.color, s.icon,
                      s.archived_at IS NOT NULL AS archived, s.auto_mp4, s.auto_heic_jpeg,
                      s.user_id AS owner_id, 1 AS mine, s.position AS place
               FROM spaces s WHERE s.user_id = ?1
               UNION ALL
               SELECT s.id, m.slug, s.name, s.color, s.icon,
                      s.archived_at IS NOT NULL, s.auto_mp4, s.auto_heic_jpeg,
                      s.user_id, 0, m.position
               FROM space_members m JOIN spaces s ON s.id = m.space_id
               WHERE m.user_id = ?1
           )
           ORDER BY place, id"#,
        user_id
    )
    .fetch_all(db)
    .await?;
    Ok(spaces)
}

#[derive(Deserialize)]
pub struct SwitchForm {
    pub(crate) slug: String,
    /// Page d'où vient la demande, pour y revenir.
    pub(crate) next: Option<String>,
}

/// `POST /space` : change l'espace actif puis revient sur la page courante.
pub async fn switch(ctx: Ctx, axum::Form(form): axum::Form<SwitchForm>) -> AppResult<Response> {
    if !ctx.spaces.iter().any(|s| s.slug == form.slug) {
        return Err(AppError::NotFound);
    }
    ctx.session.insert(ACTIVE_SPACE, &form.slug).await?;

    let next = local_path(form.next.as_deref());
    // Une note ou un fichier ouvert appartient à l'ancien espace : on revient
    // à la liste.
    let next = if next.starts_with("/notes/") {
        "/notes"
    } else if next.starts_with("/files/") {
        "/files"
    } else {
        next
    };
    // La page d'un espace prend l'adresse du nouvel espace.
    if url::is_scoped(next) {
        return Ok(see_other(&url::prefixed(&form.slug, next)));
    }
    Ok(see_other(next))
}

// ---------------------------------------------------------------------
// Gestion des espaces (réglages)
// ---------------------------------------------------------------------

/// Couleurs à proposer dans un formulaire : (clé, nom, cochée).
pub fn color_choices(selected: &str) -> Vec<(&'static str, &'static str, bool)> {
    COLORS
        .iter()
        .map(|&(key, label)| (key, label, key == selected))
        .collect()
}

/// Icônes à proposer dans un formulaire : (clé, nom, cochée).
pub fn icon_choices(selected: &str) -> Vec<(&'static str, &'static str, bool)> {
    ICONS
        .iter()
        .map(|&(key, label)| (key, label, key == selected))
        .collect()
}

/// Couleur et icône proposées pour un nouvel espace : les premières qu'aucun
/// espace n'utilise encore, pour qu'il se distingue dans l'île.
pub fn suggestion(spaces: &[Space]) -> (&'static str, &'static str) {
    let unused = |list: &[(&'static str, &'static str)], used: Vec<&str>| {
        list.iter()
            .map(|(key, _)| *key)
            .find(|key| !used.contains(key))
            .unwrap_or(list[0].0)
    };
    (
        unused(&COLORS, spaces.iter().map(|s| s.color.as_str()).collect()),
        unused(&ICONS, spaces.iter().map(|s| s.icon()).collect()),
    )
}

#[derive(Deserialize)]
pub struct SpaceForm {
    pub(crate) name: String,
    pub(crate) color: String,
    pub(crate) icon: String,
    /// Présent quand le formulaire montre les règles (modification d'un espace).
    pub(crate) rules: Option<String>,
    pub(crate) auto_mp4: Option<String>,
    pub(crate) auto_heic_jpeg: Option<String>,
}

/// Nom, couleur et icône vérifiés.
struct Valid {
    name: String,
    color: &'static str,
    icon: &'static str,
}

/// Vérifie le formulaire. `except` : l'espace modifié, qui peut garder son nom.
fn validate(form: &SpaceForm, spaces: &[Space], except: Option<i64>) -> Result<Valid, String> {
    let name = form.name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Err("Donne un nom à l'espace.".into());
    }
    if name.chars().count() > NAME_MAX_CHARS || name.chars().any(char::is_control) {
        return Err(format!(
            "Le nom d'un espace tient en {NAME_MAX_CHARS} caractères."
        ));
    }
    let lower = name.to_lowercase();
    if spaces
        .iter()
        .any(|s| Some(s.id) != except && s.name.to_lowercase() == lower)
    {
        return Err(format!("Un espace s'appelle déjà « {name} »."));
    }
    let color = COLORS
        .iter()
        .find(|(key, _)| *key == form.color)
        .ok_or("Couleur inconnue.")?
        .0;
    let icon = ICONS
        .iter()
        .find(|(key, _)| *key == form.icon)
        .ok_or("Icône inconnue.")?
        .0;
    Ok(Valid { name, color, icon })
}

/// Identifiant court tiré du nom : minuscules sans accents, chiffres et
/// tirets (« Vie de famille » → `vie-de-famille`). Unique parmi `taken`.
fn new_slug(name: &str, taken: &[&str]) -> String {
    let mut folded = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        let part = match c {
            'à' | 'â' | 'ä' | 'á' | 'ã' | 'å' => "a",
            'ç' => "c",
            'é' | 'è' | 'ê' | 'ë' => "e",
            'î' | 'ï' | 'í' | 'ì' => "i",
            'ô' | 'ö' | 'ó' | 'ò' | 'õ' => "o",
            'ù' | 'û' | 'ü' | 'ú' => "u",
            'ÿ' | 'ý' => "y",
            'ñ' => "n",
            'œ' => "oe",
            'æ' => "ae",
            c if c.is_ascii_alphanumeric() => {
                folded.push(c);
                continue;
            }
            _ => "-",
        };
        folded.push_str(part);
    }
    let words: Vec<&str> = folded.split('-').filter(|w| !w.is_empty()).collect();
    let base: String = words.join("-").chars().take(NAME_MAX_CHARS).collect();
    let base = match base.trim_end_matches('-') {
        "" => "espace".to_string(),
        base => base.to_string(),
    };
    let mut slug = base.clone();
    let mut n = 2;
    while taken.contains(&slug.as_str()) {
        slug = format!("{base}-{n}");
        n += 1;
    }
    slug
}

/// Seul le propriétaire gère un espace (nom, couleur, archivage, suppression,
/// membres) : le serveur le vérifie, le masquage des boutons ne suffit pas.
fn owner_only(space: &Space) -> AppResult<()> {
    if space.mine {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// `POST /settings/spaces` : crée un espace et en fait l'espace actif.
pub async fn create(
    ctx: Ctx,
    State(state): State<AppState>,
    axum::Form(form): axum::Form<SpaceForm>,
) -> AppResult<Response> {
    if ctx.spaces.len() >= MAX_SPACES {
        ctx.flash_error(format!(
            "{MAX_SPACES} espaces au plus : supprimes-en un pour en créer un autre."
        ))
        .await?;
        return Ok(see_other(BACK));
    }
    let all = ctx.all_spaces();
    let valid = match validate(&form, &all, None) {
        Ok(valid) => valid,
        Err(message) => {
            ctx.flash_error(message).await?;
            return Ok(see_other(BACK));
        }
    };
    // Les slugs des espaces archivés restent pris (leurs adresses aussi).
    let taken: Vec<&str> = all.iter().map(|s| s.slug.as_str()).collect();
    let slug = new_slug(&valid.name, &taken);
    sqlx::query!(
        "INSERT INTO spaces (user_id, slug, name, color, icon, position)
         VALUES (?, ?, ?, ?, ?, (SELECT coalesce(max(position), -1) + 1 FROM spaces WHERE user_id = ?))",
        ctx.user.id,
        slug,
        valid.name,
        valid.color,
        valid.icon,
        ctx.user.id
    )
    .execute(&state.db)
    .await?;
    ctx.session.insert(ACTIVE_SPACE, &slug).await?;
    ctx.flash_ok(format!("Espace « {} » créé", valid.name))
        .await?;
    Ok(see_other(BACK))
}

/// `POST /settings/spaces/{id}` : renomme un espace, change sa couleur ou son icône.
pub async fn update(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<SpaceForm>,
) -> AppResult<Response> {
    let space = ctx.space_by_id(id).ok_or(AppError::NotFound)?;
    owner_only(space)?;
    let valid = match validate(&form, &ctx.all_spaces(), Some(id)) {
        Ok(valid) => valid,
        Err(message) => {
            ctx.flash_error(message).await?;
            return Ok(see_other(BACK));
        }
    };
    sqlx::query!(
        "UPDATE spaces SET name = ?, color = ?, icon = ? WHERE id = ? AND user_id = ?",
        valid.name,
        valid.color,
        valid.icon,
        id,
        ctx.user.id
    )
    .execute(&state.db)
    .await?;
    if form.rules.is_some() {
        let (mp4, heic) = (form.auto_mp4.is_some(), form.auto_heic_jpeg.is_some());
        sqlx::query!(
            "UPDATE spaces SET auto_mp4 = ?, auto_heic_jpeg = ? WHERE id = ? AND user_id = ?",
            mp4,
            heic,
            id,
            ctx.user.id
        )
        .execute(&state.db)
        .await?;
    }
    let message = if valid.name == space.name {
        format!("Espace « {} » modifié", valid.name)
    } else {
        format!("« {} » s'appelle maintenant « {} »", space.name, valid.name)
    };
    ctx.flash_ok(message).await?;
    Ok(see_other(BACK))
}

/// `POST /settings/spaces/{id}/archive` : archive un espace (pas Perso). Il
/// quitte le sélecteur, les listes et la vue d'ensemble ; son contenu reste
/// cherchable par la palette, et il se rouvre ou se restaure depuis Réglages.
pub async fn archive(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let space = ctx
        .spaces
        .iter()
        .find(|s| s.id == id)
        .ok_or(AppError::NotFound)?;
    owner_only(space)?;
    if space.is_overview() || ctx.spaces.len() <= 1 {
        ctx.flash_error(format!("{} ne s'archive pas.", space.name))
            .await?;
        return Ok(see_other(BACK));
    }
    let now = crate::time::now();
    sqlx::query!(
        "UPDATE spaces SET archived_at = ? WHERE id = ? AND user_id = ?",
        now,
        id,
        ctx.user.id
    )
    .execute(&state.db)
    .await?;
    // L'espace actif part : on revient au premier espace en service.
    if ctx.active.id == id {
        ctx.session.remove::<String>(ACTIVE_SPACE).await?;
    }
    ctx.flash_ok(format!(
        "{} archivé : il n'est plus dans le sélecteur, ses notes restent cherchables",
        space.name
    ))
    .await?;
    Ok(see_other(BACK))
}

/// `POST /settings/spaces/{id}/restore` : remet un espace archivé en service.
pub async fn restore(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let space = ctx
        .archived
        .iter()
        .find(|s| s.id == id)
        .ok_or(AppError::NotFound)?;
    owner_only(space)?;
    if ctx.spaces.len() >= MAX_SPACES {
        ctx.flash_error(format!(
            "{MAX_SPACES} espaces au plus : archive ou supprime-en un pour restaurer {}.",
            space.name
        ))
        .await?;
        return Ok(see_other(BACK));
    }
    sqlx::query!(
        "UPDATE spaces SET archived_at = NULL WHERE id = ? AND user_id = ?",
        id,
        ctx.user.id
    )
    .execute(&state.db)
    .await?;
    ctx.flash_ok(format!("{} est de retour dans le sélecteur", space.name))
        .await?;
    Ok(see_other(BACK))
}

/// Ce que contient un espace, pour prévenir avant de le supprimer.
pub struct Content {
    pub files: i64,
    /// Taille totale des fichiers, en octets.
    pub bytes: i64,
    pub notes: i64,
    pub tasks: i64,
    pub calendars: i64,
}

impl Content {
    /// Tout le contenu de l'espace, quel qu'en soit l'auteur ; seuls les
    /// agendas sont comptés pour `user_id`, qui les a ajoutés.
    pub async fn of(db: &SqlitePool, user_id: i64, space_id: i64) -> AppResult<Self> {
        let files = sqlx::query!(
            r#"SELECT count(*) AS "n!: i64", coalesce(sum(size), 0) AS "bytes!: i64"
               FROM files WHERE space_id = ?"#,
            space_id
        )
        .fetch_one(db)
        .await?;
        let notes = sqlx::query_scalar!("SELECT count(*) FROM notes WHERE space_id = ?", space_id)
            .fetch_one(db)
            .await?;
        let tasks = sqlx::query_scalar!("SELECT count(*) FROM tasks WHERE space_id = ?", space_id)
            .fetch_one(db)
            .await?;
        let calendars = sqlx::query_scalar!(
            "SELECT count(*) FROM calendar_sources WHERE user_id = ? AND space_id = ?",
            user_id,
            space_id
        )
        .fetch_one(db)
        .await?;
        Ok(Self {
            files: files.n,
            bytes: files.bytes,
            notes,
            tasks,
            calendars,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.files + self.notes + self.tasks + self.calendars == 0
    }

    /// « 3 fichiers (1,2 Go), 12 notes et 1 agenda »
    pub fn summary(&self) -> String {
        let count = |n: i64, one: &str, many: &str| match n {
            0 => None,
            1 => Some(format!("1 {one}")),
            n => Some(format!("{n} {many}")),
        };
        let parts: Vec<String> = [
            count(self.files, "fichier", "fichiers")
                .map(|f| format!("{f} ({})", files::human_size(self.bytes))),
            count(self.notes, "note", "notes"),
            count(self.tasks, "tâche", "tâches"),
            count(self.calendars, "agenda", "agendas"),
        ]
        .into_iter()
        .flatten()
        .collect();
        match parts.as_slice() {
            [] => String::new(),
            [one] => one.clone(),
            [rest @ .., last] => format!("{} et {last}", rest.join(", ")),
        }
    }
}

#[derive(Template)]
#[template(path = "settings/space_delete.html")]
struct DeletePage {
    shell: Shell,
    space: Space,
    /// Espaces qui peuvent recevoir le contenu.
    others: Vec<Space>,
    content: Content,
}

/// `GET /settings/spaces/{id}/delete` : confirmation, avec ce que contient
/// l'espace et la proposition de transférer ce contenu dans un autre. Pour
/// Perso, la page explique pourquoi il ne se supprime pas.
pub async fn delete_page(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let space = ctx.space_by_id(id).cloned().ok_or(AppError::NotFound)?;
    owner_only(&space)?;
    // Seuls les espaces du propriétaire peuvent recevoir le contenu.
    let others = ctx
        .spaces
        .iter()
        .filter(|s| s.id != id && s.mine)
        .cloned()
        .collect();
    let content = Content::of(&state.db, ctx.user.id, id).await?;
    let page = DeletePage {
        space,
        others,
        content,
        shell: ctx.shell("settings").await?,
    };
    Ok(web::html(page))
}

#[derive(Deserialize)]
pub struct DeleteForm {
    /// `move` : le contenu passe dans l'espace `target` ; `delete` : il est
    /// supprimé avec l'espace.
    #[serde(default)]
    content: String,
    target: Option<i64>,
}

/// `POST /settings/spaces/{id}/delete`
pub async fn delete(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<DeleteForm>,
) -> AppResult<Response> {
    let space = ctx.space_by_id(id).cloned().ok_or(AppError::NotFound)?;
    owner_only(&space)?;
    if space.is_overview() {
        ctx.flash_error(format!(
            "« {} » ne se supprime pas : son agenda réunit ceux de tous les espaces.",
            space.name
        ))
        .await?;
        return Ok(see_other(BACK));
    }
    if ctx.spaces.len() <= 1 {
        ctx.flash_error("Il faut garder au moins un espace.")
            .await?;
        return Ok(see_other(BACK));
    }
    let user_id = ctx.user.id;

    // Les choix sont vérifiés avant de toucher à quoi que ce soit.
    let target = match form.content.as_str() {
        "move" => Some(
            form.target
                .filter(|&target| target != id)
                .and_then(|target| ctx.space_by_id(target))
                .filter(|s| s.mine)
                .ok_or_else(|| AppError::BadRequest("Espace de destination inconnu.".into()))?,
        ),
        "delete" => None,
        _ => {
            return Err(AppError::BadRequest(
                "Choisis quoi faire du contenu de l'espace.".into(),
            ));
        }
    };

    // Les membres s'en vont d'abord : leurs agendas rentrent dans leur Perso
    // et leurs liens de partage vers l'espace sont révoqués.
    let members = sqlx::query_scalar!(
        r#"SELECT user_id AS "user_id!: i64" FROM space_members WHERE space_id = ?"#,
        id
    )
    .fetch_all(&state.db)
    .await?;
    for member in members {
        remove_member(&state.db, id, member).await?;
    }

    let (message, next_active) = match form.content.as_str() {
        "move" => {
            let target = target.expect("destination vérifiée plus haut");
            // Tout passe dans l'espace choisi, y compris les envois en cours
            // et les conversions, puis l'espace (vide) est supprimé. Les
            // dossiers sont lus sans filtre de compte : ceux des membres
            // suivent aussi.
            let folders: Vec<crate::folders::Folder> = sqlx::query!(
                r#"SELECT id AS "id!", space_id, parent_id, name FROM folders
                   WHERE space_id = ? OR space_id = ?"#,
                id,
                target.id
            )
            .fetch_all(&state.db)
            .await?
            .into_iter()
            .map(|row| crate::folders::Folder {
                id: row.id,
                space_id: row.space_id,
                parent_id: row.parent_id,
                name: row.name,
            })
            .collect();
            let mut tx = state.db.begin().await?;
            // Ses dossiers suivent leur contenu. Un nom déjà pris dans l'espace
            // qui les reçoit devient « Math (2) » ; les sous-dossiers gardent
            // leur dossier parent, donc leur nom.
            let mut taken = crate::folders::neighbours(&folders, target.id, None, None);
            for folder in folders.iter().filter(|f| f.space_id == id && f.is_top()) {
                let name = crate::folders::free_name(&taken, &folder.name);
                sqlx::query!(
                    "UPDATE folders SET space_id = ?, name = ? WHERE id = ?",
                    target.id,
                    name,
                    folder.id
                )
                .execute(&mut *tx)
                .await?;
                taken.push(name);
            }
            sqlx::query!(
                "UPDATE folders SET space_id = ? WHERE space_id = ?",
                target.id,
                id
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query!(
                "UPDATE notes SET space_id = ? WHERE space_id = ?",
                target.id,
                id
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query!(
                "UPDATE tasks SET space_id = ? WHERE space_id = ?",
                target.id,
                id
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query!(
                "UPDATE calendar_sources SET space_id = ? WHERE space_id = ? AND user_id = ?",
                target.id,
                id,
                user_id
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query!(
                "UPDATE files SET space_id = ? WHERE space_id = ?",
                target.id,
                id
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query!(
                "UPDATE uploads SET space_id = ? WHERE space_id = ? AND user_id = ?",
                target.id,
                id,
                user_id
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query!(
                "UPDATE jobs SET space_id = ? WHERE space_id = ? AND user_id = ?",
                target.id,
                id,
                user_id
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query!(
                "DELETE FROM spaces WHERE id = ? AND user_id = ?",
                id,
                user_id
            )
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            (
                format!(
                    "Espace « {} » supprimé : son contenu est maintenant dans « {} »",
                    space.name, target.name
                ),
                Some(target.slug.clone()),
            )
        }
        "delete" => {
            // Les notes, tâches, agendas et fichiers sont supprimés un par un
            // (les déclencheurs de la recherche plein texte suivent), puis
            // l'espace : la cascade emporte le reste (étiquettes des notes,
            // événements, envois en cours, conversions).
            let mut tx = state.db.begin().await?;
            let keys = sqlx::query_scalar!("SELECT storage_key FROM files WHERE space_id = ?", id)
                .fetch_all(&mut *tx)
                .await?;
            sqlx::query!("DELETE FROM notes WHERE space_id = ?", id)
                .execute(&mut *tx)
                .await?;
            sqlx::query!("DELETE FROM tasks WHERE space_id = ?", id)
                .execute(&mut *tx)
                .await?;
            sqlx::query!(
                "DELETE FROM calendar_sources WHERE space_id = ? AND user_id = ?",
                id,
                user_id
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query!("DELETE FROM files WHERE space_id = ?", id)
                .execute(&mut *tx)
                .await?;
            sqlx::query!(
                "DELETE FROM spaces WHERE id = ? AND user_id = ?",
                id,
                user_id
            )
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            // Le disque après la base : un fichier dont la ligne a disparu
            // n'est plus servi, même si son effacement échoue.
            if let Some(config) = &state.config.files {
                for key in keys {
                    files::remove_stored(config, &key).await;
                }
            }
            (
                format!("Espace « {} » supprimé avec son contenu", space.name),
                None,
            )
        }
        _ => {
            return Err(AppError::BadRequest(
                "Choisis quoi faire du contenu de l'espace.".into(),
            ));
        }
    };

    // L'espace actif vient de disparaître : on passe à celui qui a reçu le
    // contenu, ou au premier.
    if ctx.active.id == id {
        match next_active {
            Some(slug) => ctx.session.insert(ACTIVE_SPACE, slug).await?,
            None => {
                ctx.session.remove::<String>(ACTIVE_SPACE).await?;
            }
        }
    }
    ctx.flash_ok(message).await?;
    Ok(see_other(BACK))
}

// ---------------------------------------------------------------------
// Membres et invitations (SPEC, « Espaces partagés »)
// ---------------------------------------------------------------------

/// « Prénom Nom », ou l'adresse tant que le profil est vide. Aussi utilisée
/// par les réglages (invitations reçues, « Partagé par … »).
pub(crate) fn person_name(first_name: &str, last_name: &str, email: &str) -> String {
    let name = format!("{first_name} {last_name}");
    match name.trim() {
        "" => email.to_string(),
        name => name.to_string(),
    }
}

/// Un compte affiché sur la page Membres.
pub struct Person {
    pub id: i64,
    pub name: String,
    pub email: String,
}

/// Un membre s'en va, de lui-même ou retiré, et l'espace perd sa trace chez
/// lui, en une seule transaction :
///
/// * sa ligne `space_members` disparaît (l'accès se coupe à la requête
///   suivante) ;
/// * ses agendas rattachés à l'espace rentrent dans son Perso ;
/// * les liens de partage qu'il a créés vers des fichiers de l'espace sont
///   révoqués ;
/// * si l'espace était son espace « moment », ce réglage revient au défaut.
///
/// Ce qu'il a créé reste dans l'espace.
pub(crate) async fn remove_member(db: &SqlitePool, space_id: i64, user_id: i64) -> AppResult<()> {
    let now = crate::time::now();
    let mut tx = db.begin().await?;
    sqlx::query!(
        "DELETE FROM space_members WHERE space_id = ? AND user_id = ?",
        space_id,
        user_id
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE calendar_sources
         SET space_id = (SELECT id FROM spaces WHERE user_id = ?1 AND slug = 'perso')
         WHERE user_id = ?1 AND space_id = ?2
           AND EXISTS (SELECT 1 FROM spaces WHERE user_id = ?1 AND slug = 'perso')",
        user_id,
        space_id
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE share_links SET revoked_at = ?
         WHERE user_id = ? AND revoked_at IS NULL
           AND file_id IN (SELECT id FROM files WHERE space_id = ?)",
        now,
        user_id,
        space_id
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE users SET moment_space = NULL WHERE id = ? AND moment_space = ?",
        user_id,
        space_id
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Adresse de la page Membres d'un espace.
fn members_url(space_id: i64) -> String {
    format!("/settings/spaces/{space_id}/members")
}

#[derive(Template)]
#[template(path = "settings/space_members.html")]
struct MembersPage {
    shell: Shell,
    space: Space,
    owner: Person,
    members: Vec<Person>,
    /// Invitations en attente : (identifiant de l'invitation, invité).
    pending: Vec<(i64, Person)>,
}

/// `GET /settings/spaces/{id}/members` : le propriétaire, les membres et les
/// invitations en attente. Perso n'a pas de page Membres.
pub async fn members_page(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let space = ctx.space_by_id(id).cloned().ok_or(AppError::NotFound)?;
    if space.is_overview() {
        return Err(AppError::NotFound);
    }
    let owner = sqlx::query!(
        r#"SELECT id AS "id!", email, first_name, last_name FROM users WHERE id = ?"#,
        space.owner_id
    )
    .fetch_one(&state.db)
    .await?;
    let members = sqlx::query!(
        r#"SELECT u.id AS "id!", u.email, u.first_name, u.last_name
           FROM space_members m JOIN users u ON u.id = m.user_id
           WHERE m.space_id = ? ORDER BY m.joined_at, u.id"#,
        id
    )
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|u| Person {
        id: u.id,
        name: person_name(&u.first_name, &u.last_name, &u.email),
        email: u.email,
    })
    .collect();
    // Les invitations ne regardent que le propriétaire.
    let pending = if space.mine {
        sqlx::query!(
            r#"SELECT i.id AS "inv!", u.id AS "id!", u.email, u.first_name, u.last_name
               FROM space_invitations i JOIN users u ON u.id = i.user_id
               WHERE i.space_id = ? ORDER BY i.id"#,
            id
        )
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .map(|u| {
            (
                u.inv,
                Person {
                    id: u.id,
                    name: person_name(&u.first_name, &u.last_name, &u.email),
                    email: u.email,
                },
            )
        })
        .collect()
    } else {
        Vec::new()
    };
    let page = MembersPage {
        owner: Person {
            id: owner.id,
            name: person_name(&owner.first_name, &owner.last_name, &owner.email),
            email: owner.email,
        },
        members,
        pending,
        space,
        shell: ctx.shell("settings").await?,
    };
    Ok(web::html(page))
}

/// Refus d'une invitation : le message s'affiche sur la page Membres.
async fn refuse(ctx: &Ctx, id: i64, message: impl Into<String>) -> AppResult<Response> {
    ctx.flash_error(message).await?;
    Ok(see_other(&members_url(id)))
}

#[derive(Deserialize)]
pub struct InviteForm {
    email: String,
}

/// `POST /settings/spaces/{id}/invitations` : le propriétaire invite un
/// compte existant, par son adresse. Un refus le dit et ne crée rien.
pub async fn invite(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<InviteForm>,
) -> AppResult<Response> {
    let space = ctx.space_by_id(id).ok_or(AppError::NotFound)?;
    owner_only(space)?;
    if space.is_overview() {
        // Perso n'a pas de page Membres : le message s'affiche dans les réglages.
        ctx.flash_error(format!(
            "{} reste privé : il ne se partage pas.",
            space.name
        ))
        .await?;
        return Ok(see_other(BACK));
    }
    if space.archived {
        return refuse(
            &ctx,
            id,
            format!(
                "{} est archivé : restaure-le avant d'inviter quelqu'un.",
                space.name
            ),
        )
        .await;
    }
    let email = form.email.trim().to_lowercase();
    if email.is_empty() {
        return refuse(
            &ctx,
            id,
            "Saisis l'adresse e-mail de la personne à inviter.",
        )
        .await;
    }
    // Un compte, c'est quelqu'un qui s'est déjà connecté une fois ; son
    // adresse doit aussi être encore autorisée.
    let invited = sqlx::query_scalar!(r#"SELECT id AS "id!" FROM users WHERE email = ?"#, email)
        .fetch_optional(&state.db)
        .await?
        .filter(|_| state.config.is_allowed(&email));
    let Some(invited) = invited else {
        return refuse(&ctx, id, format!(
            "Aucun compte ne porte l'adresse {email} : la personne doit d'abord se connecter une fois."
        )).await;
    };
    if invited == ctx.user.id {
        return refuse(&ctx, id, "Tu es déjà le propriétaire de cet espace.").await;
    }
    let member = sqlx::query_scalar!(
        r#"SELECT user_id AS "user_id!: i64" FROM space_members WHERE space_id = ? AND user_id = ?"#,
        id,
        invited
    )
    .fetch_optional(&state.db)
    .await?;
    if member.is_some() {
        return refuse(&ctx, id, format!("{email} est déjà membre de cet espace.")).await;
    }
    let pending = sqlx::query_scalar!(
        r#"SELECT id AS "id!: i64" FROM space_invitations WHERE space_id = ? AND user_id = ?"#,
        id,
        invited
    )
    .fetch_optional(&state.db)
    .await?;
    if pending.is_some() {
        return refuse(
            &ctx,
            id,
            format!("{email} a déjà une invitation en attente."),
        )
        .await;
    }

    let now = crate::time::now();
    sqlx::query!(
        "INSERT INTO space_invitations (space_id, user_id, invited_by, created_at) VALUES (?, ?, ?, ?)",
        id,
        invited,
        ctx.user.id,
        now
    )
    .execute(&state.db)
    .await?;
    ctx.flash_ok(format!("Invitation envoyée à {email}"))
        .await?;
    Ok(see_other(&members_url(id)))
}

/// `POST /settings/spaces/{id}/invitations/{inv}/cancel`
pub async fn cancel_invitation(
    ctx: Ctx,
    State(state): State<AppState>,
    Path((id, inv)): Path<(i64, i64)>,
) -> AppResult<Response> {
    let space = ctx.space_by_id(id).ok_or(AppError::NotFound)?;
    owner_only(space)?;
    let deleted = sqlx::query!(
        "DELETE FROM space_invitations WHERE id = ? AND space_id = ?",
        inv,
        id
    )
    .execute(&state.db)
    .await?
    .rows_affected();
    if deleted == 0 {
        return Err(AppError::NotFound);
    }
    ctx.flash_ok("Invitation annulée").await?;
    Ok(see_other(&members_url(id)))
}

/// `POST /settings/spaces/{id}/members/{user}/remove`
pub async fn remove(
    ctx: Ctx,
    State(state): State<AppState>,
    Path((id, user)): Path<(i64, i64)>,
) -> AppResult<Response> {
    let space = ctx.space_by_id(id).ok_or(AppError::NotFound)?;
    owner_only(space)?;
    let member = sqlx::query_scalar!(
        r#"SELECT user_id AS "user_id!: i64" FROM space_members WHERE space_id = ? AND user_id = ?"#,
        id,
        user
    )
    .fetch_optional(&state.db)
    .await?;
    if member.is_none() {
        return Err(AppError::NotFound);
    }
    remove_member(&state.db, id, user).await?;
    ctx.flash_ok("Membre retiré : plus d'accès à l'espace")
        .await?;
    Ok(see_other(&members_url(id)))
}

/// `POST /settings/spaces/{id}/leave` : un membre quitte l'espace. Le
/// propriétaire, lui, le supprime ou l'archive.
pub async fn leave(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let space = ctx.space_by_id(id).ok_or(AppError::NotFound)?;
    if space.mine {
        return Err(AppError::Forbidden);
    }
    remove_member(&state.db, id, ctx.user.id).await?;
    if ctx.active.id == id {
        ctx.session.remove::<String>(ACTIVE_SPACE).await?;
    }
    ctx.flash_ok(format!("Tu as quitté « {} »", space.name))
        .await?;
    Ok(see_other(BACK))
}

/// Identifiant d'adresse d'un espace rejoint : celui de l'espace, suffixé
/// (`-2`, `-3`…) s'il est déjà pris parmi `taken`.
fn joined_slug(base: &str, taken: &[String]) -> String {
    let mut slug = base.to_string();
    let mut n = 2;
    while taken.contains(&slug) {
        slug = format!("{base}-{n}");
        n += 1;
    }
    slug
}

/// `POST /settings/invitations/{inv}/accept` : l'invité seulement.
pub async fn accept_invitation(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(inv): Path<i64>,
) -> AppResult<Response> {
    let user_id = ctx.user.id;
    let invitation = sqlx::query!(
        r#"SELECT i.space_id, s.slug, s.name FROM space_invitations i
           JOIN spaces s ON s.id = i.space_id
           WHERE i.id = ? AND i.user_id = ?"#,
        inv,
        user_id
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;
    if ctx.spaces.len() >= MAX_SPACES {
        ctx.flash_error(format!(
            "{MAX_SPACES} espaces au plus : archive ou supprime-en un pour rejoindre « {} ».",
            invitation.name
        ))
        .await?;
        return Ok(see_other(BACK));
    }

    let mut tx = state.db.begin().await?;
    // Les slugs de ses espaces, archivés compris, et de ceux déjà rejoints.
    let taken: Vec<String> = sqlx::query_scalar!(
        r#"SELECT slug AS "slug!" FROM spaces WHERE user_id = ?1
           UNION SELECT slug FROM space_members WHERE user_id = ?1"#,
        user_id
    )
    .fetch_all(&mut *tx)
    .await?;
    let slug = joined_slug(&invitation.slug, &taken);
    let now = crate::time::now();
    sqlx::query!(
        "INSERT INTO space_members (space_id, user_id, slug, position, joined_at)
         VALUES (?1, ?2, ?3,
                 (SELECT coalesce(max(p), -1) + 1 FROM (
                      SELECT position AS p FROM spaces WHERE user_id = ?2
                      UNION ALL SELECT position FROM space_members WHERE user_id = ?2)),
                 ?4)",
        invitation.space_id,
        user_id,
        slug,
        now
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!("DELETE FROM space_invitations WHERE id = ?", inv)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    ctx.flash_ok(format!("Tu as rejoint « {} »", invitation.name))
        .await?;
    Ok(see_other(BACK))
}

/// `POST /settings/invitations/{inv}/decline` : l'invité seulement.
pub async fn decline_invitation(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(inv): Path<i64>,
) -> AppResult<Response> {
    let deleted = sqlx::query!(
        "DELETE FROM space_invitations WHERE id = ? AND user_id = ?",
        inv,
        ctx.user.id
    )
    .execute(&state.db)
    .await?
    .rows_affected();
    if deleted == 0 {
        return Err(AppError::NotFound);
    }
    ctx.flash_ok("Invitation refusée").await?;
    Ok(see_other("/settings"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_tire_du_nom() {
        assert_eq!(new_slug("Entreprise", &[]), "entreprise");
        assert_eq!(new_slug("  Vie de famille ! ", &[]), "vie-de-famille");
        assert_eq!(new_slug("Éco & Œuvres", &[]), "eco-oeuvres");
        assert_eq!(new_slug("🎵", &[]), "espace");
        assert_eq!(new_slug("Perso", &["perso"]), "perso-2");
        assert_eq!(new_slug("Perso", &["perso", "perso-2"]), "perso-3");
        assert_eq!(
            new_slug("Un nom bien trop long pour un slug", &[]),
            "un-nom-bien-trop-long-po"
        );
    }

    #[test]
    fn resume_du_contenu() {
        let content = |files, notes, tasks, calendars| Content {
            files,
            bytes: 1536,
            notes,
            tasks,
            calendars,
        };
        assert!(content(0, 0, 0, 0).is_empty());
        assert_eq!(content(0, 1, 0, 0).summary(), "1 note");
        assert_eq!(content(0, 2, 1, 0).summary(), "2 notes et 1 tâche");
        assert_eq!(
            content(3, 12, 0, 1).summary(),
            format!(
                "3 fichiers ({}), 12 notes et 1 agenda",
                files::human_size(1536)
            )
        );
    }
}
