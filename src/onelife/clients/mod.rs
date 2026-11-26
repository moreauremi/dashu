//! 1Life, module Clients (SPEC, « 1Life — Clients ») : un client est un
//! sous-dossier direct du dossier des clients que le compte a choisi dans ses
//! Fichiers.
//!
//! * Rien n'est copié : le nom, le contenu et la corbeille sont ceux du
//!   dossier, lus en direct (`folders::all`, qui passe par `space_access` et
//!   ne voit donc pas la corbeille). Le module y ajoute les infos
//!   (`onelife_client_infos`) et les journées (`onelife_client_days`, des
//!   événements de l'Agenda, voir [`journees`]).
//! * Toutes les adresses passent par [`super::enter`] : 404 pour un compte
//!   non autorisé ou sans session, retour à l'accueil de l'espace si 1Life y
//!   est masqué. Le numéro et le formulaire ne se lisent qu'après ce
//!   contrôle : une erreur 400 ou 422 dirait que l'outil existe.
//! * Un dossier qui n'est pas un client du compte (autre compte, pas un
//!   sous-dossier direct du dossier des clients, en corbeille) répond 404,
//!   comme une adresse qui n'existe pas.
//! * Hors de l'API, du connecteur et d'Hermès, comme le reste de 1Life.
//!
//! Dépôt public : aucun vrai nom de client ici ni dans les tests.

pub mod gantt;
pub mod journees;

use askama::Template;
use axum::extract::rejection::{FormRejection, QueryRejection};
use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use chrono::{Duration, NaiveDate, Timelike};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::AppState;
use crate::auth::session::{Ctx, Shell};
use crate::calendar::view::{self, EventRow, Grid};
use crate::commands::fold;
use crate::folders::{self, Folder};
use crate::spaces::Space;
use crate::time;
use crate::web::{self, AppError, AppResult, see_other};

/// Les infos d'un client font 20 000 caractères au plus.
pub const INFOS_MAX: usize = 20_000;

/// Niveau le plus profond du dossier des clients : un client y est un niveau
/// plus bas, ses sous-dossiers (Audit, CR, Fichiers) encore un niveau plus
/// bas, et le tout tient dans les cinq niveaux de Fichiers.
pub const FOLDER_MAX_DEPTH: usize = folders::MAX_DEPTH - 2;

/// Les sous-dossiers d'un nouveau client.
pub const SUBFOLDERS: [&str; 3] = ["Audit", "CR", "Fichiers"];

// ---------------------------------------------------------------------
// Le dossier des clients
// ---------------------------------------------------------------------

/// Le dossier des clients d'un compte, avec ce qu'il faut pour lire ses
/// clients.
pub(crate) struct Book {
    pub folder: Folder,
    pub space: Space,
    /// Tous les dossiers du compte (`folders::all`).
    pub all: Vec<Folder>,
}

impl Book {
    /// Les clients : les sous-dossiers directs, par nom (majuscules et
    /// accents mis à part).
    pub fn clients(&self) -> Vec<&Folder> {
        let mut clients: Vec<&Folder> = self
            .all
            .iter()
            .filter(|f| f.parent_id == Some(self.folder.id))
            .collect();
        clients.sort_by_cached_key(|f| (fold(&f.name), f.id));
        clients
    }

    /// Un client, d'après l'identifiant de son dossier.
    pub fn client(&self, id: i64) -> Option<&Folder> {
        self.all
            .iter()
            .find(|f| f.id == id && f.parent_id == Some(self.folder.id))
    }

    /// « Taff / Clients »
    pub fn label(&self) -> String {
        folders::label(&self.space.name, &self.all, Some(self.folder.id))
    }
}

/// Où en est le dossier des clients d'un compte.
pub(crate) enum Place {
    /// Pas encore choisi (ou effacé pour de bon : le choix s'est vidé).
    Unset,
    /// Choisi, mais plus accessible : en corbeille, espace quitté.
    Gone,
    Ready(Book),
}

/// L'identifiant du dossier des clients choisi par le compte, accessible ou
/// non.
pub(crate) async fn chosen(db: &SqlitePool, user: i64) -> AppResult<Option<i64>> {
    Ok(sqlx::query_scalar!(
        "SELECT clients_folder_id FROM onelife_settings WHERE user_id = ?",
        user
    )
    .fetch_optional(db)
    .await?
    .flatten())
}

