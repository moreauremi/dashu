//! 1Life, module Audit (SPEC, « 1Life — Audit » et « 1Life — Audit en
//! Word »).
//!
//! * Un audit, c'est un client (son dossier), une note où se prennent les
//!   notes, un modèle, une date, un titre et un approbateur. Il est au compte
//!   qui l'a commencé, et il se voit tant que sa note de notes se voit
//!   (`space_access`, qui cache la corbeille).
//! * « Générer l'audit » remplit le modèle, y joint les notes et les envoie à
//!   l'IA en tâche de fond ([`super::generation`]). Le résultat devient une
//!   nouvelle note dans le même dossier : v1, v2… Les points non repris, eux,
//!   restent sur la page de l'audit, jamais dans la note.
//! * Tout passe par [`super::enter`] : 404 pour un compte non autorisé ou
//!   sans session ; l'audit d'un autre compte répond la même 404.
//! * Hors de l'API, comme le reste de 1Life : les notes produites restent des
//!   notes ordinaires.
//!
//! Dépôt public : aucun vrai nom de client ni contenu de 1Life ici.

use askama::Template;
use axum::extract::rejection::{FormRejection, QueryRejection};
use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{Datelike, NaiveDate};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::AppState;
use crate::auth::session::{Ctx, Shell};
use crate::folders::{self, Folder};
use crate::time;
use crate::web::{self, AppError, AppResult, see_other};

use super::clients::{self, Place};
use super::fields::{self, FIELDS};
use super::generation::{self, DAILY_LIMIT, MAX_INPUT_TOKENS, Var, anthropic};
use super::settings::{self, Settings};
use super::templates::{self, BUILTIN_KEY, Choice};

/// Titre d'un audit, sans précision.
pub const DEFAULT_TITLE: &str = "Rapport d'audit ERP";
/// Longueur maximale du titre et de l'approbateur.
const TEXT_MAX: usize = 200;
/// Le sous-dossier d'un client où vont les notes et les versions.
const AUDIT_FOLDER: &str = "Audit";

/// La consigne système : générique, rien de propre à 1Life.
pub const SYSTEM: &str = "Vous rédigez des documents d'audit d'ERP pour un consultant, en français professionnel, en vouvoyant le lecteur.

Vous recevez deux textes : le modèle du document, entre <modele> et </modele>, et les notes prises pendant l'audit, entre <notes> et </notes>.

Règles :
1. Suivez les titres du modèle et leur ordre. Appliquez chaque consigne du modèle (une citation qui commence par « Consigne : »), sans la recopier.
2. N'inventez rien : ce qui manque devient « [À compléter] », et les mentions « [À compléter : …] » du modèle restent telles quelles. Gardez tels quels les chiffres, les noms propres et les termes d'Open-Prod.
3. Ne parlez jamais des notes, de leur forme ni de leurs marqueurs. Dans les notes, « OP: » marque une réponse d'Open-Prod, « manque: » une information manquante, « statut: » un état d'avancement et « !! » un point bloquant.
4. Les notes peuvent être rangées par service, parfois en tableaux « Besoin | Piste ». Redistribuez chaque élément dans le domaine du modèle qui lui correspond. Un constat n'apparaît qu'une fois, dans le domaine le plus pertinent ; ailleurs, renvoyez-y par « voir § X ».
5. Quand des éléments d'une liste restent à confirmer, écrivez « à confirmer » une seule fois pour la liste, jamais à chaque puce.
6. Répondez en Markdown seulement (titres, listes, cases à cocher, tableaux, citations), sans bloc de code autour ni phrase d'introduction.
7. Après le document, écrivez une ligne qui contient exactement « === POINTS NON REPRIS === », puis, une puce par élément, les informations des notes qui ne figurent pas dans le document. S'il n'y en a aucune, écrivez « - Aucun ».";

/// Adresse d'une page du module dans l'espace actif.
fn url(ctx: &Ctx, rest: &str) -> String {
    format!("/e/{}/1life/audit{rest}", ctx.active.slug)
}

/// « 27 novembre 2026 »
pub fn date_label(date: NaiveDate) -> String {
    format!("{} {}", time::day_month(date), date.year())
}

fn parse_date(raw: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d").ok()
}

// ---------------------------------------------------------------------
// Lecture
// ---------------------------------------------------------------------

/// Un audit du compte, dont la note de notes se voit.
#[derive(Debug, Clone)]
pub(crate) struct Audit {
    pub id: i64,
    pub client_id: i64,
    pub notes_id: i64,
    pub template: String,
    pub title: String,
    pub date: NaiveDate,
    pub approver: String,
    /// La dernière version qui se voit : son numéro et sa note.
    pub latest: Option<(i64, i64)>,
}

/// Les audits du compte, du plus récent au plus ancien.
async fn audits(db: &SqlitePool, user: i64) -> AppResult<Vec<Audit>> {
    let rows = sqlx::query!(
        r#"SELECT a.id AS "id!", a.client_id, a.notes_id, a.template, a.title, a.audit_date, a.approver,
                  (SELECT v.number FROM onelife_audit_versions v JOIN notes vn ON vn.id = v.note_id
                    WHERE v.audit_id = a.id
                      AND vn.space_id IN (SELECT space_id FROM space_access WHERE user_id = ?1)
                    ORDER BY v.number DESC LIMIT 1) AS "latest_number?: i64",
                  (SELECT v.note_id FROM onelife_audit_versions v JOIN notes vn ON vn.id = v.note_id
                    WHERE v.audit_id = a.id
                      AND vn.space_id IN (SELECT space_id FROM space_access WHERE user_id = ?1)
                    ORDER BY v.number DESC LIMIT 1) AS "latest_note?: i64"
           FROM onelife_audits a JOIN notes n ON n.id = a.notes_id
           WHERE a.user_id = ?1
             AND n.space_id IN (SELECT space_id FROM space_access WHERE user_id = ?1)
           ORDER BY a.audit_date DESC, a.id DESC"#,
        user
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| Audit {
            id: r.id,
            client_id: r.client_id,
            notes_id: r.notes_id,
            template: r.template,
            title: r.title,
            date: parse_date(&r.audit_date).unwrap_or_default(),
            approver: r.approver,
            latest: r.latest_number.zip(r.latest_note),
        })
        .collect())
}

