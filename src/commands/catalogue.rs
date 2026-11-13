//! La liste des commandes, pour la page ouverte.
//!
//! C'est la seule définition des commandes : le navigateur la reçoit pour
//! compléter la saisie, et `POST /cmd` la relit pour exécuter une ligne.
//! Ce qui dépend de la page (une note ou un fichier ouvert) et du compte
//! (ses espaces, ses tâches du jour, ses tags) y est déjà résolu.

use super::{Action, Arg, Choice, Mode, Node, Vars, day_choices, space_choices};
use crate::AppState;
use crate::auth::session::Ctx;
use crate::tools::{self, Conversion};
use crate::web::AppResult;
use crate::{dashboard, notes};

/// L'élément ouvert dans la page, s'il y en a un.
pub enum Opened {
    Nothing,
    Note {
        id: i64,
        space_id: i64,
        title: String,
        pinned: bool,
    },
    File {
        id: i64,
        space_id: i64,
        name: String,
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
            "SELECT space_id, name, mime, expires_at FROM files WHERE id = ? AND user_id = ?",
            id,
            ctx.user.id
        )
        .fetch_optional(&state.db)
        .await?;
        if let Some(file) = file {
            return Ok(Opened::File {
                id,
                space_id: file.space_id,
                name: file.name,
                mime: file.mime,
                temporary: file.expires_at.is_some(),
            });
        }
    } else if let Some(id) = item_id(page, "/notes/") {
        let note = sqlx::query!(
            r#"SELECT space_id, title, pinned AS "pinned!: bool" FROM notes WHERE id = ? AND user_id = ?"#,
            id,
            ctx.user.id
        )
        .fetch_optional(&state.db)
        .await?;
        if let Some(note) = note {
            return Ok(Opened::Note {
                id,
                space_id: note.space_id,
                title: note.title,
                pinned: note.pinned,
            });
        }
    }
    Ok(Opened::Nothing)
}

const ON_ITEM: &str = "S'emploie sur une note ou un fichier ouvert.";
const ON_NOTE: &str = "S'emploie sur une note ouverte.";
const ON_FILE: &str = "S'emploie sur un fichier ouvert.";
const NOT_KEPT: &str =
    "Ce fichier n'est pas encore rangé : garde-le d'abord dans un espace (/keep).";
const NO_CONVERSION: &str =
    "Ce fichier ne se convertit pas, ou les conversions ne sont pas activées.";
const CONFIRM: &str = "Entrée encore pour confirmer.";