/// Le dossier des clients du compte.
pub(crate) async fn place(state: &AppState, ctx: &Ctx) -> AppResult<Place> {
    let user = ctx.user.id;
    let Some(id) = chosen(&state.db, user).await? else {
        return Ok(Place::Unset);
    };
    let all = folders::all(&state.db, user).await?;
    let Some(folder) = all.iter().find(|f| f.id == id).cloned() else {
        return Ok(Place::Gone);
    };
    let Some(space) = ctx.space_by_id(folder.space_id).cloned() else {
        return Ok(Place::Gone);
    };
    Ok(Place::Ready(Book { folder, space, all }))
}

/// Le dossier des clients et le client demandé par l'adresse ; 404 sinon
/// (numéro illisible, pas de dossier des clients, dossier qui n'en est pas
/// un client).
pub(crate) async fn client_of(state: &AppState, ctx: &Ctx, raw: &str) -> AppResult<(Book, Folder)> {
    let id: i64 = raw.parse().map_err(|_| AppError::NotFound)?;
    let Place::Ready(book) = place(state, ctx).await? else {
        return Err(AppError::NotFound);
    };
    let client = book.client(id).cloned().ok_or(AppError::NotFound)?;
    Ok((book, client))
}

/// Adresse d'une page de Clients dans l'espace actif.
pub(crate) fn url(ctx: &Ctx, rest: &str) -> String {
    format!("/e/{}/1life/clients{rest}", ctx.active.slug)
}

/// La fenêtre « Ranger » en mode « dossier des clients ».
const PICKER: &str = "/destinations?clients=1";

// ---------------------------------------------------------------------
// Les journées
// ---------------------------------------------------------------------

/// Une journée d'un client : l'événement et son lien.
pub(crate) struct DayRow {
    pub event_id: i64,
    pub folder_id: i64,
    pub label: String,
    pub place_kind: String,
    pub place: String,
    pub starts_at: i64,
    pub ends_at: i64,
    pub all_day: bool,
}

/// Les journées du compte, tous clients confondus, dans l'ordre. Comme dans
/// l'Agenda, chacun ne voit que ses événements (`events.user_id`).
pub(crate) async fn days(db: &SqlitePool, user: i64) -> AppResult<Vec<DayRow>> {
    Ok(sqlx::query_as!(
        DayRow,
        r#"SELECT d.event_id AS "event_id!", d.folder_id, d.label, d.place_kind, d.place,
                  e.starts_at, e.ends_at, e.all_day AS "all_day!: bool"
           FROM onelife_client_days d JOIN events e ON e.id = d.event_id
           WHERE e.user_id = ?
           ORDER BY e.starts_at, e.id"#,
        user
    )
    .fetch_all(db)
    .await?)
}

/// « 9 h », « 12 h 30 »
fn hour(ts: i64) -> String {
    let local = time::paris(ts);
    if local.minute() == 0 {
        format!("{} h", local.hour())
    } else {
        format!("{} h {:02}", local.hour(), local.minute())
    }
}

/// « jeudi 3 décembre, 9 h – 17 h (dans 7 jours) » ; « aujourd'hui » pour
/// une journée en cours (SPEC, décision 10), « demain ».
pub fn when(starts_at: i64, ends_at: i64, all_day: bool, now: i64) -> String {
    let date = time::paris_date(starts_at);
    let delay = match (date - time::paris_date(now)).num_days() {
        ..=0 => "aujourd'hui".to_string(),
        1 => "demain".to_string(),
        n => format!("dans {n} jours"),
    };
    let hours = if all_day {
        "toute la journée".to_string()
    } else {
        format!("{} – {}", hour(starts_at), hour(ends_at))
    };
    format!("{}, {hours} ({delay})", time::weekday_day_month(date))
}

/// La prochaine journée d'un client : la première qui n'est pas finie.
fn next_of(days: &[DayRow], client: i64, now: i64) -> Option<&DayRow> {
    days.iter()
        .find(|d| d.folder_id == client && d.ends_at > now)
}

// ---------------------------------------------------------------------
// La page du module : la liste, ou la vue globale
// ---------------------------------------------------------------------

/// Une ligne de la liste.
pub struct Row {
    pub name: String,
    pub href: String,
    /// « jeudi 3 décembre, 9 h – 17 h (dans 7 jours) »
    pub next: Option<String>,
    /// « 3 fichiers », sous-dossiers compris.
    pub files: String,
}

