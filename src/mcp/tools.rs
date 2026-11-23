//! Les outils du connecteur : un par action de l'API, mêmes fonctions.
//!
//! Chaque outil porte une annotation qui dit à Claude s'il lit, écrit ou
//! détruit ; Claude s'en sert pour demander confirmation, selon ses propres
//! réglages. Une erreur de saisie (espace inconnu, date invalide, titre trop
//! long) revient comme un résultat d'outil en erreur, avec le message de
//! l'API : Claude peut corriger et réessayer.

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::AppState;
use crate::api::{self, ApiError, ApiUser};

/// Nature d'un outil, traduite en annotations MCP.
#[derive(Clone, Copy)]
enum Kind {
    /// Ne change rien.
    Read,
    /// Crée ou modifie, sans rien perdre.
    Write,
    /// Supprime, remplace ou fait sortir d'un espace partagé.
    Destructive,
}

struct Tool {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    kind: Kind,
    /// Propriétés du schéma d'entrée (JSON Schema).
    properties: fn() -> Value,
    required: &'static [&'static str],
}

fn space_prop(what: &str) -> Value {
    json!({ "type": "string", "description": format!("Identifiant court de l'espace {what} (slug, voir lister_espaces)") })
}

fn date_prop(what: &str) -> Value {
    json!({ "type": "string", "description": format!("{what}, au format AAAA-MM-JJ"), "pattern": "^\\d{4}-\\d{2}-\\d{2}$" })
}

fn id_prop(what: &str) -> Value {
    json!({ "type": "integer", "description": format!("Identifiant de {what}") })
}

fn tags_prop(what: &str) -> Value {
    json!({ "type": "array", "items": { "type": "string" }, "maxItems": 20, "description": what })
}

