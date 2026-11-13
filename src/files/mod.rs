//! Fichiers (v2) : envoi, liste, téléchargement et suppression, par espace.
//!
//! * Un fichier envoyé est une donnée, jamais une page : il est servi en
//!   téléchargement, avec une CSP `sandbox` (voir [`download`]).
//! * Sur le disque, il ne porte jamais le nom choisi par l'utilisateur, mais
//!   une clé aléatoire : `<FILES_DIR>/<2 premiers caractères>/<clé>`. Un nom
//!   piégé (`../../etc/passwd`) ne peut donc désigner aucun autre fichier.
//! * L'envoi arrive par morceaux de 50 Mo au plus (limite de Cloudflare),
//!   chacun écrit sur le disque au fil de l'eau : la mémoire de dashu ne
//!   dépend pas de la taille du fichier.
//! * Toutes les requêtes filtrent sur `user_id`, comme les notes.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Context as _;
use askama::Template;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tokio::io::AsyncWriteExt;

use crate::AppState;
use crate::auth::session::{Ctx, Shell};
use crate::config::FilesConfig;
use crate::spaces::Space;
use crate::time;
use crate::web::{self, AppError, AppResult, see_other};

/// Dossier des envois en cours, à l'intérieur de `FILES_DIR` : même disque,
/// donc un simple renommage met le fichier à sa place une fois reçu.
const INCOMING: &str = ".envois";
const NAME_MAX_CHARS: usize = 200;
/// CSP d'un fichier servi : rien ne s'exécute, même si le navigateur
/// décidait d'afficher le fichier au lieu de le télécharger.
const FILE_CSP: &str = "default-src 'none'; sandbox";

// ---------------------------------------------------------------------
// Liste
// ---------------------------------------------------------------------
//
// L'onglet Fichiers est une arborescence (SPEC, « Dossiers ») : à la racine,
// un dossier par espace et Temporaire ; dans un espace, ses dossiers, ses
// notes et ses fichiers ; dans un dossier, ses sous-dossiers et son contenu.

pub struct FileItem {
    pub id: i64,
    pub name: String,
    pub size: String,
    /// Libellé court du type : « PDF », « Image »…
    pub kind: &'static str,
    pub updated: String,
    /// Menu « Convertir » de la ligne : (valeur, libellé), vide sans outils.
    /// Sans l'extraction de pages, qui demande une saisie (page d'aperçu).
    pub conversions: Vec<(&'static str, &'static str)>,
    /// Vignette prête : affichée à gauche du nom, sinon `icon`.
    pub thumb: bool,
    pub icon: &'static str,
    /// Adresse de la fenêtre « Ranger » de la ligne.
    pub picker: String,
    /// Temporaire : « effacé dans 5 h ».
    pub expires: Option<String>,
}

/// Un endroit où lâcher ce qu'on glisse (onglet Fichiers) : un espace et,
/// sans dossier, sa racine.
pub struct DropAt {
    pub space: i64,
    pub folder: Option<i64>,
}

/// Une ligne « dossier » : le dossier d'un espace ou Temporaire (à la
/// racine), ou un dossier.
pub struct FolderRow {
    /// Dossier ; `None` pour le dossier d'un espace et pour Temporaire, qui ne
    /// se renomment ni ne se suppriment ici.
    pub id: Option<i64>,
    pub name: String,
    pub href: String,
    /// « 2 dossiers, 3 notes et 1 fichier »
    pub summary: String,
    pub empty: bool,
    pub icon: &'static str,
    /// Couleur de l'espace, pour le dossier d'un espace.
    pub color: Option<String>,
    /// Où va son contenu si on le supprime en le transférant : « École ».
    pub parent_label: String,
    /// Ce qu'on y lâche y est rangé (pas Temporaire).
    pub drop: Option<DropAt>,
    /// Fenêtre « Ranger » du dossier (pas pour le dossier d'un espace).
    pub picker: Option<String>,
}

/// Une note rangée dans le dossier montré.
pub struct NoteRow {
    pub id: i64,
    pub title: String,
    pub pinned: bool,
    pub updated: String,
    pub picker: String,
}

/// Un pas du fil d'Ariane ; le dernier (l'endroit montré) n'a pas de lien.
pub struct Crumb {
    pub label: String,
    pub href: Option<String>,
    /// Un pas du fil d'Ariane reçoit aussi ce qu'on glisse (pas « Fichiers »).
    pub drop: Option<DropAt>,
}

#[derive(Template)]
#[template(path = "files/list.html")]
struct ListPage {
    shell: Shell,
    /// Faux si `FILES_DIR` n'est pas configuré.
    enabled: bool,
    /// Taille maximale d'un fichier, lisible (« 2 Go ») et en octets (pour
    /// refuser tout de suite, dans le navigateur, un fichier trop gros).
    max_size: String,
    max_bytes: u64,
    /// Ce que montre la page : `racine`, `espace`, `dossier` ou `temporaire`.
    view: &'static str,
    crumbs: Vec<Crumb>,
    /// Adresse de la vue : retour après une action, rechargement de la liste.
    here: String,
    /// L'endroit montré, en entier : « École / Math ».
    place: String,
    folders: Vec<FolderRow>,
    notes: Vec<NoteRow>,
    items: Vec<FileItem>,
    /// Compressions en cours ou récentes (vide si les outils sont désactivés).
    jobs: crate::tools::JobsPanel,
    /// Espaces proposés pour un envoi (à la racine et dans un espace).
    spaces: Vec<Space>,
    /// Espace d'arrivée d'un envoi (coché par défaut) et, dans un dossier,
    /// son dossier : l'envoi y arrive, sans choix d'espace.
    upload_space: i64,
    upload_folder: Option<i64>,
    /// Création d'un dossier ici : (espace, dossier parent). Rien à la racine,
    /// dans Temporaire et dans un sous-dossier (trois niveaux au plus).
    new_folder: Option<(i64, Option<i64>)>,
    /// « Nouvelle note » : l'adresse qui la crée ici (dossier d'un espace et
    /// dossiers seulement).
    new_note: Option<String>,
}

#[derive(Deserialize)]
pub struct ListQuery {
    /// `racine`, `espace`, `temporaire` ou le numéro d'un dossier.
    dossier: Option<String>,
    /// Ancien filtre « Seulement Perso » : le dossier de l'espace.
    #[serde(default)]
    seul: u8,
}

/// Endroit montré par l'onglet Fichiers.
enum View {
    Root,
    Space,
    Folder(crate::folders::Folder),
    Temp,
}