/// Les boutons de la vue globale.
pub struct GanttNav {
    pub title: String,
    pub prev: String,
    pub next: String,
    pub today: String,
    pub month: String,
    pub quarter: String,
    pub quarter_on: bool,
    /// Le même Gantt, avec ou sans l'option « Seulement les clients qui ont
    /// des journées ».
    pub busy: String,
    pub busy_on: bool,
}

#[derive(Template)]
#[template(path = "onelife/clients.html")]
struct ListPage {
    shell: Shell,
    /// `unset`, `gone` ou `ready` (voir [`Place`]).
    state: &'static str,
    /// « Taff / Clients »
    folder_label: String,
    picker: String,
    /// `/e/<espace>/1life/clients`
    list_url: String,
    /// L'onglet Vue globale plutôt que la liste.
    global: bool,
    rows: Vec<Row>,
    /// Combien de clients en tout, recherche mise à part.
    total: usize,
    q: String,
    /// Un nouveau client refusé : le refus et le nom tapé.
    error: Option<String>,
    typed: String,
    gantt: Option<gantt::Gantt>,
    nav: Option<GanttNav>,
}

#[derive(Deserialize, Default)]
pub struct ListQuery {
    q: Option<String>,
    vue: Option<String>,
    du: Option<String>,
    duree: Option<String>,
    avec: Option<String>,
}

/// « 3 fichiers »
fn files_label(n: i64) -> String {
    match n {
        0 => "Aucun fichier".to_string(),
        1 => "1 fichier".to_string(),
        n => format!("{n} fichiers"),
    }
}

/// Adresse de la vue globale.
fn gantt_url(ctx: &Ctx, start: Option<NaiveDate>, span: gantt::Span, busy: bool) -> String {
    let mut params = form_urlencoded::Serializer::new(String::new());
    params.append_pair("vue", "globale");
    if let Some(start) = start {
        params.append_pair("du", &start.format("%Y-%m-%d").to_string());
    }
    if span == gantt::Span::Quarter {
        params.append_pair("duree", span.key());
    }
    if busy {
        params.append_pair("avec", "1");
    }
    url(ctx, &format!("?{}", params.finish()))
}

/// La page du module, pour [`list`] et pour un nouveau client refusé.
async fn render_list(
    state: &AppState,
    ctx: &Ctx,
    query: ListQuery,
    refused: Option<(String, String)>,
) -> AppResult<Response> {
    let now = time::now();
    let place = place(state, ctx).await?;
    let global = query.vue.as_deref() == Some("globale");
    let q = query.q.unwrap_or_default().trim().to_string();
    let (state_name, folder_label, rows, total, gantt, nav) = match &place {
        Place::Unset => ("unset", String::new(), Vec::new(), 0, None, None),
        Place::Gone => ("gone", String::new(), Vec::new(), 0, None, None),
        Place::Ready(book) => {
            let clients = book.clients();
            let days = days(&state.db, ctx.user.id).await?;
            let total = clients.len();
            let (rows, gantt, nav) = if global {
                let today = time::paris_date(now);
                let span = gantt::Span::parse(query.duree.as_deref());
                let busy = query.avec.as_deref() == Some("1");
                let start = time::monday_of(
                    query
                        .du
                        .as_deref()
                        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
                        .unwrap_or(today),
                );
                let step = Duration::weeks(span.weeks());
                let list: Vec<gantt::Client> = clients
                    .iter()
                    .map(|c| gantt::Client {
                        id: c.id,
                        name: c.name.clone(),
                        href: url(ctx, &format!("/{}", c.id)),
                    })
                    .collect();
                let bars: Vec<gantt::Day> = days
                    .iter()
                    .map(|d| gantt::Day {
                        client: d.folder_id,
                        label: d.label.clone(),
                        starts_at: d.starts_at,
                        ends_at: d.ends_at,
                        all_day: d.all_day,
                    })
                    .collect();
                let chart = gantt::build(start, span, &list, &bars, now, busy);
                let nav = GanttNav {
                    title: chart.label.clone(),
                    prev: gantt_url(ctx, Some(start - step), span, busy),
                    next: gantt_url(ctx, Some(start + step), span, busy),
                    today: gantt_url(ctx, None, span, busy),
                    month: gantt_url(ctx, Some(start), gantt::Span::Month, busy),
                    quarter: gantt_url(ctx, Some(start), gantt::Span::Quarter, busy),
                    quarter_on: span == gantt::Span::Quarter,
                    busy: gantt_url(ctx, Some(start), span, !busy),
                    busy_on: busy,
                };
                (Vec::new(), Some(chart), Some(nav))
            } else {
                let contents = folders::contents(&state.db, ctx.user.id, &book.all).await?;
                let wanted = fold(&q);
                let rows = clients
                    .iter()
                    .filter(|c| wanted.is_empty() || fold(&c.name).contains(&wanted))
                    .map(|c| Row {
                        name: c.name.clone(),
                        href: url(ctx, &format!("/{}", c.id)),
                        next: next_of(&days, c.id, now)
                            .map(|d| when(d.starts_at, d.ends_at, d.all_day, now)),
                        files: files_label(contents.get(&c.id).map_or(0, |n| n.files)),
                    })
                    .collect();
                (rows, None, None)
            };
            ("ready", book.label(), rows, total, gantt, nav)
        }
    };
    let (error, typed) = match refused {
        Some((error, typed)) => (Some(error), typed),
        None => (None, String::new()),
    };
    let status = if error.is_some() {
        StatusCode::UNPROCESSABLE_ENTITY
    } else {
        StatusCode::OK
    };
    let page = ListPage {
        shell: ctx.shell(super::TOOL).await?,
        state: state_name,
        folder_label,
        picker: PICKER.to_string(),
        list_url: url(ctx, ""),
        global,
        rows,
        total,
        q,
        error,
        typed,
        gantt,
        nav,
    };
    Ok((status, web::html(page)).into_response())
}

