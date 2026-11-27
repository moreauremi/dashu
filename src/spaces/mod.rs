//! Espaces. Tout contenu (notes, tâches, agendas, fichiers) appartient à un
//! espace ; l'espace actif est gardé en session et filtre toutes les pages.
//!
//! Un compte naît avec Perso, École et Taff. On peut ensuite en créer, en
//! modifier (nom, couleur, icône) et en supprimer depuis les réglages. Le
//! slug, identifiant court utilisé par l'API et la session, ne change pas
//! quand on renomme un espace.

use askama::Template;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Response;
use serde::{Deserialize, Serialize};
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

/// Retour aux réglages après une action qui fait quitter l'espace actif
/// (archivé, supprimé, quitté) : l'onglet passe sur `next`, ou sur le premier
/// autre espace en service. Sans cela, l'en-tête de l'onglet garderait
/// l'espace parti.
fn back_leaving(
    ctx: &Ctx,
    headers: &HeaderMap,
    gone: i64,
    next: Option<&str>,
) -> AppResult<Response> {
    if ctx.active.id != gone {
        return Ok(see_other(BACK));
    }
    let slug = next.or_else(|| {
        ctx.spaces
            .iter()
            .find(|s| s.id != gone)
            .map(|s| s.slug.as_str())
    });
    match slug {
        Some(slug) => url::go_to(headers, BACK, slug),
        None => Ok(see_other(BACK)),
    }
}

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
               FROM spaces s
               WHERE s.user_id = ?1 AND s.trash_of IS NULL AND s.deleted_at IS NULL
               UNION ALL
               SELECT s.id, m.slug, s.name, s.color, s.icon,
                      s.archived_at IS NOT NULL, s.auto_mp4, s.auto_heic_jpeg,
                      s.user_id, 0, m.position
               FROM space_members m JOIN spaces s ON s.id = m.space_id
               WHERE m.user_id = ?1 AND s.deleted_at IS NULL
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

/// `POST /space` : change l'espace actif puis revient sur la page courante,
/// dans le nouvel espace. Depuis htmx (l'île, le balayage), la réponse est un
/// `HX-Location` qui porte le nouvel espace : une redirection garderait
/// l'en-tête de l'onglet, resté sur l'ancien, et la page ne changerait pas.
pub async fn switch(
    ctx: Ctx,
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Form(form): axum::Form<SwitchForm>,
) -> AppResult<Response> {
    let target = ctx
        .spaces
        .iter()
        .find(|s| s.slug == form.slug)
        .ok_or(AppError::NotFound)?;
    ctx.session.insert(ACTIVE_SPACE, &form.slug).await?;
    let next = switch_target(&state.db, &ctx, target, local_path(form.next.as_deref())).await?;
    url::go_to(&headers, &next, &form.slug)
}

