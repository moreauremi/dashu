//! Vue globale de Clients (SPEC, « 1Life — Clients ») : un diagramme de
//! Gantt des journées, une ligne par client, une colonne par jour, une barre
//! par journée.
//!
//! Tout est calculé ici, en pixels : le gabarit (`onelife/_gantt.html`) ne
//! fait que poser les nombres dans les attributs `x`, `y`, `width` et
//! `height` du SVG. Ni `style=` ni script (CSP), aucune bibliothèque.

use chrono::{Datelike, Duration, NaiveDate, Timelike, Weekday};

use crate::time;

/// Largeur d'une colonne (un jour), en pixels.
pub const COL: i64 = 28;
/// Hauteur d'une ligne (un client).
pub const ROW: i64 = 32;
/// Largeur de la colonne des noms, dans le SVG (elle défile avec lui).
pub const NAMES: i64 = 176;
/// Hauteur de l'en-tête : le mois, puis le numéro du jour.
pub const HEAD: i64 = 44;
/// Marge d'une barre dans sa case.
const PAD: i64 = 2;
/// Hauteur d'une barre.
const BAR: i64 = 20;
/// Un nom plus long est coupé dans le SVG (le texte n'y revient pas à la
/// ligne) ; il reste entier dans l'info-bulle.
const NAME_CHARS: usize = 22;

/// La période affichée : 4 semaines ou 3 mois (13 semaines), à partir d'un
/// lundi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
    Month,
    Quarter,
}

impl Span {
    /// `duree=` : `4s` (par défaut) ou `3m`.
    pub fn parse(raw: Option<&str>) -> Self {
        match raw {
            Some("3m") => Self::Quarter,
            _ => Self::Month,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Month => "4s",
            Self::Quarter => "3m",
        }
    }

    pub fn weeks(self) -> i64 {
        match self {
            Self::Month => 4,
            Self::Quarter => 13,
        }
    }
}

/// La moitié de colonne qu'occupe une journée (SPEC, décision 5 : déduite
/// des horaires, jamais stockée).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Half {
    /// Fin à 13 h au plus tard : la moitié gauche.
    Morning,
    /// Début à 13 h au plus tôt : la moitié droite.
    Afternoon,
    /// Tout le reste : la colonne entière.
    Full,
}

/// La moitié occupée par une journée, d'après ses horaires (heure de Paris).
/// Une journée entière, ou sur plusieurs jours, prend toute la colonne.
pub fn half(starts_at: i64, ends_at: i64, all_day: bool) -> Half {
    if all_day || time::paris_date(starts_at) != time::paris_date(ends_at - 1) {
        return Half::Full;
    }
    let minutes = |ts: i64| {
        let local = time::paris(ts);
        local.hour() * 60 + local.minute()
    };
    if minutes(ends_at) <= 13 * 60 {
        Half::Morning
    } else if minutes(starts_at) >= 13 * 60 {
        Half::Afternoon
    } else {
        Half::Full
    }
}

/// Une journée d'un client, telle que lue en base.
pub struct Day {
    pub client: i64,
    pub label: String,
    pub starts_at: i64,
    pub ends_at: i64,
    pub all_day: bool,
}

/// Un client : son dossier, son nom, l'adresse de sa fiche.
pub struct Client {
    pub id: i64,
    pub name: String,
    pub href: String,
}

/// Une colonne de l'en-tête.
pub struct Column {
    pub x: i64,
    /// Milieu de la colonne, pour centrer le numéro du jour.
    pub mid: i64,
    pub day: u32,
    /// Nom du mois, sur la première colonne et le 1er de chaque mois.
    pub month: Option<String>,
    pub weekend: bool,
    pub past: bool,
    pub today: bool,
}

/// Une barre : une journée dans sa colonne.
pub struct Bar {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
    /// Info-bulle : client, date, horaires et intitulé.
    pub title: String,
    pub past: bool,
}

/// Une ligne : un client et ses barres.
pub struct Line {
    pub y: i64,
    /// Ligne de base du nom.
    pub text_y: i64,
    pub name: String,
    /// Le nom entier (info-bulle), s'il a été coupé.
    pub full_name: String,
    pub href: String,
    pub bars: Vec<Bar>,
}

