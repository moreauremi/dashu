//! Palette de commandes (Ctrl+K / Cmd+K) : accès rapide aux pages, aux
//! actions et aux notes de tous les espaces.

use askama::Template;
use axum::extract::{Query, State};
use axum::response::Response;
use serde::Deserialize;

use crate::AppState;
use crate::auth::session::Ctx;
use crate::notes::{self, markdown};
use crate::spaces::Space;
use crate::web::{self, AppResult, escape};

struct Command {
    label: &'static str,
    href: &'static str,
    icon: &'static str,
    /// Raccourci clavier affiché à droite.
    keys: &'static str,
    /// Mots supplémentaires qui font remonter la commande.
    keywords: &'static str,
}

const COMMANDS: [Command; 6] = [
    Command {
        label: "Nouvelle note",
        href: "/notes/new",
        icon: "plus",
        keys: "N",
        keywords: "creer ecrire ajouter",
    },
    Command {
        label: "Dashboard",
        href: "/",
        icon: "dashboard",
        keys: "G D",
        keywords: "accueil home",
    },
    Command {
        label: "Notes",
        href: "/notes",
        icon: "notes",
        keys: "G N",
        keywords: "liste",
    },
    Command {
        label: "Agenda",
        href: "/calendar",
        icon: "calendar",
        keys: "G C",
        keywords: "calendrier emploi du temps cours",
    },
    Command {
        label: "Réglages",
        href: "/settings",
        icon: "settings",
        keys: "",
        keywords: "parametres mot de passe theme google ical",
    },
    Command {
        label: "Guide de style",
        href: "/styleguide",
        icon: "eye",
        keys: "",
        keywords: "design couleurs",
    },
];

struct NoteHit {
    id: i64,
    title_html: String,
    excerpt_html: String,
    space_name: String,
    space_color: String,
}

#[derive(Template)]
#[template(path = "search/_palette.html")]
struct PaletteResults<'a> {
    commands: Vec<&'a Command>,
    /// Espaces vers lesquels on peut basculer.
    spaces: Vec<&'a Space>,
    notes: Vec<NoteHit>,
    notes_heading: &'static str,
    csrf: &'a str,
}

#[derive(Deserialize)]
pub struct PaletteQuery {
    q: Option<String>,
}

/// `GET /search/palette?q=…`
pub async fn palette(
    ctx: Ctx,
    State(state): State<AppState>,
    Query(query): Query<PaletteQuery>,
) -> AppResult<Response> {
    let q = query.q.unwrap_or_default();
    let needle = fold(q.trim());

    let commands: Vec<&Command> = COMMANDS
        .iter()
        .filter(|c| {
            needle.is_empty() || fold(c.label).contains(&needle) || c.keywords.contains(&needle)
        })
        .collect();

    let spaces: Vec<&Space> = ctx
        .spaces
        .iter()
        .filter(|s| s.id != ctx.active.id)
        .filter(|s| needle.is_empty() || fold(&format!("espace {}", s.name)).contains(&needle))
        .collect();

    let space_of = |id: i64| {
        ctx.space_by_id(id)
            .map(|s| (s.name.clone(), s.color.clone()))
            .unwrap_or_default()
    };

    let (notes, notes_heading) = match notes::fts_query(&q) {
        // Recherche plein texte dans tous les espaces de l'utilisateur.
        Some(fts) => {
            let rows = sqlx::query!(
                r#"SELECT n.id AS "id!", n.title AS "title!", n.space_id AS "space_id!",
                          highlight(notes_fts, 0, char(1), char(2)) AS "title_marked!: String",
                          snippet(notes_fts, 1, char(1), char(2), '…', 12) AS "snippet!: String"
                   FROM notes_fts
                   JOIN notes n ON n.id = notes_fts.rowid
                   WHERE notes_fts MATCH ?1 AND n.user_id = ?2
                   ORDER BY bm25(notes_fts, 5.0, 1.0)
                   LIMIT 8"#,
                fts,
                ctx.user.id
            )
            .fetch_all(&state.db)
            .await?;
            let hits = rows
                .into_iter()
                .map(|row| {
                    let (space_name, space_color) = space_of(row.space_id);
                    NoteHit {
                        id: row.id,
                        title_html: if row.title.trim().is_empty() {
                            "Sans titre".into()
                        } else {
                            notes::mark_matches(&row.title_marked)
                        },
                        excerpt_html: notes::mark_matches(&row.snippet),
                        space_name,
                        space_color,
                    }
                })
                .collect();
            (hits, "Notes")
        }
        // Palette juste ouverte : les dernières notes modifiées.
        None => {
            let rows = sqlx::query!(
                r#"SELECT id AS "id!", title, space_id, substr(body_md, 1, 200) AS "head!: String"
                   FROM notes WHERE user_id = ? ORDER BY updated_at DESC LIMIT 5"#,
                ctx.user.id
            )
            .fetch_all(&state.db)
            .await?;
            let hits = rows
                .into_iter()
                .map(|row| {
                    let (space_name, space_color) = space_of(row.space_id);
                    NoteHit {
                        id: row.id,
                        title_html: if row.title.trim().is_empty() {
                            "Sans titre".into()
                        } else {
                            escape(&row.title)
                        },
                        excerpt_html: escape(&markdown::excerpt(&row.head, 70)),
                        space_name,
                        space_color,
                    }
                })
                .collect();
            (hits, "Notes récentes")
        }
    };

    Ok(web::html(PaletteResults {
        commands,
        spaces,
        notes,
        notes_heading,
        csrf: &ctx.csrf,
    }))
}

/// Minuscules sans accents, pour comparer « reglages » et « Réglages ».
fn fold(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| match c {
            'à' | 'â' | 'ä' => 'a',
            'ç' => 'c',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparaison_sans_accents() {
        assert_eq!(fold("Réglages"), "reglages");
        assert_eq!(fold("École"), "ecole");
        assert!(fold("Passer à l'espace École").contains(&fold("ECOLE")));
    }
}