/// Un audit du compte d'après l'adresse ; 404 sinon (numéro illisible,
/// autre compte, notes à la corbeille).
async fn audit_of(db: &SqlitePool, user: i64, raw: &str) -> AppResult<Audit> {
    let id: i64 = raw.parse().map_err(|_| AppError::NotFound)?;
    audits(db, user)
        .await?
        .into_iter()
        .find(|a| a.id == id)
        .ok_or(AppError::NotFound)
}

/// Le nom d'un client, d'après son dossier.
fn client_name(all: &[Folder], id: i64) -> String {
    all.iter()
        .find(|f| f.id == id)
        .map_or_else(|| "Client introuvable".to_string(), |f| f.name.clone())
}

/// Une ligne de la liste des audits (page Audit et fiche client).
pub struct Row {
    pub client: String,
    pub date: String,
    pub title: String,
    pub generated: bool,
    pub href: String,
    pub notes: String,
    /// La dernière version.
    pub latest: Option<Latest>,
}

/// Le lien vers la dernière version d'un audit.
pub struct Latest {
    pub href: String,
    /// « v2 »
    pub label: String,
}

fn row(ctx: &Ctx, all: &[Folder], audit: &Audit) -> Row {
    let page = url(ctx, &format!("/{}", audit.id));
    let back = crate::files::with_origin("", Some(&page));
    Row {
        client: client_name(all, audit.client_id),
        date: date_label(audit.date),
        title: audit.title.clone(),
        generated: audit.latest.is_some(),
        notes: format!("/notes/{}{back}", audit.notes_id),
        latest: audit.latest.map(|(number, note)| Latest {
            href: format!("/notes/{note}{back}"),
            label: format!("v{number}"),
        }),
        href: page,
    }
}

/// Les audits d'un client, pour sa fiche.
pub(crate) async fn rows_for_client(
    state: &AppState,
    ctx: &Ctx,
    client: i64,
) -> AppResult<Vec<Row>> {
    let all = folders::all(&state.db, ctx.user.id).await?;
    Ok(audits(&state.db, ctx.user.id)
        .await?
        .iter()
        .filter(|a| a.client_id == client)
        .map(|a| row(ctx, &all, a))
        .collect())
}

/// Adresse de « Commencer un audit » pour un client.
pub(crate) fn new_url(ctx: &Ctx, client: Option<i64>) -> String {
    match client {
        Some(id) => url(ctx, &format!("/nouveau?client={id}")),
        None => url(ctx, "/nouveau"),
    }
}

// ---------------------------------------------------------------------
// La page Audit : la liste
// ---------------------------------------------------------------------

/// Un client du filtre.
pub struct FilterChoice {
    pub id: i64,
    pub name: String,
    pub selected: bool,
}

#[derive(Template)]
#[template(path = "onelife/audits.html")]
struct ListPage {
    shell: Shell,
    home: String,
    list_url: String,
    new_url: String,
    help_url: String,
    rows: Vec<Row>,
    clients: Vec<FilterChoice>,
    filtered: bool,
}

#[derive(Deserialize, Default)]
pub struct ListQuery {
    client: Option<String>,
}

/// `GET /1life/audit?client=…` : tous les audits du compte.
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
    let wanted: Option<i64> = query.client.as_deref().and_then(|c| c.trim().parse().ok());
    let all = folders::all(&state.db, ctx.user.id).await?;
    let audits = audits(&state.db, ctx.user.id).await?;
    let mut clients: Vec<FilterChoice> = Vec::new();
    for audit in &audits {
        if !clients.iter().any(|c| c.id == audit.client_id) {
            clients.push(FilterChoice {
                id: audit.client_id,
                name: client_name(&all, audit.client_id),
                selected: Some(audit.client_id) == wanted,
            });
        }
    }
    clients.sort_by_cached_key(|c| crate::commands::fold(&c.name));
    let rows = audits
        .iter()
        .filter(|a| wanted.is_none_or(|w| a.client_id == w))
        .map(|a| row(&ctx, &all, a))
        .collect();
    Ok(web::html(ListPage {
        shell: ctx.shell(super::TOOL).await?,
        home: format!("/e/{}/1life", ctx.active.slug),
        list_url: url(&ctx, ""),
        new_url: new_url(&ctx, None),
        help_url: url(&ctx, "/modeles"),
        rows,
        clients,
        filtered: wanted.is_some(),
    }))
}

// ---------------------------------------------------------------------
// Commencer un audit
// ---------------------------------------------------------------------

/// Un client proposé.
pub struct ClientChoice {
    pub id: i64,
    pub name: String,
    pub selected: bool,
}

#[derive(Template)]
#[template(path = "onelife/audit_new.html")]
struct NewPage {
    shell: Shell,
    list_url: String,
    action: String,
    clients_url: String,
    /// Pas de dossier des clients, ou aucun client : rien à choisir.
    no_clients: bool,
    clients: Vec<ClientChoice>,
    date: String,
    title: String,
    approver: String,
    templates: Vec<Choice>,
    template: String,
    error: Option<String>,
    max: usize,
}

#[derive(Deserialize, Default)]
pub struct NewQuery {
    client: Option<String>,
}

