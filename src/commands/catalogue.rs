//! La liste des commandes, pour la page ouverte.
//!
//! C'est la seule définition des commandes : le navigateur la reçoit pour
//! compléter la saisie, et `POST /cmd` la relit pour exécuter une ligne.
//! Ce qui dépend de la page (une note ou un fichier ouvert) et du compte
//! (ses espaces) y est déjà résolu.

use super::{Action, Arg, Choice, Mode, Node, day_choices, space_choices};
use crate::AppState;
use crate::auth::session::Ctx;
use crate::tools::{self, Conversion};
use crate::web::AppResult;

/// L'élément ouvert dans la page, s'il y en a un.
pub enum Opened {
    Nothing,
    Note {
        id: i64,
        space_id: i64,
    },
    File {
        id: i64,
        space_id: i64,
        mime: String,
        /// Fichier de l'onglet Outils, pas encore rangé dans un espace.
        temporary: bool,
    },
}

/// Identifiant de `/files/12` ou `/notes/3`, selon le préfixe.
fn item_id(page: &str, prefix: &str) -> Option<i64> {
    page.split(['?', '#'])
        .next()?
        .strip_prefix(prefix)?
        .parse()
        .ok()
}

/// Ce qu'affiche la page (adresse sans l'espace) : une note, un fichier de
/// l'utilisateur, ou rien de tel.
pub async fn opened(ctx: &Ctx, state: &AppState, page: &str) -> AppResult<Opened> {
    if let Some(id) = item_id(page, "/files/") {
        let file = sqlx::query!(
            "SELECT space_id, mime, expires_at FROM files WHERE id = ? AND user_id = ?",
            id,
            ctx.user.id
        )
        .fetch_optional(&state.db)
        .await?;
        if let Some(file) = file {
            return Ok(Opened::File {
                id,
                space_id: file.space_id,
                mime: file.mime,
                temporary: file.expires_at.is_some(),
            });
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
            return Ok(Opened::Note { id, space_id });
        }
    }
    Ok(Opened::Nothing)
}

const ON_ITEM: &str = "S'emploie sur une note ou un fichier ouvert.";
const ON_FILE: &str = "S'emploie sur un fichier ouvert.";
const NOT_KEPT: &str = "Ce fichier n'est pas encore rangé : garde-le d'abord dans un espace.";
const NO_CONVERSION: &str =
    "Ce fichier ne se convertit pas, ou les conversions ne sont pas activées.";

