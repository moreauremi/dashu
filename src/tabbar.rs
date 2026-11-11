//! Barre de raccourcis du téléphone : en bas de l'écran, quatre entrées au
//! plus, choisies parmi les pages et actions de l'application (Réglages).
//!
//! Le choix est enregistré sur le compte (`users.mobile_tabs`), comme le
//! thème : il suit l'utilisateur d'un téléphone à l'autre.

/// Une entrée possible de la barre.
#[derive(Debug, PartialEq, Eq)]
pub struct Tab {
    /// Identifiant enregistré en base et utilisé dans le formulaire.
    pub key: &'static str,
    pub label: &'static str,
    /// Icône du sprite (`static/icons.svg`).
    pub icon: &'static str,
    /// Adresse de la page ; vide pour une action (la recherche ouvre la palette).
    pub href: &'static str,
}

/// Nombre maximal d'entrées dans la barre.
pub const MAX: usize = 4;

/// Toutes les entrées possibles, dans l'ordre où elles s'affichent.
pub const ALL: [Tab; 7] = [
    Tab {
        key: "dashboard",
        label: "Dashboard",
        icon: "dashboard",
        href: "/",
    },
    Tab {
        key: "notes",
        label: "Notes",
        icon: "notes",
        href: "/notes",
    },
    Tab {
        key: "files",
        label: "Fichiers",
        icon: "files",
        href: "/files",
    },
    Tab {
        key: "calendar",
        label: "Agenda",
        icon: "calendar",
        href: "/calendar",
    },
    Tab {
        key: "new-note",
        label: "Nouvelle note",
        icon: "plus",
        href: "/notes/new",
    },
    Tab {
        key: "search",
        label: "Rechercher",
        icon: "search",
        href: "",
    },
    Tab {
        key: "settings",
        label: "Réglages",
        icon: "settings",
        href: "/settings",
    },
];

/// Choix par défaut, tant que l'utilisateur n'a rien réglé.
const DEFAULT: [&str; MAX] = ["dashboard", "notes", "calendar", "search"];

/// Entrées à afficher, d'après la valeur enregistrée (`None` : par défaut).
/// Les identifiants inconnus sont ignorés ; au-delà de quatre, on coupe.
pub fn selected(stored: Option<&str>) -> Vec<&'static Tab> {
    let keys: Vec<&str> = match stored {
        Some(text) => text.split(',').map(str::trim).collect(),
        None => DEFAULT.to_vec(),
    };
    let tabs: Vec<&'static Tab> = ALL
        .iter()
        .filter(|tab| keys.contains(&tab.key))
        .take(MAX)
        .collect();
    if tabs.is_empty() {
        selected(None)
    } else {
        tabs
    }
}

/// Valeur à enregistrer pour ces identifiants, ou un message d'erreur.
pub fn encode(keys: &[&str]) -> Result<String, String> {
    let chosen: Vec<&str> = ALL
        .iter()
        .map(|tab| tab.key)
        .filter(|key| keys.contains(key))
        .collect();
    match chosen.len() {
        0 => Err("Choisis au moins un raccourci.".into()),
        n if n > MAX => Err(format!("{MAX} raccourcis au plus.")),
        _ => Ok(chosen.join(",")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(tabs: &[&Tab]) -> Vec<&'static str> {
        tabs.iter().map(|t| t.key).collect()
    }

    #[test]
    fn choix_par_defaut() {
        assert_eq!(
            keys(&selected(None)),
            ["dashboard", "notes", "calendar", "search"]
        );
    }

    #[test]
    fn ordre_fixe_inconnus_ignores_et_quatre_au_plus() {
        assert_eq!(
            keys(&selected(Some("settings,calendar,inconnu"))),
            ["calendar", "settings"]
        );
        assert_eq!(
            keys(&selected(Some("dashboard,notes,calendar,new-note,search"))),
            ["dashboard", "notes", "calendar", "new-note"]
        );
        // Valeur abîmée : on retombe sur le choix par défaut.
        assert_eq!(keys(&selected(Some(""))), keys(&selected(None)));
    }

    #[test]
    fn encodage_valide() {
        assert_eq!(
            encode(&["search", "dashboard"]).as_deref(),
            Ok("dashboard,search")
        );
        assert!(encode(&[]).is_err());
        assert!(encode(&["dashboard", "notes", "calendar", "search", "settings"]).is_err());
        assert!(encode(&["inconnu"]).is_err());
    }
}