/// `GET /files?dossier=…` : un endroit de l'arborescence. Sans précision, le
/// dossier de l'espace actif ; depuis Perso, la racine (tous les espaces).
pub async fn list(
    ctx: Ctx,
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> AppResult<Response> {
    use crate::folders::{self, view_url};

    let user = ctx.user.id;
    let all = folders::all(&state.db, user).await?;
    let view = match query.dossier.as_deref() {
        Some("racine") => View::Root,
        Some("espace") => View::Space,
        Some("temporaire") => View::Temp,
        Some(raw) => {
            let folder = raw
                .parse::<i64>()
                .ok()
                .and_then(|id| all.iter().find(|f| f.id == id))
                .ok_or(AppError::NotFound)?;
            // Dossier d'un autre espace : il s'ouvre dans le sien.
            if folder.space_id != ctx.active.id {
                let space = ctx.space_by_id(folder.space_id).ok_or(AppError::NotFound)?;
                return Ok(see_other(&view_url(&space.slug, Some(folder.id))));
            }
            View::Folder(folder.clone())
        }
        None if query.seul == 1 || !ctx.active.is_overview() => View::Space,
        None => View::Root,
    };

    let space = &ctx.active;
    let now = time::now();
    let tools = state.config.work_dir.is_some();
    let root_crumb = Crumb {
        label: "Fichiers".into(),
        href: Some("/files?dossier=racine".into()),
        drop: None,
    };
    // Dossier montré (`None` : la racine de l'espace), pour les vues d'un espace.
    let (name, here, crumbs, inside) = match &view {
        View::Root => (
            "racine",
            "/files?dossier=racine".to_string(),
            vec![Crumb {
                href: None,
                ..root_crumb
            }],
            None,
        ),
        View::Temp => (
            "temporaire",
            "/files?dossier=temporaire".to_string(),
            vec![
                root_crumb,
                Crumb {
                    label: "Temporaire".into(),
                    href: None,
                    drop: None,
                },
            ],
            None,
        ),
        View::Space | View::Folder(_) => {
            let folder = match &view {
                View::Folder(f) => Some(f.id),
                _ => None,
            };
            let mut crumbs = vec![
                root_crumb,
                Crumb {
                    label: space.name.clone(),
                    href: Some(view_url(&space.slug, None)),
                    drop: Some(DropAt {
                        space: space.id,
                        folder: None,
                    }),
                },
            ];
            crumbs.extend(folders::chain(&all, folder).into_iter().map(|f| Crumb {
                label: f.name.clone(),
                href: Some(view_url(&space.slug, Some(f.id))),
                drop: Some(DropAt {
                    space: space.id,
                    folder: Some(f.id),
                }),
            }));
            if let Some(last) = crumbs.last_mut() {
                last.href = None;
            }
            let name = if folder.is_some() {
                "dossier"
            } else {
                "espace"
            };
            (name, view_url(&space.slug, folder), crumbs, Some(folder))
        }
    };

    let contents = folders::contents(&state.db, user, &all).await?;
    let folder_rows = match (&view, inside) {
        (View::Root, _) => root_rows(&ctx, &state.db, &all).await?,
        (_, Some(parent)) => all
            .iter()
            .filter(|f| f.space_id == space.id && f.parent_id == parent)
            .map(|f| {
                let content = contents.get(&f.id).copied().unwrap_or_default();
                FolderRow {
                    id: Some(f.id),
                    name: f.name.clone(),
                    href: view_url(&space.slug, Some(f.id)),
                    summary: content.describe(),
                    empty: content.is_empty(),
                    icon: "folder",
                    color: None,
                    parent_label: folders::label(&space.name, &all, f.parent_id),
                    drop: Some(DropAt {
                        space: space.id,
                        folder: Some(f.id),
                    }),
                    picker: Some(folders::picker_url("dossier", f.id, Some(&here))),
                }
            })
            .collect(),
        _ => Vec::new(),
    };

    let notes = match inside {
        Some(folder) => sqlx::query!(
            r#"SELECT id AS "id!", title, pinned AS "pinned!: bool", updated_at
               FROM notes WHERE user_id = ?1 AND space_id = ?2 AND folder_id IS ?3
               ORDER BY pinned DESC, updated_at DESC, id DESC"#,
            user,
            space.id,
            folder
        )
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .map(|row| NoteRow {
            id: row.id,
            title: if row.title.trim().is_empty() {
                "Sans titre".into()
            } else {
                row.title
            },
            pinned: row.pinned,
            updated: time::ago(row.updated_at, now),
            picker: folders::picker_url("note", row.id, Some(&here)),
        })
        .collect(),
        None => Vec::new(),
    };

    let temp = matches!(view, View::Temp);
    let items = if matches!(view, View::Root) {
        Vec::new()
    } else {
        let folder = inside.flatten();
        sqlx::query!(
            r#"SELECT id AS "id!", name, mime, size, updated_at, thumb, expires_at
               FROM files
               WHERE user_id = ?1
                 AND CASE WHEN ?2 THEN expires_at IS NOT NULL
                          ELSE expires_at IS NULL AND space_id = ?3 AND folder_id IS ?4 END
               ORDER BY updated_at DESC, id DESC"#,
            user,
            temp,
            space.id,
            folder
        )
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .map(|row| FileItem {
            id: row.id,
            size: human_size(row.size),
            kind: kind_label(&row.mime),
            conversions: menu(tools, &row.mime, false),
            thumb: row.thumb == 1,
            icon: icon_for(&row.mime),
            picker: folders::picker_url("fichier", row.id, Some(&here)),
            expires: row
                .expires_at
                .map(|at| format!("effacé dans {}", crate::tools::time_left(at, now))),
            name: row.name,
            updated: time::ago(row.updated_at, now),
        })
        .collect()
    };

    let place = match &view {
        View::Root => "Fichiers".to_string(),
        View::Temp => "Temporaire".to_string(),
        View::Space | View::Folder(_) => folders::label(&space.name, &all, inside.flatten()),
    };
    let new_folder = match &view {
        View::Space => Some((space.id, None)),
        View::Folder(f) if f.is_top() => Some((space.id, Some(f.id))),
        _ => None,
    };
    let new_note = match &view {
        View::Space => Some("/notes".to_string()),
        View::Folder(f) => Some(format!("/notes?dossier={}", f.id)),
        _ => None,
    };
    let files = state.config.files.as_ref();
    let page = ListPage {
        shell: ctx.shell("files").await?,
        enabled: files.is_some(),
        max_size: files
            .map(|f| human_size(i64::try_from(f.max_upload_bytes).unwrap_or(i64::MAX)))
            .unwrap_or_default(),
        max_bytes: files.map(|f| f.max_upload_bytes).unwrap_or_default(),
        view: name,
        crumbs,
        here,
        place,
        folders: folder_rows,
        notes,
        items,
        // Le suivi des conversions : celles de l'espace montré (de tous les
        // espaces dans Perso, à la racine et dans Temporaire).
        jobs: crate::tools::panel(&state, user, inside.and(ctx.scope(false))).await?,
        spaces: ctx.spaces.clone(),
        upload_space: space.id,
        upload_folder: match &view {
            View::Folder(f) => Some(f.id),
            _ => None,
        },
        new_folder,
        new_note,
    };
    Ok(web::html(page))
}

/// La racine de l'onglet Fichiers : le dossier de chaque espace (pas les
/// archivés), puis Temporaire.
async fn root_rows(
    ctx: &Ctx,
    db: &SqlitePool,
    all: &[crate::folders::Folder],
) -> AppResult<Vec<FolderRow>> {
    use crate::folders::{Contents, view_url};
    let user = ctx.user.id;
    let mut counts: std::collections::HashMap<i64, Contents> = std::collections::HashMap::new();
    for row in sqlx::query!(
        r#"SELECT space_id, count(*) AS "n!: i64" FROM notes WHERE user_id = ? GROUP BY space_id"#,
        user
    )
    .fetch_all(db)
    .await?
    {
        counts.entry(row.space_id).or_default().notes = row.n;
    }
    for row in sqlx::query!(
        r#"SELECT space_id, count(*) AS "n!: i64" FROM files
           WHERE user_id = ? AND expires_at IS NULL GROUP BY space_id"#,
        user
    )
    .fetch_all(db)
    .await?
    {
        counts.entry(row.space_id).or_default().files = row.n;
    }
    for folder in all.iter().filter(|f| f.is_top()) {
        counts.entry(folder.space_id).or_default().folders += 1;
    }

    let mut rows: Vec<FolderRow> = ctx
        .spaces
        .iter()
        .map(|space| {
            let content = counts.get(&space.id).copied().unwrap_or_default();
            FolderRow {
                id: None,
                name: space.name.clone(),
                href: view_url(&space.slug, None),
                summary: content.describe(),
                empty: content.is_empty(),
                icon: "folder",
                color: Some(space.color.clone()),
                parent_label: String::new(),
                drop: Some(DropAt {
                    space: space.id,
                    folder: None,
                }),
                picker: None,
            }
        })
        .collect();
    let waiting = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!: i64" FROM files WHERE user_id = ? AND expires_at IS NOT NULL"#,
        user
    )
    .fetch_one(db)
    .await?;
    rows.push(FolderRow {
        id: None,
        name: "Temporaire".into(),
        href: "/files?dossier=temporaire".into(),
        summary: if waiting == 0 {
            "Vide · les fichiers de l'onglet Outils pas encore rangés".into()
        } else {
            format!(
                "{} · effacés 24 heures après leur dépôt",
                Contents {
                    files: waiting,
                    ..Contents::default()
                }
                .describe()
            )
        },
        empty: waiting == 0,
        icon: "clock",
        color: None,
        parent_label: String::new(),
        drop: None,
        picker: None,
    });
    Ok(rows)
}

