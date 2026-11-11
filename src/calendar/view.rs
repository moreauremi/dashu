//! Mise en page de l'emploi du temps : transforme une liste d'événements en
//! colonnes de jours prêtes à afficher.
//!
//! Une journée est une grille dont chaque rangée vaut 5 minutes. Chaque
//! événement reçoit une rangée de début et de fin (classes `gs…`/`ge…`, voir
//! build.rs) et, s'il en chevauche d'autres, une colonne parmi 2, 3 ou 4.

use chrono::{Datelike, Duration, NaiveDate, Timelike};

use super::ical;
use crate::time;

/// Minutes par rangée de la grille.
const SLOT_MINUTES: u32 = 5;
const SLOTS_PER_HOUR: u32 = 60 / SLOT_MINUTES;
/// Plage affichée par défaut ; elle s'étend si un événement en sort.
const DEFAULT_FIRST_HOUR: u32 = 8;
const DEFAULT_LAST_HOUR: u32 = 18;
/// Hauteur minimale d'un événement : 3 rangées (15 minutes), pour rester lisible.
const MIN_SLOTS: u32 = 3;
/// Au-delà de 4 événements simultanés, les suivants partagent la dernière colonne.
const MAX_COLUMNS: u8 = 4;

/// Événement tel que lu en base.
pub struct EventRow {
    pub title: String,
    pub location: String,
    pub starts_at: i64,
    pub ends_at: i64,
    pub all_day: bool,
    /// Dans la vue d'ensemble de Perso : nom de l'espace d'où vient
    /// l'événement (« École »). Vide pour un événement de l'espace affiché.
    pub tag: String,
    /// Couleur de cet espace (`blue`, `green`…), vide avec `tag`.
    pub tag_color: String,
}

pub struct Grid {
    pub days: Vec<DayColumn>,
    pub hours: Vec<HourMark>,
    /// Nombre d'heures affichées (classe `tgh…`).
    pub hours_count: u32,
    pub first_hour: u32,
    pub is_empty: bool,
}

pub struct HourMark {
    pub label: String,
    pub slot: u32,
}

pub struct DayColumn {
    pub iso: String,
    /// « lun. »
    pub weekday: &'static str,
    pub day: u32,
    /// « mardi 10 novembre », pour les lecteurs d'écran.
    pub label: String,
    pub is_today: bool,
    /// Événements sur la journée entière (vacances, férié…).
    pub all_day: Vec<String>,
    pub events: Vec<EventBox>,
    /// Rangée de la ligne « maintenant », pour la colonne d'aujourd'hui.
    pub now_slot: Option<u32>,
}

pub struct EventBox {
    pub subject: String,
    pub details: String,
    pub location: String,
    /// « 08:00 – 09:55 »
    pub time: String,
    pub start_slot: u32,
    pub end_slot: u32,
    /// Indice dans la palette des matières (classe `ev-c…`).
    pub color: u8,
    /// Nombre de colonnes du groupe d'événements qui se chevauchent, et
    /// colonne occupée par celui-ci (classes `k…` et `p…`).
    pub columns: u8,
    pub column: u8,
    /// Classe de densité selon la hauteur disponible : `is-compact` (une
    /// ligne), `is-medium` (deux lignes) ou vide (tout est affiché).
    pub density: &'static str,
    pub past: bool,
    /// Espace d'origine dans la vue d'ensemble (voir [`EventRow::tag`]).
    pub tag: String,
    pub tag_color: String,
}

/// Événement d'une journée, en minutes depuis minuit (heure de Paris).
struct Placed<'a> {
    row: &'a EventRow,
    start_min: u32,
    end_min: u32,
}