/// `GET /1life/clients?q=…` : la liste ; `?vue=globale` : le Gantt.
pub async fn list(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> AppResult<Response> {
    let ctx = match super::enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let query = query.map(|Query(q)| q).unwrap_or_default();
    render_list(&state, &ctx, query, None).await
}

#[derive(Deserialize, Default)]
pub struct NewForm {
    #[serde(default)]
    name: String,
}

/// `POST /1life/clients` : un nouveau client, c'est-à-dire un dossier dans le
/// dossier des clients, avec ses sous-dossiers Audit, CR et Fichiers ; puis
/// sa fiche.
pub async fn create(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    form: Result<Form<NewForm>, FormRejection>,
) -> AppResult<Response> {
    let ctx = match super::enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let typed = form.map(|Form(f)| f.name).unwrap_or_default();
    let Place::Ready(book) = place(&state, &ctx).await? else {
        ctx.flash_error("Choisis d'abord le dossier des clients.")
            .await?;
        return Ok(see_other(&url(&ctx, "")));
    };
    let name = folders::clean_name(&typed);
    let refusal = if name.is_empty() {
        Some("Donne un nom au client.".to_string())
    } else if book
        .clients()
        .iter()
        .any(|c| c.name.to_lowercase() == name.to_lowercase())
    {
        Some(format!("Un client « {name} » existe déjà."))
    } else {
        None
    };
    if let Some(error) = refusal {
        return render_list(&state, &ctx, ListQuery::default(), Some((error, typed))).await;
    }
    let user = ctx.user.id;
    let (id, name) =
        match folders::create_in(&state.db, user, book.space.id, Some(book.folder.id), &name)
            .await?
        {
            Ok(created) => created,
            Err(error) => {
                return render_list(&state, &ctx, ListQuery::default(), Some((error, typed))).await;
            }
        };
    let mut missing = Vec::new();
    for sub in SUBFOLDERS {
        if folders::create_in(&state.db, user, book.space.id, Some(id), sub)
            .await?
            .is_err()
        {
            missing.push(sub);
        }
    }
    tracing::info!(user, client = id, "1life : client créé");
    if missing.is_empty() {
        ctx.flash_ok(format!("Client « {name} » créé")).await?;
    } else {
        // Le dossier des clients a été déplacé trop bas depuis qu'il a été
        // choisi : le client existe, ses sous-dossiers ne tiennent pas.
        ctx.flash_error(format!(
            "Client « {name} » créé, sans {} : cinq niveaux de dossiers au plus.",
            missing.join(", ")
        ))
        .await?;
    }
    Ok(see_other(&url(&ctx, &format!("/{id}"))))
}

#[derive(Deserialize, Default)]
pub struct ChooseForm {
    #[serde(default)]
    space_id: String,
    #[serde(default)]
    folder_id: String,
}

/// `POST /1life/clients/dossier` : enregistre le dossier des clients choisi
/// dans la fenêtre « Ranger » (un dossier des niveaux 1 à 3 ; un espace ne
/// se choisit pas). Il remplace le choix d'avant.
pub async fn choose(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    form: Result<Form<ChooseForm>, FormRejection>,
) -> AppResult<Response> {
    let ctx = match super::enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let form = form.map(|Form(f)| f).unwrap_or_default();
    let user = ctx.user.id;
    let all = folders::all(&state.db, user).await?;
    let back = see_other(&url(&ctx, ""));
    // Un espace seul (pas de dossier) : refusé, un espace n'est pas un dossier.
    let Ok(id) = form.folder_id.trim().parse::<i64>() else {
        ctx.flash_error("Choisis un dossier : un espace ne peut pas être le dossier des clients.")
            .await?;
        return Ok(back);
    };
    // Le dossier d'un autre compte, ou d'un autre espace que celui annoncé :
    // 404, comme partout.
    let space: Option<i64> = form.space_id.trim().parse().ok();
    let folder = all
        .iter()
        .find(|f| f.id == id)
        .filter(|f| space.is_none_or(|s| s == f.space_id))
        .ok_or(AppError::NotFound)?;
    if folders::depth(&all, Some(folder.id)) > FOLDER_MAX_DEPTH {
        ctx.flash_error(format!(
            "« {} » est trop bas : choisis un dossier des niveaux 1 à {FOLDER_MAX_DEPTH}, pour que chaque client ait la place de ses sous-dossiers.",
            folder.name
        ))
        .await?;
        return Ok(back);
    }
    let now = time::now();
    sqlx::query!(
        "INSERT INTO onelife_settings (user_id, clients_folder_id, updated_at) VALUES (?, ?, ?)
         ON CONFLICT (user_id) DO UPDATE SET clients_folder_id = excluded.clients_folder_id,
                                             updated_at = excluded.updated_at",
        user,
        folder.id,
        now
    )
    .execute(&state.db)
    .await?;
    let space = ctx
        .space_by_id(folder.space_id)
        .map(|s| s.name.clone())
        .unwrap_or_default();
    ctx.flash_ok(format!(
        "Dossier des clients : {}",
        folders::label(&space, &all, Some(folder.id))
    ))
    .await?;
    Ok(back)
}

// ---------------------------------------------------------------------
// La fiche d'un client
// ---------------------------------------------------------------------

/// L'encart Infos, rendu ou en édition (`onelife/_infos.html`).
pub struct Infos {
    /// Adresse des formulaires : `…/{id}/infos`.
    pub action: String,
    /// La fiche, pour « Modifier » sans JavaScript.
    pub fiche: String,
    pub body: String,
    /// Le Markdown rendu, nettoyé par ammonia.
    pub html: String,
    pub editing: bool,
    pub error: Option<String>,
    pub max: usize,
}

impl Infos {
    fn new(fiche: &str, body: String, editing: bool, error: Option<String>) -> Self {
        Self {
            action: format!("{fiche}/infos"),
            fiche: fiche.to_string(),
            html: crate::notes::markdown::render(&body),
            body,
            editing,
            error,
            max: INFOS_MAX,
        }
    }
}

/// Les infos d'un client (vides s'il n'en a pas encore).
async fn stored_infos(db: &SqlitePool, folder: i64) -> AppResult<String> {
    Ok(sqlx::query_scalar!(
        "SELECT body_md FROM onelife_client_infos WHERE folder_id = ?",
        folder
    )
    .fetch_optional(db)
    .await?
    .unwrap_or_default())
}

/// Un pas du fil d'Ariane de l'encart Fichiers.
pub struct PanelCrumb {
    pub label: String,
    /// Vide pour le dernier pas (le dossier montré).
    pub href: String,
}

pub struct PanelFolder {
    pub name: String,
    pub href: String,
    pub summary: String,
}

pub struct PanelNote {
    pub id: i64,
    pub title: String,
}

pub struct PanelFile {
    pub name: String,
    pub href: String,
    pub thumb: Option<String>,
    pub icon: &'static str,
    pub meta: String,
}

/// La zone de dépôt de l'encart : celle de la page Fichiers
/// (`static/js/files.js`), réglée sur le dossier montré.
pub struct Upload {
    pub folder: i64,
    pub max_bytes: u64,
    pub max_label: String,
}

/// L'encart Fichiers : le contenu d'un dossier du client.
pub struct Panel {
    pub crumbs: Vec<PanelCrumb>,
    pub folders: Vec<PanelFolder>,
    pub notes: Vec<PanelNote>,
    pub files: Vec<PanelFile>,
    pub upload: Option<Upload>,
    /// Le même dossier dans l'onglet Fichiers.
    pub open: String,
}

#[derive(Template)]
#[template(path = "onelife/client.html")]
struct ClientPage {
    shell: Shell,
    name: String,
    list_url: String,
    /// Le dossier du client dans l'onglet Fichiers.
    files_url: String,
    infos: Infos,
    next: Option<String>,
    grid: Grid,
    week_title: String,
    prev_week: String,
    this_week: String,
    next_week: String,
    new_day: String,
    panel: Panel,
}

#[derive(Template)]
#[template(path = "onelife/_infos.html")]
struct InfosFragment {
    infos: Infos,
}

#[derive(Deserialize, Default)]
pub struct ShowQuery {
    semaine: Option<String>,
    dossier: Option<String>,
    modifier: Option<String>,
}

/// L'adresse de la fiche, avec ses paramètres.
fn fiche_url(ctx: &Ctx, client: i64, semaine: Option<NaiveDate>, dossier: Option<i64>) -> String {
    let mut params = form_urlencoded::Serializer::new(String::new());
    if let Some(day) = semaine {
        params.append_pair("semaine", &day.format("%Y-%m-%d").to_string());
    }
    if let Some(folder) = dossier {
        params.append_pair("dossier", &folder.to_string());
    }
    let query = params.finish();
    let path = url(ctx, &format!("/{client}"));
    if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    }
}