/// Menu « Convertir » d'un fichier : vide sans outils ; avec ou sans
/// l'extraction de pages d'un PDF (qui demande une saisie).
fn menu(tools: bool, mime: &str, with_pages: bool) -> Vec<(&'static str, &'static str)> {
    if !tools {
        return Vec::new();
    }
    crate::tools::conversions_for(mime)
        .into_iter()
        .filter(|c| with_pages || *c != crate::tools::Conversion::Pages)
        .map(|c| (c.key(), c.label()))
        .collect()
}

#[derive(Deserialize)]
pub struct KeepForm {
    pub(crate) space_id: i64,
    /// Dossier de cet espace où le ranger ; vide : sa racine.
    pub(crate) folder_id: Option<String>,
    /// Page où revenir ; l'onglet Outils sans précision.
    pub(crate) next: Option<String>,
}

/// `POST /files/{id}/keep` : garde dans un espace (et un dossier) un fichier
/// de l'onglet Outils. Il rejoint la liste de cet espace et n'expire plus.
pub async fn keep(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<KeepForm>,
) -> AppResult<Response> {
    let target = ctx.space_by_id(form.space_id).ok_or(AppError::NotFound)?;
    let folder = crate::folders::parse_id(form.folder_id.as_deref())?;
    let folder = crate::folders::target(&state.db, ctx.user.id, target.id, folder).await?;
    let kept = sqlx::query!(
        "UPDATE files SET space_id = ?, folder_id = ?, expires_at = NULL
         WHERE id = ? AND user_id = ? AND expires_at IS NOT NULL",
        target.id,
        folder,
        id,
        ctx.user.id
    )
    .execute(&state.db)
    .await?
    .rows_affected();
    if kept == 0 {
        return Err(AppError::NotFound);
    }
    let place = place_label(&ctx, &state.db, target, folder).await?;
    ctx.flash_ok(format!("Fichier gardé dans {place}")).await?;
    Ok(see_other(match form.next.as_deref() {
        Some(next) => web::local_path(Some(next)),
        None => "/tools",
    }))
}

/// « École » ou « École / Math » : l'endroit où un élément vient d'être rangé.
pub(crate) async fn place_label(
    ctx: &Ctx,
    db: &SqlitePool,
    space: &Space,
    folder: Option<i64>,
) -> AppResult<String> {
    Ok(match folder {
        Some(_) => {
            let all = crate::folders::all(db, ctx.user.id).await?;
            crate::folders::label(&space.name, &all, folder)
        }
        None => space.name.clone(),
    })
}

#[derive(Deserialize)]
pub struct MoveForm {
    pub(crate) space_id: i64,
    /// Dossier de cet espace où le ranger ; vide : sa racine.
    pub(crate) folder_id: Option<String>,
    /// Page où revenir : la liste, ou l'aperçu du fichier.
    pub(crate) next: Option<String>,
}

/// `POST /files/{id}/move` : range un fichier dans un espace et un dossier
/// (la racine de l'espace sans dossier). Sa vignette et son grand aperçu le
/// suivent (rangés à côté de lui, sous sa clé, ils ne dépendent ni de
/// l'espace ni du dossier).
pub async fn move_to_space(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<MoveForm>,
) -> AppResult<Response> {
    // L'espace cible doit appartenir à l'utilisateur, le dossier à cet espace.
    let target = ctx.space_by_id(form.space_id).ok_or(AppError::NotFound)?;
    let folder = crate::folders::parse_id(form.folder_id.as_deref())?;
    let folder = crate::folders::target(&state.db, ctx.user.id, target.id, folder).await?;
    let moved = sqlx::query!(
        "UPDATE files SET space_id = ?, folder_id = ? WHERE id = ? AND user_id = ?",
        target.id,
        folder,
        id,
        ctx.user.id
    )
    .execute(&state.db)
    .await?
    .rows_affected();
    if moved == 0 {
        return Err(AppError::NotFound);
    }
    let message = match folder {
        Some(_) => format!(
            "Fichier rangé dans {}",
            place_label(&ctx, &state.db, target, folder).await?
        ),
        None => format!("Fichier déplacé vers {}", target.name),
    };
    ctx.flash_ok(message).await?;
    Ok(see_other(match form.next.as_deref() {
        Some(next) => web::local_path(Some(next)),
        None => "/files",
    }))
}

// ---------------------------------------------------------------------
// Envoi, par morceaux
// ---------------------------------------------------------------------
//
// Un fichier part en morceaux de 50 Mo au plus (static/js/files.js) :
// Cloudflare refuse toute requête de plus de 100 Mo, et un morceau raté se
// renvoie seul. Le déroulé :
//   1. `POST /files/uploads?name=…&size=…` ouvre un envoi et renvoie son
//      identifiant ; la taille totale est vérifiée tout de suite ;
//   2. `PUT /files/uploads/{id}?offset=…` ajoute un morceau, dans l'ordre ;
//   3. au dernier morceau, le fichier rejoint la liste (réponse 201).
// Un envoi en cours survit à un redémarrage de dashu ; abandonné, il est
// effacé au bout de 24 heures (`cleanup_stale`).