pub struct Gantt {
    pub width: i64,
    pub height: i64,
    pub columns: Vec<Column>,
    pub lines: Vec<Line>,
    /// Abscisse du trait d'aujourd'hui, si aujourd'hui est dans la période.
    pub today_x: Option<i64>,
    /// Résumé pour les lecteurs d'écran.
    pub label: String,
}

/// « 09:00 – 17:00 », ou « Journée entière ».
fn hours(day: &Day) -> String {
    if day.all_day {
        "Journée entière".to_string()
    } else {
        format!("{} – {}", time::hm(day.starts_at), time::hm(day.ends_at))
    }
}

/// Le nom coupé à [`NAME_CHARS`] caractères, avec « … ».
fn short_name(name: &str) -> String {
    if name.chars().count() <= NAME_CHARS {
        name.to_string()
    } else {
        let cut: String = name.chars().take(NAME_CHARS - 1).collect();
        format!("{}…", cut.trim_end())
    }
}

/// Les jours couverts par une journée (un seul, sauf pour un événement
/// étiré sur plusieurs jours dans l'Agenda).
fn dates_of(day: &Day) -> Vec<NaiveDate> {
    let first = time::paris_date(day.starts_at);
    let last = time::paris_date((day.ends_at - 1).max(day.starts_at));
    let mut out = Vec::new();
    let mut date = first;
    // Garde-fou : une période affichée ne dépasse pas 13 semaines.
    while date <= last && out.len() < 120 {
        out.push(date);
        date += Duration::days(1);
    }
    out
}