#[derive(Deserialize, Default, Clone)]
pub struct NewForm {
    #[serde(default)]
    client: String,
    #[serde(default)]
    date: String,
    #[serde(default)]
    titre: String,
    #[serde(default)]
    approbateur: String,
    #[serde(default)]
    modele: String,
}

/// Le formulaire « Commencer un audit », pour [`new_page`] et pour un refus.
async fn render_new(
    state: &AppState,
    ctx: &Ctx,
    form: NewForm,
    error: Option<String>,
) -> AppResult<Response> {
    let place = clients::place(state, ctx).await?;
    let wanted: Option<i64> = form.client.trim().parse().ok();
    let clients: Vec<ClientChoice> = match &place {
        Place::Ready(book) => book
            .clients()
            .iter()
            .map(|c| ClientChoice {
                id: c.id,
                name: c.name.clone(),
                selected: Some(c.id) == wanted,
            })
            .collect(),
        _ => Vec::new(),
    };
    let settings = settings::load(&state.db, ctx.user.id).await?;
    let templates = templates::choices(state, ctx, &settings).await?;
    let template = if form.modele.is_empty() {
        settings.template().to_string()
    } else {
        form.modele.clone()
    };
    let status = if error.is_some() {
        StatusCode::UNPROCESSABLE_ENTITY
    } else {
        StatusCode::OK
    };
    let page = NewPage {
        shell: ctx.shell(super::TOOL).await?,
        list_url: url(ctx, ""),
        action: url(ctx, ""),
        clients_url: format!("/e/{}/1life/clients", ctx.active.slug),
        no_clients: clients.is_empty(),
        clients,
        date: form.date,
        title: form.titre,
        approver: form.approbateur,
        templates,
        template,
        error,
        max: TEXT_MAX,
    };
    Ok((status, web::html(page)).into_response())
}

/// `GET /1life/audit/nouveau?client=…` : le formulaire, prérempli (date du
/// jour, titre par défaut, approbateur et modèle des réglages).
pub async fn new_page(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    query: Result<Query<NewQuery>, QueryRejection>,
) -> AppResult<Response> {
    let ctx = match super::enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let query = query.map(|Query(q)| q).unwrap_or_default();
    let settings = settings::load(&state.db, ctx.user.id).await?;
    let form = NewForm {
        client: query.client.unwrap_or_default(),
        date: time::paris_date(time::now()).format("%Y-%m-%d").to_string(),
        titre: DEFAULT_TITLE.into(),
        approbateur: settings.approver.clone(),
        modele: String::new(),
    };
    render_new(&state, &ctx, form, None).await
}

/// `POST /1life/audit` : crée la note de notes dans le sous-dossier Audit du
/// client (créé s'il manque), puis l'audit, et ouvre l'éditeur.
pub async fn create(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    form: Result<Form<NewForm>, FormRejection>,
) -> AppResult<Response> {
    let ctx = match super::enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let form = form.map(|Form(f)| f).unwrap_or_default();
    let user = ctx.user.id;
    let Place::Ready(book) = clients::place(&state, &ctx).await? else {
        ctx.flash_error("Choisis d'abord le dossier des clients.")
            .await?;
        return Ok(see_other(&format!("/e/{}/1life/clients", ctx.active.slug)));
    };
    // Un dossier qui n'est pas un client du compte : 404, comme partout.
    let client_id: i64 = form.client.trim().parse().map_err(|_| AppError::NotFound)?;
    let client = book.client(client_id).cloned().ok_or(AppError::NotFound)?;
    let Some(date) = parse_date(&form.date) else {
        let error = "Date illisible : choisis le jour de l'audit.".to_string();
        return render_new(&state, &ctx, form, Some(error)).await;
    };
    let title = match form.titre.trim() {
        "" => DEFAULT_TITLE.to_string(),
        t => t.to_string(),
    };
    let approver = form.approbateur.trim().to_string();
    if title.chars().count() > TEXT_MAX || approver.chars().count() > TEXT_MAX {
        let error = format!("{TEXT_MAX} caractères au plus pour le titre et l'approbateur.");
        return render_new(&state, &ctx, form, Some(error)).await;
    }
    let settings = settings::load(&state.db, user).await?;
    let template = form.modele.trim();
    let template = if templates::choices(&state, &ctx, &settings)
        .await?
        .iter()
        .any(|c| c.key == template)
    {
        template.to_string()
    } else {
        BUILTIN_KEY.to_string()
    };

    // Le sous-dossier Audit du client, créé s'il manque. S'il ne tient pas
    // (cinq niveaux au plus), les notes vont dans le dossier du client.
    let existing = book
        .all
        .iter()
        .find(|f| f.parent_id == Some(client.id) && f.name.to_lowercase() == "audit")
        .map(|f| f.id);
    let folder = match existing {
        Some(id) => id,
        None => {
            match folders::create_in(
                &state.db,
                user,
                client.space_id,
                Some(client.id),
                AUDIT_FOLDER,
            )
            .await?
            {
                Ok((id, _)) => id,
                Err(error) => {
                    tracing::info!(user, client = client.id, %error, "1life : pas de dossier Audit");
                    client.id
                }
            }
        }
    };
    let now = time::now();
    let notes_title: String = format!("Notes d'audit — {} — {}", client.name, date_label(date))
        .chars()
        .take(crate::notes::TITLE_MAX_CHARS)
        .collect();
    let audit_date = date.format("%Y-%m-%d").to_string();
    let mut tx = state.db.begin().await?;
    let notes_id = sqlx::query!(
        "INSERT INTO notes (user_id, space_id, folder_id, title, body_md, created_at, updated_at)
         VALUES (?, ?, ?, ?, '', ?, ?)",
        user,
        client.space_id,
        folder,
        notes_title,
        now,
        now
    )
    .execute(&mut *tx)
    .await?
    .last_insert_rowid();
    let id = sqlx::query!(
        "INSERT INTO onelife_audits (user_id, client_id, notes_id, template, title, audit_date, approver, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        user,
        client.id,
        notes_id,
        template,
        title,
        audit_date,
        approver,
        now
    )
    .execute(&mut *tx)
    .await?
    .last_insert_rowid();
    tx.commit().await?;
    tracing::info!(user, audit = id, "1life : audit commencé");
    let page = url(&ctx, &format!("/{id}"));
    Ok(see_other(&crate::files::with_origin(
        &format!("/notes/{notes_id}"),
        Some(&page),
    )))
}