/// Taille maximale d'un morceau, côté serveur. Le navigateur envoie des
/// morceaux de 50 Mo, sous la limite de Cloudflare (100 Mo par requête).
pub const CHUNK_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Un envoi sans nouvelles depuis plus longtemps est abandonné.
const STALE_UPLOAD_SECONDS: i64 = 24 * 3600;
/// En-tête qui donne au navigateur le nombre d'octets déjà reçus.
pub const RECEIVED_HEADER: &str = "x-upload-received";
/// En-tête de la réponse au dernier morceau : l'identifiant du fichier.
pub const FILE_ID_HEADER: &str = "x-file-id";

/// Envois dont un morceau est en train d'arriver : un seul à la fois par
/// envoi, sinon deux morceaux pourraient s'écrire au même endroit.
pub type Busy = Arc<Mutex<HashSet<String>>>;

struct BusyGuard {
    set: Busy,
    id: String,
}

impl BusyGuard {
    fn take(set: &Busy, id: &str) -> Option<Self> {
        let mut busy = set.lock().unwrap_or_else(|e| e.into_inner());
        busy.insert(id.to_string()).then(|| Self {
            set: set.clone(),
            id: id.to_string(),
        })
    }
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.set
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}

fn too_large(max: u64) -> AppError {
    AppError::TooLarge(format!(
        "Fichier trop volumineux : {} au plus.",
        human_size(i64::try_from(max).unwrap_or(i64::MAX))
    ))
}

#[derive(Deserialize)]
pub struct StartQuery {
    name: String,
    size: u64,
    /// Espace du fichier ; l'espace actif sans précision.
    space: Option<i64>,
    /// Dossier du fichier (qui donne son espace) ; la racine sans précision.
    folder: Option<i64>,
    /// `1` : fichier de l'onglet Outils, pas encore rangé.
    #[serde(default)]
    tools: u8,
}

/// `POST /files/uploads?name=…&size=…&space=…&folder=…` : ouvre un envoi
/// dans l'espace et le dossier choisis (la racine de l'espace actif par
/// défaut) et renvoie son identifiant.
pub async fn start_upload(
    ctx: Ctx,
    State(state): State<AppState>,
    Query(query): Query<StartQuery>,
) -> AppResult<Response> {
    let files = state.config.files.as_ref().ok_or(AppError::NotFound)?;
    // L'espace et le dossier choisis doivent appartenir à l'utilisateur, et
    // le dossier à cet espace. Un fichier de l'onglet Outils n'a pas de dossier.
    let (space_id, folder_id) = match query.folder.filter(|_| query.tools != 1) {
        Some(id) => {
            let folder = crate::folders::find(&state.db, ctx.user.id, id)
                .await?
                .ok_or(AppError::NotFound)?;
            if query.space.is_some_and(|space| space != folder.space_id) {
                return Err(AppError::NotFound);
            }
            (folder.space_id, Some(folder.id))
        }
        None => match query.space {
            Some(id) => (ctx.space_by_id(id).ok_or(AppError::NotFound)?.id, None),
            None => (ctx.active.id, None),
        },
    };
    if query.size > files.max_upload_bytes {
        return Err(too_large(files.max_upload_bytes));
    }
    let size = i64::try_from(query.size).map_err(|_| too_large(files.max_upload_bytes))?;
    let name = clean_name(&query.name);

    let id = web::csrf::random_token();
    let part = part_path(&files.dir, &id);
    if let Some(parent) = part.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("dossier des envois {}", parent.display()))?;
    }
    tokio::fs::File::create(&part)
        .await
        .with_context(|| format!("création de {}", part.display()))?;
    let now = time::now();
    let for_tools = i64::from(query.tools == 1);
    sqlx::query!(
        "INSERT INTO uploads (id, user_id, space_id, folder_id, name, size, received, for_tools, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?, ?)",
        id,
        ctx.user.id,
        space_id,
        folder_id,
        name,
        size,
        for_tools,
        now,
        now
    )
    .execute(&state.db)
    .await?;
    Ok((StatusCode::CREATED, id).into_response())
}

#[derive(Deserialize)]
pub struct ChunkQuery {
    offset: u64,
}

