//! Chargement des fixtures dans la base jetable de la démo.
//!
//! Presque tout passe par l'API JSON (`/api/v1`), comme le ferait un
//! assistant : les mêmes vérifications qu'en production s'appliquent. Seuls
//! trois réglages n'ont pas de route et vont directement en base : les dates
//! des notes (pour qu'elles n'aient pas toutes été écrites à la même minute),
//! la source de l'emploi du temps (aucun flux iCal n'est téléchargé) et ses
//! cours, rangés par la même fonction que la synchro (`calendar::sync::store`).
//!
//! Requêtes SQL écrites avec `sqlx::query` (non vérifiées à la compilation) :
//! ce module n'est compilé qu'avec la feature `demo`, et `.sqlx/` ne garde
//! que les requêtes du serveur.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use anyhow::{Context, bail};
use axum::http::{Method, StatusCode};
use chrono::{Datelike, Days, NaiveDate, NaiveTime};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Browser, media};
use crate::calendar::ical::ParsedEvent;
use crate::{AppState, time};

#[derive(Deserialize)]
pub struct Fixtures {
    pub user: User,
    spaces: Vec<SpaceTools>,
    folders: Vec<Folder>,
    notes: Vec<Note>,
    tasks: Vec<Task>,
    events: Vec<Event>,
    timetable: Timetable,
    files: Vec<File>,
}

#[derive(Deserialize)]
pub struct User {
    pub email: String,
    first_name: String,
    last_name: String,
}

#[derive(Deserialize)]
struct SpaceTools {
    slug: String,
    tools: BTreeMap<String, bool>,
}

/// Un dossier : à la racine d'un espace (`space`) ou dans un autre (`parent`,
/// la clé d'un dossier déjà décrit plus haut).
#[derive(Deserialize)]
struct Folder {
    key: String,
    name: String,
    space: Option<String>,
    parent: Option<String>,
}

#[derive(Deserialize)]
struct Note {
    title: String,
    space: String,
    folder: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    pinned: bool,
    /// Dernière modification, en jours avant aujourd'hui.
    days_ago: u64,
    body_md: String,
}

#[derive(Deserialize)]
struct Task {
    title: String,
    space: String,
    /// Jour prévu, en jours à partir d'aujourd'hui (négatif : passé).
    days: i64,
    #[serde(default)]
    done: bool,
}

#[derive(Deserialize)]
struct Event {
    title: String,
    space: String,
    days: i64,
    #[serde(default)]
    start: String,
    #[serde(default)]
    end: String,
    #[serde(default)]
    all_day: bool,
    #[serde(default)]
    location: String,
}

#[derive(Deserialize)]
struct Timetable {
    space: String,
    name: String,
    weeks_before: i64,
    weeks_after: i64,
    courses: Vec<Course>,
}

#[derive(Deserialize)]
struct Course {
    /// 1 = lundi … 5 = vendredi.
    weekday: u64,
    start: String,
    end: String,
    title: String,
    location: String,
}

#[derive(Deserialize)]
struct File {
    name: String,
    space: String,
    folder: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    kind: String,
    #[serde(default)]
    lines: Vec<String>,
    #[serde(default)]
    text: String,
}

/// Le jour `days` jours après `today` (avant si négatif).
fn shift(today: NaiveDate, days: i64) -> anyhow::Result<NaiveDate> {
    let moved = if days >= 0 {
        today.checked_add_days(Days::new(days.unsigned_abs()))
    } else {
        today.checked_sub_days(Days::new(days.unsigned_abs()))
    };
    moved.context("date hors limites")
}

fn hm(text: &str) -> anyhow::Result<NaiveTime> {
    NaiveTime::parse_from_str(text, "%H:%M").with_context(|| format!("heure invalide : {text}"))
}

fn id_of(value: &Value) -> anyhow::Result<i64> {
    value["id"]
        .as_i64()
        .with_context(|| format!("réponse sans id : {value}"))
}