// ---------------------------------------------------------------------
// La demande : le modèle rempli et les notes
// ---------------------------------------------------------------------

/// Ce qui partirait chez Anthropic pour un audit, et ce qu'on en sait avant.
struct Prepared {
    /// Le message : le modèle rempli, puis les notes.
    user: String,
    /// Les variables vides ou inconnues, par leur nom lisible.
    missing: Vec<String>,
    template_label: String,
    /// Le modèle choisi n'existe plus : le modèle intégré l'a remplacé.
    replaced: bool,
    /// Tokens d'entrée estimés, consigne système comprise.
    estimate: u64,
}

/// Les variables d'un audit : les champs de la fiche, l'audit, le consultant.
async fn variables(
    state: &AppState,
    ctx: &Ctx,
    audit: &Audit,
    client: &str,
    settings: &Settings,
) -> AppResult<Vec<Var>> {
    let values = fields::stored(&state.db, audit.client_id).await?;
    let mut vars: Vec<Var> = FIELDS
        .iter()
        .map(|f| Var {
            name: format!("client.{}", f.key),
            label: f.label.to_string(),
            value: fields::value(&values, f.key, client),
        })
        .collect();
    let mut push = |name: &str, label: &str, value: String| {
        vars.push(Var {
            name: name.into(),
            label: label.into(),
            value,
        });
    };
    push("audit.date", "Date de l'audit", date_label(audit.date));
    push("audit.titre", "Titre de l'audit", audit.title.clone());
    push("audit.approbateur", "Approbateur", audit.approver.clone());
    push(
        "consultant.nom",
        "Nom du consultant",
        settings.consultant(&ctx.user),
    );
    push(
        "consultant.fonction",
        "Fonction du consultant",
        settings.consultant_role.clone(),
    );
    push(
        "consultant.email",
        "E-mail du consultant",
        settings.consultant_email.clone(),
    );
    Ok(vars)
}

async fn prepare(state: &AppState, ctx: &Ctx, audit: &Audit, client: &str) -> AppResult<Prepared> {
    let settings = settings::load(&state.db, ctx.user.id).await?;
    let template = templates::load(state, ctx, &audit.template).await?;
    let vars = variables(state, ctx, audit, client, &settings).await?;
    let filled = generation::fill(&template.body, &vars);
    let notes: String = sqlx::query_scalar!(
        "SELECT body_md FROM notes WHERE id = ?2
           AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?1)",
        ctx.user.id,
        audit.notes_id
    )
    .fetch_optional(&state.db)
    .await?
    .unwrap_or_default();
    let user = format!(
        "<modele>\n{}\n</modele>\n\n<notes>\n{}\n</notes>\n\nRédigez le document d'audit à partir de ces notes, en suivant le modèle.",
        filled.text.trim(),
        notes.trim()
    );
    let estimate = generation::estimate_tokens(SYSTEM) + generation::estimate_tokens(&user);
    Ok(Prepared {
        user,
        missing: filled.missing,
        template_label: template.label,
        replaced: template.replaced,
        estimate,
    })
}

/// Générations du compte depuis minuit, heure de Paris.
async fn runs_today(db: &SqlitePool, user: i64) -> AppResult<i64> {
    let since = time::day_start(time::paris_date(time::now()));
    Ok(sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "n!: i64" FROM onelife_audit_runs WHERE user_id = ? AND started_at >= ?"#,
        user,
        since
    )
    .fetch_one(db)
    .await?)
}

/// Un modèle d'IA proposé, avec son coût estimé.
pub struct ModelOption {
    pub id: String,
    pub label: String,
    pub selected: bool,
}

fn model_options(state: &AppState, settings: &Settings, estimate: u64) -> Vec<ModelOption> {
    let models = &state.config.ai.models;
    let chosen = settings.model(models).to_string();
    models
        .iter()
        .map(|m| ModelOption {
            label: format!(
                "{} — {}",
                generation::model_label(m),
                generation::cost_ceiling(m, estimate)
            ),
            selected: *m == chosen,
            id: m.clone(),
        })
        .collect()
}

/// Le formulaire « Générer l'audit » (page de l'audit et éditeur).
pub struct Generate {
    pub action: String,
    pub models: Vec<ModelOption>,
    /// La clé est là : sinon, bouton grisé et « IA non configurée ».
    pub ready: bool,
    /// Trop long pour être envoyé : le message, et le bouton grisé.
    pub too_long: Option<String>,
    /// « ≈ 12 345 tokens envoyés »
    pub estimate: String,
}

fn too_long(estimate: u64) -> Option<String> {
    (estimate > MAX_INPUT_TOKENS).then(|| {
        format!(
            "Demande trop longue : environ {} tokens, {} au plus. Rien n'est envoyé : raccourcis les notes ou le modèle.",
            generation::thousands(estimate as i64),
            generation::thousands(MAX_INPUT_TOKENS as i64)
        )
    })
}

/// Lien vers l'aide du README quand l'IA n'est pas configurée.
pub const README_AI: &str = "https://github.com/moreauremi/dashu/blob/v1/README.md#ia-de-1life";

// ---------------------------------------------------------------------
// La page d'un audit
// ---------------------------------------------------------------------