pub fn build(dates: &[NaiveDate], events: &[EventRow], now: i64) -> Grid {
    let today = time::paris_date(now);

    // 1. Répartition des événements par jour.
    let mut per_day: Vec<(NaiveDate, Vec<Placed>, Vec<String>)> = Vec::new();
    for &date in dates {
        let day_start = time::day_start(date);
        let day_end = time::day_start(date + Duration::days(1));
        let mut timed = Vec::new();
        let mut all_day = Vec::new();

        for row in events
            .iter()
            .filter(|e| e.ends_at > day_start && e.starts_at < day_end)
        {
            if row.all_day {
                all_day.push(row.title.clone());
                continue;
            }
            // Un événement qui déborde sur la veille ou le lendemain est
            // coupé aux limites de la journée.
            let start_min = if row.starts_at <= day_start {
                0
            } else {
                minute_of_day(row.starts_at)
            };
            let end_min = if row.ends_at >= day_end {
                24 * 60
            } else {
                minute_of_day(row.ends_at)
            };
            if end_min > start_min {
                timed.push(Placed {
                    row,
                    start_min,
                    end_min,
                });
            }
        }
        timed.sort_by_key(|p| (p.start_min, std::cmp::Reverse(p.end_min)));
        per_day.push((date, timed, all_day));
    }

    // 2. Plage horaire commune à tous les jours affichés.
    let all_timed = || per_day.iter().flat_map(|(_, timed, _)| timed.iter());
    let first_hour = all_timed()
        .map(|p| p.start_min / 60)
        .min()
        .unwrap_or(DEFAULT_FIRST_HOUR)
        .min(DEFAULT_FIRST_HOUR);
    let last_hour = all_timed()
        .map(|p| p.end_min.div_ceil(60))
        .max()
        .unwrap_or(DEFAULT_LAST_HOUR)
        .clamp(DEFAULT_LAST_HOUR, 24);
    let hours_count = last_hour - first_hour;
    let total_slots = hours_count * SLOTS_PER_HOUR;
    let to_slot =
        |minute: u32| (minute.saturating_sub(first_hour * 60) + SLOT_MINUTES / 2) / SLOT_MINUTES;

    // 3. Colonnes de jours.
    let is_empty = per_day
        .iter()
        .all(|(_, timed, all_day)| timed.is_empty() && all_day.is_empty());
    let days = per_day
        .into_iter()
        .map(|(date, timed, all_day)| {
            let mut boxes: Vec<EventBox> = timed
                .iter()
                .map(|p| {
                    let start_slot = to_slot(p.start_min).min(total_slots - MIN_SLOTS);
                    let end_slot = to_slot(p.end_min).clamp(start_slot + MIN_SLOTS, total_slots);
                    EventBox {
                        subject: ical::subject(&p.row.title).to_string(),
                        details: ical::details(&p.row.title),
                        location: p.row.location.clone(),
                        time: format!(
                            "{} – {}",
                            time::hm(p.row.starts_at),
                            time::hm(p.row.ends_at)
                        ),
                        start_slot,
                        end_slot,
                        color: ical::subject_color(&p.row.title),
                        columns: 1,
                        column: 0,
                        density: match end_slot - start_slot {
                            0..10 => "is-compact",
                            10..14 => "is-medium",
                            _ => "",
                        },
                        past: p.row.ends_at <= now,
                        tag: p.row.tag.clone(),
                        tag_color: p.row.tag_color.clone(),
                    }
                })
                .collect();
            assign_columns(&mut boxes);

            let is_today = date == today;
            let now_minute = minute_of_day(now);
            let now_slot =
                (is_today && now_minute >= first_hour * 60 && now_minute < last_hour * 60)
                    .then(|| to_slot(now_minute).min(total_slots));

            DayColumn {
                iso: date.format("%Y-%m-%d").to_string(),
                weekday: time::weekday_short(date.weekday()),
                day: date.day(),
                label: format!(
                    "{} {}",
                    time::weekday(date.weekday()),
                    time::day_month(date)
                ),
                is_today,
                all_day,
                events: boxes,
                now_slot,
            }
        })
        .collect();

    let hours = (first_hour..last_hour)
        .map(|hour| HourMark {
            label: format!("{hour:02}:00"),
            slot: (hour - first_hour) * SLOTS_PER_HOUR,
        })
        .collect();

    Grid {
        days,
        hours,
        hours_count,
        first_hour,
        is_empty,
    }
}

fn minute_of_day(ts: i64) -> u32 {
    let local = time::paris(ts);
    local.hour() * 60 + local.minute()
}

/// Place côte à côte les événements qui se chevauchent.
///
/// Les événements (déjà triés par heure de début) sont regroupés en paquets
/// qui se chevauchent de proche en proche. Dans un paquet, chacun prend la
/// première colonne libre ; le paquet entier est ensuite divisé en autant de
/// colonnes qu'il en a fallu.
fn assign_columns(boxes: &mut [EventBox]) {
    let mut group_start = 0;
    let mut group_end_slot = 0;
    // Rangée de fin du dernier événement placé dans chaque colonne du paquet.
    let mut column_ends: Vec<u32> = Vec::new();

    for index in 0..boxes.len() {
        if index > group_start && boxes[index].start_slot >= group_end_slot {
            close_group(&mut boxes[group_start..index], column_ends.len());
            group_start = index;
            column_ends.clear();
        }
        let start = boxes[index].start_slot;
        let column = match column_ends.iter().position(|&end| end <= start) {
            Some(free) => free,
            None => {
                column_ends.push(0);
                column_ends.len() - 1
            }
        };
        column_ends[column] = boxes[index].end_slot;
        boxes[index].column = column as u8;
        group_end_slot = if index == group_start {
            boxes[index].end_slot
        } else {
            group_end_slot.max(boxes[index].end_slot)
        };
    }
    close_group(&mut boxes[group_start..], column_ends.len());
}