/// Toutes les commandes, dans l'ordre où la barre les propose.
pub async fn tree(ctx: &Ctx, state: &AppState, opened: &Opened) -> AppResult<Vec<Node>> {
    let spaces = |prefix: &str| space_choices(ctx.spaces.iter(), prefix);
    // #date et #cours, proposés dans les textes libres (titres, tags…).
    let vars = Vars::now(ctx, state).await?.choices();
    let with_vars = |mut values: Vec<Choice>| {
        values.extend(vars.iter().cloned());
        values
    };

    let agenda_words = {
        let mut words = vec![
            Choice::new("jour", "Vue d'un jour"),
            Choice::new("semaine", "Vue de la semaine"),
        ];
        words.extend(day_choices(""));
        words
    };
    let task_options = {
        let mut options = spaces("@");
        options.extend(day_choices("+"));
        with_vars(options)
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
        .arg(Arg::new(Mode::Free, "<texte>", true).values(with_vars(Vec::new()))),
        Node::new("space", "Changer d'espace", Action::Space).arg(
            Arg::new(Mode::Choice, "<espace>", true).values(space_choices(
                ctx.spaces.iter().filter(|s| s.id != ctx.active.id),
                "",
            )),
        ),
        Node::group(
            "add",
            "Créer une tâche, une note, un espace, envoyer un fichier",
            vec![
                Node::new("task", "Ajouter une tâche", Action::AddTask)
                    .aliases(&["tache"])
                    .arg(
                        Arg::new(
                            Mode::Free,
                            "<titre> [@espace] [+quand : +demain, +lundi]",
                            true,
                        )
                        .values(task_options),
                    ),
                Node::new("note", "Créer une note", Action::AddNote).arg(
                    Arg::new(Mode::Free, "[titre] [@espace]", false).values(with_vars(spaces("@"))),
                ),
                {
                    let file = Node::new("file", "Envoyer un fichier", Action::AddFile)
                        .aliases(&["fichier"]);
                    if state.config.files.is_some() {
                        file
                    } else {
                        file.off("Les fichiers ne sont pas activés sur ce serveur.")
                    }
                },
                Node::new("space", "Créer un espace", Action::AddSpace)
                    .aliases(&["espace"])
                    .arg(Arg::new(Mode::Free, "<nom>", true).values(with_vars(Vec::new()))),
                Node::new("agenda", "Ajouter un agenda (flux iCal)", Action::AddAgenda)
                    .aliases(&["calendar"])
                    .arg(
                        Arg::new(Mode::Free, "<url https://…> [@espace] [nom]", true)
                            .values(spaces("@")),
                    ),
            ],
        ),
    ];

    // Les tâches du jour : à cocher, à décocher, à supprimer.
    let tasks = dashboard::day_tasks(&state.db, ctx).await?;
    let task_choices = |done: Option<bool>| -> Vec<Choice> {
        tasks
            .iter()
            .filter(|t| done.is_none_or(|d| t.done == d))
            .map(|t| {
                let help = ctx.tag_of(t.space_id).map(|s| s.name).unwrap_or_default();
                Choice::new(t.title.clone(), help)
            })
            .collect()
    };
    let with_tasks = |node: Node, choices: Vec<Choice>, none: &'static str| {
        if choices.is_empty() {
            node.off(none)
        } else {
            node.arg(Arg::new(Mode::Line, "<tâche>", true).values(choices))
        }
    };
    tree.push(with_tasks(
        Node::new("done", "Cocher une tâche du jour", Action::Done),
        task_choices(Some(false)),
        "Aucune tâche à cocher aujourd'hui.",
    ));
    tree.push(with_tasks(
        Node::new(
            "undone",
            "Décocher une tâche cochée aujourd'hui",
            Action::Undone,
        ),
        task_choices(Some(true)),
        "Aucune tâche cochée aujourd'hui.",
    ));

    // Supprimer : la note ou le fichier ouvert, ou une tâche du jour.
    let delete_task = with_tasks(
        Node::new("task", "Supprimer une tâche du jour", Action::DeleteTask)
            .aliases(&["tache"])
            .confirm(format!("Supprimer cette tâche ? {CONFIRM}")),
        task_choices(None),
        "Aucune tâche du jour.",
    );
    let delete = |help: &str| Node {
        children: vec![delete_task.clone()],
        ..Node::new("delete", help, Action::Delete)
    };
    tree.push(match opened {
        Opened::Note { title, .. } => delete("Supprimer cette note, ou une tâche (/delete task)")
            .runnable()
            .confirm(format!("Supprimer la note « {} » ? {CONFIRM}", shown(title))),
        Opened::File { name, .. } => delete("Supprimer ce fichier, ou une tâche (/delete task)")
            .runnable()
            .confirm(format!("Supprimer le fichier « {name} » ? {CONFIRM}")),
        Opened::Nothing if delete_task.off.is_none() => delete("Supprimer une tâche du jour"),
        Opened::Nothing => delete("Supprimer").off(
            "S'emploie sur une note ou un fichier ouvert, ou pour une tâche du jour (/delete task).",
        ),
    });

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

    // Sur la page d'une note : tags et épinglage.
    let tag = Node::new("tag", "Ajouter un tag à cette note", Action::Tag);
    let untag = Node::new("untag", "Retirer un tag de cette note", Action::Untag);
    let pin = |help: &str| Node::new("pin", help, Action::Pin).aliases(&["epingler"]);
    match opened {
        Opened::Note { id, pinned, .. } => {
            let mine: Vec<String> = notes::tags_of(&state.db, ctx.user.id, *id)
                .await?
                .into_iter()
                .map(|t| t.name)
                .collect();
            let others: Vec<Choice> = notes::all_tag_names(&state.db, ctx.user.id)
                .await?
                .into_iter()
                .filter(|name| !mine.contains(name))
                .map(|name| Choice::new(name, ""))
                .collect();
            tree.push(tag.arg(Arg::new(Mode::Free, "<tag>", true).values(with_vars(others))));
            tree.push(if mine.is_empty() {
                untag.off("Cette note n'a pas de tag.")
            } else {
                untag.arg(
                    Arg::new(Mode::Line, "<tag>", true)
                        .values(mine.into_iter().map(|name| Choice::new(name, "")).collect()),
                )
            });
            tree.push(pin(if *pinned {
                "Désépingler cette note"
            } else {
                "Épingler cette note"
            }));
        }
        _ => {
            tree.push(tag.off(ON_NOTE));
            tree.push(untag.off(ON_NOTE));
            tree.push(pin("Épingler une note").off(ON_NOTE));
        }
    }

    // Sur la page d'un fichier : le garder (onglet Outils), le télécharger.
    let keep =
        Node::new("keep", "Garder ce fichier dans un espace", Action::Keep).aliases(&["garder"]);
    tree.push(match opened {
        Opened::File {
            temporary: true, ..
        } => keep.arg(Arg::new(Mode::Choice, "<espace>", true).values(spaces(""))),
        Opened::File { .. } => keep.off("Ce fichier est déjà rangé dans un espace."),
        _ => keep.off("S'emploie sur un fichier de l'onglet Outils, pas encore rangé."),
    });
    let download =
        Node::new("download", "Télécharger ce fichier", Action::Download).aliases(&["telecharger"]);
    tree.push(if is_file {
        download
    } else {
        download.off(ON_FILE)
    });

    // Réglages et compte.
    tree.extend([
        Node::group(
            "theme",
            "Changer de thème",
            vec![
                Node::new("sombre", "Thème sombre", Action::Theme("dark")).aliases(&["dark"]),
                Node::new("clair", "Thème clair", Action::Theme("light")).aliases(&["light"]),
                Node::new("systeme", "Suivre le système", Action::Theme("system"))
                    .aliases(&["system", "auto"]),
            ],
        ),
        Node::group(
            "size",
            "Taille de l'interface",
            vec![
                Node::new("petit", "Interface plus petite", Action::Size("small"))
                    .aliases(&["small"]),
                Node::new("normal", "Taille normale", Action::Size("normal")),
                Node::new("grand", "Interface plus grande", Action::Size("large"))
                    .aliases(&["large"]),
            ],
        )
        .aliases(&["taille"]),
        Node::new(
            "sidebar",
            if ctx.user.sidebar_floating {
                "Ancrer la barre latérale"
            } else {
                "Détacher la barre latérale (cachée à gauche)"
            },
            Action::Sidebar,
        ),
        Node::new("refresh", "Resynchroniser l'agenda", Action::Refresh),
        Node::new("logout", "Se déconnecter", Action::Logout),
    ]);

    Ok(tree)
}

/// Titre d'une note tel qu'affiché.
fn shown(title: &str) -> &str {
    if title.trim().is_empty() {
        "Sans titre"
    } else {
        title
    }
}