/// `PUT /files/uploads/{id}?offset=…` : ajoute un morceau. `offset` doit être
/// le nombre d'octets déjà reçus ; sinon la réponse 409 donne le bon dans
/// l'en-tête `x-upload-received`, et le navigateur reprend de là.
pub async fn upload_chunk(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ChunkQuery>,
    body: Body,
) -> AppResult<Response> {
    let files = state.config.files.as_ref().ok_or(AppError::NotFound)?;
    if !is_token(&id) {
        return Err(AppError::NotFound);
    }
    let Some(_busy) = BusyGuard::take(&state.uploads_busy, &id) else {
        return Err(AppError::BadRequest(
            "Un morceau de ce fichier est déjà en cours d'envoi.".into(),
        ));
    };
    let upload = sqlx::query!(
        "SELECT space_id, folder_id, name, size, received, for_tools FROM uploads WHERE id = ? AND user_id = ?",
        id,
        ctx.user.id
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;
    let (size, received) = (upload.size as u64, upload.received as u64);
    if query.offset != received {
        return Ok(out_of_order(received));
    }

    let part = part_path(&files.dir, &id);
    let Ok(mut out) = tokio::fs::OpenOptions::new().append(true).open(&part).await else {
        // Fichier partiel disparu : l'envoi ne peut pas reprendre.
        sqlx::query!("DELETE FROM uploads WHERE id = ?", id)
            .execute(&state.db)
            .await?;
        return Err(AppError::BadRequest("Envoi perdu : recommence-le.".into()));
    };
    // Coupure entre l'écriture d'un morceau et l'enregistrement en base : on
    // revient à ce que la base connaît.
    if out.metadata().await?.len() != received {
        out.set_len(received).await?;
    }

    let room = (size - received).min(CHUNK_MAX_BYTES);
    let written = match append(body, &mut out, room).await {
        Ok(written) => written,
        Err(error) => {
            let _ = out.set_len(received).await;
            return Err(match error {
                Receive::TooLarge if room < CHUNK_MAX_BYTES => {
                    AppError::TooLarge("Le fichier est plus gros qu'annoncé.".into())
                }
                Receive::TooLarge => AppError::TooLarge("Morceau trop gros.".into()),
                Receive::Interrupted => AppError::BadRequest("Envoi interrompu.".into()),
                Receive::Disk(error) => {
                    AppError::Internal(anyhow::Error::new(error).context("écriture d'un envoi"))
                }
            });
        }
    };
    out.sync_data().await?;
    drop(out);

    let received = received + written;
    if received < size {
        let (received_db, now) = (received as i64, time::now());
        sqlx::query!(
            "UPDATE uploads SET received = ?, updated_at = ? WHERE id = ?",
            received_db,
            now,
            id
        )
        .execute(&state.db)
        .await?;
        return Ok((
            StatusCode::OK,
            [(RECEIVED_HEADER, received.to_string())],
            "Morceau reçu",
        )
            .into_response());
    }

    let file_id = finish(
        &state,
        files,
        &id,
        (upload.space_id, upload.folder_id),
        &upload.name,
        ctx.user.id,
        upload.for_tools == 1,
    )
    .await?;
    // L'identifiant du fichier : l'éditeur de notes s'en sert pour insérer
    // une image collée.
    Ok((
        StatusCode::CREATED,
        [(FILE_ID_HEADER, file_id.to_string())],
        "Fichier reçu",
    )
        .into_response())
}

/// Dernier morceau reçu : le fichier partiel prend sa place définitive et
/// rejoint la liste.
async fn finish(
    state: &AppState,
    files: &FilesConfig,
    id: &str,
    (space_id, folder_id): (i64, Option<i64>),
    name: &str,
    user_id: i64,
    for_tools: bool,
) -> AppResult<i64> {
    let part = part_path(&files.dir, id);
    let sha256 = hash_file(part.clone())
        .await
        .context("empreinte d'un fichier reçu")?;
    let size = tokio::fs::metadata(&part).await?.len() as i64;

    let key = web::csrf::random_token();
    let dest = stored_path(&files.dir, &key);
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::rename(&part, &dest)
        .await
        .context("mise en place d'un fichier reçu")?;

    let mime = mime_for(name);
    let now = time::now();
    // Onglet Outils : pas encore rangé, effacé au bout de 24 heures.
    let expires_at = for_tools.then_some(now + crate::tools::TOOLS_KEEP_SECONDS);
    let saved = async {
        let mut tx = state.db.begin().await?;
        let file_id = sqlx::query!(
            "INSERT INTO files (user_id, space_id, folder_id, name, mime, size, sha256, storage_key, expires_at, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            user_id,
            space_id,
            folder_id,
            name,
            mime,
            size,
            sha256,
            key,
            expires_at,
            now,
            now
        )
        .execute(&mut *tx)
        .await?
        .last_insert_rowid();
        sqlx::query!("DELETE FROM uploads WHERE id = ?", id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await.map(|()| file_id)
    }
    .await;
    match saved {
        Ok(file_id) => {
            // La vignette se fait ensuite, sans faire attendre l'envoi.
            if let Err(error) = crate::tools::enqueue_thumbnail(state, file_id).await {
                tracing::warn!(error = format!("{error:#}"), "demande de vignette");
            }
            // Règles de l'espace (vidéo en MP4, HEIC doublée d'un JPEG). Un
            // fichier de l'onglet Outils n'est encore dans aucun espace.
            if !for_tools
                && let Err(error) = crate::tools::apply_rules(state, user_id, file_id).await
            {
                tracing::warn!(error = format!("{error:?}"), "règles de l'espace");
            }
            Ok(file_id)
        }
        Err(error) => {
            let _ = tokio::fs::remove_file(&dest).await;
            Err(error.into())
        }
    }
}

/// Réponse 409 : le morceau n'est pas celui attendu. Le corps est du HTML
/// (comme un fragment htmx) pour que l'en-tête arrive tel quel au navigateur.
fn out_of_order(received: u64) -> Response {
    (
        StatusCode::CONFLICT,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
            (
                header::HeaderName::from_static(RECEIVED_HEADER),
                received.to_string(),
            ),
        ],
        r#"<p class="frag-error" role="alert">Morceau inattendu : reprise de l'envoi.</p>"#,
    )
        .into_response()
}

enum Receive {
    TooLarge,
    Interrupted,
    Disk(std::io::Error),
}

impl From<std::io::Error> for Receive {
    fn from(error: std::io::Error) -> Self {
        Self::Disk(error)
    }
}

/// Ajoute le corps de la requête à `out`, au fil de l'eau, `room` octets au
/// plus. Renvoie le nombre d'octets écrits.
async fn append(mut body: Body, out: &mut tokio::fs::File, room: u64) -> Result<u64, Receive> {
    let mut written: u64 = 0;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| Receive::Interrupted)?;
        let Ok(data) = frame.into_data() else {
            continue;
        };
        written += data.len() as u64;
        if written > room {
            return Err(Receive::TooLarge);
        }
        out.write_all(&data).await?;
    }
    out.flush().await?;
    Ok(written)
}

/// Empreinte SHA-256 d'un fichier, lu par blocs de 1 Mo hors du fil
/// asynchrone (un fichier de 2 Go prend quelques secondes).
pub(crate) async fn hash_file(path: PathBuf) -> std::io::Result<String> {
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let mut file = std::fs::File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 1 << 20];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        Ok(to_hex(&hasher.finalize()))
    })
    .await
    .map_err(std::io::Error::other)?
}

/// Efface les envois abandonnés (24 heures sans nouvelles) et les fichiers
/// partiels qu'aucun envoi ne référence plus. Ceux de moins d'une heure sont
/// laissés : un envoi peut être en train de s'ouvrir.
pub async fn cleanup_stale(db: &SqlitePool, config: &FilesConfig) -> anyhow::Result<()> {
    // Fichiers de l'onglet Outils jamais gardés : effacés à leur échéance.
    let now = time::now();
    let expired = sqlx::query_scalar!(
        r#"DELETE FROM files WHERE expires_at IS NOT NULL AND expires_at < ?
           RETURNING storage_key AS "storage_key!""#,
        now
    )
    .fetch_all(db)
    .await?;
    for key in expired {
        remove_stored(config, &key).await;
    }

    let limit = now - STALE_UPLOAD_SECONDS;
    sqlx::query!("DELETE FROM uploads WHERE updated_at < ?", limit)
        .execute(db)
        .await?;
    let live: HashSet<String> = sqlx::query_scalar!(r#"SELECT id AS "id!" FROM uploads"#)
        .fetch_all(db)
        .await?
        .into_iter()
        .collect();

    let Ok(mut entries) = tokio::fs::read_dir(config.dir.join(INCOMING)).await else {
        return Ok(());
    };
    let recent = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let young = entry
            .metadata()
            .await
            .and_then(|m| m.modified())
            .is_ok_and(|modified| modified > recent);
        if !live.contains(id) && !young {
            let _ = tokio::fs::remove_file(&path).await;
        }
    }
    Ok(())
}

/// Nettoyage des envois abandonnés : au démarrage, puis toutes les heures.
pub fn spawn_cleanup(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
        loop {
            tick.tick().await;
            if let Some(files) = &state.config.files
                && let Err(error) = cleanup_stale(&state.db, files).await
            {
                tracing::warn!(error = format!("{error:#}"), "nettoyage des envois");
            }
        }
    });
}

/// Fichier partiel d'un envoi : `<dossier>/.envois/<id>.part`.
fn part_path(dir: &FsPath, id: &str) -> PathBuf {
    dir.join(INCOMING).join(format!("{id}.part"))
}

