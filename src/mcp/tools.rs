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

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

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

fn folder_prop(what: &str) -> Value {
    json!({ "type": ["integer", "null"], "description": format!("Dossier {what} ; null : la racine") })
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
                "remove_tags": tags_prop("Tags à retirer"),
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
        name: "ranger_note",
        title: "Ranger une note",
        description: "Range une note dans un dossier de son espace, ou à sa racine (folder: null).",
        kind: Kind::Write,
        properties: || json!({ "id": id_prop("la note"), "folder": folder_prop("de son espace") }),
        required: &["id", "folder"],
    },
    Tool {
        name: "supprimer_note",
        title: "Supprimer une note",
        description: "Met une note dans la corbeille : elle disparaît de partout, et se restaure pendant 30 jours.",
        kind: Kind::Destructive,
        properties: || json!({ "id": id_prop("la note") }),
        required: &["id"],
    },
    Tool {
        name: "lister_fichiers",
        title: "Lister des fichiers",
        description: "Les fichiers rangés, récents d'abord, ou ceux de Temporaire (temporary: true). Filtre par espace, dossier, tag, et recherche sur le nom.",
        kind: Kind::Read,
        properties: || {
            json!({
                "space": space_prop("(tous si absent)"),
                "folder": { "type": "integer", "description": "Seulement ce dossier" },
                "tag": { "type": "string", "description": "Seulement les fichiers qui portent ce tag" },
                "q": { "type": "string", "description": "Mots du nom (accents ignorés)" },
                "temporary": { "type": "boolean", "description": "Les fichiers de Temporaire (onglet Outils)" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200, "description": "50 par défaut" },
            })
        },
        required: &[],
    },
    Tool {
        name: "lire_fichier",
        title: "Lire un fichier",
        description: "Les informations d'un fichier, et son contenu en base64 si contenu: true (10 Mo au plus ; au-delà, passer par l'API ou le site).",
        kind: Kind::Read,
        properties: || {
            json!({
                "id": id_prop("le fichier"),
                "contenu": { "type": "boolean", "description": "Joindre le contenu, en base64" },
            })
        },
        required: &["id"],
    },
    Tool {
        name: "envoyer_fichier",
        title: "Envoyer un fichier",
        description: "Envoie un fichier de 10 Mo au plus, en base64, dans un espace ou un dossier (ou dans Temporaire).",
        kind: Kind::Write,
        properties: || {
            json!({
                "name": { "type": "string", "description": "Nom du fichier, avec son extension" },
                "content_base64": { "type": "string", "description": "Contenu, en base64" },
                "space": space_prop("d'arrivée"),
                "folder": { "type": "integer", "description": "Dossier d'arrivée (il donne l'espace)" },
                "temporary": { "type": "boolean", "description": "Dans Temporaire, effacé au bout de 24 heures" },
            })
        },
        required: &["name", "content_base64"],
    },
    Tool {
        name: "modifier_fichier",
        title: "Modifier un fichier",
        description: "Renomme (l'extension ne change pas), range dans un espace ou un dossier, garde un fichier de Temporaire (keep: true), pose ou retire des tags.",
        kind: Kind::Write,
        properties: || {
            json!({
                "id": id_prop("le fichier"),
                "name": { "type": "string", "description": "Nouveau nom" },
                "space": space_prop("où le ranger"),
                "folder": folder_prop("de l'espace"),
                "keep": { "type": "boolean", "description": "Garde un fichier de Temporaire dans space" },
                "add_tags": tags_prop("Tags à poser"),
                "remove_tags": tags_prop("Tags à retirer"),
            })
        },
        required: &["id"],
    },
    Tool {
        name: "supprimer_fichier",
        title: "Supprimer un fichier",
        description: "Met un fichier dans la corbeille (30 jours pour le restaurer) ; ses liens de partage sont révoqués.",
        kind: Kind::Destructive,
        properties: || json!({ "id": id_prop("le fichier") }),
        required: &["id"],
    },
    Tool {
        name: "lister_liens",
        title: "Lister les liens de partage",
        description: "Les liens de partage encore valables d'un fichier.",
        kind: Kind::Read,
        properties: || json!({ "id": id_prop("le fichier") }),
        required: &["id"],
    },
    Tool {
        name: "creer_lien",
        title: "Créer un lien de partage",
        description: "Rend un fichier public par un lien temporaire (1h, 1j ou 7j), révocable. L'adresse n'est donnée qu'une fois.",
        kind: Kind::Destructive,
        properties: || {
            json!({
                "id": id_prop("le fichier"),
                "duration": { "type": "string", "enum": ["1h", "1j", "7j"], "description": "Durée du lien" },
            })
        },
        required: &["id", "duration"],
    },
    Tool {
        name: "revoquer_lien",
        title: "Révoquer un lien de partage",
        description: "Le lien ne marche plus, tout de suite.",
        kind: Kind::Write,
        properties: || json!({ "id": id_prop("le fichier"), "link": id_prop("le lien") }),
        required: &["id", "link"],
    },
    Tool {
        name: "lister_dossiers",
        title: "Lister les dossiers",
        description: "L'arborescence : chaque dossier avec son espace et son dossier parent (trois niveaux au plus : l'espace, un dossier, un sous-dossier).",
        kind: Kind::Read,
        properties: || json!({ "space": space_prop("(tous si absent)") }),
        required: &[],
    },
    Tool {
        name: "creer_dossier",
        title: "Créer un dossier",
        description: "Crée un dossier dans un espace, ou un sous-dossier dans un dossier du premier niveau.",
        kind: Kind::Write,
        properties: || {
            json!({
                "name": { "type": "string", "description": "Nom du dossier" },
                "space": space_prop("où le créer"),
                "parent": { "type": "integer", "description": "Dossier du premier niveau où le créer" },
            })
        },
        required: &["name"],
    },
    Tool {
        name: "modifier_dossier",
        title: "Modifier un dossier",
        description: "Renomme un dossier, ou le déplace avec tout son contenu dans un autre espace ou dossier.",
        kind: Kind::Write,
        properties: || {
            json!({
                "id": id_prop("le dossier"),
                "name": { "type": "string", "description": "Nouveau nom" },
                "space": space_prop("où l'emmener"),
                "parent": folder_prop("du premier niveau"),
            })
        },
        required: &["id"],
    },
    Tool {
        name: "supprimer_dossier",
        title: "Supprimer un dossier",
        description: "Met un dossier dans la corbeille avec tout ce qu'il contient (notes, fichiers, sous-dossiers) ; 30 jours pour le restaurer.",
        kind: Kind::Destructive,
        properties: || json!({ "id": id_prop("le dossier") }),
        required: &["id"],
    },
    Tool {
        name: "lister_corbeille",
        title: "Lister la corbeille",
        description: "Ce qui a été supprimé : quoi, d'où, par qui, par où, quand.",
        kind: Kind::Read,
        properties: || json!({}),
        required: &[],
    },
    Tool {
        name: "restaurer",
        title: "Restaurer depuis la corbeille",
        description: "Remet un élément de la corbeille à sa place (à la racine de son espace si son dossier n'existe plus).",
        kind: Kind::Write,
        properties: || {
            json!({
                "kind": { "type": "string", "enum": ["note", "fichier", "dossier", "espace"] },
                "id": id_prop("l'élément"),
            })
        },
        required: &["kind", "id"],
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
    #[serde(default)]
    remove_tags: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoteFiling {
    id: i64,
    folder: Option<i64>,
}

/// Un identifiant, puis les champs d'une route d'API.
#[derive(Deserialize)]
struct WithId<T> {
    id: i64,
    #[serde(flatten)]
    rest: T,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileRead {
    id: i64,
    #[serde(default)]
    contenu: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileUpload {
    name: String,
    content_base64: String,
    space: Option<String>,
    folder: Option<i64>,
    #[serde(default)]
    temporary: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkNew {
    id: i64,
    duration: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkRevoke {
    id: i64,
    link: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Restore {
    kind: String,
    id: i64,
}

/// Par MCP, un fichier se lit et s'envoie jusqu'à 10 Mo.
const MCP_FILE_MAX: usize = 10 * 1024 * 1024;

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
                remove_tags: edit.remove_tags,
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
        "ranger_note" => {
            let filing: NoteFiling = args(arguments)?;
            let changes = api::NoteChanges {
                folder: Some(filing.folder),
                ..Default::default()
            };
            to_value(api::note_update(state, user, filing.id, changes).await?)
        }
        "supprimer_note" => {
            let ById { id } = args(arguments)?;
            api::note_delete(state, user, id).await?;
            Ok(json!({ "dans_la_corbeille": id }))
        }
        "lister_fichiers" => to_value(api::files_list(state, user, args(arguments)?).await?),
        "lire_fichier" => read_file(state, user, args(arguments)?).await,
        "envoyer_fichier" => send_file(state, user, args(arguments)?).await,
        "modifier_fichier" => {
            let edit: WithId<api::FileChanges> = args(arguments)?;
            to_value(api::file_update(state, user, edit.id, edit.rest).await?)
        }
        "supprimer_fichier" => {
            let ById { id } = args(arguments)?;
            api::file_delete(state, user, id).await?;
            Ok(json!({ "dans_la_corbeille": id }))
        }
        "lister_liens" => {
            let ById { id } = args(arguments)?;
            to_value(api::links_list(&state.db, user, id).await?)
        }
        "creer_lien" => {
            let link: LinkNew = args(arguments)?;
            to_value(api::link_create(state, user, link.id, &link.duration).await?)
        }
        "revoquer_lien" => {
            let link: LinkRevoke = args(arguments)?;
            crate::files::share::revoke_link(&state.db, user.id, link.id, link.link).await?;
            Ok(json!({ "revoque": link.link }))
        }
        "lister_dossiers" => to_value(api::folders_list(&state.db, user, args(arguments)?).await?),
        "creer_dossier" => to_value(api::folder_create(&state.db, user, args(arguments)?).await?),
        "modifier_dossier" => {
            let edit: WithId<api::FolderChanges> = args(arguments)?;
            to_value(api::folder_update(&state.db, user, edit.id, edit.rest).await?)
        }
        "supprimer_dossier" => {
            let ById { id } = args(arguments)?;
            api::folder_delete(&state.db, user, id).await?;
            Ok(json!({ "dans_la_corbeille": id }))
        }
        "lister_corbeille" => to_value(api::trash_list(&state.db, user).await?),
        "restaurer" => {
            let restore: Restore = args(arguments)?;
            api::trash_restore(&state.db, user, &restore.kind, restore.id).await
        }
        "lire_agenda" => to_value(api::calendar_events(state, user, args(arguments)?).await?),
        "rafraichir_agenda" => to_value(api::calendar_refresh(state, user).await?),
        _ => Err(ApiError::bad_request(format!("outil inconnu « {name} »"))),
    }
}

/// `lire_fichier` : les informations, et le contenu en base64 sur demande.
async fn read_file(state: &AppState, user: &ApiUser, read: FileRead) -> Result<Value, ApiError> {
    let info = api::fetch_file(&state.db, user, read.id).await?;
    let mut value = to_value(info)?;
    if read.contenu {
        let config = state
            .config
            .files
            .as_ref()
            .ok_or_else(|| ApiError::bad_request("fichiers désactivés sur ce serveur"))?;
        let (_, size, key) = api::stored(&state.db, user, read.id).await?;
        if usize::try_from(size).unwrap_or(usize::MAX) > MCP_FILE_MAX {
            return Err(ApiError::bad_request(
                "fichier de plus de 10 Mo : télécharge-le par l'API (GET /api/v1/files/{id}/download) ou sur le site",
            ));
        }
        let bytes = tokio::fs::read(crate::files::stored_path(&config.dir, &key))
            .await
            .map_err(|e| ApiError::internal(e.into()))?;
        value["content_base64"] = Value::String(BASE64.encode(bytes));
    }
    Ok(value)
}

/// `envoyer_fichier` : le même envoi que le site, en un seul morceau.
async fn send_file(
    state: &AppState,
    user: &ApiUser,
    upload: FileUpload,
) -> Result<Value, ApiError> {
    if upload.content_base64.len() > MCP_FILE_MAX / 3 * 4 + 4 {
        return Err(ApiError::bad_request(
            "fichier de plus de 10 Mo : envoie-le par l'API (POST /api/v1/files/uploads) ou sur le site",
        ));
    }
    let bytes = BASE64
        .decode(upload.content_base64.trim())
        .map_err(|_| ApiError::bad_request("content_base64 n'est pas du base64 valide"))?;
    let size = bytes.len() as u64;
    let place = api::upload_place(
        &state.db,
        user,
        &api::NewUpload {
            name: upload.name.clone(),
            size,
            space: upload.space,
            folder: upload.folder,
            temporary: upload.temporary,
        },
    )
    .await?;
    let id = crate::files::open_upload(state, user.id, place, &upload.name, size, upload.temporary)
        .await?;
    match crate::files::receive(state, user.id, &id, 0, axum::body::Body::from(bytes)).await? {
        crate::files::Received::Done(file) => {
            to_value(api::fetch_file(&state.db, user, file).await?)
        }
        _ => Err(ApiError::bad_request("envoi incomplet")),
    }
}
