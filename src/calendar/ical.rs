//! Lecture d'un flux iCalendar (.ics), tel que l'exporte Pronote.
//!
//! On en tire une liste d'événements dont les horaires sont des instants UTC.
//! Quatre écritures de la date existent dans le format :
//!
//!   * `DTSTART:20261110T070000Z` : heure UTC (le cas de Pronote) ;
//!   * `DTSTART;TZID=Europe/Paris:20261110T080000` : heure locale d'un fuseau ;
//!   * `DTSTART:20261110T080000` : heure « flottante », prise comme heure de Paris ;
//!   * `DTSTART;VALUE=DATE:20261110` : événement sur la journée entière.
//!
//! Limite connue : les récurrences (`RRULE`) ne sont pas développées. Pronote
//! n'en produit pas, il exporte chaque cours comme un événement distinct.

use chrono::NaiveTime;
use chrono_tz::Tz;
use icalendar::{
    Calendar, CalendarComponent, CalendarDateTime, Component, DatePerhapsTime, EventLike,
    EventStatus,
};

use crate::time::{PARIS, local_to_ts};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEvent {
    pub uid: String,
    pub title: String,
    pub location: String,
    /// Instants Unix (secondes, UTC).
    pub starts_at: i64,
    pub ends_at: i64,
    pub all_day: bool,
}

/// Analyse le texte d'un fichier .ics.
pub fn parse(text: &str) -> Result<Vec<ParsedEvent>, String> {
    if !text.contains("BEGIN:VCALENDAR") {
        return Err("ce n'est pas un calendrier iCal (BEGIN:VCALENDAR absent)".into());
    }
    let calendar: Calendar = text
        .parse()
        .map_err(|e| format!("fichier iCal illisible : {e}"))?;

    let mut events = Vec::new();
    for component in &calendar.components {
        let CalendarComponent::Event(event) = component else {
            continue;
        };
        // Un cours annulé reste dans le flux avec le statut CANCELLED.
        if event.get_status() == Some(EventStatus::Cancelled) {
            continue;
        }
        let Some(start) = event.get_start() else {
            continue;
        };

        let (starts_at, all_day) = to_instant(&start);
        let ends_at = match event.get_end() {
            Some(end) => to_instant(&end).0,
            // Pas de fin : une journée entière pour un événement daté, une heure sinon.
            None if all_day => starts_at + 24 * 3600,
            None => starts_at + 3600,
        };
        if ends_at <= starts_at {
            continue;
        }

        let title = unescape(event.get_summary().unwrap_or(""))
            .trim()
            .to_string();
        events.push(ParsedEvent {
            uid: event.get_uid().unwrap_or("").to_string(),
            title: if title.is_empty() {
                "(sans titre)".into()
            } else {
                title
            },
            location: unescape(event.get_location().unwrap_or(""))
                .trim()
                .to_string(),
            starts_at,
            ends_at,
            all_day,
        });
    }
    events.sort_by_key(|e| (e.starts_at, e.ends_at));
    Ok(events)
}

/// Convertit une date iCal en instant Unix. Le booléen vaut vrai pour une
/// date sans heure (journée entière).
fn to_instant(value: &DatePerhapsTime) -> (i64, bool) {
    match value {
        DatePerhapsTime::DateTime(CalendarDateTime::Utc(dt)) => (dt.timestamp(), false),
        DatePerhapsTime::DateTime(CalendarDateTime::WithTimezone { date_time, tzid }) => {
            // Fuseau inconnu de la base IANA : on retombe sur Paris.
            let tz = tzid.parse::<Tz>().unwrap_or(PARIS);
            (local_to_ts(*date_time, tz), false)
        }
        DatePerhapsTime::DateTime(CalendarDateTime::Floating(date_time)) => {
            (local_to_ts(*date_time, PARIS), false)
        }
        DatePerhapsTime::Date(date) => (local_to_ts(date.and_time(NaiveTime::MIN), PARIS), true),
    }
}

/// Retire l'échappement du format iCal (`\,` `\;` `\n` `\\`).
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push(' '),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// Matière d'un cours : le début du titre, avant le premier « - ».
/// Pronote écrit « MATHS - DUPONT J. - Salle 12 ».
pub fn subject(title: &str) -> &str {
    title.split(" - ").next().unwrap_or(title).trim()
}

/// Reste du titre après la matière, s'il y en a un.
pub fn details(title: &str) -> String {
    title
        .split(" - ")
        .skip(1)
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Nombre de couleurs de la palette des matières (classes `.ev-c0` à `.ev-c11`).
pub const SUBJECT_COLORS: u64 = 12;

/// Couleur d'une matière, dérivée de son nom : le même cours a toujours la
/// même couleur, sans rien avoir à configurer.
pub fn subject_color(title: &str) -> u8 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in subject(title).to_lowercase().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    // Les bits de poids faible de FNV sont peu mélangés : sans ce repli des
    // bits hauts, beaucoup de matières tombaient sur la même couleur.
    ((hash ^ (hash >> 32)) % SUBJECT_COLORS) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matiere_et_couleur_stables() {
        assert_eq!(subject("MATHS - DUPONT J. - Salle 12"), "MATHS");
        assert_eq!(
            details("MATHS - DUPONT J. - Salle 12"),
            "DUPONT J. · Salle 12"
        );
        assert_eq!(subject("Réunion"), "Réunion");
        assert_eq!(details("Réunion"), "");
        // Même matière, autre prof, autre casse : même couleur.
        assert_eq!(
            subject_color("MATHS - DUPONT J."),
            subject_color("Maths - MARTIN A.")
        );
        assert!(u64::from(subject_color("ANGLAIS")) < SUBJECT_COLORS);
    }

    #[test]
    fn echappement_ical() {
        assert_eq!(
            unescape(r"Salle 12\, bât. B\; étage 2\nNote"),
            "Salle 12, bât. B; étage 2 Note"
        );
    }

    #[test]
    fn refuse_ce_qui_n_est_pas_un_calendrier() {
        assert!(parse("<html>Erreur 404</html>").is_err());
    }
}