/// Vrai pour un identifiant tel que dashu les fabrique : 64 caractères
/// hexadécimaux. Rien d'autre ne peut donc atteindre un chemin.
fn is_token(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

// ---------------------------------------------------------------------
// Téléchargement et suppression
// ---------------------------------------------------------------------

/// `GET /files/{id}/download`
pub async fn download(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let files = state.config.files.as_ref().ok_or(AppError::NotFound)?;
    let row = sqlx::query!(
        "SELECT name, size, storage_key FROM files WHERE id = ? AND user_id = ?",
        id,
        ctx.user.id
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;

    let path = stored_path(&files.dir, &row.storage_key);
    let file = tokio::fs::File::open(&path)
        .await
        .with_context(|| format!("fichier #{id} absent du disque ({})", path.display()))?;

    let mut response = Body::from_stream(tokio_util::io::ReaderStream::new(file)).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(row.size));
    headers.insert(header::CONTENT_DISPOSITION, attachment(&row.name));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(FILE_CSP),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    Ok(response)
}

#[derive(Deserialize)]
pub struct DeleteForm {
    /// Page où revenir (l'onglet Outils) ; la liste sans précision.
    pub(crate) next: Option<String>,
}

/// `POST /files/{id}/delete`
pub async fn delete(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Form(form): axum::Form<DeleteForm>,
) -> AppResult<Response> {
    let key = sqlx::query_scalar!(
        r#"DELETE FROM files WHERE id = ? AND user_id = ? RETURNING storage_key AS "storage_key!""#,
        id,
        ctx.user.id
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;

    if let Some(files) = &state.config.files {
        remove_stored(files, &key).await;
    }
    ctx.flash_ok("Fichier supprimé").await?;
    Ok(see_other(match form.next.as_deref() {
        Some(next) => web::local_path(Some(next)),
        None => "/files",
    }))
}

/// Efface du disque le contenu d'un fichier, sa vignette et son grand aperçu.
/// À appeler une fois la ligne de `files` supprimée.
pub async fn remove_stored(config: &FilesConfig, key: &str) {
    let path = stored_path(&config.dir, key);
    for path in [thumb_path(&path), preview_path(&path), path] {
        if let Err(error) = tokio::fs::remove_file(&path).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%error, path = %path.display(), "suppression d'un fichier du disque");
        }
    }
}

// ---------------------------------------------------------------------
// Aperçu
// ---------------------------------------------------------------------

/// Texte montré dans l'aperçu : les 200 premiers Ko.
const TEXT_PREVIEW_BYTES: u64 = 200 * 1024;

#[derive(Template)]
#[template(path = "files/view.html")]
struct ViewPage {
    shell: Shell,
    id: i64,
    /// Le dossier du fichier (ou Temporaire) : adresse et nom, pour revenir.
    back: String,
    back_label: String,
    /// Adresse de la fenêtre « Ranger ».
    picker: String,
    name: String,
    /// « Vidéo · 98 Mo · il y a 2 min »
    meta: String,
    /// Menu « Convertir » : (valeur, libellé), extraction de pages comprise.
    conversions: Vec<(&'static str, &'static str)>,
    /// Fichier de l'onglet Outils pas encore gardé : « encore 23 h ».
    expires: Option<String>,
    /// Copie compressée ou convertie dont l'original existe encore : la
    /// tâche, pour « Remplacer l'original ».
    replace_job: Option<i64>,
    /// Ce que la page montre : `image` (le fichier), `picture` (le grand
    /// aperçu), `video`, `audio`, `text`, `markdown`, `pending` (aperçu en
    /// préparation) ou `none`.
    show: &'static str,
    /// Vidéo : image affichée avant la lecture.
    poster: bool,
    /// Vidéo .mov d'iPhone (HEVC) : tous les navigateurs ne la lisent pas.
    quicktime: bool,
    /// Texte échappé, ou HTML du Markdown nettoyé par ammonia.
    text_html: String,
    truncated: bool,
    icon: &'static str,
}

/// Types que le navigateur affiche tels quels : images bitmap, vidéos, sons.
/// Jamais le HTML, le SVG ni le PDF, qui peuvent contenir du code.
fn inline_safe(mime: &str) -> bool {
    matches!(
        mime,
        "image/jpeg" | "image/png" | "image/gif" | "image/webp"
    ) || mime.starts_with("video/")
        || mime.starts_with("audio/")
}

/// Types montrés comme du texte (échappé : un HTML ou un SVG s'y lit, sans
/// jamais s'exécuter).
fn is_text(mime: &str) -> bool {
    mime.starts_with("text/") || mime == "application/json" || mime == "image/svg+xml"
}

/// `GET /files/{id}` : la page d'aperçu d'un fichier.
pub async fn view(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    let files = state.config.files.as_ref().ok_or(AppError::NotFound)?;
    let row = sqlx::query!(
        "SELECT space_id, folder_id, name, mime, size, storage_key, updated_at, thumb, preview, expires_at FROM files WHERE id = ? AND user_id = ?",
        id,
        ctx.user.id
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;
    // Fichier d'un autre espace : il s'ouvre dans le sien (sauf ceux de
    // l'onglet Outils, qui n'appartiennent encore à aucune liste).
    if row.space_id != ctx.active.id && row.expires_at.is_none() {
        let space = ctx.space_by_id(row.space_id).ok_or(AppError::NotFound)?;
        return Ok(see_other(&crate::spaces::url::prefixed(
            &space.slug,
            &format!("/files/{id}"),
        )));
    }
    let tools = state.config.work_dir.is_some();
    let replace_job = sqlx::query_scalar!(
        r#"SELECT id AS "id!" FROM jobs
           WHERE result_file_id = ? AND user_id = ? AND status = 'done' AND file_id IS NOT NULL
           ORDER BY id DESC LIMIT 1"#,
        id,
        ctx.user.id
    )
    .fetch_optional(&state.db)
    .await?;

    let mut text_html = String::new();
    let mut truncated = false;
    let show = match row.mime.as_str() {
        m if inline_safe(m) && m.starts_with("image/") => "image",
        m if m.starts_with("video/") => "video",
        m if m.starts_with("audio/") => "audio",
        "image/heic" | "image/heif" | "application/pdf" => match (row.preview, row.thumb) {
            (1, _) => "picture",
            (_, 0) if tools => "pending",
            _ => "none",
        },
        m if is_text(m) => {
            let path = stored_path(&files.dir, &row.storage_key);
            let text = read_start(&path, TEXT_PREVIEW_BYTES).await?;
            truncated = row.size as u64 > TEXT_PREVIEW_BYTES;
            if m == "text/markdown" {
                text_html = crate::notes::markdown::render(&text);
                "markdown"
            } else {
                text_html = web::escape(&text);
                "text"
            }
        }
        _ => "none",
    };

    // Retour au dossier du fichier ; Temporaire pour un fichier de l'onglet
    // Outils, qui n'est encore dans aucun dossier.
    let (back, back_label) = if row.expires_at.is_some() {
        (
            "/files?dossier=temporaire".to_string(),
            "Temporaire".to_string(),
        )
    } else {
        let all = crate::folders::all(&state.db, ctx.user.id).await?;
        (
            crate::folders::view_url(&ctx.active.slug, row.folder_id),
            crate::folders::label(&ctx.active.name, &all, row.folder_id),
        )
    };
    let page = ViewPage {
        shell: ctx.shell("files").await?,
        id,
        back,
        back_label,
        picker: crate::folders::picker_url("fichier", id, Some(&format!("/files/{id}"))),
        meta: format!(
            "{} · {} · {}",
            kind_label(&row.mime),
            human_size(row.size),
            time::ago(row.updated_at, time::now())
        ),
        conversions: menu(tools, &row.mime, true),
        replace_job,
        expires: row.expires_at.map(|at| {
            format!(
                "Onglet Outils : pas encore rangé, effacé dans {}",
                crate::tools::time_left(at, time::now())
            )
        }),
        show,
        poster: row.preview == 1,
        quicktime: row.mime == "video/quicktime",
        text_html,
        truncated,
        icon: icon_for(&row.mime),
        name: row.name,
    };
    Ok(web::html(page))
}

/// Début d'un fichier, en texte (les octets invalides deviennent « � »).
async fn read_start(path: &FsPath, max: u64) -> AppResult<String> {
    use tokio::io::AsyncReadExt;
    let file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("fichier absent du disque ({})", path.display()))?;
    let mut bytes = Vec::new();
    file.take(max).read_to_end(&mut bytes).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// `GET /files/{id}/raw` : le fichier tel quel, pour `<img>`, `<video>` et
/// `<audio>` (types sans code seulement). Accepte les demandes partielles
/// (`Range`) : sans elles, Safari ne lit pas une vidéo, et on ne peut pas
/// avancer dedans.
pub async fn raw(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let files = state.config.files.as_ref().ok_or(AppError::NotFound)?;
    let row = sqlx::query!(
        "SELECT name, mime, size, storage_key FROM files WHERE id = ? AND user_id = ?",
        id,
        ctx.user.id
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;
    if !inline_safe(&row.mime) {
        return Err(AppError::NotFound);
    }
    let total = row.size as u64;
    let wanted = parse_range(
        headers.get(header::RANGE).and_then(|v| v.to_str().ok()),
        total,
    );
    let (status, start, len) = match wanted {
        Wanted::Full => (StatusCode::OK, 0, total),
        Wanted::Part(start, end) => (StatusCode::PARTIAL_CONTENT, start, end - start + 1),
        Wanted::Unsatisfiable => {
            return Ok((
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{total}"))],
            )
                .into_response());
        }
    };

    let path = stored_path(&files.dir, &row.storage_key);
    let mut response = (status, file_body(&path, start, len).await?).into_response();
    let h = response.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_str(&row.mime)?);
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if status == StatusCode::PARTIAL_CONTENT {
        let range = format!("bytes {start}-{}/{total}", start + len - 1);
        h.insert(header::CONTENT_RANGE, HeaderValue::from_str(&range)?);
    }
    h.insert(
        header::CONTENT_DISPOSITION,
        disposition("inline", &row.name),
    );
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(FILE_CSP),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=3600"),
    );
    Ok(response)
}