/// Une version générée.
pub struct VersionRow {
    pub label: String,
    pub title: String,
    pub href: String,
    pub model: String,
    pub tokens: String,
    pub cost: String,
    pub when: String,
}

/// Un point non repris.
pub struct Point {
    pub id: i64,
    pub text: String,
    pub done: bool,
    pub action: String,
}

/// La dernière génération, si elle compte pour la page.
pub struct RunState {
    pub running: bool,
    /// Un échec, ou un avertissement après une réussite (séparateur absent).
    pub message: Option<String>,
    pub failed: bool,
    pub poll: String,
}

#[derive(Template)]
#[template(path = "onelife/audit.html")]
struct AuditPage {
    shell: Shell,
    list_url: String,
    help_url: String,
    title: String,
    client: String,
    fiche: Option<String>,
    date: String,
    approver: String,
    template_label: String,
    replaced: bool,
    notes: String,
    missing: Vec<String>,
    generate: Generate,
    run: RunState,
    today: i64,
    limit: i64,
    versions: Vec<VersionRow>,
    points: Vec<Point>,
    /// « v2 », pour le titre des points non repris.
    points_of: String,
    word: Option<String>,
    readme: &'static str,
}

#[derive(Template)]
#[template(path = "onelife/_generation.html")]
struct GenerationFragment {
    run: RunState,
}

#[derive(Template)]
#[template(path = "onelife/_point.html")]
struct PointFragment {
    point: Point,
}

/// La dernière génération de l'audit, par ce compte.
async fn last_run(db: &SqlitePool, user: i64, audit: i64) -> AppResult<Option<(String, String)>> {
    Ok(sqlx::query!(
        "SELECT status, message FROM onelife_audit_runs WHERE user_id = ? AND audit_id = ?
         ORDER BY id DESC LIMIT 1",
        user,
        audit
    )
    .fetch_optional(db)
    .await?
    .map(|r| (r.status, r.message)))
}

fn run_state(ctx: &Ctx, audit: i64, last: Option<(String, String)>) -> RunState {
    let (status, message) = last.unwrap_or_default();
    RunState {
        running: status == "running",
        failed: status == "failed",
        message: (!message.is_empty() && status != "running").then_some(message),
        poll: url(ctx, &format!("/{audit}/generation")),
    }
}

/// `GET /1life/audit/{id}` : la page d'un audit.
pub async fn show(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Response> {
    let ctx = match super::enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let user = ctx.user.id;
    let audit = audit_of(&state.db, user, &id).await?;
    let all = folders::all(&state.db, user).await?;
    let client = client_name(&all, audit.client_id);
    let fiche = match clients::place(&state, &ctx).await? {
        Place::Ready(book) if book.client(audit.client_id).is_some() => Some(format!(
            "/e/{}/1life/clients/{}",
            ctx.active.slug, audit.client_id
        )),
        _ => None,
    };
    let page_url = url(&ctx, &format!("/{}", audit.id));
    let back = crate::files::with_origin("", Some(&page_url));
    let prepared = prepare(&state, &ctx, &audit, &client).await?;
    let settings = settings::load(&state.db, user).await?;

    let versions = sqlx::query!(
        r#"SELECT v.id AS "id!", v.number, v.note_id, v.model, v.input_tokens, v.output_tokens,
                  v.cost_micro, v.created_at, n.title
           FROM onelife_audit_versions v JOIN notes n ON n.id = v.note_id
           WHERE v.audit_id = ?2
             AND n.space_id IN (SELECT space_id FROM space_access WHERE user_id = ?1)
           ORDER BY v.number DESC"#,
        user,
        audit.id
    )
    .fetch_all(&state.db)
    .await?;
    let now = time::now();
    let latest_version = versions.first().map(|v| (v.id, v.number));
    let versions_rows = versions
        .into_iter()
        .map(|v| VersionRow {
            label: format!("v{}", v.number),
            title: crate::notes::display_title(&v.title),
            href: format!("/notes/{}{back}", v.note_id),
            model: generation::model_label(&v.model),
            tokens: format!(
                "{} + {} tokens",
                generation::thousands(v.input_tokens),
                generation::thousands(v.output_tokens)
            ),
            cost: if v.cost_micro < 0 {
                "coût inconnu".into()
            } else {
                generation::money(v.cost_micro)
            },
            when: time::ago(v.created_at, now),
        })
        .collect();
    let points = match latest_version {
        Some((version, _)) => sqlx::query!(
            r#"SELECT id AS "id!", text, done AS "done!: bool" FROM onelife_audit_points
               WHERE version_id = ? ORDER BY position"#,
            version
        )
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .map(|p| Point {
            action: url(&ctx, &format!("/{}/points/{}", audit.id, p.id)),
            id: p.id,
            text: p.text,
            done: p.done,
        })
        .collect(),
        None => Vec::new(),
    };
    let last = last_run(&state.db, user, audit.id).await?;
    let page = AuditPage {
        shell: ctx.shell(super::TOOL).await?,
        list_url: url(&ctx, ""),
        help_url: url(&ctx, "/modeles"),
        title: audit.title.clone(),
        client,
        fiche,
        date: date_label(audit.date),
        approver: audit.approver.clone(),
        template_label: prepared.template_label.clone(),
        replaced: prepared.replaced,
        notes: format!("/notes/{}{back}", audit.notes_id),
        missing: prepared.missing.clone(),
        generate: Generate {
            action: url(&ctx, &format!("/{}/generer", audit.id)),
            models: model_options(&state, &settings, prepared.estimate),
            ready: state.config.ai.key.is_some(),
            too_long: too_long(prepared.estimate),
            estimate: format!(
                "≈ {} tokens envoyés",
                generation::thousands(prepared.estimate as i64)
            ),
        },
        run: run_state(&ctx, audit.id, last),
        today: runs_today(&state.db, user).await?,
        limit: DAILY_LIMIT,
        versions: versions_rows,
        points,
        points_of: latest_version
            .map(|(_, n)| format!("v{n}"))
            .unwrap_or_default(),
        word: state
            .config
            .work_dir
            .is_some()
            .then(|| url(&ctx, &format!("/{}/word", audit.id))),
        readme: README_AI,
    };
    Ok(web::html(page))
}

// ---------------------------------------------------------------------
// Générer
// ---------------------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct GenerateForm {
    #[serde(default)]
    modele_ia: String,
}

