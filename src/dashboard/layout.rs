//! Disposition du dashboard : les widgets d'un espace, dans l'ordre, chacun
//! avec sa taille en cases de la grille bento (largeur 1 à 4 colonnes,
//! hauteur 1 à 4 rangées). La grille comble elle-même les trous
//! (`grid-auto-flow: dense`) : l'ordre et les tailles suffisent.
//!
//! Enregistrée dans `spaces.dashboard` sous une forme courte et lisible,
//! « courses:2x2,tasks:2x2,notes:2x1 ». NULL : la disposition par défaut.

use sqlx::SqlitePool;

use crate::web::AppResult;

/// Un type de widget : un par outil.
pub struct Kind {
    /// Identifiant, aussi dans l'adresse du fragment : `/widgets/{key}`.
    pub key: &'static str,
    pub title: &'static str,
    /// Outil auquel il appartient, affiché dans le catalogue.
    pub tool: &'static str,
    pub icon: &'static str,
    /// Lien de l'en-tête de la carte : (adresse, texte).
    pub link: Option<(&'static str, &'static str)>,
}

pub const KINDS: [Kind; 5] = [
    Kind {
        key: "courses",
        title: "Prochains cours",
        tool: "Agenda",
        icon: "calendar",
        link: Some(("/calendar", "Agenda")),
    },
    Kind {
        key: "tasks",
        title: "Tâches du jour",
        tool: "Tâches",
        icon: "tasks",
        link: None,
    },
    Kind {
        key: "notes",
        title: "Notes",
        tool: "Notes",
        icon: "notes",
        link: Some(("/notes", "Toutes les notes")),
    },
    Kind {
        key: "files",
        title: "Fichiers récents",
        tool: "Fichiers",
        icon: "files",
        link: Some(("/files", "Tous les fichiers")),
    },
    Kind {
        key: "settings",
        title: "Réglages rapides",
        tool: "Réglages",
        icon: "settings",
        link: Some(("/settings", "Réglages")),
    },
];

/// Tailles proposées d'un clic : (lettre, nom, largeur, hauteur). Entre les
/// deux, la poignée du coin donne n'importe quelle taille de la grille.
pub const SIZES: [(&str, &str, u8, u8); 3] = [
    ("S", "Petit", 1, 1),
    ("M", "Moyen", 2, 1),
    ("L", "Grand", 2, 2),
];

/// Colonnes et rangées qu'un widget peut couvrir au plus.
pub const MAX_SPAN: u8 = 4;

/// Disposition d'un espace qui n'a jamais été réglée.
pub const DEFAULT: &str = "courses:2x2,tasks:2x2,notes:2x1,files:2x1";

#[derive(Clone, Copy)]
pub struct Widget {
    pub kind: &'static Kind,
    pub w: u8,
    pub h: u8,
}

impl Widget {
    /// Vrai si la taille correspond à l'une des tailles d'un clic.
    pub fn is_size(&self, w: &u8, h: &u8) -> bool {
        self.w == *w && self.h == *h
    }
}

pub fn kind(key: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|k| k.key == key)
}

/// Lit une disposition. Refuse un type inconnu, un widget en double ou une
/// taille hors de la grille.
pub fn parse(text: &str) -> Result<Vec<Widget>, String> {
    let mut widgets: Vec<Widget> = Vec::new();
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (key, size) = part
            .split_once(':')
            .ok_or_else(|| format!("widget mal écrit : « {part} »"))?;
        let kind = kind(key).ok_or_else(|| format!("widget inconnu : « {key} »"))?;
        if widgets.iter().any(|w| w.kind.key == key) {
            return Err(format!("widget en double : « {key} »"));
        }
        let (w, h) = size
            .split_once('x')
            .and_then(|(w, h)| Some((w.parse::<u8>().ok()?, h.parse::<u8>().ok()?)))
            .filter(|&(w, h)| (1..=MAX_SPAN).contains(&w) && (1..=MAX_SPAN).contains(&h))
            .ok_or_else(|| format!("taille impossible pour « {key} » : « {size} »"))?;
        widgets.push(Widget { kind, w, h });
    }
    Ok(widgets)
}

pub fn encode(widgets: &[Widget]) -> String {
    widgets
        .iter()
        .map(|w| format!("{}:{}x{}", w.kind.key, w.w, w.h))
        .collect::<Vec<_>>()
        .join(",")
}

/// Disposition de l'espace. `files` : faux si les fichiers ne sont pas
/// activés sur ce serveur (le widget Fichiers est alors retiré).
pub async fn load(db: &SqlitePool, space_id: i64, files: bool) -> AppResult<Vec<Widget>> {
    let saved = sqlx::query_scalar!("SELECT dashboard FROM spaces WHERE id = ?", space_id)
        .fetch_optional(db)
        .await?
        .flatten();
    // Une disposition illisible (écrite à la main dans la base ?) ne doit pas
    // casser la page d'accueil : on revient à celle par défaut.
    let widgets = saved
        .and_then(|text| parse(&text).ok())
        .unwrap_or_else(|| parse(DEFAULT).expect("disposition par défaut valide"));
    Ok(widgets
        .into_iter()
        .filter(|w| files || w.kind.key != "files")
        .collect())
}

/// Enregistre la disposition d'un espace ; `None` revient à celle par défaut.
pub async fn save(
    db: &SqlitePool,
    user_id: i64,
    space_id: i64,
    widgets: Option<&[Widget]>,
) -> AppResult<()> {
    let text = widgets.map(encode);
    sqlx::query!(
        "UPDATE spaces SET dashboard = ? WHERE id = ? AND user_id = ?",
        text,
        space_id,
        user_id
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Taille demandée à un fragment de widget (`/widgets/notes?w=2&h=1`).
/// Absente : la taille d'avant le dashboard modulaire.
#[derive(serde::Deserialize, Default)]
pub struct SizeQuery {
    w: Option<u8>,
    h: Option<u8>,
}

impl SizeQuery {
    /// (largeur, hauteur), ramenées dans la grille ; `None` sans paramètres.
    pub fn get(&self) -> Option<(u8, u8)> {
        match (self.w, self.h) {
            (Some(w), Some(h)) => Some((w.clamp(1, MAX_SPAN), h.clamp(1, MAX_SPAN))),
            _ => None,
        }
    }

    /// Nombre d'éléments de liste qui tiennent dans le widget : `per_row`
    /// selon la hauteur (1 à 4 rangées), un seul pour un petit carré,
    /// `legacy` sans taille.
    pub fn items(&self, per_row: [i64; 4], legacy: i64) -> i64 {
        match self.get() {
            None => legacy,
            Some((1, 1)) => 1,
            Some((_, h)) => per_row[usize::from(h - 1)],
        }
    }

    /// Petit carré (1 × 1) : le widget se résume.
    pub fn compact(&self) -> bool {
        self.get() == Some((1, 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lecture_et_ecriture() {
        let widgets = parse(" courses:2x2, notes:1x3 ,").unwrap();
        assert_eq!(encode(&widgets), "courses:2x2,notes:1x3");
        assert!(parse("").unwrap().is_empty());
        assert!(parse(DEFAULT).is_ok());
    }

    #[test]
    fn refus() {
        for bad in [
            "meteo:1x1",
            "notes:2x2,notes:1x1",
            "notes:0x1",
            "notes:5x1",
            "notes:2",
            "notes",
            "notes:axb",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }
}
