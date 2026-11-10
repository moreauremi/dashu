//! Dates et heures. La base stocke des instants Unix (secondes, UTC) ; tout ce
//! qui est affiché passe par ce module pour être converti en Europe/Paris et
//! écrit en français.

use chrono::{
    DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc,
    Weekday,
};
use chrono_tz::Tz;

pub const PARIS: Tz = chrono_tz::Europe::Paris;

const DAYS: [&str; 7] = [
    "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche",
];
const DAYS_SHORT: [&str; 7] = ["lun.", "mar.", "mer.", "jeu.", "ven.", "sam.", "dim."];
const MONTHS: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];
const MONTHS_SHORT: [&str; 12] = [
    "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.",
    "déc.",
];

/// Instant présent, en secondes Unix.
pub fn now() -> i64 {
    Utc::now().timestamp()
}

/// Un instant Unix vu depuis Paris.
pub fn paris(ts: i64) -> DateTime<Tz> {
    DateTime::from_timestamp(ts, 0)
        .unwrap_or_default()
        .with_timezone(&PARIS)
}

/// Le jour qu'il est à Paris à l'instant `ts`.
pub fn paris_date(ts: i64) -> NaiveDate {
    paris(ts).date_naive()
}

/// Convertit une heure locale d'un fuseau en instant Unix.
///
/// Deux cas limites au changement d'heure : l'heure qui existe deux fois en
/// octobre (on prend la première) et celle qui n'existe pas en mars (on avance
/// d'une heure, comme le fait une horloge).
pub fn local_to_ts(local: NaiveDateTime, tz: Tz) -> i64 {
    match tz.from_local_datetime(&local) {
        LocalResult::Single(dt) => dt.timestamp(),
        LocalResult::Ambiguous(first, _) => first.timestamp(),
        LocalResult::None => match tz.from_local_datetime(&(local + Duration::hours(1))) {
            LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) => dt.timestamp(),
            LocalResult::None => local.and_utc().timestamp(),
        },
    }
}

/// Minuit à Paris au début du jour donné, en instant Unix.
pub fn day_start(date: NaiveDate) -> i64 {
    local_to_ts(date.and_time(NaiveTime::MIN), PARIS)
}

/// Lundi de la semaine qui contient `date`.
pub fn monday_of(date: NaiveDate) -> NaiveDate {
    date - Duration::days(i64::from(date.weekday().num_days_from_monday()))
}

/// « 08:05 »
pub fn hm(ts: i64) -> String {
    paris(ts).format("%H:%M").to_string()
}

/// « mardi 10 novembre 2026 »
pub fn date_long(date: NaiveDate) -> String {
    format!(
        "{} {} {}",
        weekday(date.weekday()),
        day_month(date),
        date.year()
    )
}

/// « mar. 10 nov. »
pub fn date_short(date: NaiveDate) -> String {
    format!(
        "{} {} {}",
        DAYS_SHORT[date.weekday().num_days_from_monday() as usize],
        day_number(date),
        MONTHS_SHORT[date.month0() as usize]
    )
}

/// « 10 novembre » (« 1er » pour le premier du mois)
pub fn day_month(date: NaiveDate) -> String {
    format!("{} {}", day_number(date), MONTHS[date.month0() as usize])
}

pub fn weekday(day: Weekday) -> &'static str {
    DAYS[day.num_days_from_monday() as usize]
}

pub fn weekday_short(day: Weekday) -> &'static str {
    DAYS_SHORT[day.num_days_from_monday() as usize]
}

pub fn month_name(month0: u32) -> &'static str {
    MONTHS[month0 as usize % 12]
}

fn day_number(date: NaiveDate) -> String {
    if date.day() == 1 {
        "1er".into()
    } else {
        date.day().to_string()
    }
}

/// « Aujourd'hui », « Demain », « Hier », sinon « mar. 10 nov. »
pub fn relative_day(date: NaiveDate, today: NaiveDate) -> String {
    match (date - today).num_days() {
        0 => "Aujourd'hui".into(),
        1 => "Demain".into(),
        -1 => "Hier".into(),
        _ => date_short(date),
    }
}

/// Ancienneté lisible d'un instant : « à l'instant », « il y a 5 min », « hier »…
pub fn ago(ts: i64, now: i64) -> String {
    let seconds = (now - ts).max(0);
    let days = (paris_date(now) - paris_date(ts)).num_days();
    match (seconds, days) {
        (s, _) if s < 60 => "à l'instant".into(),
        (s, _) if s < 3600 => format!("il y a {} min", s / 60),
        (s, 0) => format!("il y a {} h", s / 3600),
        (_, 1) => "hier".into(),
        (_, d) if d < 7 => format!("il y a {d} jours"),
        _ => {
            let date = paris_date(ts);
            if date.year() == paris_date(now).year() {
                format!(
                    "{} {}",
                    day_number(date),
                    MONTHS_SHORT[date.month0() as usize]
                )
            } else {
                format!(
                    "{} {} {}",
                    day_number(date),
                    MONTHS_SHORT[date.month0() as usize],
                    date.year()
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(y: i32, m: u32, d: u32, h: u32, min: u32) -> i64 {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0)
            .unwrap()
            .timestamp()
    }

    #[test]
    fn paris_suit_l_heure_d_ete() {
        // Hiver : UTC+1. Été : UTC+2.
        assert_eq!(hm(ts(2026, 1, 15, 7, 0)), "08:00");
        assert_eq!(hm(ts(2026, 7, 15, 6, 0)), "08:00");
    }

    #[test]
    fn local_to_ts_gere_les_changements_d_heure() {
        let d = |y, m, d, h, min| {
            NaiveDate::from_ymd_opt(y, m, d)
                .unwrap()
                .and_hms_opt(h, min, 0)
                .unwrap()
        };
        // 29 mars 2026, 02:30 n'existe pas à Paris : on tombe à 03:30 (01:30 UTC).
        assert_eq!(
            local_to_ts(d(2026, 3, 29, 2, 30), PARIS),
            ts(2026, 3, 29, 1, 30)
        );
        // 25 octobre 2026, 02:30 existe deux fois : on prend la première (heure d'été).
        assert_eq!(
            local_to_ts(d(2026, 10, 25, 2, 30), PARIS),
            ts(2026, 10, 25, 0, 30)
        );
    }

    #[test]
    fn dates_en_francais() {
        let date = NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();
        assert_eq!(date_long(date), "mardi 10 novembre 2026");
        assert_eq!(date_short(date), "mar. 10 nov.");
        assert_eq!(
            monday_of(date),
            NaiveDate::from_ymd_opt(2026, 11, 9).unwrap()
        );
        assert_eq!(
            day_month(NaiveDate::from_ymd_opt(2026, 5, 1).unwrap()),
            "1er mai"
        );
    }

    #[test]
    fn anciennete() {
        let now = ts(2026, 11, 10, 15, 0);
        assert_eq!(ago(now - 20, now), "à l'instant");
        assert_eq!(ago(now - 300, now), "il y a 5 min");
        assert_eq!(ago(now - 2 * 3600, now), "il y a 2 h");
        assert_eq!(ago(now - 26 * 3600, now), "hier");
        assert_eq!(ago(ts(2026, 9, 2, 10, 0), now), "2 sept.");
    }
}