/// `len` octets d'un fichier à partir de `start`, servis en continu.
async fn file_body(path: &FsPath, start: u64, len: u64) -> AppResult<Body> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("fichier absent du disque ({})", path.display()))?;
    if start > 0 {
        file.seek(std::io::SeekFrom::Start(start)).await?;
    }
    Ok(Body::from_stream(tokio_util::io::ReaderStream::new(
        file.take(len),
    )))
}

/// Partie demandée par l'en-tête `Range`.
#[derive(Debug, PartialEq, Eq)]
pub enum Wanted {
    /// Tout le fichier (pas d'en-tête, ou un en-tête qu'on ne gère pas).
    Full,
    /// Les octets `start` à `end`, inclus.
    Part(u64, u64),
    /// Hors du fichier : réponse 416.
    Unsatisfiable,
}

/// Lit `Range: bytes=…` (une seule plage ; plusieurs : tout le fichier).
pub fn parse_range(header: Option<&str>, total: u64) -> Wanted {
    let Some(spec) = header.and_then(|h| h.trim().strip_prefix("bytes=")) else {
        return Wanted::Full;
    };
    let Some((first, last)) = spec.trim().split_once('-') else {
        return Wanted::Full;
    };
    if spec.contains(',') {
        return Wanted::Full;
    }
    let (first, last) = (first.trim(), last.trim());
    let range = if first.is_empty() {
        // « bytes=-500 » : les 500 derniers octets.
        match last.parse::<u64>() {
            Ok(0) => return Wanted::Unsatisfiable,
            Ok(n) => (total.saturating_sub(n), total.saturating_sub(1)),
            Err(_) => return Wanted::Full,
        }
    } else {
        let Ok(start) = first.parse::<u64>() else {
            return Wanted::Full;
        };
        let end = if last.is_empty() {
            total.saturating_sub(1)
        } else {
            match last.parse::<u64>() {
                Ok(end) => end.min(total.saturating_sub(1)),
                Err(_) => return Wanted::Full,
            }
        };
        (start, end)
    };
    match range {
        (start, end) if total > 0 && start <= end && start < total => Wanted::Part(start, end),
        _ => Wanted::Unsatisfiable,
    }
}

/// `GET /files/{id}/thumb` : la vignette de la liste.
pub async fn thumb(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    derived_image(&ctx, &state, id, false).await
}

/// `GET /files/{id}/preview` : le grand aperçu (HEIC, PDF, image de vidéo).
pub async fn preview(
    ctx: Ctx,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    derived_image(&ctx, &state, id, true).await
}

/// Vignette ou grand aperçu : des JPEG fabriqués par le service de
/// conversion, donc sans risque à afficher.
async fn derived_image(ctx: &Ctx, state: &AppState, id: i64, big: bool) -> AppResult<Response> {
    let files = state.config.files.as_ref().ok_or(AppError::NotFound)?;
    let row = sqlx::query!(
        "SELECT storage_key, thumb, preview FROM files WHERE id = ? AND user_id = ?",
        id,
        ctx.user.id
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;
    let ready = if big { row.preview } else { row.thumb };
    if ready != 1 {
        return Err(AppError::NotFound);
    }
    let stored = stored_path(&files.dir, &row.storage_key);
    let path = if big {
        preview_path(&stored)
    } else {
        thumb_path(&stored)
    };
    let len = tokio::fs::metadata(&path)
        .await
        .with_context(|| format!("aperçu absent du disque ({})", path.display()))?
        .len();
    let mut response = file_body(&path, 0, len).await?.into_response();
    let h = response.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(FILE_CSP),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=86400"),
    );
    Ok(response)
}

// ---------------------------------------------------------------------
// Stockage
// ---------------------------------------------------------------------

/// Au démarrage : crée le dossier des envois. Un dossier inutilisable
/// n'empêche pas dashu de démarrer : seuls les fichiers sont touchés, et
/// l'erreur est signalée dans les logs. Les envois restés en plan sont
/// effacés plus tard, par `cleanup_stale` : un envoi en cours peut reprendre
/// après un redémarrage (une mise à jour, par exemple).
pub fn prepare_storage(config: &FilesConfig) {
    if let Err(error) = std::fs::create_dir_all(config.dir.join(INCOMING)) {
        tracing::warn!(%error, dir = %config.dir.display(), "dossier des fichiers inutilisable");
    }
}

/// Emplacement du contenu d'un fichier : `<dossier>/<ab>/<abcdef…>`.
pub fn stored_path(dir: &FsPath, key: &str) -> PathBuf {
    dir.join(&key[..2]).join(key)
}