const TOOLS: &[Tool] = &[
    Tool {
        name: "lister_espaces",
        title: "Lister les espaces",
        description: "Les espaces du compte, espaces partagés rejoints compris : identifiant court (slug) et nom.",
        kind: Kind::Read,
        properties: || json!({}),
        required: &[],
    },
    Tool {
        name: "lister_taches",
        title: "Lister les tâches",
        description: "Les tâches d'un espace, ou de tous : ouvertes (par défaut), faites ou toutes.",
        kind: Kind::Read,
        properties: || {
            json!({
                "space": space_prop("(toutes si absent)"),
                "status": { "type": "string", "enum": ["open", "done", "all"], "description": "open (par défaut), done ou all" },
            })
        },
        required: &[],
    },
    Tool {
        name: "creer_tache",
        title: "Créer une tâche",
        description: "Crée une tâche dans un espace. Sans échéance, elle est pour aujourd'hui.",
        kind: Kind::Write,
        properties: || {
            json!({
                "title": { "type": "string", "maxLength": 200, "description": "Titre de la tâche" },
                "space": space_prop("où la créer"),
                "due_date": date_prop("Jour prévu (aujourd'hui par défaut)"),
            })
        },
        required: &["title", "space"],
    },
    Tool {
        name: "modifier_tache",
        title: "Modifier une tâche",
        description: "Change le titre ou l'échéance d'une tâche, ou la coche et la décoche.",
        kind: Kind::Write,
        properties: || {
            json!({
                "id": id_prop("la tâche"),
                "title": { "type": "string", "maxLength": 200, "description": "Nouveau titre" },
                "done": { "type": "boolean", "description": "true : faite ; false : à faire" },
                "due_date": date_prop("Nouveau jour prévu"),
            })
        },
        required: &["id"],
    },
    Tool {
        name: "supprimer_tache",
        title: "Supprimer une tâche",
        description: "Supprime une tâche, définitivement.",
        kind: Kind::Destructive,
        properties: || json!({ "id": id_prop("la tâche") }),
        required: &["id"],
    },
    Tool {
        name: "chercher_notes",
        title: "Chercher des notes",
        description: "Notes récentes (épinglées d'abord), ou recherche plein texte dans le titre et le texte (accents ignorés). Filtre par espace et par tag.",
        kind: Kind::Read,
        properties: || {
            json!({
                "q": { "type": "string", "description": "Mots à chercher (absent : notes récentes)" },
                "space": space_prop("(tous si absent)"),
                "tag": { "type": "string", "description": "Seulement les notes qui portent ce tag" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 100, "description": "Nombre de notes, 20 par défaut" },
            })
        },
        required: &[],
    },
    Tool {
        name: "lire_note",
        title: "Lire une note",
        description: "Une note complète, avec son texte Markdown et ses tags.",
        kind: Kind::Read,
        properties: || json!({ "id": id_prop("la note") }),
        required: &["id"],
    },
    Tool {
        name: "creer_note",
        title: "Créer une note",
        description: "Crée une note : titre, texte Markdown, espace, tags.",
        kind: Kind::Write,
        properties: || {
            json!({
                "title": { "type": "string", "description": "Titre" },
                "body_md": { "type": "string", "description": "Texte, en Markdown" },
                "space": space_prop("où la créer"),
                "tags": tags_prop("Tags à poser (20 au plus)"),
            })
        },
        required: &["title", "space"],
    },
    Tool {
        name: "modifier_note",
        title: "Modifier une note",
        description: "Change le titre, ajoute du texte à la fin (après une ligne vide), épingle ou désépingle, ajoute des tags. Ne remplace jamais le texte existant.",
        kind: Kind::Write,
        properties: || {
            json!({
                "id": id_prop("la note"),
                "title": { "type": "string", "description": "Nouveau titre" },
                "append": { "type": "string", "description": "Texte Markdown à ajouter à la fin" },
                "pinned": { "type": "boolean", "description": "Épinglée en tête de liste" },
                "add_tags": tags_prop("Tags à ajouter (20 au plus)"),
            })
        },
        required: &["id"],
    },
    Tool {
        name: "remplacer_note",
        title: "Remplacer le texte d'une note",
        description: "Remplace tout le texte d'une note par un nouveau texte Markdown. L'ancien texte est perdu.",
        kind: Kind::Destructive,
        properties: || {
            json!({
                "id": id_prop("la note"),
                "body_md": { "type": "string", "description": "Nouveau texte complet, en Markdown" },
                "title": { "type": "string", "description": "Nouveau titre (facultatif)" },
            })
        },
        required: &["id", "body_md"],
    },
    Tool {
        name: "deplacer_note",
        title: "Déplacer une note",
        description: "Change une note d'espace ; elle arrive à la racine du nouvel espace. Sortie d'un espace partagé, elle n'est plus vue des autres membres.",
        kind: Kind::Destructive,
        properties: || json!({ "id": id_prop("la note"), "space": space_prop("d'arrivée") }),
        required: &["id", "space"],
    },
    Tool {
        name: "lire_agenda",
        title: "Lire l'agenda",
        description: "Les événements entre deux jours (62 jours au plus) : cours des flux iCal et événements ajoutés dans dashu. L'espace Perso réunit les agendas de tous les espaces.",
        kind: Kind::Read,
        properties: || {
            json!({
                "from": date_prop("Premier jour (aujourd'hui par défaut)"),
                "to": date_prop("Dernier jour inclus (from + 6 jours par défaut)"),
                "space": space_prop("(tous si absent)"),
            })
        },
        required: &[],
    },
    Tool {
        name: "rafraichir_agenda",
        title: "Rafraîchir l'agenda",
        description: "Resynchronise tout de suite les flux iCal du compte (Pronote…).",
        kind: Kind::Write,
        properties: || json!({}),
        required: &[],
    },
];

pub fn exists(name: &str) -> bool {
    TOOLS.iter().any(|t| t.name == name)
}

/// Réponse à `tools/list`.
pub fn list() -> Vec<Value> {
    TOOLS
        .iter()
        .map(|tool| {
            let (read_only, destructive) = match tool.kind {
                Kind::Read => (true, false),
                Kind::Write => (false, false),
                Kind::Destructive => (false, true),
            };
            json!({
                "name": tool.name,
                "title": tool.title,
                "description": tool.description,
                "inputSchema": {
                    "type": "object",
                    "properties": (tool.properties)(),
                    "required": tool.required,
                    "additionalProperties": false,
                },
                "annotations": {
                    "title": tool.title,
                    "readOnlyHint": read_only,
                    "destructiveHint": destructive,
                    "idempotentHint": read_only,
                    "openWorldHint": false,
                },
            })
        })
        .collect()
}