/// Ce que la tâche de fond doit savoir.
struct Job {
    run: i64,
    user: i64,
    audit: Audit,
    client: String,
    request: anthropic::Request,
}

/// `POST /1life/audit/{id}/generer` : lance la génération en tâche de fond,
/// après les garde-fous (clé, taille, limite du jour, une à la fois), puis
/// ramène à la page de l'audit, qui suit la génération.
pub async fn generate(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    form: Result<Form<GenerateForm>, FormRejection>,
) -> AppResult<Response> {
    let ctx = match super::enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let user = ctx.user.id;
    let audit = audit_of(&state.db, user, &id).await?;
    let page = see_other(&url(&ctx, &format!("/{}", audit.id)));
    let form = form.map(|Form(f)| f).unwrap_or_default();
    let ai = &state.config.ai;
    let model = form.modele_ia.trim();
    if !ai.models.iter().any(|m| m == model) {
        return Err(AppError::BadRequest(
            "Modèle d'IA inconnu : choisis-en un dans la liste.".into(),
        ));
    }
    if ai.key.is_none() {
        ctx.flash_error("IA non configurée : rien n'est envoyé (README, « IA de 1Life »).")
            .await?;
        return Ok(page);
    }
    let all = folders::all(&state.db, user).await?;
    let client = client_name(&all, audit.client_id);
    let prepared = prepare(&state, &ctx, &audit, &client).await?;
    if let Some(message) = too_long(prepared.estimate) {
        ctx.flash_error(message).await?;
        return Ok(page);
    }
    if runs_today(&state.db, user).await? >= DAILY_LIMIT {
        ctx.flash_error(format!(
            "{DAILY_LIMIT} générations par jour au plus : rien n'est envoyé. Réessaie demain."
        ))
        .await?;
        return Ok(page);
    }
    let now = time::now();
    let inserted = sqlx::query!(
        "INSERT INTO onelife_audit_runs (user_id, audit_id, model, status, started_at)
         VALUES (?, ?, ?, 'running', ?)",
        user,
        audit.id,
        model,
        now
    )
    .execute(&state.db)
    .await;
    let run = match inserted {
        Ok(done) => done.last_insert_rowid(),
        Err(error)
            if error
                .as_database_error()
                .is_some_and(|e| e.is_unique_violation()) =>
        {
            ctx.flash_error("Une génération est déjà en cours : attends qu'elle finisse.")
                .await?;
            return Ok(page);
        }
        Err(error) => return Err(error.into()),
    };
    tracing::info!(
        user,
        audit = audit.id,
        run,
        model,
        "1life : génération lancée"
    );
    let job = Job {
        run,
        user,
        client,
        request: anthropic::Request {
            model: model.to_string(),
            system: SYSTEM.to_string(),
            user: prepared.user,
        },
        audit,
    };
    let background = state.clone();
    tokio::spawn(async move {
        run_job(background, job).await;
    });
    Ok(page)
}

/// La génération elle-même : l'appel, puis la note, la version et ses
/// points, d'un seul coup. En cas d'échec, rien n'est créé : seul le message
/// reste, sur la page de l'audit.
async fn run_job(state: AppState, job: Job) {
    let ai = &state.config.ai;
    let outcome = match anthropic::call(&state.http, ai, &job.request).await {
        Ok(reply) => match save_version(&state, &job, reply).await {
            Ok(outcome) => outcome,
            Err(error) => {
                tracing::error!(error = ?error, run = job.run, "1life : génération non enregistrée");
                Err("La génération n'a pas pu être enregistrée : rien n'est créé.".to_string())
            }
        },
        Err(failure) => {
            tracing::info!(run = job.run, ?failure, "1life : génération en échec");
            Err(failure.message(ai.timeout.as_secs()))
        }
    };
    let now = time::now();
    let (status, message) = match outcome {
        Ok(warning) => ("done", warning),
        Err(message) => ("failed", message),
    };
    let saved = sqlx::query!(
        "UPDATE onelife_audit_runs SET status = ?, message = ?, finished_at = ?
         WHERE id = ? AND status = 'running'",
        status,
        message,
        now,
        job.run
    )
    .execute(&state.db)
    .await;
    if let Err(error) = saved {
        tracing::error!(%error, run = job.run, "1life : fin de génération non enregistrée");
    }
}