/// Remplit la base, et laisse `browser` connecté au compte démo.
pub async fn load(state: &AppState, browser: &mut Browser, fx: &Fixtures) -> anyhow::Result<()> {
    // --- Compte : mot de passe tiré au sort, jamais écrit nulle part ------
    let password = super::hex(&rand::random::<[u8; 16]>());
    crate::auth::set_password(&state.db, &fx.user.email, password.clone()).await?;
    let user_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE email = ?")
        .bind(&fx.user.email)
        .fetch_one(&state.db)
        .await?;
    crate::spaces::ensure_defaults(&state.db, user_id)
        .await
        .map_err(|e| anyhow::anyhow!("espaces du compte démo : {e:?}"))?;
    // Pas d'`instance::ensure_admin` : le compte démo n'administre rien, la
    // page d'administration n'existe donc pas pour lui.
    browser.login(&fx.user.email, &password).await?;

    browser
        .api(
            Method::PATCH,
            "/settings",
            Some(json!({
                "first_name": fx.user.first_name,
                "last_name": fx.user.last_name,
            })),
        )
        .await?;
    for space in &fx.spaces {
        browser
            .api(
                Method::PUT,
                &format!("/spaces/{}/tools", space.slug),
                Some(json!({ "tools": space.tools })),
            )
            .await?;
    }

    // --- Dossiers ---------------------------------------------------------
    let mut folders: HashMap<&str, i64> = HashMap::new();
    for folder in &fx.folders {
        let body = match (&folder.space, &folder.parent) {
            (Some(space), None) => json!({ "name": folder.name, "space": space }),
            (None, Some(parent)) => {
                let parent = folders
                    .get(parent.as_str())
                    .with_context(|| format!("dossier parent inconnu : {parent}"))?;
                json!({ "name": folder.name, "parent": parent })
            }
            _ => bail!("dossier {} : soit space, soit parent", folder.key),
        };
        let created = browser.api(Method::POST, "/folders", Some(body)).await?;
        folders.insert(&folder.key, id_of(&created)?);
    }
    let folder = |key: &Option<String>| -> anyhow::Result<Option<i64>> {
        key.as_deref()
            .map(|k| {
                folders
                    .get(k)
                    .copied()
                    .with_context(|| format!("dossier inconnu : {k}"))
            })
            .transpose()
    };

    let now = time::now();
    let today = time::paris_date(now);

    // --- Notes --------------------------------------------------------------
    for (rank, note) in fx.notes.iter().enumerate() {
        let created = browser
            .api(
                Method::POST,
                "/notes",
                Some(json!({
                    "title": note.title,
                    "body_md": note.body_md,
                    "space": note.space,
                    "tags": note.tags,
                })),
            )
            .await?;
        let id = id_of(&created)?;
        let mut changes = serde_json::Map::new();
        if note.pinned {
            changes.insert("pinned".into(), json!(true));
        }
        if let Some(folder) = folder(&note.folder)? {
            changes.insert("folder".into(), json!(folder));
        }
        if !changes.is_empty() {
            browser
                .api(
                    Method::PATCH,
                    &format!("/notes/{id}"),
                    Some(Value::Object(changes)),
                )
                .await?;
        }
        // Quelques minutes d'écart entre deux notes du même jour : l'ordre
        // « dernières modifiées » reste celui des fixtures.
        let at = now - (note.days_ago as i64) * 86_400 - (rank as i64) * 420;
        sqlx::query("UPDATE notes SET created_at = ?1, updated_at = ?1 WHERE id = ?2")
            .bind(at)
            .bind(id)
            .execute(&state.db)
            .await?;
    }

    // --- Tâches -------------------------------------------------------------
    for task in &fx.tasks {
        let due = shift(today, task.days)?.format("%Y-%m-%d").to_string();
        let created = browser
            .api(
                Method::POST,
                "/tasks",
                Some(json!({ "title": task.title, "space": task.space, "due_date": due })),
            )
            .await?;
        if task.done {
            let id = id_of(&created)?;
            browser
                .api(
                    Method::PATCH,
                    &format!("/tasks/{id}"),
                    Some(json!({ "done": true })),
                )
                .await?;
        }
    }

    // --- Événements de l'agenda ---------------------------------------------
    for event in &fx.events {
        let date = shift(today, event.days)?.format("%Y-%m-%d").to_string();
        browser
            .api(
                Method::POST,
                "/events",
                Some(json!({
                    "title": event.title,
                    "date": date,
                    "start": event.start,
                    "end": event.end,
                    "all_day": event.all_day,
                    "location": event.location,
                    "space": event.space,
                })),
            )
            .await?;
    }

    // --- Emploi du temps (comme un flux Pronote déjà synchronisé) -----------
    let timetable = &fx.timetable;
    let space_id: i64 = sqlx::query_scalar("SELECT id FROM spaces WHERE user_id = ? AND slug = ?")
        .bind(user_id)
        .bind(&timetable.space)
        .fetch_one(&state.db)
        .await?;
    // Une adresse en .invalid : elle ne peut mener nulle part, et la démo ne
    // la demande jamais (aucune synchro n'est lancée).
    let source_id: i64 = sqlx::query_scalar(
        "INSERT INTO calendar_sources (user_id, space_id, name, ical_url, last_fetched_at, last_attempt_at, created_at)
         VALUES (?1, ?2, ?3, 'https://emploi-du-temps.invalid/demo.ics', ?4, ?4, ?4) RETURNING id",
    )
    .bind(user_id)
    .bind(space_id)
    .bind(&timetable.name)
    .bind(now)
    .fetch_one(&state.db)
    .await?;
    let monday = shift(today, -i64::from(today.weekday().num_days_from_monday()))?;
    let mut courses = Vec::new();
    for week in -timetable.weeks_before..=timetable.weeks_after {
        for (n, course) in timetable.courses.iter().enumerate() {
            if !(1..=5).contains(&course.weekday) {
                bail!("cours {} : weekday va de 1 à 5", course.title);
            }
            let day = shift(monday, week * 7 + course.weekday as i64 - 1)?;
            courses.push(ParsedEvent {
                uid: format!("demo-{day}-{n}"),
                title: course.title.clone(),
                location: course.location.clone(),
                starts_at: time::local_to_ts(day.and_time(hm(&course.start)?), time::PARIS),
                ends_at: time::local_to_ts(day.and_time(hm(&course.end)?), time::PARIS),
                all_day: false,
            });
        }
    }
    crate::calendar::sync::store(&state.db, source_id, &courses, now).await?;

    // --- Fichiers -----------------------------------------------------------
    for file in &fx.files {
        let bytes = media::render(&file.name, &file.kind, &file.lines, &file.text)?;
        let mut start = json!({ "name": file.name, "size": bytes.len() });
        match folder(&file.folder)? {
            Some(folder) => start["folder"] = json!(folder),
            None => start["space"] = json!(file.space),
        }
        let upload = browser
            .api(Method::POST, "/files/uploads", Some(start))
            .await?;
        let upload = upload["upload"]
            .as_str()
            .with_context(|| format!("envoi sans identifiant : {upload}"))?
            .to_string();
        let reply = browser
            .api_raw(
                Method::PUT,
                &format!("/files/uploads/{upload}?offset=0"),
                Some(bytes),
                "application/octet-stream",
            )
            .await?;
        if reply.status != StatusCode::CREATED {
            bail!("envoi de {} : {} {}", file.name, reply.status, reply.text());
        }
        if !file.tags.is_empty() {
            let id = id_of(&reply.json()?)?;
            browser
                .api(
                    Method::PATCH,
                    &format!("/files/{id}"),
                    Some(json!({ "add_tags": file.tags })),
                )
                .await?;
        }
    }

    wait_for_thumbnails(state).await
}

/// Attend que le service de conversion ait fait les vignettes des images, et
/// les range (`tools::collect`, ce que fait le serveur toutes les secondes).
async fn wait_for_thumbnails(state: &AppState) -> anyhow::Result<()> {
    for _ in 0..240 {
        crate::tools::collect(state).await?;
        let pending: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM jobs WHERE kind = 'thumbnail' AND status IN ('queued', 'running')",
        )
        .fetch_one(&state.db)
        .await?;
        if pending == 0 {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    bail!("les vignettes de la démo ne sont pas prêtes au bout d'une minute")
}