/// Construit le Gantt de la période qui commence le lundi `start`.
/// `clients` est déjà trié par nom ; `only_busy` ne garde que les clients qui
/// ont au moins une journée sur la période.
pub fn build(
    start: NaiveDate,
    span: Span,
    clients: &[Client],
    days: &[Day],
    now: i64,
    only_busy: bool,
) -> Gantt {
    let count = span.weeks() * 7;
    let end = start + Duration::days(count);
    let today = time::paris_date(now);

    let columns: Vec<Column> = (0..count)
        .map(|i| {
            let date = start + Duration::days(i);
            let x = NAMES + i * COL;
            Column {
                x,
                mid: x + COL / 2,
                day: date.day(),
                month: (i == 0 || date.day() == 1).then(|| {
                    let name = time::month_name(date.month0());
                    if i == 0 || date.month() == 1 {
                        format!("{name} {}", date.year())
                    } else {
                        name.to_string()
                    }
                }),
                weekend: matches!(date.weekday(), Weekday::Sat | Weekday::Sun),
                past: date < today,
                today: date == today,
            }
        })
        .collect();

    let mut lines = Vec::new();
    for client in clients {
        let row = lines.len() as i64;
        let y = HEAD + row * ROW;
        let mut bars = Vec::new();
        for day in days.iter().filter(|d| d.client == client.id) {
            let side = half(day.starts_at, day.ends_at, day.all_day);
            for date in dates_of(day) {
                if date < start || date >= end {
                    continue;
                }
                let x = NAMES + (date - start).num_days() * COL;
                let (x, width) = match side {
                    Half::Morning => (x + PAD, COL / 2 - PAD),
                    Half::Afternoon => (x + COL / 2, COL / 2 - PAD),
                    Half::Full => (x + PAD, COL - 2 * PAD),
                };
                let mut title = format!(
                    "{} · {} · {}",
                    client.name,
                    time::weekday_day_month(date),
                    hours(day)
                );
                if !day.label.is_empty() {
                    title.push_str(" · ");
                    title.push_str(&day.label);
                }
                bars.push(Bar {
                    x,
                    y: y + (ROW - BAR) / 2,
                    width,
                    height: BAR,
                    title,
                    past: day.ends_at <= now,
                });
            }
        }
        if only_busy && bars.is_empty() {
            continue;
        }
        let name = short_name(&client.name);
        lines.push(Line {
            y,
            text_y: y + ROW / 2 + 5,
            full_name: client.name.clone(),
            name,
            href: client.href.clone(),
            bars,
        });
    }

    let today_x =
        (today >= start && today < end).then(|| NAMES + (today - start).num_days() * COL + COL / 2);
    let last = end - Duration::days(1);
    Gantt {
        width: NAMES + count * COL,
        height: HEAD + (lines.len() as i64).max(1) * ROW,
        label: format!(
            "Journées des clients du {} au {}",
            time::day_month(start),
            time::date_long(last)
        ),
        columns,
        lines,
        today_x,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(date: NaiveDate, h: u32, m: u32) -> i64 {
        time::local_to_ts(date.and_hms_opt(h, m, 0).unwrap(), time::PARIS)
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn la_moitie_se_deduit_des_horaires() {
        let d = date(2026, 12, 3);
        assert_eq!(half(at(d, 9, 0), at(d, 12, 30), false), Half::Morning);
        assert_eq!(half(at(d, 9, 0), at(d, 13, 0), false), Half::Morning);
        assert_eq!(half(at(d, 13, 30), at(d, 17, 0), false), Half::Afternoon);
        assert_eq!(half(at(d, 13, 0), at(d, 18, 0), false), Half::Afternoon);
        assert_eq!(half(at(d, 9, 0), at(d, 17, 0), false), Half::Full);
        assert_eq!(half(at(d, 12, 0), at(d, 14, 0), false), Half::Full);
        let next = time::day_start(d + Duration::days(1));
        assert_eq!(half(time::day_start(d), next, true), Half::Full);
    }

    #[test]
    fn barres_placees_dans_leur_colonne() {
        // Lundi 30 novembre 2026 ; maintenant : mercredi 2 décembre à 10 h.
        let monday = date(2026, 11, 30);
        let now = at(date(2026, 12, 2), 10, 0);
        let clients = [
            Client {
                id: 1,
                name: "Atelier Test".into(),
                href: "/1life/clients/1".into(),
            },
            Client {
                id: 2,
                name: "Client Exemple".into(),
                href: "/1life/clients/2".into(),
            },
        ];
        let thursday = date(2026, 12, 3);
        let days = [
            Day {
                client: 1,
                label: "Paramétrage".into(),
                starts_at: at(thursday, 9, 0),
                ends_at: at(thursday, 12, 30),
                all_day: false,
            },
            Day {
                client: 1,
                label: String::new(),
                starts_at: at(monday, 13, 30),
                ends_at: at(monday, 17, 0),
                all_day: false,
            },
        ];
        let g = build(monday, Span::Month, &clients, &days, now, false);
        assert_eq!(g.columns.len(), 28);
        assert_eq!(g.width, NAMES + 28 * COL);
        assert_eq!(g.lines.len(), 2);
        let bars = &g.lines[0].bars;
        // Jeudi matin : 4e colonne, moitié gauche.
        assert_eq!(bars[0].x, NAMES + 3 * COL + PAD);
        assert_eq!(bars[0].width, COL / 2 - PAD);
        assert!(bars[0].title.contains("jeudi 3 décembre"));
        assert!(bars[0].title.contains("09:00 – 12:30"));
        assert!(bars[0].title.ends_with("Paramétrage"));
        assert!(!bars[0].past);
        // Lundi après-midi : 1re colonne, moitié droite, passé.
        assert_eq!(bars[1].x, NAMES + COL / 2);
        assert!(bars[1].past);
        // Week-ends, aujourd'hui, mois.
        assert!(g.columns[5].weekend && g.columns[6].weekend && !g.columns[4].weekend);
        assert!(g.columns[2].today && g.columns[1].past && !g.columns[3].past);
        assert_eq!(g.today_x, Some(NAMES + 2 * COL + COL / 2));
        assert_eq!(g.columns[0].month.as_deref(), Some("novembre 2026"));
        assert_eq!(g.columns[1].month.as_deref(), Some("décembre"));
        // Seulement les clients qui ont des journées.
        let g = build(monday, Span::Month, &clients, &days, now, true);
        assert_eq!(g.lines.len(), 1);
        // Hors période : pas de barre, pas de trait.
        let later = monday + Duration::days(28);
        let g = build(later, Span::Quarter, &clients, &days, now, false);
        assert_eq!(g.columns.len(), 91);
        assert!(g.lines.iter().all(|l| l.bars.is_empty()));
        assert_eq!(g.today_x, None);
    }

    #[test]
    fn un_nom_long_est_coupe() {
        assert_eq!(short_name("Court"), "Court");
        let long = "Un nom de client vraiment beaucoup trop long";
        let short = short_name(long);
        assert!(short.ends_with('…'));
        assert!(short.chars().count() <= NAME_CHARS);
    }
}