/// Enregistre une réponse : la note, la version et ses points, dans une
/// transaction. Rend l'éventuel avertissement (séparateur absent), ou un
/// message d'échec lisible.
async fn save_version(
    state: &AppState,
    job: &Job,
    reply: anthropic::Reply,
) -> AppResult<Result<String, String>> {
    let split = generation::split_points(&reply.text);
    if split.document.trim().is_empty() {
        return Ok(Err(anthropic::Failure::Unreadable.message(0)));
    }
    let user = job.user;
    // La note de notes, là où elle est maintenant : la version va à côté.
    let notes = sqlx::query!(
        "SELECT space_id, folder_id FROM notes WHERE id = ?2
           AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?1)",
        user,
        job.audit.notes_id
    )
    .fetch_optional(&state.db)
    .await?;
    let Some(notes) = notes else {
        return Ok(Err(
            "Les notes de l'audit ne sont plus accessibles (corbeille ?) : rien n'est créé.".into(),
        ));
    };
    let now = time::now();
    let input = u64::try_from(reply.input_tokens).unwrap_or(0);
    let output = u64::try_from(reply.output_tokens).unwrap_or(0);
    let cost = generation::cost_micro(&job.request.model, input, output).unwrap_or(-1);
    let mut tx = state.db.begin().await?;
    let number: i64 = sqlx::query_scalar!(
        r#"SELECT COALESCE(MAX(number), 0) + 1 AS "n!: i64" FROM onelife_audit_versions WHERE audit_id = ?"#,
        job.audit.id
    )
    .fetch_one(&mut *tx)
    .await?;
    let mut title = format!("Audit — {} — {}", job.client, date_label(job.audit.date));
    if number > 1 {
        title.push_str(&format!(" (v{number})"));
    }
    let title: String = title.chars().take(crate::notes::TITLE_MAX_CHARS).collect();
    let note = sqlx::query!(
        "INSERT INTO notes (user_id, space_id, folder_id, title, body_md, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        user,
        notes.space_id,
        notes.folder_id,
        title,
        split.document,
        now,
        now
    )
    .execute(&mut *tx)
    .await?
    .last_insert_rowid();
    let version = sqlx::query!(
        "INSERT INTO onelife_audit_versions (audit_id, number, note_id, model, input_tokens, output_tokens, cost_micro, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        job.audit.id,
        number,
        note,
        job.request.model,
        reply.input_tokens,
        reply.output_tokens,
        cost,
        now
    )
    .execute(&mut *tx)
    .await?
    .last_insert_rowid();
    for (position, text) in split.points.iter().flatten().enumerate() {
        let position = position as i64;
        sqlx::query!(
            "INSERT INTO onelife_audit_points (version_id, position, text) VALUES (?, ?, ?)",
            version,
            position,
            text
        )
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query!(
        "UPDATE onelife_audit_runs SET version_id = ? WHERE id = ?",
        version,
        job.run
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    tracing::info!(
        user,
        audit = job.audit.id,
        version = number,
        input = reply.input_tokens,
        output = reply.output_tokens,
        "1life : audit généré"
    );
    Ok(Ok(if split.points.is_none() {
        "L'IA n'a pas séparé les points non repris : la liste est vide, relis le document."
            .to_string()
    } else {
        String::new()
    }))
}

/// Au démarrage : une génération coupée par un arrêt du serveur est marquée
/// échouée (rien n'a été créé), pour qu'une autre puisse partir.
pub async fn recover(db: &SqlitePool) -> anyhow::Result<()> {
    let now = time::now();
    let done = sqlx::query!(
        "UPDATE onelife_audit_runs
         SET status = 'failed', finished_at = ?,
             message = 'Génération interrompue par un redémarrage du serveur : rien n''est créé.'
         WHERE status = 'running'",
        now
    )
    .execute(db)
    .await?;
    if done.rows_affected() > 0 {
        tracing::info!(n = done.rows_affected(), "1life : générations interrompues");
    }
    Ok(())
}

/// `GET /1life/audit/{id}/generation` : l'indicateur (htmx, toutes les deux
/// secondes). Une fois la génération finie, la page se recharge sur le
/// résultat.
pub async fn status(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Response> {
    let ctx = match super::enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let audit = audit_of(&state.db, ctx.user.id, &id).await?;
    let last = last_run(&state.db, ctx.user.id, audit.id).await?;
    let run = run_state(&ctx, audit.id, last);
    let running = run.running;
    let mut response = web::html(GenerationFragment { run });
    if !running {
        response
            .headers_mut()
            .insert("hx-refresh", HeaderValue::from_static("true"));
    }
    Ok(response)
}

// ---------------------------------------------------------------------
// Points non repris
// ---------------------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct PointForm {
    done: Option<String>,
}

/// `POST /1life/audit/{id}/points/{point}` : coche ou décoche un point.
pub async fn toggle_point(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    Path((id, point)): Path<(String, String)>,
    headers: HeaderMap,
    form: Result<Form<PointForm>, FormRejection>,
) -> AppResult<Response> {
    let ctx = match super::enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let audit = audit_of(&state.db, ctx.user.id, &id).await?;
    let point: i64 = point.parse().map_err(|_| AppError::NotFound)?;
    let done = form
        .map(|Form(f)| f)
        .unwrap_or_default()
        .done
        .is_some_and(|d| !d.is_empty());
    let row = sqlx::query!(
        r#"UPDATE onelife_audit_points SET done = ?
           WHERE id = ? AND version_id IN (SELECT id FROM onelife_audit_versions WHERE audit_id = ?)
           RETURNING id AS "id!", text"#,
        done,
        point,
        audit.id
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;
    if !web::is_fragment_request(&headers) {
        return Ok(see_other(&url(&ctx, &format!("/{}", audit.id))));
    }
    Ok(web::html(PointFragment {
        point: Point {
            action: url(&ctx, &format!("/{}/points/{}", audit.id, row.id)),
            id: row.id,
            text: row.text,
            done,
        },
    }))
}

// ---------------------------------------------------------------------
// Aide
// ---------------------------------------------------------------------

/// Une variable, dans l'aide.
pub struct HelpVar {
    pub name: String,
    pub label: String,
}

#[derive(Template)]
#[template(path = "onelife/audit_help.html")]
struct HelpPage {
    shell: Shell,
    list_url: String,
    settings_url: String,
    vars: Vec<HelpVar>,
    builtin: &'static str,
}

/// Toutes les variables, dans l'ordre : la fiche client, l'audit, le
/// consultant.
pub fn variable_names() -> Vec<HelpVar> {
    let mut vars: Vec<HelpVar> = FIELDS
        .iter()
        .map(|f| HelpVar {
            name: format!("client.{}", f.key),
            label: f.label.to_string(),
        })
        .collect();
    for (name, label) in [
        ("audit.date", "Date de l'audit"),
        ("audit.titre", "Titre de l'audit"),
        ("audit.approbateur", "Approbateur"),
        ("consultant.nom", "Nom du consultant"),
        ("consultant.fonction", "Fonction du consultant"),
        ("consultant.email", "E-mail du consultant"),
    ] {
        vars.push(HelpVar {
            name: name.into(),
            label: label.into(),
        });
    }
    vars
}

/// `GET /1life/audit/modeles` : l'aide des modèles d'audit.
pub async fn help(ctx: Result<Ctx, Response>) -> AppResult<Response> {
    let ctx = match super::enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    Ok(web::html(HelpPage {
        shell: ctx.shell(super::TOOL).await?,
        list_url: url(&ctx, ""),
        settings_url: format!("/e/{}/1life/reglages", ctx.active.slug),
        vars: variable_names(),
        builtin: templates::BUILTIN,
    }))
}

// ---------------------------------------------------------------------
// Word
// ---------------------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct WordQuery {
    note: Option<String>,
}

/// `GET /1life/audit/{id}/word?note=…` : une note de l'audit en Word (la
/// dernière version, sans précision ; à défaut, les notes), avec le `.docx`
/// du dossier des modèles comme document de référence s'il y en a un.
pub async fn word(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    query: Result<Query<WordQuery>, QueryRejection>,
) -> AppResult<Response> {
    let ctx = match super::enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let user = ctx.user.id;
    let audit = audit_of(&state.db, user, &id).await?;
    if state.config.work_dir.is_none() {
        return Err(AppError::NotFound);
    }
    let query = query.map(|Query(q)| q).unwrap_or_default();
    let wanted: Option<i64> = match query.note.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(raw) => Some(raw.parse().map_err(|_| AppError::NotFound)?),
    };
    let note = match wanted {
        None => audit.latest.map_or(audit.notes_id, |(_, note)| note),
        Some(note) if note == audit.notes_id => note,
        Some(note) => {
            let ours = sqlx::query_scalar!(
                r#"SELECT 1 AS "ok!: i64" FROM onelife_audit_versions WHERE audit_id = ? AND note_id = ?"#,
                audit.id,
                note
            )
            .fetch_optional(&state.db)
            .await?;
            ours.map(|_| note).ok_or(AppError::NotFound)?
        }
    };
    let settings = settings::load(&state.db, user).await?;
    let reference = templates::reference_docx(&state, &ctx, &settings).await?;
    crate::notes::export::docx_download(&state, user, note, reference).await
}