/// `GET /1life/clients/{id}` : la fiche (`?semaine=`, `?dossier=`,
/// `?modifier=1` pour ouvrir les infos en édition).
pub async fn show(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    query: Result<Query<ShowQuery>, QueryRejection>,
) -> AppResult<Response> {
    let ctx = match super::enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let (book, client) = client_of(&state, &ctx, &id).await?;
    let query = query.map(|Query(q)| q).unwrap_or_default();
    let user = ctx.user.id;
    let now = time::now();
    let today = time::paris_date(now);

    // Infos.
    let fiche = url(&ctx, &format!("/{}", client.id));
    let body = stored_infos(&state.db, client.id).await?;
    let editing = query.modifier.as_deref() == Some("1");
    let infos = Infos::new(&fiche, body, editing, None);

    // Journées : la semaine demandée, dans la grille de l'Agenda.
    let monday = time::monday_of(
        query
            .semaine
            .as_deref()
            .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
            .unwrap_or(today),
    );
    let all_days = days(&state.db, user).await?;
    let mine: Vec<&DayRow> = all_days
        .iter()
        .filter(|d| d.folder_id == client.id)
        .collect();
    let week_start = time::day_start(monday);
    let week_end = time::day_start(monday + Duration::days(7));
    let rows: Vec<EventRow> = mine
        .iter()
        .filter(|d| d.ends_at > week_start && d.starts_at < week_end)
        .map(|d| EventRow {
            title: if d.label.is_empty() {
                "Journée".to_string()
            } else {
                d.label.clone()
            },
            location: journees::location(&d.place_kind, &d.place),
            starts_at: d.starts_at,
            ends_at: d.ends_at,
            all_day: d.all_day,
            tag: String::new(),
            tag_color: String::new(),
            event_id: Some(d.event_id),
        })
        .collect();
    // Samedi et dimanche ne s'affichent que s'il s'y passe quelque chose,
    // comme dans l'Agenda.
    let saturday = time::day_start(monday + Duration::days(5));
    let weekend = rows.iter().any(|e| e.ends_at > saturday);
    let dates: Vec<NaiveDate> = (0..if weekend { 7 } else { 5 })
        .map(|i| monday + Duration::days(i))
        .collect();
    let mut grid = view::build(&dates, &rows, now);
    let client_id = client.id;
    grid.relink(|event| url(&ctx, &format!("/{client_id}/journees/{event}")));
    let next =
        next_of(&all_days, client.id, now).map(|d| when(d.starts_at, d.ends_at, d.all_day, now));

    // Fichiers : le dossier demandé, s'il est le client ou un de ses
    // sous-dossiers ; sinon le dossier du client.
    let below = folders::descendants(&book.all, client.id);
    let shown_id = query
        .dossier
        .as_deref()
        .and_then(|d| d.parse::<i64>().ok())
        .filter(|id| *id == client.id || below.contains(id))
        .unwrap_or(client.id);
    let week = (monday != time::monday_of(today)).then_some(monday);
    let here = fiche_url(
        &ctx,
        client.id,
        week,
        (shown_id != client.id).then_some(shown_id),
    );
    let panel = panel(&state, &ctx, &book, &client, shown_id, &here).await?;

    let page = ClientPage {
        shell: ctx.shell(super::TOOL).await?,
        name: client.name.clone(),
        list_url: url(&ctx, ""),
        files_url: folders::view_url(&book.space.slug, Some(client.id)),
        infos,
        next,
        week_title: format!("Semaine du {}", time::day_month(monday)),
        prev_week: fiche_url(&ctx, client.id, Some(monday - Duration::days(7)), None),
        this_week: fiche_url(&ctx, client.id, None, None),
        next_week: fiche_url(&ctx, client.id, Some(monday + Duration::days(7)), None),
        new_day: url(&ctx, &format!("/{}/journees/new", client.id)),
        grid,
        panel,
    };
    Ok(web::html(page))
}

