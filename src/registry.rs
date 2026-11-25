//! Registre des outils (SPEC, « Outils par espace »).
//!
//! Un outil est une entrée de la barre latérale : Dashboard, Notes, Fichiers…
//! [`TOOLS`] les déclare tous, une seule fois. La barre latérale, la barre de
//! commandes (`/go`), la barre de raccourcis du téléphone, les réglages de
//! chaque espace et l'API en sont tirés : ajouter un outil, c'est ajouter une
//! entrée ici, plus ses pages.
//!
//! Chaque compte choisit, espace par espace, les outils affichés (table
//! `space_tools`). Pas de ligne : la valeur par défaut de l'entrée. C'est un
//! réglage d'affichage, pas une permission : aucun droit d'accès n'en dépend.

use axum::response::Response;
use sqlx::SqlitePool;

use crate::web::AppResult;

/// Un outil de la barre latérale.
#[derive(Debug, PartialEq, Eq)]
pub struct Tool {
    /// Identifiant fixe : enregistré en base, employé par l'API. C'est aussi
    /// l'entrée de menu mise en avant (`Shell::nav`).
    pub id: &'static str,
    /// Nom affiché.
    pub name: &'static str,
    /// Icône du sprite (`static/icons.svg`).
    pub icon: &'static str,
    /// Adresse de la page, sans l'espace.
    pub route: &'static str,
    /// L'adresse prend l'espace en préfixe (`/e/ecole/notes`). L'onglet Outils
    /// ne le prend pas : il vaut pour tous les espaces.
    pub scoped: bool,
    /// Raccourci clavier (`g n`), vide s'il n'y en a pas.
    pub key: &'static str,
    /// Mot de `/go` dans la barre de commandes, et ses autres formes.
    pub command: &'static str,
    pub aliases: &'static [&'static str],
    /// Affiché tant que le compte n'a rien réglé dans l'espace.
    pub default_shown: bool,
    /// Faux pour l'accueil de l'espace, qui reste toujours.
    pub maskable: bool,
    /// Réservé aux comptes autorisés sur le serveur (`dashu tools grant`).
    pub reserved: bool,
}

/// Tous les outils, dans l'ordre de la barre latérale.
pub const TOOLS: &[Tool] = &[
    Tool {
        id: "dashboard",
        name: "Dashboard",
        icon: "dashboard",
        route: "/",
        scoped: true,
        key: "g d",
        command: "dashboard",
        aliases: &["accueil"],
        default_shown: true,
        maskable: false,
        reserved: false,
    },
    Tool {
        id: "notes",
        name: "Notes",
        icon: "notes",
        route: "/notes",
        scoped: true,
        key: "g n",
        command: "notes",
        aliases: &[],
        default_shown: true,
        maskable: true,
        reserved: false,
    },
    Tool {
        id: "files",
        name: "Fichiers",
        icon: "files",
        route: "/files",
        scoped: true,
        key: "g f",
        command: "fichiers",
        aliases: &["files"],
        default_shown: true,
        maskable: true,
        reserved: false,
    },
    Tool {
        id: "calendar",
        name: "Agenda",
        icon: "calendar",
        route: "/calendar",
        scoped: true,
        key: "g c",
        command: "agenda",
        aliases: &["calendar", "calendrier"],
        default_shown: true,
        maskable: true,
        reserved: false,
    },
    Tool {
        id: "tools",
        name: "Outils",
        icon: "tools",
        route: "/tools",
        scoped: false,
        key: "g o",
        command: "outils",
        aliases: &["tools"],
        default_shown: true,
        maskable: true,
        reserved: false,
    },
];

/// Message montré quand on ouvre l'adresse d'un outil masqué.
pub const HIDDEN: &str = "Cet outil est masqué dans cet espace.";

/// L'outil qui porte cet identifiant.
pub fn get(id: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.id == id)
}

/// L'outil dont la page est à cette adresse (sans l'espace, ni paramètres).
pub fn at(path: &str) -> Option<&'static Tool> {
    let path = path.split(['?', '#']).next().unwrap_or(path);
    TOOLS.iter().find(|t| t.route == path)
}

/// Ce qu'un compte voit des outils dans un espace : ceux qu'il peut régler
/// (les réservés seulement s'il y est autorisé) et ceux qu'il affiche.
#[derive(Debug, Clone, Default)]
pub struct Shown {
    /// Outils réglables par ce compte, dans l'ordre du registre, chacun avec
    /// son état (affiché ou non).
    pub tools: Vec<(&'static Tool, bool)>,
}

impl Shown {
    /// Réglage d'un compte, d'après ses lignes de `space_tools` et ses
    /// autorisations : chaque outil du registre, sa ligne s'il en a une,
    /// sinon sa valeur par défaut.
    pub fn from_rows(rows: &[(String, bool)], granted: &[String]) -> Self {
        let tools = TOOLS
            .iter()
            .filter(|t| !t.reserved || granted.iter().any(|g| g == t.id))
            .map(|t| {
                let stored = rows.iter().find(|(id, _)| id == t.id).map(|(_, s)| *s);
                let shown = !t.maskable || stored.unwrap_or(t.default_shown);
                (t, shown)
            })
            .collect();
        Self { tools }
    }

    /// Outils affichés, dans l'ordre du registre.
    pub fn visible(&self) -> impl Iterator<Item = &'static Tool> + '_ {
        self.tools.iter().filter(|(_, s)| *s).map(|(t, _)| *t)
    }

