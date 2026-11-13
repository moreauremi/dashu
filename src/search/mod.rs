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

const COMMANDS: [Command; 7] = [
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
        label: "Fichiers",
        href: "/files",
        icon: "files",
        keys: "G F",
        keywords: "documents envoyer telecharger pdf photos",
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

/// Action sur l'élément de la page ouverte (fichier, note) : un formulaire.
struct PageAction {
    label: String,
    icon: &'static str,
    action: String,
    /// Champs cachés du formulaire.
    fields: Vec<(&'static str, String)>,
}

#[derive(Template)]
#[template(path = "search/_palette.html")]
struct PaletteResults<'a> {
    /// « Sur cette page » : conversions et déplacements de l'élément ouvert.
    actions: Vec<PageAction>,
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
    /// Page ouverte (sans l'espace de l'adresse) : `/files/12`, `/notes/3`…
    page: Option<String>,
}

/// Identifiant de `/files/12` ou `/notes/3`, selon le préfixe.
fn item_id(page: &str, prefix: &str) -> Option<i64> {
    page.strip_prefix(prefix)?.parse().ok()
}

/// Actions de l'élément de la page ouverte, filtrées par la saisie.
async fn page_actions(
    ctx: &Ctx,
    state: &AppState,
    page: &str,
    needle: &str,
) -> AppResult<Vec<PageAction>> {
    let mut actions = Vec::new();
    let mut move_to = |kind: &str, id: i64, space_id: i64, back: String| {
        for space in ctx.spaces.iter().filter(|s| s.id != space_id) {
            actions.push(PageAction {
                label: format!("Déplacer vers {}", space.name),
                icon: "move",
                action: format!("/{kind}/{id}/move"),
                fields: vec![("space_id", space.id.to_string()), ("next", back.clone())],
            });
        }
    };
    if let Some(id) = item_id(page, "/files/") {
        let file = sqlx::query!(
            "SELECT space_id, mime, expires_at FROM files WHERE id = ? AND user_id = ?",
            id,
            ctx.user.id
        )
        .fetch_optional(&state.db)
        .await?;
        if let Some(file) = file {
            let mut converts = Vec::new();
            if state.config.work_dir.is_some() {
                for to in crate::tools::conversions_for(&file.mime) {
                    let label = match to {
                        crate::tools::Conversion::Compress => "Compresser ce fichier".to_string(),
                        crate::tools::Conversion::Mp3 => "Extraire le son en MP3".to_string(),
                        crate::tools::Conversion::Pages => continue,
                        // « En JPEG » -> « Convertir en JPEG ».
                        other => {
                            format!("Convertir en {}", other.label().trim_start_matches("En "))
                        }
                    };
                    converts.push(PageAction {
                        label,
                        icon: "convert",
                        action: format!("/files/{id}/convert"),
                        fields: vec![("to", to.key().into()), ("next", format!("/files/{id}"))],
                    });
                }
            }
            if file.expires_at.is_none() {
                move_to("files", id, file.space_id, format!("/files/{id}"));
            }
            actions.splice(0..0, converts);
        }
    } else if let Some(id) = item_id(page, "/notes/") {
        let space_id = sqlx::query_scalar!(
            "SELECT space_id FROM notes WHERE id = ? AND user_id = ?",
            id,
            ctx.user.id
        )
        .fetch_optional(&state.db)
        .await?;
        if let Some(space_id) = space_id {
            move_to("notes", id, space_id, "/notes".into());
        }
    }
    actions.retain(|a| needle.is_empty() || fold(&a.label).contains(needle));
    Ok(actions)
}

/// `GET /search/palette?q=…`
pub async fn palette(
    ctx: Ctx,
    State(state): State<AppState>,
    Query(query): Query<PaletteQuery>,
) -> AppResult<Response> {
    let q = query.q.unwrap_or_default();
    let needle = fold(q.trim());
    let actions = page_actions(
        &ctx,
        &state,
        query.page.as_deref().unwrap_or_default(),
        &needle,
    )
    .await?;

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
                        excerpt_html: notes::mark_matches(&markdown::plain(&row.snippet)),
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
        actions,
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