/// L'encart Fichiers d'une fiche : le dossier `shown` (le client ou un de
/// ses sous-dossiers). `here` : l'adresse de la fiche, pour le retour.
async fn panel(
    state: &AppState,
    ctx: &Ctx,
    book: &Book,
    client: &Folder,
    shown: i64,
    here: &str,
) -> AppResult<Panel> {
    let user = ctx.user.id;
    let link = |folder: i64| {
        fiche_url(
            ctx,
            client.id,
            None,
            (folder != client.id).then_some(folder),
        )
    };
    // Le fil d'Ariane part du nom du client.
    let chain = folders::chain(&book.all, Some(shown));
    let mut crumbs: Vec<PanelCrumb> = chain
        .iter()
        .skip_while(|f| f.id != client.id)
        .map(|f| PanelCrumb {
            label: f.name.clone(),
            href: link(f.id),
        })
        .collect();
    if let Some(last) = crumbs.last_mut() {
        last.href.clear();
    }
    let contents = folders::contents(&state.db, user, &book.all).await?;
    let sub_folders = book
        .all
        .iter()
        .filter(|f| f.parent_id == Some(shown))
        .map(|f| PanelFolder {
            name: f.name.clone(),
            href: link(f.id),
            summary: contents.get(&f.id).copied().unwrap_or_default().describe(),
        })
        .collect();
    let notes = sqlx::query!(
        r#"SELECT id AS "id!", title FROM notes
           WHERE folder_id = ?2 AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?1)
           ORDER BY pinned DESC, updated_at DESC, id DESC"#,
        user,
        shown
    )
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|row| PanelNote {
        id: row.id,
        title: if row.title.trim().is_empty() {
            "Sans titre".to_string()
        } else {
            row.title
        },
    })
    .collect();
    let now = time::now();
    let back = form_urlencoded::Serializer::new(String::new())
        .append_pair("depuis", here)
        .finish();
    let files = sqlx::query!(
        r#"SELECT id AS "id!", name, mime, size, updated_at, thumb FROM files
           WHERE folder_id = ?2 AND expires_at IS NULL
             AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?1)
           ORDER BY updated_at DESC, id DESC"#,
        user,
        shown
    )
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|row| PanelFile {
        href: format!("/files/{}?{back}", row.id),
        thumb: (row.thumb == 1).then(|| format!("/files/{}/thumb", row.id)),
        icon: crate::files::icon_for(&row.mime),
        meta: format!(
            "{} · {} · {}",
            crate::files::kind_label(&row.mime),
            crate::files::human_size(row.size),
            time::ago(row.updated_at, now)
        ),
        name: row.name,
    })
    .collect();
    let upload = state.config.files.as_ref().map(|f| Upload {
        folder: shown,
        max_bytes: f.max_upload_bytes,
        max_label: crate::files::human_size(i64::try_from(f.max_upload_bytes).unwrap_or(i64::MAX)),
    });
    Ok(Panel {
        crumbs,
        folders: sub_folders,
        notes,
        files,
        upload,
        open: folders::view_url(&book.space.slug, Some(shown)),
    })
}