    /// L'outil est affiché. Faux pour un outil inconnu ou réservé à d'autres.
    pub fn is_shown(&self, id: &str) -> bool {
        self.tools.iter().any(|(t, s)| t.id == id && *s)
    }

    /// Le compte peut régler cet outil : il existe, et s'il est réservé, le
    /// compte y est autorisé. Un outil réservé à d'autres répond comme un
    /// outil inconnu.
    pub fn tool(&self, id: &str) -> Option<&'static Tool> {
        self.tools.iter().map(|(t, _)| *t).find(|t| t.id == id)
    }
}

/// Réglage d'un compte dans un espace.
pub async fn load(db: &SqlitePool, user_id: i64, space_id: i64) -> AppResult<Shown> {
    let rows = sqlx::query!(
        r#"SELECT tool, shown AS "shown!: bool" FROM space_tools
           WHERE user_id = ? AND space_id = ?"#,
        user_id,
        space_id
    )
    .fetch_all(db)
    .await?;
    let rows: Vec<(String, bool)> = rows.into_iter().map(|r| (r.tool, r.shown)).collect();
    // Aucun outil n'est encore réservé : personne n'a d'autorisation.
    Ok(Shown::from_rows(&rows, &[]))
}

/// Affiche ou masque un outil dans un espace, pour ce compte. L'espace doit
/// être accessible au compte (vérifié par l'appelant) ; l'outil doit être
/// réglable (`Shown::tool`, vérifié par l'appelant).
pub async fn store(
    db: &SqlitePool,
    user_id: i64,
    space_id: i64,
    tool: &Tool,
    shown: bool,
) -> AppResult<()> {
    if !tool.maskable {
        return Ok(());
    }
    sqlx::query!(
        "INSERT INTO space_tools (user_id, space_id, tool, shown) VALUES (?, ?, ?, ?)
         ON CONFLICT (user_id, space_id, tool) DO UPDATE SET shown = excluded.shown",
        user_id,
        space_id,
        tool.id,
        shown
    )
    .execute(db)
    .await?;
    Ok(())
}

/// La liste contient cet outil (pour les gabarits, qui n'ont pas de closures).
pub fn contains(tools: &[&Tool], id: &str) -> bool {
    tools.iter().any(|t| t.id == id)
}

/// L'entrée de menu mise en avant (`Shell::nav`) est cet outil.
pub fn is_current(nav: &str, tool: &Tool) -> bool {
    nav == tool.id
}

/// Adresses dont la page prend l'espace en préfixe (voir `spaces::url`).
pub fn is_scoped_route(path: &str) -> bool {
    TOOLS.iter().any(|t| t.scoped && t.route == path)
}

/// Raccourci du téléphone qui dépend d'un outil (« Nouvelle note » suit
/// Notes) ; `None` s'il ne dépend d'aucun.
pub fn tool_of_tab(key: &str) -> Option<&'static str> {
    match key {
        "new-note" => Some("notes"),
        key => get(key).map(|t| t.id),
    }
}

/// Réponse à l'adresse d'un outil masqué : retour à l'accueil de l'espace,
/// avec un message et un lien vers les réglages. `None` si l'outil est
/// affiché.
pub async fn guard(ctx: &crate::auth::session::Ctx, id: &str) -> AppResult<Option<Response>> {
    if ctx.tools.is_shown(id) {
        return Ok(None);
    }
    ctx.flash_link("error", HIDDEN, "/settings#espaces", "Ouvrir les réglages")
        .await?;
    // `spaces::url::rewrite` remet l'espace de la requête devant « / ».
    Ok(Some(crate::web::see_other("/")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registre_coherent() {
        for (i, tool) in TOOLS.iter().enumerate() {
            assert!(
                TOOLS[..i].iter().all(|t| t.id != tool.id),
                "identifiant en double : {}",
                tool.id
            );
            assert!(
                TOOLS[..i].iter().all(|t| t.command != tool.command),
                "mot de commande en double : {}",
                tool.command
            );
            assert!(tool.route.starts_with('/'));
            // L'accueil reste toujours ; un outil caché par défaut doit
            // pouvoir être affiché, donc être masquable.
            assert!(tool.maskable || tool.default_shown);
        }
        assert!(!get("dashboard").unwrap().maskable);
    }

    #[test]
    fn valeurs_par_defaut_et_reglages() {
        let shown = Shown::from_rows(&[], &[]);
        assert!(
            TOOLS
                .iter()
                .filter(|t| !t.reserved)
                .all(|t| shown.is_shown(t.id) == t.default_shown)
        );

        let rows = vec![
            ("files".to_string(), false),
            ("dashboard".to_string(), false),
            ("inconnu".to_string(), true),
        ];
        let shown = Shown::from_rows(&rows, &[]);
        assert!(!shown.is_shown("files"));
        assert!(shown.is_shown("notes"));
        // L'accueil ne se masque pas, même avec une ligne en base.
        assert!(shown.is_shown("dashboard"));
        assert!(!shown.is_shown("inconnu") && shown.tool("inconnu").is_none());
        assert_eq!(at("/files?q=x").map(|t| t.id), Some("files"));
        assert_eq!(tool_of_tab("new-note"), Some("notes"));
        assert_eq!(tool_of_tab("search"), None);
    }
}