// ---------------------------------------------------------------------
// L'éditeur d'une note d'audit
// ---------------------------------------------------------------------

/// Ce que l'en-tête de l'éditeur montre pour une note d'audit.
pub struct EditorLink {
    /// La page de l'audit.
    pub page: String,
    /// La note de notes (sinon, une version).
    pub is_notes: bool,
    /// Une version : « v2 · Haiku 5.5 · 0,003 $ ».
    pub info: String,
    pub generate: Option<Generate>,
    pub word: Option<String>,
    pub readme: &'static str,
}

/// La note `note` est-elle une note d'audit du compte ? Seulement pour un
/// compte autorisé à 1Life ; `None` sinon.
pub(crate) async fn editor_link(
    state: &AppState,
    ctx: &Ctx,
    note: i64,
) -> AppResult<Option<EditorLink>> {
    if ctx.tools.tool(super::TOOL).is_none() {
        return Ok(None);
    }
    let user = ctx.user.id;
    let found = sqlx::query!(
        r#"SELECT a.id AS "id!", v.number AS "number?: i64", v.model AS "model?: String",
                  v.cost_micro AS "cost?: i64"
           FROM onelife_audits a
           LEFT JOIN onelife_audit_versions v ON v.audit_id = a.id AND v.note_id = ?2
           WHERE a.user_id = ?1 AND (a.notes_id = ?2 OR v.note_id = ?2)
           LIMIT 1"#,
        user,
        note
    )
    .fetch_optional(&state.db)
    .await?;
    let Some(found) = found else {
        return Ok(None);
    };
    let Some(audit) = audits(&state.db, user)
        .await?
        .into_iter()
        .find(|a| a.id == found.id)
    else {
        return Ok(None);
    };
    let page = url(ctx, &format!("/{}", audit.id));
    let word = state
        .config
        .work_dir
        .is_some()
        .then(|| url(ctx, &format!("/{}/word?note={note}", audit.id)));
    let is_notes = audit.notes_id == note;
    let generate = if is_notes {
        let all = folders::all(&state.db, user).await?;
        let client = client_name(&all, audit.client_id);
        let prepared = prepare(state, ctx, &audit, &client).await?;
        let settings = settings::load(&state.db, user).await?;
        Some(Generate {
            action: url(ctx, &format!("/{}/generer", audit.id)),
            models: model_options(state, &settings, prepared.estimate),
            ready: state.config.ai.key.is_some(),
            too_long: too_long(prepared.estimate),
            estimate: format!(
                "≈ {} tokens envoyés",
                generation::thousands(prepared.estimate as i64)
            ),
        })
    } else {
        None
    };
    let info = match (found.number, found.model, found.cost) {
        (Some(number), Some(model), Some(cost)) => format!(
            "v{number} · {} · {}",
            generation::model_label(&model),
            if cost < 0 {
                "coût inconnu".to_string()
            } else {
                generation::money(cost)
            }
        ),
        _ => String::new(),
    };
    Ok(Some(EditorLink {
        page,
        is_notes,
        info,
        generate,
        word,
        readme: README_AI,
    }))
}