// ---------------------------------------------------------------------
// Les infos
// ---------------------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct InfosQuery {
    modifier: Option<String>,
    auto: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct InfosForm {
    #[serde(default)]
    body: String,
}

/// L'encart seul (htmx), ou la fiche entière.
fn infos_reply(headers: &HeaderMap, status: StatusCode, infos: Infos) -> Response {
    if web::is_fragment_request(headers) {
        (status, web::html(InfosFragment { infos })).into_response()
    } else {
        let target = if infos.editing {
            format!("{}?modifier=1", infos.fiche)
        } else {
            infos.fiche
        };
        see_other(&target)
    }
}

/// `GET /1life/clients/{id}/infos` : l'encart Infos (htmx), rendu ou, avec
/// `?modifier=1`, en édition.
pub async fn infos(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    query: Result<Query<InfosQuery>, QueryRejection>,
) -> AppResult<Response> {
    let ctx = match super::enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let (_, client) = client_of(&state, &ctx, &id).await?;
    let query = query.map(|Query(q)| q).unwrap_or_default();
    let fiche = url(&ctx, &format!("/{}", client.id));
    let body = stored_infos(&state.db, client.id).await?;
    let editing = query.modifier.as_deref() == Some("1");
    Ok(infos_reply(
        &headers,
        StatusCode::OK,
        Infos::new(&fiche, body, editing, None),
    ))
}

