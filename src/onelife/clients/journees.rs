//! Les journées d'un client (SPEC, « 1Life — Clients ») : des événements de
//! l'Agenda (`events`), reliés au client par `onelife_client_days`.
//!
//! Rien n'est refait ici : la vérification, l'enregistrement, la copie dans
//! Google Agenda et la suppression sont ceux de `calendar::events`. Le module
//! ne fait que construire le formulaire d'événement (titre « Client —
//! Intitulé », lieu « Sur site · … », horaires du créneau), un par jour
//! ouvré, et garder le lien.

use askama::Template;
use axum::extract::rejection::FormRejection;
use axum::extract::{Form, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{Datelike, Duration, NaiveDate, Weekday};
use serde::Deserialize;

use super::{client_of, url};
use crate::AppState;
use crate::auth::session::{Ctx, Shell};
use crate::calendar::events::{self, EventForm};
use crate::calendar::google;
use crate::folders::Folder;
use crate::time;
use crate::web::{self, AppError, AppResult, see_other};

/// Au plus 20 jours ouvrés d'un coup (quatre semaines).
pub const DAYS_MAX: u32 = 20;
/// Longueurs maximales, en caractères.
pub const LABEL_MAX: usize = 100;
pub const PLACE_MAX: usize = 150;

/// Les créneaux proposés : (valeur, libellé, début, fin).
pub const SLOTS: [(&str, &str, &str, &str); 3] = [
    ("journee", "Journée (9 h – 17 h)", "09:00", "17:00"),
    ("matin", "Matin (9 h – 12 h 30)", "09:00", "12:30"),
    (
        "apres-midi",
        "Après-midi (13 h 30 – 17 h)",
        "13:30",
        "17:00",
    ),
];

/// Le créneau qui correspond à des horaires (`libre` sinon) : une journée
/// modifiée dans l'Agenda se rouvre ainsi avec le bon choix coché.
pub fn slot_of(start: &str, end: &str) -> &'static str {
    SLOTS
        .iter()
        .find(|(_, _, s, e)| *s == start && *e == end)
        .map_or("libre", |(key, ..)| key)
}

/// Le lieu de l'événement : « Sur site · Lyon », « À distance ».
pub fn location(kind: &str, place: &str) -> String {
    let kind = if kind == "distance" {
        "À distance"
    } else {
        "Sur site"
    };
    if place.is_empty() {
        kind.to_string()
    } else {
        format!("{kind} · {place}")
    }
}

/// Le titre de l'événement : « Atelier Test — Paramétrage ».
fn title(client: &str, label: &str) -> String {
    if label.is_empty() {
        client.to_string()
    } else {
        format!("{client} — {label}")
    }
}

/// Les `count` jours ouvrés à partir de `first` (lui compris), week-ends
/// sautés. Les jours fériés ne le sont pas (SPEC, décision 4).
pub fn workdays(first: NaiveDate, count: u32) -> Vec<NaiveDate> {
    let mut out = Vec::new();
    let mut date = first;
    while out.len() < count as usize {
        if !matches!(date.weekday(), Weekday::Sat | Weekday::Sun) {
            out.push(date);
        }
        date += Duration::days(1);
    }
    out
}