/// Vignette d'un fichier, à côté de lui : `<clé>.thumb.jpg`.
pub fn thumb_path(stored: &FsPath) -> PathBuf {
    stored.with_extension("thumb.jpg")
}

/// Grand aperçu d'un fichier, à côté de lui : `<clé>.preview.jpg`.
pub fn preview_path(stored: &FsPath) -> PathBuf {
    stored.with_extension("preview.jpg")
}

// ---------------------------------------------------------------------
// Noms, types, tailles
// ---------------------------------------------------------------------

/// Nom affiché du fichier. Le navigateur n'envoie que le nom, mais on ne s'y
/// fie pas : on garde ce qui suit le dernier séparateur, sans caractère de
/// contrôle, et 200 caractères au plus.
pub fn clean_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control())
        .take(NAME_MAX_CHARS)
        .collect();
    let cleaned = cleaned.trim();
    match cleaned {
        "" | "." | ".." => "fichier".into(),
        _ => cleaned.into(),
    }
}

/// Type MIME d'après l'extension. Celui annoncé par le navigateur n'est pas
/// utilisé : il ne prouve rien.
pub fn mime_for(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "svg" => "image/svg+xml",
        "txt" | "log" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "zip" => "application/zip",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "wav" => "audio/wav",
        "ogg" | "opus" => "audio/ogg",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        _ => "application/octet-stream",
    }
}

/// Icône du sprite selon le type, quand il n'y a pas de vignette.
pub fn icon_for(mime: &str) -> &'static str {
    match mime {
        m if m.starts_with("image/") => "file-image",
        m if m.starts_with("video/") => "file-video",
        m if m.starts_with("audio/") => "file-audio",
        "application/zip" => "file-archive",
        "application/json" | "text/html" | "text/csv" => "file-code",
        m if m.contains("spreadsheetml") => "file-sheet",
        "application/pdf" => "file",
        m if m.starts_with("text/")
            || m.contains("wordprocessingml")
            || m.contains("opendocument") =>
        {
            "file"
        }
        _ => "file-blank",
    }
}

/// Libellé court affiché dans la liste.
pub fn kind_label(mime: &str) -> &'static str {
    match mime {
        "application/pdf" => "PDF",
        "application/zip" => "Archive",
        m if m.starts_with("image/") => "Image",
        m if m.starts_with("video/") => "Vidéo",
        m if m.starts_with("audio/") => "Audio",
        m if m.starts_with("text/") => "Texte",
        m if m.contains("wordprocessingml")
            || m.contains("opendocument")
            || m == "application/msword" =>
        {
            "Document"
        }
        m if m.contains("spreadsheetml") => "Tableur",
        m if m.contains("presentationml") => "Présentation",
        _ => "Fichier",
    }
}

/// Taille lisible, en français : « 512 o », « 1,5 Mo », « 34 Ko ».
pub fn human_size(bytes: i64) -> String {
    const UNITS: [&str; 4] = ["Ko", "Mo", "Go", "To"];
    if bytes < 1024 {
        return format!("{} o", bytes.max(0));
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let text = if value < 10.0 {
        format!("{value:.1}").replace('.', ",")
    } else {
        format!("{value:.0}")
    };
    let text = text.strip_suffix(",0").unwrap_or(&text).to_string();
    format!("{text} {}", UNITS[unit])
}

/// En-tête `Content-Disposition` d'un téléchargement.
fn attachment(name: &str) -> HeaderValue {
    disposition("attachment", name)
}

/// En-tête `Content-Disposition` : un nom ASCII de secours, puis le vrai nom
/// encodé (RFC 6266), accents compris.
fn disposition(kind: &str, name: &str) -> HeaderValue {
    let fallback: String = name
        .chars()
        .map(|c| {
            if c == ' ' || (c.is_ascii_graphic() && c != '"' && c != '\\') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut encoded = String::with_capacity(name.len() * 3);
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&byte) {
            encoded.push(byte as char);
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    HeaderValue::from_str(&format!(
        "{kind}; filename=\"{fallback}\"; filename*=UTF-8''{encoded}"
    ))
    .unwrap_or_else(|_| HeaderValue::from_static("attachment"))
}

fn to_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noms_nettoyes() {
        assert_eq!(clean_name("rapport.pdf"), "rapport.pdf");
        assert_eq!(clean_name("../../etc/passwd"), "passwd");
        assert_eq!(clean_name("C:\\Users\\moi\\photo.jpg"), "photo.jpg");
        assert_eq!(clean_name("a\u{0}b\nc.txt"), "abc.txt");
        assert_eq!(clean_name(".."), "fichier");
        assert_eq!(clean_name("   "), "fichier");
        assert_eq!(clean_name(&"é".repeat(300)).chars().count(), 200);
    }

    #[test]
    fn types_d_apres_l_extension() {
        assert_eq!(mime_for("Photo.JPG"), "image/jpeg");
        assert_eq!(mime_for("cours.pdf"), "application/pdf");
        assert_eq!(mime_for("page.html"), "text/html");
        assert_eq!(mime_for("sans-extension"), "application/octet-stream");
        assert_eq!(kind_label("image/png"), "Image");
        assert_eq!(kind_label(mime_for("cv.docx")), "Document");
    }

    #[test]
    fn tailles_en_francais() {
        assert_eq!(human_size(0), "0 o");
        assert_eq!(human_size(512), "512 o");
        assert_eq!(human_size(1024), "1 Ko");
        assert_eq!(human_size(1536), "1,5 Ko");
        assert_eq!(human_size(34 * 1024), "34 Ko");
        assert_eq!(human_size(2048 * 1024 * 1024), "2 Go");
    }

    #[test]
    fn nom_de_telechargement_encode() {
        let value = attachment("Résumé \"final\".pdf");
        assert_eq!(
            value.to_str().unwrap(),
            "attachment; filename=\"R_sum_ _final_.pdf\"; filename*=UTF-8''R%C3%A9sum%C3%A9%20%22final%22.pdf"
        );
    }

    #[test]
    fn plages_demandees() {
        assert_eq!(parse_range(None, 100), Wanted::Full);
        assert_eq!(parse_range(Some("bytes=0-"), 100), Wanted::Part(0, 99));
        assert_eq!(parse_range(Some("bytes=10-19"), 100), Wanted::Part(10, 19));
        assert_eq!(parse_range(Some("bytes=90-500"), 100), Wanted::Part(90, 99));
        assert_eq!(parse_range(Some("bytes=-30"), 100), Wanted::Part(70, 99));
        assert_eq!(parse_range(Some("bytes=-300"), 100), Wanted::Part(0, 99));
        assert_eq!(parse_range(Some("bytes=100-"), 100), Wanted::Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=20-10"), 100), Wanted::Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=-0"), 100), Wanted::Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=0-1,5-6"), 100), Wanted::Full);
        assert_eq!(parse_range(Some("octets=0-1"), 100), Wanted::Full);
        assert_eq!(parse_range(Some("bytes=0-"), 0), Wanted::Unsatisfiable);
    }

    #[test]
    fn chemin_de_stockage() {
        let key = "abcdef0123";
        assert_eq!(
            stored_path(FsPath::new("/srv/f"), key),
            PathBuf::from("/srv/f/ab/abcdef0123")
        );
    }
}