/// Résultat d'outil réussi : la réponse de l'API, en JSON.
fn success(value: Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_default() }],
        "isError": false,
    })
}

/// Résultat d'outil en erreur : le message de l'API, que Claude peut lire.
fn failure(message: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": message }],
        "isError": true,
    })
}

/// Arguments lus comme ceux de la route d'API correspondante.
fn args<T: DeserializeOwned>(arguments: Value) -> Result<T, ApiError> {
    serde_json::from_value(arguments)
        .map_err(|error| ApiError::bad_request(format!("arguments invalides : {error}")))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ById {
    id: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskEdit {
    id: i64,
    title: Option<String>,
    done: Option<bool>,
    due_date: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoteEdit {
    id: i64,
    title: Option<String>,
    append: Option<String>,
    pinned: Option<bool>,
    #[serde(default)]
    add_tags: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoteReplace {
    id: i64,
    body_md: String,
    title: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoteMove {
    id: i64,
    space: String,
}

fn to_value<T: serde::Serialize>(value: T) -> Result<Value, ApiError> {
    serde_json::to_value(value).map_err(|e| ApiError::internal(e.into()))
}

/// Appelle un outil. Une erreur devient un résultat en erreur, jamais une
/// erreur du protocole : Claude lit le message et peut corriger.
pub async fn call(state: &AppState, user: &ApiUser, name: &str, arguments: Value) -> Value {
    match run(state, user, name, arguments).await {
        Ok(value) => success(value),
        Err(error) => failure(error.message()),
    }
}

async fn run(
    state: &AppState,
    user: &ApiUser,
    name: &str,
    arguments: Value,
) -> Result<Value, ApiError> {
    match name {
        "lister_espaces" => to_value(api::spaces_out(user)),
        "lister_taches" => to_value(api::tasks_list(state, user, args(arguments)?).await?),
        "creer_tache" => to_value(api::task_create(state, user, args(arguments)?).await?),
        "modifier_tache" => {
            let edit: TaskEdit = args(arguments)?;
            let changes = api::TaskChanges {
                title: edit.title,
                done: edit.done,
                due_date: edit.due_date,
            };
            to_value(api::task_update(state, user, edit.id, changes).await?)
        }
        "supprimer_tache" => {
            let ById { id } = args(arguments)?;
            api::task_delete(state, user, id).await?;
            Ok(json!({ "supprimee": id }))
        }
        "chercher_notes" => to_value(api::notes_list(state, user, args(arguments)?).await?),
        "lire_note" => {
            let ById { id } = args(arguments)?;
            to_value(api::fetch_note(&state.db, user, id).await?)
        }
        "creer_note" => to_value(api::note_create(state, user, args(arguments)?).await?),
        "modifier_note" => {
            let edit: NoteEdit = args(arguments)?;
            let changes = api::NoteChanges {
                title: edit.title,
                append: edit.append,
                pinned: edit.pinned,
                add_tags: edit.add_tags,
                ..Default::default()
            };
            to_value(api::note_update(state, user, edit.id, changes).await?)
        }
        "remplacer_note" => {
            let edit: NoteReplace = args(arguments)?;
            let changes = api::NoteChanges {
                title: edit.title,
                body_md: Some(edit.body_md),
                ..Default::default()
            };
            to_value(api::note_update(state, user, edit.id, changes).await?)
        }
        "deplacer_note" => {
            let edit: NoteMove = args(arguments)?;
            let changes = api::NoteChanges {
                space: Some(edit.space),
                ..Default::default()
            };
            to_value(api::note_update(state, user, edit.id, changes).await?)
        }
        "lire_agenda" => to_value(api::calendar_events(state, user, args(arguments)?).await?),
        "rafraichir_agenda" => to_value(api::calendar_refresh(state, user).await?),
        _ => Err(ApiError::bad_request(format!("outil inconnu « {name} »"))),
    }
}
