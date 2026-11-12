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
}

/// Slug de Perso, l'espace « vue d'ensemble » : son agenda (page Agenda,
/// widget « Prochains cours », API) réunit les agendas de tous les espaces.
/// Il ne se supprime pas ; renommé, il garde ce rôle (le slug ne change pas).
pub const OVERVIEW: &str = "perso";

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

pub async fn list(db: &SqlitePool, user_id: i64) -> AppResult<Vec<Space>> {
    let spaces = sqlx::query_as!(
        Space,
        r#"SELECT id AS "id!", slug, name, color, icon FROM spaces WHERE user_id = ? ORDER BY position, id"#,
        user_id
    )
    .fetch_all(db)
    .await?;
    Ok(spaces)
}

#[derive(Deserialize)]
pub struct SwitchForm {
    slug: String,
    /// Page d'où vient la demande, pour y revenir.
    next: Option<String>,
}

/// `POST /space` : change l'espace actif puis revient sur la page courante.
pub async fn switch(ctx: Ctx, axum::Form(form): axum::Form<SwitchForm>) -> AppResult<Response> {
    if !ctx.spaces.iter().any(|s| s.slug == form.slug) {
        return Err(AppError::NotFound);
    }
    ctx.session.insert(ACTIVE_SPACE, &form.slug).await?;

    let next = local_path(form.next.as_deref());
    // Une note ouverte appartient à l'ancien espace : on revient à la liste.
    let next = if next.starts_with("/notes/") {
        "/notes"
    } else {
        next
    };
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
    name: String,
    color: String,
    icon: String,
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
    let valid = match validate(&form, &ctx.spaces, None) {
        Ok(valid) => valid,
        Err(message) => {
            ctx.flash_error(message).await?;
            return Ok(see_other(BACK));
        }
    };
    let taken: Vec<&str> = ctx.spaces.iter().map(|s| s.slug.as_str()).collect();
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
    let valid = match validate(&form, &ctx.spaces, Some(id)) {
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
    let message = if valid.name == space.name {
        format!("Espace « {} » modifié", valid.name)
    } else {
        format!("« {} » s'appelle maintenant « {} »", space.name, valid.name)
    };
    ctx.flash_ok(message).await?;
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
    pub async fn of(db: &SqlitePool, user_id: i64, space_id: i64) -> AppResult<Self> {
        let files = sqlx::query!(
            r#"SELECT count(*) AS "n!: i64", coalesce(sum(size), 0) AS "bytes!: i64"
               FROM files WHERE user_id = ? AND space_id = ?"#,
            user_id,
            space_id
        )
        .fetch_one(db)
        .await?;
        let notes = sqlx::query_scalar!(
            "SELECT count(*) FROM notes WHERE user_id = ? AND space_id = ?",
            user_id,
            space_id
        )
        .fetch_one(db)
        .await?;
        let tasks = sqlx::query_scalar!(
            "SELECT count(*) FROM tasks WHERE user_id = ? AND space_id = ?",
            user_id,
            space_id
        )
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
    let others = ctx.spaces.iter().filter(|s| s.id != id).cloned().collect();
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

    let (message, next_active) = match form.content.as_str() {
        "move" => {
            let target = form
                .target
                .filter(|&target| target != id)
                .and_then(|target| ctx.space_by_id(target))
                .ok_or_else(|| AppError::BadRequest("Espace de destination inconnu.".into()))?;
            // Tout passe dans l'espace choisi, y compris les envois en cours
            // et les conversions, puis l'espace (vide) est supprimé.
            let mut tx = state.db.begin().await?;
            sqlx::query!(
                "UPDATE notes SET space_id = ? WHERE space_id = ? AND user_id = ?",
                target.id,
                id,
                user_id
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query!(
                "UPDATE tasks SET space_id = ? WHERE space_id = ? AND user_id = ?",
                target.id,
                id,
                user_id
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
                "UPDATE files SET space_id = ? WHERE space_id = ? AND user_id = ?",
                target.id,
                id,
                user_id
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
            let keys = sqlx::query_scalar!(
                "SELECT storage_key FROM files WHERE space_id = ? AND user_id = ?",
                id,
                user_id
            )
            .fetch_all(&mut *tx)
            .await?;
            sqlx::query!(
                "DELETE FROM notes WHERE space_id = ? AND user_id = ?",
                id,
                user_id
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query!(
                "DELETE FROM tasks WHERE space_id = ? AND user_id = ?",
                id,
                user_id
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query!(
                "DELETE FROM calendar_sources WHERE space_id = ? AND user_id = ?",
                id,
                user_id
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query!(
                "DELETE FROM files WHERE space_id = ? AND user_id = ?",
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