/// Toutes les commandes, dans l'ordre où la barre les propose.
pub fn tree(ctx: &Ctx, state: &AppState, opened: &Opened) -> Vec<Node> {
    let spaces = |prefix: &str| space_choices(ctx.spaces.iter(), prefix);

    let agenda_words = {
        let mut words = vec![
            Choice::new("jour", "Vue d'un jour"),
            Choice::new("semaine", "Vue de la semaine"),
        ];
        words.extend(day_choices());
        words
    };

    let mut tree = vec![
        Node::new("help", "Toutes les commandes", Action::Help).aliases(&["aide"]),
        Node::group(
            "go",
            "Aller à une page",
            vec![
                Node::new("dashboard", "Dashboard", Action::GoPage("/")).aliases(&["accueil"]),
                Node::new("notes", "Notes", Action::GoPage("/notes")),
                Node::new("fichiers", "Fichiers", Action::GoPage("/files")).aliases(&["files"]),
                Node::new("agenda", "Agenda", Action::GoAgenda)
                    .aliases(&["calendar", "calendrier"])
                    .arg(
                        Arg::new(
                            Mode::Free,
                            "[jour|semaine] [quand : demain, lundi, 14/11]",
                            false,
                        )
                        .values(agenda_words),
                    ),
                Node::new("outils", "Outils", Action::GoPage("/tools")).aliases(&["tools"]),
                Node::new("reglages", "Réglages", Action::GoPage("/settings"))
                    .aliases(&["settings", "parametres"]),
                Node::new("guide", "Guide de style", Action::GoPage("/styleguide"))
                    .aliases(&["styleguide"]),
            ],
        ),
        Node::new("open", "Ouvrir une note ou un fichier", Action::Open).arg(Arg::new(
            Mode::Remote,
            "<titre ou mot du texte>",
            true,
        )),
        Node::new(
            "search",
            "Chercher dans les notes de l'espace",
            Action::Search,
        )
        .arg(Arg::new(Mode::Free, "<texte>", true)),
        Node::new("space", "Changer d'espace", Action::Space).arg(
            Arg::new(Mode::Choice, "<espace>", true).values(space_choices(
                ctx.spaces.iter().filter(|s| s.id != ctx.active.id),
                "",
            )),
        ),
        Node::group(
            "add",
            "Créer une tâche, une note, envoyer un fichier",
            vec![
                Node::new("task", "Ajouter une tâche du jour", Action::AddTask)
                    .aliases(&["tache"])
                    .arg(Arg::new(Mode::Free, "<titre> [@espace]", true).values(spaces("@"))),
                Node::new("note", "Créer une note", Action::AddNote)
                    .arg(Arg::new(Mode::Free, "[titre] [@espace]", false).values(spaces("@"))),
                {
                    let file = Node::new("file", "Envoyer un fichier", Action::AddFile)
                        .aliases(&["fichier"]);
                    if state.config.files.is_some() {
                        file
                    } else {
                        file.off("Les fichiers ne sont pas activés sur ce serveur.")
                    }
                },
            ],
        ),
    ];

    // Sur la page d'une note ou d'un fichier : le déplacer.
    let item_space = match opened {
        Opened::Note { space_id, .. } => Some(*space_id),
        Opened::File {
            space_id,
            temporary: false,
            ..
        } => Some(*space_id),
        _ => None,
    };
    let move_to =
        Node::new("move", "Déplacer vers un autre espace", Action::Move).aliases(&["deplacer"]);
    tree.push(match (item_space, opened) {
        (Some(space_id), _) => move_to.arg(Arg::new(Mode::Choice, "<espace>", true).values(
            space_choices(ctx.spaces.iter().filter(|s| s.id != space_id), ""),
        )),
        (None, Opened::File { .. }) => move_to.off(NOT_KEPT),
        (None, _) => move_to.off(ON_ITEM),
    });

    // Sur la page d'un fichier : ses conversions, comme son menu « Convertir ».
    let conversions = match opened {
        Opened::File { mime, .. } if state.config.work_dir.is_some() => {
            tools::conversions_for(mime)
        }
        _ => Vec::new(),
    };
    let is_file = matches!(opened, Opened::File { .. });
    let convert = Node::new("convert", "Convertir ce fichier", Action::Group);
    let outputs: Vec<Node> = conversions
        .iter()
        .filter(|c| **c != Conversion::Compress)
        .map(|c| {
            let node = Node::new(c.key(), c.label(), Action::Convert(*c));
            match c {
                Conversion::Jpg => node.aliases(&["jpeg"]),
                Conversion::Pages => node.arg(Arg::new(Mode::Free, "<pages : 2-4 ou 3>", true)),
                _ => node,
            }
        })
        .collect();
    tree.push(match (is_file, outputs.is_empty()) {
        (true, false) => Node {
            children: outputs,
            ..convert
        },
        (true, true) => convert.off(NO_CONVERSION),
        (false, _) => convert.off(ON_FILE),
    });
    let compress = Node::new(
        "compress",
        "Compresser ce fichier (une copie plus légère)",
        Action::Compress,
    );
    tree.push(
        match (is_file, conversions.contains(&Conversion::Compress)) {
            (true, true) => compress,
            (true, false) => compress.off(NO_CONVERSION),
            (false, _) => compress.off(ON_FILE),
        },
    );

    tree
}