/// Un texte saisi : retours à la ligne de Windows ramenés à `\n`, sans
/// espaces en fin.
fn clean_body(raw: &str) -> String {
    raw.replace("\r\n", "\n").trim_end().to_string()
}

/// `POST /1life/clients/{id}/infos` : enregistre les infos. Avec `?auto=1`
/// (enregistrement automatique pendant la frappe), répond « Enregistré à
/// 14:32 » ; sinon (« Terminé »), l'encart rendu.
pub async fn save_infos(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    query: Result<Query<InfosQuery>, QueryRejection>,
    form: Result<Form<InfosForm>, FormRejection>,
) -> AppResult<Response> {
    let ctx = match super::enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let (_, client) = client_of(&state, &ctx, &id).await?;
    let auto = query
        .map(|Query(q)| q)
        .unwrap_or_default()
        .auto
        .is_some_and(|a| a == "1");
    let body = clean_body(&form.map(|Form(f)| f.body).unwrap_or_default());
    let fiche = url(&ctx, &format!("/{}", client.id));
    if body.chars().count() > INFOS_MAX {
        let error = "Les infos font 20 000 caractères au plus : rien n'est enregistré.".to_string();
        if auto {
            return Ok(
                (StatusCode::UNPROCESSABLE_ENTITY, Html(web::escape(&error))).into_response(),
            );
        }
        return Ok(infos_reply(
            &headers,
            StatusCode::UNPROCESSABLE_ENTITY,
            Infos::new(&fiche, body, true, Some(error)),
        ));
    }
    let user = ctx.user.id;
    let now = time::now();
    sqlx::query!(
        "INSERT INTO onelife_client_infos (folder_id, body_md, updated_at, updated_by) VALUES (?, ?, ?, ?)
         ON CONFLICT (folder_id) DO UPDATE SET body_md = excluded.body_md,
                                               updated_at = excluded.updated_at,
                                               updated_by = excluded.updated_by",
        client.id,
        body,
        now,
        user
    )
    .execute(&state.db)
    .await?;
    if auto {
        return Ok(Html(format!("Enregistré à {}", time::hm(now))).into_response());
    }
    Ok(infos_reply(
        &headers,
        StatusCode::OK,
        Infos::new(&fiche, body, false, None),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> i64 {
        let date = NaiveDate::from_ymd_opt(y, m, d).unwrap();
        time::local_to_ts(date.and_hms_opt(h, min, 0).unwrap(), time::PARIS)
    }

    #[test]
    fn prochaine_journee_en_toutes_lettres() {
        let now = at(2026, 11, 26, 10, 0);
        assert_eq!(
            when(at(2026, 12, 3, 9, 0), at(2026, 12, 3, 17, 0), false, now),
            "jeudi 3 décembre, 9 h – 17 h (dans 7 jours)"
        );
        assert_eq!(
            when(
                at(2026, 11, 27, 13, 30),
                at(2026, 11, 27, 17, 0),
                false,
                now
            ),
            "vendredi 27 novembre, 13 h 30 – 17 h (demain)"
        );
        // Une journée en cours compte : « aujourd'hui ».
        assert_eq!(
            when(at(2026, 11, 26, 9, 0), at(2026, 11, 26, 12, 30), false, now),
            "jeudi 26 novembre, 9 h – 12 h 30 (aujourd'hui)"
        );
    }

    #[test]
    fn nombre_de_fichiers() {
        assert_eq!(files_label(0), "Aucun fichier");
        assert_eq!(files_label(1), "1 fichier");
        assert_eq!(files_label(4), "4 fichiers");
    }

    #[test]
    fn le_dossier_des_clients_laisse_la_place_aux_sous_dossiers() {
        // Dossier des clients au niveau 3, client au 4, Audit au 5.
        assert_eq!(FOLDER_MAX_DEPTH + 2, folders::MAX_DEPTH);
    }
}