fn close_group(group: &mut [EventBox], columns: usize) {
    let columns = (columns.max(1) as u8).min(MAX_COLUMNS);
    for event in group {
        event.columns = columns;
        event.column = event.column.min(columns - 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> i64 {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0)
            .unwrap()
            .timestamp()
    }

    fn event(title: &str, starts_at: i64, ends_at: i64) -> EventRow {
        EventRow {
            title: title.into(),
            location: String::new(),
            starts_at,
            ends_at,
            all_day: false,
            tag: String::new(),
            tag_color: String::new(),
        }
    }

    #[test]
    fn un_cours_de_8h_a_10h_en_hiver() {
        // 10 novembre 2026 : Paris est en UTC+1, 08:00 locale = 07:00 UTC.
        let date = NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();
        let events = [event(
            "MATHS - DUPONT",
            utc(2026, 11, 10, 7, 0),
            utc(2026, 11, 10, 9, 0),
        )];
        let grid = build(&[date], &events, utc(2026, 11, 10, 11, 0));

        assert_eq!((grid.first_hour, grid.hours_count), (8, 10));
        let ev = &grid.days[0].events[0];
        assert_eq!(ev.time, "08:00 – 10:00");
        assert_eq!((ev.start_slot, ev.end_slot), (0, 24));
        assert_eq!(ev.subject, "MATHS");
        assert!(ev.past);
        // 11:00 UTC = midi à Paris = 4 heures après 8h = rangée 48.
        assert_eq!(grid.days[0].now_slot, Some(48));
    }

    #[test]
    fn meme_heure_locale_ete_comme_hiver() {
        // 08:00 à Paris : 06:00 UTC en été, 07:00 UTC en hiver.
        let summer = NaiveDate::from_ymd_opt(2026, 6, 15).unwrap();
        let winter = NaiveDate::from_ymd_opt(2026, 12, 15).unwrap();
        let events = [
            event("SI", utc(2026, 6, 15, 6, 0), utc(2026, 6, 15, 8, 0)),
            event("SI", utc(2026, 12, 15, 7, 0), utc(2026, 12, 15, 9, 0)),
        ];
        let grid = build(&[summer, winter], &events, 0);
        for day in &grid.days {
            assert_eq!(day.events[0].time, "08:00 – 10:00");
            assert_eq!(day.events[0].start_slot, 0);
        }
    }

    #[test]
    fn la_plage_s_etend_pour_un_cours_matinal_ou_tardif() {
        let date = NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();
        let events = [
            event("SPORT", utc(2026, 11, 10, 6, 30), utc(2026, 11, 10, 7, 0)), // 07:30 locale
            event("SOIR", utc(2026, 11, 10, 18, 0), utc(2026, 11, 10, 19, 30)), // 19:00 – 20:30
        ];
        let grid = build(&[date], &events, 0);
        assert_eq!(grid.first_hour, 7);
        assert_eq!(grid.hours_count, 14); // 7h -> 21h
    }

    #[test]
    fn evenements_simultanes_cote_a_cote() {
        let date = NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();
        let events = [
            event("A", utc(2026, 11, 10, 8, 0), utc(2026, 11, 10, 10, 0)),
            event("B", utc(2026, 11, 10, 9, 0), utc(2026, 11, 10, 11, 0)),
            event("C", utc(2026, 11, 10, 13, 0), utc(2026, 11, 10, 14, 0)),
        ];
        let grid = build(&[date], &events, 0);
        let evs = &grid.days[0].events;
        assert_eq!((evs[0].columns, evs[0].column), (2, 0));
        assert_eq!((evs[1].columns, evs[1].column), (2, 1));
        // C ne chevauche personne : pleine largeur.
        assert_eq!((evs[2].columns, evs[2].column), (1, 0));
    }

    #[test]
    fn journee_entiere_a_part() {
        let date = NaiveDate::from_ymd_opt(2026, 11, 11).unwrap();
        let start = time::day_start(date);
        let events = [EventRow {
            title: "Férié".into(),
            location: String::new(),
            starts_at: start,
            ends_at: start + 86_400,
            all_day: true,
            tag: String::new(),
            tag_color: String::new(),
        }];
        let grid = build(&[date], &events, 0);
        assert_eq!(grid.days[0].all_day, vec!["Férié".to_string()]);
        assert!(grid.days[0].events.is_empty());
        assert!(!grid.is_empty);
    }
}