/// La page où arriver en changeant d'espace : la même, dans le nouvel espace,
/// quand elle y a un sens.
async fn switch_target(
    db: &SqlitePool,
    ctx: &Ctx,
    target: &Space,
    next: &str,
) -> AppResult<String> {
    let (path, query) = next.split_once('?').unwrap_or((next, ""));
    // Une note ou un fichier ouvert appartient à l'ancien espace : on revient
    // à la liste ; un fichier de Temporaire, à l'onglet Outils où il attend.
    if path.starts_with("/notes/") {
        return Ok("/notes".into());
    }
    if let Some(id) = path.strip_prefix("/files/") {
        let temporary = match id.parse::<i64>() {
            // Même requête que `files::keep_in` (cache de sqlx).
            Ok(id) => sqlx::query_scalar!(
                "SELECT size FROM files WHERE id = ? AND user_id = ? AND expires_at IS NOT NULL",
                id,
                ctx.user.id
            )
            .fetch_optional(db)
            .await?
            .is_some(),
            Err(_) => false,
        };
        return Ok(if temporary { "/tools" } else { "/files" }.into());
    }
    // Un outil masqué dans le nouvel espace : son accueil, sans message
    // d'erreur (changer d'espace n'est pas une faute).
    if let Some(tool) = crate::registry::at(path)
        && !crate::registry::load(db, ctx.user.id, target.id)
            .await?
            .is_shown(tool.id)
    {
        return Ok("/".into());
    }
    // Un dossier appartient à l'ancien espace : on garde « racine » ou
    // « temporaire », un numéro de dossier mène à la racine du nouvel espace
    // (sinon `files::list` renverrait dans l'ancien).
    let kept: Vec<(String, String)> = form_urlencoded::parse(query.as_bytes())
        .filter(|(key, value)| !(key == "dossier" && value.parse::<i64>().is_ok()))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    Ok(if kept.is_empty() {
        path.to_string()
    } else {
        format!(
            "{path}?{}",
            form_urlencoded::Serializer::new(String::new())
                .extend_pairs(kept)
                .finish()
        )
    })
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
pub(crate) fn new_slug(name: &str, taken: &[&str]) -> String {
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
pub(crate) fn owner_only(space: &Space) -> AppResult<()> {
    if space.mine {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// Espaces en service : les archivés ne comptent pas dans la limite.
fn in_service(spaces: &[Space]) -> usize {
    spaces.iter().filter(|s| !s.archived).count()
}

/// Crée un espace. Rend son slug et son nom, ou le refus à montrer. Aussi
/// pour l'API. `spaces` : tous ceux du compte, archivés compris.
pub(crate) async fn create_space(
    db: &SqlitePool,
    user_id: i64,
    spaces: &[Space],
    form: &SpaceForm,
) -> AppResult<Result<(String, String), String>> {
    if in_service(spaces) >= MAX_SPACES {
        return Ok(Err(format!(
            "{MAX_SPACES} espaces au plus : supprimes-en un pour en créer un autre."
        )));
    }
    let valid = match validate(form, spaces, None) {
        Ok(valid) => valid,
        Err(message) => return Ok(Err(message)),
    };
    // Les slugs des espaces archivés restent pris (leurs adresses aussi).
    let taken: Vec<&str> = spaces.iter().map(|s| s.slug.as_str()).collect();
    let slug = new_slug(&valid.name, &taken);
    sqlx::query!(
        "INSERT INTO spaces (user_id, slug, name, color, icon, position)
         VALUES (?, ?, ?, ?, ?, (SELECT coalesce(max(position), -1) + 1 FROM spaces WHERE user_id = ?))",
        user_id,
        slug,
        valid.name,
        valid.color,
        valid.icon,
        user_id
    )
    .execute(db)
    .await?;
    Ok(Ok((slug, valid.name)))
}

/// `POST /settings/spaces` : crée un espace et en fait l'espace actif.
pub async fn create(
    ctx: Ctx,
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Form(form): axum::Form<SpaceForm>,
) -> AppResult<Response> {
    match create_space(&state.db, ctx.user.id, &ctx.all_spaces(), &form).await? {
        Ok((slug, name)) => {
            ctx.session.insert(ACTIVE_SPACE, &slug).await?;
            ctx.flash_ok(format!("Espace « {name} » créé")).await?;
            // L'onglet passe sur le nouvel espace (pas seulement la session).
            url::go_to(&headers, BACK, &slug)
        }
        Err(message) => {
            ctx.flash_error(message).await?;
            Ok(see_other(BACK))
        }
    }
}

/// Change le nom, la couleur, l'icône et, si le formulaire les porte, les
/// règles d'un espace (propriétaire seulement). Rend le message à montrer,
/// ou le refus. Aussi pour l'API.
pub(crate) async fn update_space(
    db: &SqlitePool,
    user_id: i64,
    spaces: &[Space],
    space: &Space,
    form: &SpaceForm,
) -> AppResult<Result<String, String>> {
    owner_only(space)?;
    let id = space.id;
    let valid = match validate(form, spaces, Some(id)) {
        Ok(valid) => valid,
        Err(message) => return Ok(Err(message)),
    };
    sqlx::query!(
        "UPDATE spaces SET name = ?, color = ?, icon = ? WHERE id = ? AND user_id = ?",
        valid.name,
        valid.color,
        valid.icon,
        id,
        user_id
    )
    .execute(db)
    .await?;
    if form.rules.is_some() {
        let (mp4, heic) = (form.auto_mp4.is_some(), form.auto_heic_jpeg.is_some());
        sqlx::query!(
            "UPDATE spaces SET auto_mp4 = ?, auto_heic_jpeg = ? WHERE id = ? AND user_id = ?",
            mp4,
            heic,
            id,
            user_id
        )
        .execute(db)
        .await?;
    }
    Ok(Ok(if valid.name == space.name {
        format!("Espace « {} » modifié", valid.name)
    } else {
        format!("« {} » s'appelle maintenant « {} »", space.name, valid.name)
    }))
}

/// `POST /settings/spaces/{id}` : renomme un espace, change sa couleur ou son icône.
pub async fn update(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<SpaceForm>,
) -> AppResult<Response> {
    let space = ctx.space_by_id(id).ok_or(AppError::NotFound)?;
    match update_space(&state.db, ctx.user.id, &ctx.all_spaces(), space, &form).await? {
        Ok(message) => ctx.flash_ok(message).await?,
        Err(message) => ctx.flash_error(message).await?,
    }
    Ok(see_other(BACK))
}

/// Archive un espace (pas Perso, propriétaire seulement). Il quitte le
/// sélecteur, les listes et la vue d'ensemble ; son contenu reste cherchable
/// par la palette. Rend le message à montrer, ou le refus. Aussi pour l'API.
pub(crate) async fn archive_space(
    db: &SqlitePool,
    user_id: i64,
    spaces: &[Space],
    space: &Space,
) -> AppResult<Result<String, String>> {
    owner_only(space)?;
    if space.archived {
        return Ok(Err(format!("{} est déjà archivé.", space.name)));
    }
    if space.is_overview() || in_service(spaces) <= 1 {
        return Ok(Err(format!("{} ne s'archive pas.", space.name)));
    }
    let now = crate::time::now();
    sqlx::query!(
        "UPDATE spaces SET archived_at = ? WHERE id = ? AND user_id = ?",
        now,
        space.id,
        user_id
    )
    .execute(db)
    .await?;
    Ok(Ok(format!(
        "{} archivé : il n'est plus dans le sélecteur, ses notes restent cherchables",
        space.name
    )))
}

/// `POST /settings/spaces/{id}/archive` : archive un espace, qui se rouvre ou
/// se restaure depuis Réglages.
pub async fn archive(
    ctx: Ctx,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let space = ctx
        .spaces
        .iter()
        .find(|s| s.id == id)
        .ok_or(AppError::NotFound)?;
    match archive_space(&state.db, ctx.user.id, &ctx.all_spaces(), space).await? {
        Ok(message) => {
            // L'espace actif part : on revient au premier espace en service.
            if ctx.active.id == id {
                ctx.session.remove::<String>(ACTIVE_SPACE).await?;
            }
            ctx.flash_ok(message).await?;
            back_leaving(&ctx, &headers, id, None)
        }
        Err(message) => {
            ctx.flash_error(message).await?;
            Ok(see_other(BACK))
        }
    }
}

/// Remet un espace archivé en service (propriétaire seulement). Rend le
/// message à montrer, ou le refus. Aussi pour l'API.
pub(crate) async fn unarchive_space(
    db: &SqlitePool,
    user_id: i64,
    spaces: &[Space],
    space: &Space,
) -> AppResult<Result<String, String>> {
    owner_only(space)?;
    if !space.archived {
        return Ok(Err(format!("{} n'est pas archivé.", space.name)));
    }
    if in_service(spaces) >= MAX_SPACES {
        return Ok(Err(format!(
            "{MAX_SPACES} espaces au plus : archive ou supprime-en un pour restaurer {}.",
            space.name
        )));
    }
    sqlx::query!(
        "UPDATE spaces SET archived_at = NULL WHERE id = ? AND user_id = ?",
        space.id,
        user_id
    )
    .execute(db)
    .await?;
    Ok(Ok(format!(
        "{} est de retour dans le sélecteur",
        space.name
    )))
}

#[derive(Deserialize)]
pub struct RestoreForm {
    /// Page d'où vient la demande (le bandeau d'un espace archivé), pour y
    /// revenir ; Réglages sans précision.
    next: Option<String>,
}

/// `POST /settings/spaces/{id}/restore` : remet un espace archivé en service.
pub async fn restore(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<RestoreForm>,
) -> AppResult<Response> {
    let space = ctx
        .archived
        .iter()
        .find(|s| s.id == id)
        .ok_or(AppError::NotFound)?;
    match unarchive_space(&state.db, ctx.user.id, &ctx.all_spaces(), space).await? {
        Ok(message) => ctx.flash_ok(message).await?,
        Err(message) => ctx.flash_error(message).await?,
    }
    Ok(see_other(match form.next.as_deref() {
        Some(next) => local_path(Some(next)),
        None => BACK,
    }))
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

/// Ce que dit une suppression d'espace réussie.
pub(crate) struct Removed {
    pub(crate) message: String,
    /// Espace qui a reçu le contenu : l'espace actif y passe.
    pub(crate) next_active: Option<String>,
}

/// Fait passer tout le contenu d'un espace dans `target`, y compris les
/// envois en cours et les conversions de `user_id`, le propriétaire.
async fn transfer_content(db: &SqlitePool, user_id: i64, id: i64, target: i64) -> AppResult<()> {
    let folders: Vec<crate::folders::Folder> = sqlx::query!(
        r#"SELECT id AS "id!", space_id, parent_id, name FROM folders
                   WHERE space_id = ? OR space_id = ?"#,
        id,
        target
    )
    .fetch_all(db)
    .await?
    .into_iter()
    .map(|row| crate::folders::Folder {
        id: row.id,
        space_id: row.space_id,
        parent_id: row.parent_id,
        name: row.name,
    })
    .collect();
    let mut tx = db.begin().await?;
    // Ses dossiers suivent leur contenu. Un nom déjà pris dans l'espace
    // qui les reçoit devient « Math (2) » ; les sous-dossiers gardent
    // leur dossier parent, donc leur nom.
    let mut taken = crate::folders::neighbours(&folders, target, None, None);
    for folder in folders.iter().filter(|f| f.space_id == id && f.is_top()) {
        let name = crate::folders::free_name(&taken, &folder.name);
        sqlx::query!(
            "UPDATE folders SET space_id = ?, name = ? WHERE id = ?",
            target,
            name,
            folder.id
        )
        .execute(&mut *tx)
        .await?;
        taken.push(name);
    }
    sqlx::query!(
        "UPDATE folders SET space_id = ? WHERE space_id = ?",
        target,
        id
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE notes SET space_id = ? WHERE space_id = ?",
        target,
        id
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE tasks SET space_id = ? WHERE space_id = ?",
        target,
        id
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE calendar_sources SET space_id = ? WHERE space_id = ? AND user_id = ?",
        target,
        id,
        user_id
    )
    .execute(&mut *tx)
    .await?;
    // Événements de dashu : ceux des membres sont déjà revenus dans
    // leur Perso (remove_member) ; ceux du propriétaire suivent.
    sqlx::query!(
        "UPDATE events SET space_id = ? WHERE space_id = ?",
        target,
        id
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE files SET space_id = ? WHERE space_id = ?",
        target,
        id
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE uploads SET space_id = ? WHERE space_id = ? AND user_id = ?",
        target,
        id,
        user_id
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE jobs SET space_id = ? WHERE space_id = ? AND user_id = ?",
        target,
        id,
        user_id
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Supprime un espace (propriétaire seulement, jamais Perso) : `content` vaut
/// `move`, et tout passe dans l'espace `target`, ou `delete`, et tout part
/// avec lui dans la corbeille. Rend ce qu'il faut dire, ou le refus. Aussi
/// pour l'API.
pub(crate) async fn delete_space(
    db: &SqlitePool,
    who: crate::trash::Who<'_>,
    spaces: &[Space],
    space: &Space,
    content: &str,
    target: Option<i64>,
) -> AppResult<Result<Removed, String>> {
    owner_only(space)?;
    if space.is_overview() {
        return Ok(Err(format!(
            "« {} » ne se supprime pas : son agenda réunit ceux de tous les espaces.",
            space.name
        )));
    }
    // Supprimer un espace archivé ne retire rien au sélecteur.
    if !space.archived && in_service(spaces) <= 1 {
        return Ok(Err("Il faut garder au moins un espace.".into()));
    }
    let id = space.id;

    // Les choix sont vérifiés avant de toucher à quoi que ce soit.
    let target = match content {
        "move" => Some(
            target
                .filter(|&target| target != id)
                .and_then(|target| spaces.iter().find(|s| s.id == target))
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
    .fetch_all(db)
    .await?;
    for member in members {
        remove_member(db, id, member).await?;
    }

    Ok(Ok(match target {
        Some(target) => {
            transfer_content(db, who.user, id, target.id).await?;
            // L'espace, vide, part dans la corbeille.
            crate::trash::space(db, who, id).await?;
            Removed {
                message: format!(
                    "Espace « {} » mis dans la corbeille : son contenu est maintenant dans « {} »",
                    space.name, target.name
                ),
                next_active: Some(target.slug.clone()),
            }
        }
        None => {
            // L'espace et tout son contenu partent dans la corbeille : 30 jours
            // pour le restaurer (voir src/trash).
            crate::trash::space(db, who, id).await?;
            Removed {
                message: format!(
                    "Espace « {} » mis dans la corbeille avec son contenu (Réglages › Corbeille)",
                    space.name
                ),
                next_active: None,
            }
        }
    }))
}

/// `POST /settings/spaces/{id}/delete`
pub async fn delete(
    ctx: Ctx,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<DeleteForm>,
) -> AppResult<Response> {
    let space = ctx.space_by_id(id).cloned().ok_or(AppError::NotFound)?;
    let who = crate::trash::Who::site(ctx.user.id);
    let all = ctx.all_spaces();
    let removed =
        match delete_space(&state.db, who, &all, &space, &form.content, form.target).await? {
            Ok(removed) => removed,
            Err(message) => {
                ctx.flash_error(message).await?;
                return Ok(see_other(BACK));
            }
        };

    // L'espace actif vient de disparaître : on passe à celui qui a reçu le
    // contenu, ou au premier.
    if ctx.active.id == id {
        match &removed.next_active {
            Some(slug) => ctx.session.insert(ACTIVE_SPACE, slug).await?,
            None => {
                ctx.session.remove::<String>(ACTIVE_SPACE).await?;
            }
        }
    }
    ctx.flash_ok(removed.message.clone()).await?;
    back_leaving(&ctx, &headers, id, removed.next_active.as_deref())
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

/// Un compte affiché sur la page Membres, ou renvoyé par l'API.
#[derive(Serialize)]
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
/// * ses agendas et ses événements rattachés à l'espace rentrent dans son Perso ;
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
    // Ses événements de dashu aussi : ils sont à lui, comme ses agendas.
    sqlx::query!(
        "UPDATE events
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

/// Qui a accès à un espace.
pub(crate) struct Roster {
    pub(crate) owner: Person,
    pub(crate) members: Vec<Person>,
    /// Invitations en attente : (identifiant de l'invitation, invité). Vide
    /// pour un membre : elles ne regardent que le propriétaire.
    pub(crate) pending: Vec<(i64, Person)>,
}

/// Le propriétaire, les membres et les invitations en attente d'un espace.
/// Perso n'a pas de membres (404). Aussi pour l'API.
pub(crate) async fn roster(db: &SqlitePool, space: &Space) -> AppResult<Roster> {
    if space.is_overview() {
        return Err(AppError::NotFound);
    }
    let id = space.id;
    let owner = sqlx::query!(
        r#"SELECT id AS "id!", email, first_name, last_name FROM users WHERE id = ?"#,
        space.owner_id
    )
    .fetch_one(db)
    .await?;
    let members = sqlx::query!(
        r#"SELECT u.id AS "id!", u.email, u.first_name, u.last_name
           FROM space_members m JOIN users u ON u.id = m.user_id
           WHERE m.space_id = ? ORDER BY m.joined_at, u.id"#,
        id
    )
    .fetch_all(db)
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
        .fetch_all(db)
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
    Ok(Roster {
        owner: Person {
            id: owner.id,
            name: person_name(&owner.first_name, &owner.last_name, &owner.email),
            email: owner.email,
        },
        members,
        pending,
    })
}

/// `GET /settings/spaces/{id}/members` : le propriétaire, les membres et les
/// invitations en attente. Perso n'a pas de page Membres.
pub async fn members_page(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let space = ctx.space_by_id(id).cloned().ok_or(AppError::NotFound)?;
    let Roster {
        owner,
        members,
        pending,
    } = roster(&state.db, &space).await?;
    let page = MembersPage {
        owner,
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

/// Le propriétaire invite un compte existant, par son adresse. Rend le
/// numéro de l'invitation et l'adresse, ou le refus (rien n'est créé). Aussi
/// pour l'API.
pub(crate) async fn invite_to(
    state: &AppState,
    user_id: i64,
    space: &Space,
    raw_email: &str,
) -> AppResult<Result<(i64, String), String>> {
    owner_only(space)?;
    let id = space.id;
    if space.is_overview() {
        return Ok(Err(format!(
            "{} reste privé : il ne se partage pas.",
            space.name
        )));
    }
    if space.archived {
        return Ok(Err(format!(
            "{} est archivé : restaure-le avant d'inviter quelqu'un.",
            space.name
        )));
    }
    let email = raw_email.trim().to_lowercase();
    if email.is_empty() {
        return Ok(Err(
            "Saisis l'adresse e-mail de la personne à inviter.".into()
        ));
    }
    // Un compte, c'est quelqu'un qui s'est déjà connecté une fois ; son
    // adresse doit aussi être encore autorisée.
    let invited = sqlx::query_scalar!(r#"SELECT id AS "id!" FROM users WHERE email = ?"#, email)
        .fetch_optional(&state.db)
        .await?
        .filter(|_| state.config.is_allowed(&email));
    let Some(invited) = invited else {
        return Ok(Err(format!(
            "Aucun compte ne porte l'adresse {email} : la personne doit d'abord se connecter une fois."
        )));
    };
    if invited == user_id {
        return Ok(Err("Tu es déjà le propriétaire de cet espace.".into()));
    }
    let member = sqlx::query_scalar!(
        r#"SELECT user_id AS "user_id!: i64" FROM space_members WHERE space_id = ? AND user_id = ?"#,
        id,
        invited
    )
    .fetch_optional(&state.db)
    .await?;
    if member.is_some() {
        return Ok(Err(format!("{email} est déjà membre de cet espace.")));
    }
    let pending = sqlx::query_scalar!(
        r#"SELECT id AS "id!: i64" FROM space_invitations WHERE space_id = ? AND user_id = ?"#,
        id,
        invited
    )
    .fetch_optional(&state.db)
    .await?;
    if pending.is_some() {
        return Ok(Err(format!("{email} a déjà une invitation en attente.")));
    }

    let now = crate::time::now();
    let invitation = sqlx::query!(
        "INSERT INTO space_invitations (space_id, user_id, invited_by, created_at) VALUES (?, ?, ?, ?)",
        id,
        invited,
        user_id,
        now
    )
    .execute(&state.db)
    .await?
    .last_insert_rowid();
    Ok(Ok((invitation, email)))
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
    match invite_to(&state, ctx.user.id, space, &form.email).await? {
        Ok((_, email)) => {
            ctx.flash_ok(format!("Invitation envoyée à {email}"))
                .await?;
            Ok(see_other(&members_url(id)))
        }
        // Perso n'a pas de page Membres : le message s'affiche dans les réglages.
        Err(message) if space.is_overview() => {
            ctx.flash_error(message).await?;
            Ok(see_other(BACK))
        }
        Err(message) => refuse(&ctx, id, message).await,
    }
}

/// Annule une invitation en attente (propriétaire seulement). Aussi pour l'API.
pub(crate) async fn cancel_invite(db: &SqlitePool, space: &Space, inv: i64) -> AppResult<()> {
    owner_only(space)?;
    let deleted = sqlx::query!(
        "DELETE FROM space_invitations WHERE id = ? AND space_id = ?",
        inv,
        space.id
    )
    .execute(db)
    .await?
    .rows_affected();
    if deleted == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// `POST /settings/spaces/{id}/invitations/{inv}/cancel`
pub async fn cancel_invitation(
    ctx: Ctx,
    State(state): State<AppState>,
    Path((id, inv)): Path<(i64, i64)>,
) -> AppResult<Response> {
    let space = ctx.space_by_id(id).ok_or(AppError::NotFound)?;
    cancel_invite(&state.db, space, inv).await?;
    ctx.flash_ok("Invitation annulée").await?;
    Ok(see_other(&members_url(id)))
}

/// Retire un membre (propriétaire seulement). Aussi pour l'API.
pub(crate) async fn remove_from(db: &SqlitePool, space: &Space, user: i64) -> AppResult<()> {
    owner_only(space)?;
    let member = sqlx::query_scalar!(
        r#"SELECT user_id AS "user_id!: i64" FROM space_members WHERE space_id = ? AND user_id = ?"#,
        space.id,
        user
    )
    .fetch_optional(db)
    .await?;
    if member.is_none() {
        return Err(AppError::NotFound);
    }
    remove_member(db, space.id, user).await
}

/// `POST /settings/spaces/{id}/members/{user}/remove`
pub async fn remove(
    ctx: Ctx,
    State(state): State<AppState>,
    Path((id, user)): Path<(i64, i64)>,
) -> AppResult<Response> {
    let space = ctx.space_by_id(id).ok_or(AppError::NotFound)?;
    remove_from(&state.db, space, user).await?;
    ctx.flash_ok("Membre retiré : plus d'accès à l'espace")
        .await?;
    Ok(see_other(&members_url(id)))
}

/// Un membre quitte l'espace. Le propriétaire, lui, le supprime ou
/// l'archive. Aussi pour l'API.
pub(crate) async fn leave_space(db: &SqlitePool, user_id: i64, space: &Space) -> AppResult<()> {
    if space.mine {
        return Err(AppError::Forbidden);
    }
    remove_member(db, space.id, user_id).await
}

/// `POST /settings/spaces/{id}/leave`
pub async fn leave(
    ctx: Ctx,
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let space = ctx.space_by_id(id).ok_or(AppError::NotFound)?;
    leave_space(&state.db, ctx.user.id, space).await?;
    if ctx.active.id == id {
        ctx.session.remove::<String>(ACTIVE_SPACE).await?;
    }
    ctx.flash_ok(format!("Tu as quitté « {} »", space.name))
        .await?;
    back_leaving(&ctx, &headers, id, None)
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

/// L'invité accepte : l'espace rejoint prend une place dans son île. Rend
/// l'identifiant et le nom de l'espace, ou le refus. Aussi pour l'API.
pub(crate) async fn accept_for(
    db: &SqlitePool,
    user_id: i64,
    spaces: &[Space],
    inv: i64,
) -> AppResult<Result<(i64, String), String>> {
    let invitation = sqlx::query!(
        r#"SELECT i.space_id, s.slug, s.name FROM space_invitations i
           JOIN spaces s ON s.id = i.space_id
           WHERE i.id = ? AND i.user_id = ?"#,
        inv,
        user_id
    )
    .fetch_optional(db)
    .await?
    .ok_or(AppError::NotFound)?;
    if in_service(spaces) >= MAX_SPACES {
        return Ok(Err(format!(
            "{MAX_SPACES} espaces au plus : archive ou supprime-en un pour rejoindre « {} ».",
            invitation.name
        )));
    }

    let mut tx = db.begin().await?;
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
    Ok(Ok((invitation.space_id, invitation.name)))
}

/// `POST /settings/invitations/{inv}/accept` : l'invité seulement.
pub async fn accept_invitation(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(inv): Path<i64>,
) -> AppResult<Response> {
    match accept_for(&state.db, ctx.user.id, &ctx.all_spaces(), inv).await? {
        Ok((_, name)) => ctx.flash_ok(format!("Tu as rejoint « {name} »")).await?,
        Err(message) => ctx.flash_error(message).await?,
    }
    Ok(see_other(BACK))
}

/// L'invité refuse (404 pour l'invitation d'un autre compte). Aussi pour l'API.
pub(crate) async fn decline_for(db: &SqlitePool, user_id: i64, inv: i64) -> AppResult<()> {
    let deleted = sqlx::query!(
        "DELETE FROM space_invitations WHERE id = ? AND user_id = ?",
        inv,
        user_id
    )
    .execute(db)
    .await?
    .rows_affected();
    if deleted == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// `POST /settings/invitations/{inv}/decline` : l'invité seulement.
pub async fn decline_invitation(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(inv): Path<i64>,
) -> AppResult<Response> {
    decline_for(&state.db, ctx.user.id, inv).await?;
    ctx.flash_ok("Invitation refusée").await?;
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