/// Une ligne de texte : sans caractère de contrôle ni espaces en trop.
fn clean_line(raw: &str, max: usize) -> String {
    let line = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    line.chars()
        .take(max)
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[derive(Deserialize, Default)]
pub struct DayForm {
    #[serde(default)]
    date: String,
    /// Nombre de jours ouvrés (ajout seulement).
    #[serde(default)]
    jours: String,
    /// `journee`, `matin`, `apres-midi` ou `libre`.
    #[serde(default)]
    creneau: String,
    #[serde(default)]
    start: String,
    #[serde(default)]
    end: String,
    #[serde(default)]
    label: String,
    /// `site` ou `distance`.
    #[serde(default)]
    lieu: String,
    #[serde(default)]
    place: String,
    google: Option<String>,
}

/// Une saisie vérifiée : de quoi construire chaque événement.
struct Checked {
    dates: Vec<NaiveDate>,
    start: String,
    end: String,
    label: String,
    kind: &'static str,
    place: String,
}

/// Vérifie la saisie (`several` : l'ajout, qui accepte plusieurs jours).
fn check(form: &DayForm, several: bool) -> Result<Checked, String> {
    let first = NaiveDate::parse_from_str(form.date.trim(), "%Y-%m-%d")
        .map_err(|_| "Le jour n'est pas une date valide.".to_string())?;
    let count: u32 = if several {
        match form.jours.trim() {
            "" => 1,
            raw => raw
                .parse()
                .ok()
                .filter(|n| (1..=DAYS_MAX).contains(n))
                .ok_or(format!("Entre 1 et {DAYS_MAX} jours ouvrés."))?,
        }
    } else {
        1
    };
    if several && matches!(first.weekday(), Weekday::Sat | Weekday::Sun) {
        return Err(
            "Le premier jour tombe un week-end : choisis un jour du lundi au vendredi.".into(),
        );
    }
    let (start, end) = match SLOTS.iter().find(|(key, ..)| *key == form.creneau) {
        Some((_, _, start, end)) => (start.to_string(), end.to_string()),
        None if form.creneau == "libre" => (form.start.trim().into(), form.end.trim().into()),
        None => return Err("Choisis un créneau.".into()),
    };
    let dates = if several {
        workdays(first, count)
    } else {
        vec![first]
    };
    Ok(Checked {
        dates,
        start,
        end,
        label: clean_line(&form.label, LABEL_MAX),
        kind: if form.lieu == "distance" {
            "distance"
        } else {
            "site"
        },
        place: clean_line(&form.place, PLACE_MAX),
    })
}

/// Le formulaire d'événement d'une journée, tel que l'Agenda le vérifie.
fn event_form(client: &Folder, checked: &Checked, date: NaiveDate, google: bool) -> EventForm {
    EventForm {
        title: title(&client.name, &checked.label),
        date: date.format("%Y-%m-%d").to_string(),
        start: checked.start.clone(),
        end: checked.end.clone(),
        all_day: None,
        location: location(checked.kind, &checked.place),
        space_id: client.space_id.to_string(),
        google: google.then(|| "on".to_string()),
    }
}

#[derive(Template)]
#[template(path = "onelife/day.html")]
struct DayPage {
    shell: Shell,
    client: String,
    /// La fiche du client : « Annuler » et retour.
    back: String,
    /// Adresse du formulaire.
    action: String,
    /// La journée qu'on modifie : son formulaire de suppression.
    delete: Option<String>,
    date: String,
    jours: String,
    start: String,
    end: String,
    label: String,
    lieu: String,
    place: String,
    /// Google Agenda relié : l'adresse du compte, et la case cochée ou non.
    google_email: Option<String>,
    google: bool,
    slots: Vec<Choice>,
    error: Option<String>,
}

/// Un créneau proposé dans le formulaire.
pub struct Choice {
    pub key: &'static str,
    pub text: &'static str,
    pub checked: bool,
}

/// Les créneaux, celui de `current` coché ; « Horaires libres » en dernier.
fn choices(current: &str) -> Vec<Choice> {
    SLOTS
        .iter()
        .map(|(key, text, ..)| (*key, *text))
        .chain([("libre", "Horaires libres")])
        .map(|(key, text)| Choice {
            key,
            text,
            checked: key == current,
        })
        .collect()
}

impl DayPage {
    async fn render(self) -> AppResult<Response> {
        let status = if self.error.is_some() {
            StatusCode::UNPROCESSABLE_ENTITY
        } else {
            StatusCode::OK
        };
        Ok((status, web::html(self)).into_response())
    }
}

/// La fiche, sur la semaine d'un jour.
fn fiche_on(ctx: &Ctx, client: i64, date: NaiveDate) -> String {
    super::fiche_url(ctx, client, Some(time::monday_of(date)), None)
}

/// `GET /1life/clients/{id}/journees/new` : le formulaire d'ajout.
pub async fn new_page(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Response> {
    let ctx = match super::super::enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let (_, client) = client_of(&state, &ctx, &id).await?;
    let linked = google::linked(&state, ctx.user.id).await?;
    // Le prochain jour ouvré.
    let today = time::paris_date(time::now());
    let first = workdays(today + Duration::days(1), 1)[0];
    DayPage {
        back: url(&ctx, &format!("/{}", client.id)),
        action: url(&ctx, &format!("/{}/journees", client.id)),
        delete: None,
        client: client.name,
        date: first.format("%Y-%m-%d").to_string(),
        jours: "1".into(),
        start: "09:00".into(),
        end: "17:00".into(),
        label: String::new(),
        lieu: "site".into(),
        place: String::new(),
        google: linked.is_some(),
        google_email: linked.map(|l| l.email),
        slots: choices("journee"),
        error: None,
        shell: ctx.shell(super::super::TOOL).await?,
    }
    .render()
    .await
}

/// Le formulaire à nouveau, avec ce qui a été tapé et le refus.
async fn refused(
    ctx: &Ctx,
    state: &AppState,
    client: &Folder,
    event: Option<i64>,
    form: DayForm,
    error: String,
) -> AppResult<Response> {
    let linked = google::linked(state, ctx.user.id).await?;
    DayPage {
        back: url(ctx, &format!("/{}", client.id)),
        action: match event {
            Some(event) => url(ctx, &format!("/{}/journees/{event}", client.id)),
            None => url(ctx, &format!("/{}/journees", client.id)),
        },
        delete: event.map(|event| url(ctx, &format!("/{}/journees/{event}/delete", client.id))),
        client: client.name.clone(),
        date: form.date,
        jours: form.jours,
        slots: choices(&form.creneau),
        start: form.start,
        end: form.end,
        label: form.label,
        lieu: form.lieu,
        place: form.place,
        google: form.google.is_some(),
        google_email: linked.map(|l| l.email),
        error: Some(error),
        shell: ctx.shell(super::super::TOOL).await?,
    }
    .render()
    .await
}

/// `POST /1life/clients/{id}/journees` : ajoute une journée, ou une par jour
/// ouvré, dans l'espace du dossier client ; elles partent vers Google si la
/// case est cochée.
pub async fn create(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    form: Result<Form<DayForm>, FormRejection>,
) -> AppResult<Response> {
    let ctx = match super::super::enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let (_, client) = client_of(&state, &ctx, &id).await?;
    let form = form.map(|Form(f)| f).unwrap_or_default();
    let checked = match check(&form, true) {
        Ok(checked) => checked,
        Err(error) => return refused(&ctx, &state, &client, None, form, error).await,
    };
    let google = form.google.is_some();
    // Tout est vérifié avant d'écrire : un refus n'ajoute rien.
    let spaces = ctx.all_spaces();
    let mut valid = Vec::new();
    for &date in &checked.dates {
        match events::validate_in(&spaces, &event_form(&client, &checked, date, google)) {
            Ok(v) => valid.push(v),
            Err(error) => return refused(&ctx, &state, &client, None, form, error).await,
        }
    }
    let user = ctx.user.id;
    let now = time::now();
    for v in &valid {
        let event = events::insert(&state, user, v, google).await?;
        sqlx::query!(
            "INSERT INTO onelife_client_days (event_id, folder_id, label, place_kind, place, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
            event,
            client.id,
            checked.label,
            checked.kind,
            checked.place,
            now
        )
        .execute(&state.db)
        .await?;
    }
    tracing::info!(
        user,
        client = client.id,
        days = valid.len(),
        "1life : journées ajoutées"
    );
    let first = checked.dates[0];
    let message = match checked.dates.as_slice() {
        [one] => format!("Journée du {} ajoutée", time::weekday_day_month(*one)),
        [.., last] => format!(
            "{} journées ajoutées, du {} au {}",
            checked.dates.len(),
            time::weekday_day_month(first),
            time::weekday_day_month(*last)
        ),
        [] => String::new(),
    };
    ctx.flash_ok(message).await?;
    Ok(see_other(&fiche_on(&ctx, client.id, first)))
}

/// Une journée enregistrée : l'événement et son lien.
struct Link {
    event: i64,
    stored: events::Stored,
    label: String,
    kind: String,
    place: String,
}

/// Une journée de ce client, d'après le numéro de l'adresse ; 404 si
/// l'événement n'est pas une journée de ce client, ou pas au compte.
async fn day_of(state: &AppState, ctx: &Ctx, client: &Folder, raw: &str) -> AppResult<Link> {
    let event: i64 = raw.parse().map_err(|_| AppError::NotFound)?;
    let link = sqlx::query!(
        "SELECT label, place_kind, place FROM onelife_client_days WHERE event_id = ? AND folder_id = ?",
        event,
        client.id
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound)?;
    let stored = events::owned(state, ctx.user.id, event).await?;
    Ok(Link {
        event,
        stored,
        label: link.label,
        kind: link.place_kind,
        place: link.place,
    })
}

/// `GET /1life/clients/{id}/journees/{event}` : une journée, dans le
/// formulaire du module.
pub async fn edit_page(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    Path((id, event)): Path<(String, String)>,
) -> AppResult<Response> {
    let ctx = match super::super::enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let (_, client) = client_of(&state, &ctx, &id).await?;
    let Link {
        event,
        stored,
        label,
        kind,
        place,
    } = day_of(&state, &ctx, &client, &event).await?;
    let linked = google::linked(&state, ctx.user.id).await?;
    let start = time::hm(stored.starts_at);
    let end = time::hm(stored.ends_at);
    DayPage {
        back: url(&ctx, &format!("/{}", client.id)),
        action: url(&ctx, &format!("/{}/journees/{event}", client.id)),
        delete: Some(url(
            &ctx,
            &format!("/{}/journees/{event}/delete", client.id),
        )),
        client: client.name,
        date: time::paris_date(stored.starts_at)
            .format("%Y-%m-%d")
            .to_string(),
        jours: "1".into(),
        slots: choices(slot_of(&start, &end)),
        start,
        end,
        label,
        lieu: kind,
        place,
        google: stored.google,
        google_email: linked.map(|l| l.email),
        error: None,
        shell: ctx.shell(super::super::TOOL).await?,
    }
    .render()
    .await
}

/// `POST /1life/clients/{id}/journees/{event}` : enregistre une journée. Le
/// titre reprend le nom actuel du client (SPEC, décision 6).
pub async fn update(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    Path((id, event)): Path<(String, String)>,
    form: Result<Form<DayForm>, FormRejection>,
) -> AppResult<Response> {
    let ctx = match super::super::enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let (_, client) = client_of(&state, &ctx, &id).await?;
    let Link { event, stored, .. } = day_of(&state, &ctx, &client, &event).await?;
    let form = form.map(|Form(f)| f).unwrap_or_default();
    let checked = match check(&form, false) {
        Ok(checked) => checked,
        Err(error) => return refused(&ctx, &state, &client, Some(event), form, error).await,
    };
    let date = checked.dates[0];
    let google = form.google.is_some();
    // L'événement reste dans son espace (SPEC, décision 7) : seul l'ajout
    // prend celui du dossier.
    let mut event_form = event_form(&client, &checked, date, google);
    event_form.space_id = stored.space_id.to_string();
    let valid = match events::validate_in(&ctx.all_spaces(), &event_form) {
        Ok(valid) => valid,
        Err(error) => return refused(&ctx, &state, &client, Some(event), form, error).await,
    };
    let user = ctx.user.id;
    events::save(&state, user, event, &valid, Some(google)).await?;
    sqlx::query!(
        "UPDATE onelife_client_days SET label = ?, place_kind = ?, place = ? WHERE event_id = ?",
        checked.label,
        checked.kind,
        checked.place,
        event
    )
    .execute(&state.db)
    .await?;
    ctx.flash_ok(format!(
        "Journée du {} enregistrée",
        time::weekday_day_month(date)
    ))
    .await?;
    Ok(see_other(&fiche_on(&ctx, client.id, date)))
}

/// `POST /1life/clients/{id}/journees/{event}/delete` : supprime la journée
/// (l'événement, sa copie chez Google et le lien).
pub async fn delete(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    Path((id, event)): Path<(String, String)>,
) -> AppResult<Response> {
    let ctx = match super::super::enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let (_, client) = client_of(&state, &ctx, &id).await?;
    let Link { event, stored, .. } = day_of(&state, &ctx, &client, &event).await?;
    let user = ctx.user.id;
    events::remove(&state, user, event).await?;
    // Le lien part avec l'événement (ON DELETE CASCADE) ; au cas où.
    sqlx::query!("DELETE FROM onelife_client_days WHERE event_id = ?", event)
        .execute(&state.db)
        .await?;
    let date = time::paris_date(stored.starts_at);
    ctx.flash_ok(format!(
        "Journée du {} supprimée",
        time::weekday_day_month(date)
    ))
    .await?;
    Ok(see_other(&fiche_on(&ctx, client.id, date)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn jours_ouvres_week_ends_sautes() {
        // Jeudi 3 décembre 2026, trois jours : jeudi, vendredi, lundi.
        assert_eq!(
            workdays(date(2026, 12, 3), 3),
            [date(2026, 12, 3), date(2026, 12, 4), date(2026, 12, 7)]
        );
        assert_eq!(workdays(date(2026, 12, 5), 1), [date(2026, 12, 7)]);
    }

    #[test]
    fn creneaux_lieux_et_titres() {
        assert_eq!(slot_of("09:00", "17:00"), "journee");
        assert_eq!(slot_of("09:00", "12:30"), "matin");
        assert_eq!(slot_of("13:30", "17:00"), "apres-midi");
        assert_eq!(slot_of("10:00", "16:00"), "libre");
        assert_eq!(location("site", "Lyon"), "Sur site · Lyon");
        assert_eq!(location("distance", ""), "À distance");
        assert_eq!(
            title("Atelier Test", "Paramétrage"),
            "Atelier Test — Paramétrage"
        );
        assert_eq!(title("Atelier Test", ""), "Atelier Test");
    }

    #[test]
    fn saisie_verifiee() {
        let form = |date: &str, jours: &str, creneau: &str| DayForm {
            date: date.into(),
            jours: jours.into(),
            creneau: creneau.into(),
            label: "  Formation \t ".into(),
            ..DayForm::default()
        };
        let ok = check(&form("2026-12-03", "3", "matin"), true).unwrap();
        assert_eq!(ok.dates.len(), 3);
        assert_eq!((ok.start.as_str(), ok.end.as_str()), ("09:00", "12:30"));
        assert_eq!(ok.label, "Formation");
        assert_eq!(ok.kind, "site");
        for (bad, message) in [
            (form("", "1", "journee"), "date valide"),
            (form("2026-12-03", "21", "journee"), "Entre 1 et 20"),
            (form("2026-12-03", "0", "journee"), "Entre 1 et 20"),
            (form("2026-12-05", "1", "journee"), "week-end"),
            (form("2026-12-03", "1", "nuit"), "créneau"),
        ] {
            let error = check(&bad, true).err().expect(message);
            assert!(error.contains(message), "{error}");
        }
        // Modifier : un seul jour, même un samedi.
        assert_eq!(
            check(&form("2026-12-05", "9", "journee"), false)
                .unwrap()
                .dates,
            [date(2026, 12, 5)]
        );
    }
}
