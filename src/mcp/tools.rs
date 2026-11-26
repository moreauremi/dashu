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

fn color_prop() -> Value {
    let colors: Vec<&str> = crate::spaces::COLORS.iter().map(|(key, _)| *key).collect();
    json!({ "type": "string", "enum": colors, "description": "Couleur de l'espace" })
}

fn icon_prop() -> Value {
    let icons: Vec<&str> = crate::spaces::ICONS.iter().map(|(key, _)| *key).collect();
    json!({ "type": "string", "enum": icons, "description": "Icône de l'espace" })
}

const TOOLS: &[Tool] = &[
    Tool {
        name: "lister_espaces",
        title: "Lister les espaces",
        description: "Les espaces du compte, espaces partagés rejoints compris : identifiant court (slug), nom, couleur, icône, archivé ou non, propriétaire ou membre (mine), règles automatiques.",
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
        name: "deplacer_tache",
        title: "Déplacer une tâche",
        description: "Change une tâche d'espace. Sortie d'un espace partagé, elle n'est plus visible de ses membres.",
        kind: Kind::Destructive,
        properties: || {
            json!({
                "id": id_prop("la tâche"),
                "space": space_prop("où la déplacer"),
            })
        },
        required: &["id", "space"],
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
        description: "L'arborescence : chaque dossier avec son espace et son dossier parent (cinq niveaux de dossiers au plus sous l'espace).",
        kind: Kind::Read,
        properties: || json!({ "space": space_prop("(tous si absent)") }),
        required: &[],
    },
    Tool {
        name: "creer_dossier",
        title: "Créer un dossier",
        description: "Crée un dossier dans un espace, ou un sous-dossier dans un dossier (cinq niveaux de dossiers au plus sous l'espace).",
        kind: Kind::Write,
        properties: || {
            json!({
                "name": { "type": "string", "description": "Nom du dossier" },
                "space": space_prop("où le créer"),
                "parent": { "type": "integer", "description": "Dossier où le créer" },
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
                "parent": folder_prop("où le mettre"),
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
        name: "formats_possibles",
        title: "Formats possibles d'un fichier",
        description: "Ce que le menu « Convertir » propose pour ce fichier : les valeurs de `to` pour convertir (compress pour une copie plus légère).",
        kind: Kind::Read,
        properties: || json!({ "id": id_prop("le fichier") }),
        required: &["id"],
    },
    Tool {
        name: "convertir",
        title: "Convertir un fichier",
        description: "Lance une conversion (ou une compression avec to: compress) ; le résultat arrive à côté de l'original. Suivre avec suivre_conversion.",
        kind: Kind::Write,
        properties: || {
            json!({
                "id": id_prop("le fichier"),
                "to": { "type": "string", "description": "Une valeur de formats_possibles" },
                "pages": { "type": "string", "description": "Pour to: pages, la plage à extraire : « 2-4 » ou « 3 »" },
            })
        },
        required: &["id", "to"],
    },
    Tool {
        name: "lister_conversions",
        title: "Lister les conversions",
        description: "Les conversions du compte, récentes d'abord, ou seulement celles en cours (active: true).",
        kind: Kind::Read,
        properties: || json!({ "active": { "type": "boolean", "description": "Seulement celles en file ou en cours" } }),
        required: &[],
    },
    Tool {
        name: "suivre_conversion",
        title: "Suivre une conversion",
        description: "Où en est une conversion : en file, en cours (progression), terminée (avec le fichier produit) ou en échec (avec l'erreur).",
        kind: Kind::Read,
        properties: || json!({ "job": id_prop("la conversion") }),
        required: &["job"],
    },
    Tool {
        name: "remplacer_original",
        title: "Remplacer l'original par le résultat",
        description: "Le résultat d'une conversion terminée prend la place, le dossier et le nom de l'original ; l'original part dans la corbeille.",
        kind: Kind::Destructive,
        properties: || json!({ "job": id_prop("la conversion terminée") }),
        required: &["job"],
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
        name: "creer_evenement",
        title: "Créer un événement",
        description: "Ajoute un événement à l'agenda de dashu (en heures, à l'heure de Paris, ou sur la journée entière) ; il est copié dans Google Agenda si l'agenda est relié, sauf google: false. Un événement est à son auteur, même dans un espace partagé.",
        kind: Kind::Write,
        properties: || {
            json!({
                "title": { "type": "string", "description": "Titre" },
                "date": date_prop("Le jour"),
                "start": { "type": "string", "description": "Heure de début, HH:MM (sauf all_day)" },
                "end": { "type": "string", "description": "Heure de fin, HH:MM (sauf all_day)" },
                "all_day": { "type": "boolean", "description": "Sur la journée entière" },
                "location": { "type": "string", "description": "Lieu" },
                "space": space_prop("de l'événement"),
                "google": { "type": "boolean", "description": "Aussi dans Google Agenda (vrai par défaut)" },
            })
        },
        required: &["title", "date", "space"],
    },
    Tool {
        name: "modifier_evenement",
        title: "Modifier un événement",
        description: "Change un événement créé dans dashu (son identifiant est le champ event de lire_agenda) ; les champs absents restent. Sa copie dans Google Agenda suit.",
        kind: Kind::Write,
        properties: || {
            json!({
                "id": id_prop("l'événement"),
                "title": { "type": "string" },
                "date": date_prop("Le jour"),
                "start": { "type": "string", "description": "HH:MM" },
                "end": { "type": "string", "description": "HH:MM" },
                "all_day": { "type": "boolean" },
                "location": { "type": "string" },
                "space": space_prop("de l'événement"),
                "google": { "type": "boolean", "description": "Aussi dans Google Agenda" },
            })
        },
        required: &["id"],
    },
    Tool {
        name: "supprimer_evenement",
        title: "Supprimer un événement",
        description: "Supprime pour de bon un événement créé dans dashu, et sa copie dans Google Agenda.",
        kind: Kind::Destructive,
        properties: || json!({ "id": id_prop("l'événement") }),
        required: &["id"],
    },
    Tool {
        name: "lister_sources_agenda",
        title: "Lister les agendas iCal",
        description: "Les agendas iCal du compte (Pronote…), avec leur espace, leur domaine et leur dernière synchronisation. L'adresse complète, un secret, n'est jamais redonnée.",
        kind: Kind::Read,
        properties: || json!({}),
        required: &[],
    },
    Tool {
        name: "ajouter_source_agenda",
        title: "Ajouter un agenda iCal",
        description: "Ajoute un agenda par son lien iCal (HTTPS, adresse publique) à un espace ; sa première synchronisation part aussitôt.",
        kind: Kind::Write,
        properties: || {
            json!({
                "name": { "type": "string", "description": "Nom de l'agenda" },
                "url": { "type": "string", "description": "Lien iCal, https:// ou webcal://" },
                "space": space_prop("de l'agenda"),
            })
        },
        required: &["name", "url", "space"],
    },
    Tool {
        name: "retirer_source_agenda",
        title: "Retirer un agenda iCal",
        description: "Retire un agenda iCal et les événements qu'il avait importés.",
        kind: Kind::Destructive,
        properties: || json!({ "id": id_prop("l'agenda") }),
        required: &["id"],
    },
    Tool {
        name: "rafraichir_agenda",
        title: "Rafraîchir l'agenda",
        description: "Resynchronise tout de suite les flux iCal du compte (Pronote…).",
        kind: Kind::Write,
        properties: || json!({}),
        required: &[],
    },
    Tool {
        name: "creer_espace",
        title: "Créer un espace",
        description: "Crée un espace (8 en service au plus). Sans couleur ni icône, prend les premières libres.",
        kind: Kind::Write,
        properties: || {
            json!({
                "name": { "type": "string", "maxLength": 24, "description": "Nom de l'espace" },
                "color": color_prop(),
                "icon": icon_prop(),
            })
        },
        required: &["name"],
    },
    Tool {
        name: "modifier_espace",
        title: "Modifier un espace",
        description: "Renomme un espace, change sa couleur, son icône ou ses règles automatiques (propriétaire seulement). Le slug ne change pas ; les champs absents restent.",
        kind: Kind::Write,
        properties: || {
            json!({
                "space": space_prop("à modifier"),
                "name": { "type": "string", "maxLength": 24, "description": "Nouveau nom" },
                "color": color_prop(),
                "icon": icon_prop(),
                "auto_mp4": { "type": "boolean", "description": "Règle : une vidéo qui arrive est convertie en MP4" },
                "auto_heic_jpeg": { "type": "boolean", "description": "Règle : une photo HEIC qui arrive est doublée d'un JPEG" },
            })
        },
        required: &["space"],
    },
    Tool {
        name: "archiver_espace",
        title: "Archiver un espace",
        description: "Archive un espace (propriétaire seulement, pas Perso) : il quitte le sélecteur et les listes, pour lui et pour ses membres, et se désarchive à tout moment.",
        kind: Kind::Destructive,
        properties: || json!({ "space": space_prop("à archiver") }),
        required: &["space"],
    },
    Tool {
        name: "desarchiver_espace",
        title: "Désarchiver un espace",
        description: "Remet un espace archivé en service (propriétaire seulement).",
        kind: Kind::Write,
        properties: || json!({ "space": space_prop("à remettre en service") }),
        required: &["space"],
    },
    Tool {
        name: "supprimer_espace",
        title: "Supprimer un espace",
        description: "Met un espace dans la corbeille (propriétaire seulement, pas Perso) : ses membres perdent l'accès. Dire quoi faire du contenu : content « trash » (il part avec l'espace, 30 jours pour le restaurer) ou transfer_to (il passe dans cet autre espace).",
        kind: Kind::Destructive,
        properties: || {
            json!({
                "space": space_prop("à supprimer"),
                "content": { "type": "string", "enum": ["trash"], "description": "trash : le contenu part dans la corbeille avec l'espace" },
                "transfer_to": space_prop("qui reçoit le contenu (à la place de content)"),
            })
        },
        required: &["space"],
    },
    Tool {
        name: "lister_membres",
        title: "Lister les membres",
        description: "Le propriétaire et les membres d'un espace ; pour le propriétaire, les invitations en attente.",
        kind: Kind::Read,
        properties: || json!({ "space": space_prop("partagé") }),
        required: &["space"],
    },
    Tool {
        name: "inviter_membre",
        title: "Inviter dans un espace",
        description: "Invite un compte existant dans un espace, par son adresse e-mail (propriétaire seulement). Une fois l'invitation acceptée, il voit et modifie tout le contenu de l'espace.",
        kind: Kind::Destructive,
        properties: || {
            json!({
                "space": space_prop("à partager"),
                "email": { "type": "string", "description": "Adresse du compte à inviter" },
            })
        },
        required: &["space", "email"],
    },
    Tool {
        name: "annuler_invitation",
        title: "Annuler une invitation",
        description: "Annule une invitation en attente (propriétaire seulement).",
        kind: Kind::Destructive,
        properties: || {
            json!({
                "space": space_prop("de l'invitation"),
                "id": id_prop("l'invitation (voir lister_membres)"),
            })
        },
        required: &["space", "id"],
    },
    Tool {
        name: "retirer_membre",
        title: "Retirer un membre",
        description: "Retire un membre d'un espace (propriétaire seulement) : son accès se coupe, ce qu'il a créé reste.",
        kind: Kind::Destructive,
        properties: || {
            json!({
                "space": space_prop("partagé"),
                "id": id_prop("le compte à retirer (voir lister_membres)"),
            })
        },
        required: &["space", "id"],
    },
    Tool {
        name: "quitter_espace",
        title: "Quitter un espace",
        description: "Quitte un espace partagé rejoint : l'accès se coupe, ce qu'on y a créé reste. Le propriétaire ne quitte pas son espace.",
        kind: Kind::Destructive,
        properties: || json!({ "space": space_prop("à quitter") }),
        required: &["space"],
    },
    Tool {
        name: "lister_invitations",
        title: "Lister les invitations reçues",
        description: "Les invitations reçues, en attente : l'espace et qui invite.",
        kind: Kind::Read,
        properties: || json!({}),
        required: &[],
    },
    Tool {
        name: "accepter_invitation",
        title: "Accepter une invitation",
        description: "Accepte une invitation reçue : l'espace rejoint apparaît dans les espaces du compte.",
        kind: Kind::Write,
        properties: || json!({ "id": id_prop("l'invitation") }),
        required: &["id"],
    },
    Tool {
        name: "refuser_invitation",
        title: "Refuser une invitation",
        description: "Refuse une invitation reçue.",
        kind: Kind::Destructive,
        properties: || json!({ "id": id_prop("l'invitation") }),
        required: &["id"],
    },
    Tool {
        name: "lire_tableau_de_bord",
        title: "Lire un tableau de bord",
        description: "La disposition du tableau de bord d'un espace : ses widgets, leur taille, et les widgets qui existent.",
        kind: Kind::Read,
        properties: || json!({ "space": space_prop("du tableau de bord") }),
        required: &["space"],
    },
    Tool {
        name: "modifier_tableau_de_bord",
        title: "Modifier un tableau de bord",
        description: "Remplace la disposition du tableau de bord d'un espace. Format : « calendar:2x3,tasks:2x2 » (widget:largeurxhauteur, de 1 à 4) ; chaîne vide : aucun widget ; null : la disposition par défaut.",
        kind: Kind::Write,
        properties: || {
            json!({
                "space": space_prop("du tableau de bord"),
                "layout": { "type": ["string", "null"], "description": "La disposition" },
            })
        },
        required: &["space", "layout"],
    },
    Tool {
        name: "lire_outils_espace",
        title: "Lire les outils d'un espace",
        description: "Les outils de la barre latérale dans un espace (notes, fichiers, agenda, outils…), et s'ils y sont affichés. Masquer un outil ne change que l'affichage du site : son contenu reste lisible et modifiable par l'API.",
        kind: Kind::Read,
        properties: || json!({ "space": space_prop("concerné") }),
        required: &["space"],
    },
    Tool {
        name: "modifier_outils_espace",
        title: "Afficher ou masquer des outils",
        description: "Affiche (true) ou masque (false) des outils dans la barre latérale d'un espace, par exemple {\"files\": false}. Les outils absents gardent leur réglage ; le dashboard ne se masque pas. Rien n'est supprimé.",
        kind: Kind::Write,
        properties: || {
            json!({
                "space": space_prop("concerné"),
                "tools": { "type": "object", "additionalProperties": { "type": "boolean" }, "description": "Identifiant de l'outil -> affiché" },
            })
        },
        required: &["space", "tools"],
    },
    Tool {
        name: "lire_reglages",
        title: "Lire les réglages",
        description: "Profil et affichage du compte : prénom, nom, thème, taille de l'interface, barre latérale, raccourcis du téléphone, espace du moment.",
        kind: Kind::Read,
        properties: || json!({}),
        required: &[],
    },
    Tool {
        name: "modifier_reglages",
        title: "Modifier les réglages",
        description: "Change le profil ou l'affichage du compte ; les champs absents restent. Le mot de passe, la connexion Google et les jetons ne se changent que sur le site.",
        kind: Kind::Write,
        properties: || {
            json!({
                "first_name": { "type": "string", "maxLength": 40, "description": "Prénom" },
                "last_name": { "type": "string", "maxLength": 40, "description": "Nom" },
                "theme": { "type": "string", "enum": ["dark", "light", "system"] },
                "ui_size": { "type": "string", "enum": ["small", "normal", "large"] },
                "sidebar_floating": { "type": "boolean", "description": "true : barre latérale détachée ; false : ancrée" },
                "mobile_tabs": { "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": 4, "description": "Raccourcis de la barre du téléphone (voir lire_reglages)" },
                "moment_space": { "type": ["string", "null"], "description": "Slug de l'espace des jours sans cours ; null : le dernier utilisé" },
            })
        },
        required: &[],
    },
    Tool {
        name: "changer_avatar",
        title: "Changer la photo de profil",
        description: "Remplace la photo de profil : une image JPEG, PNG ou WebP de 2 Mo au plus, en base64, recadrée au carré.",
        kind: Kind::Write,
        properties: || json!({ "content_base64": { "type": "string", "description": "L'image, en base64" } }),
        required: &["content_base64"],
    },
    Tool {
        name: "retirer_avatar",
        title: "Retirer la photo de profil",
        description: "Retire la photo de profil : les initiales reviennent.",
        kind: Kind::Destructive,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Convert {
    id: i64,
    to: String,
    pages: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ByJob {
    job: i64,
}

/// Un espace, puis les champs d'une route d'API.
#[derive(Deserialize)]
struct InSpace<T> {
    space: String,
    #[serde(flatten)]
    rest: T,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BySpace {
    space: String,
}

/// Une invitation ou un membre d'un espace.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpaceItem {
    space: String,
    id: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewAvatar {
    content_base64: String,
}

/// Même limite que la route d'API (corps de 2 Mo).
const AVATAR_MAX: usize = 2 * 1024 * 1024;

/// Par MCP, un fichier se lit et s'envoie jusqu'à 10 Mo.
const MCP_FILE_MAX: usize = 10 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoteReplace {
    id: i64,
    body_md: String,
    title: Option<String>,
}

/// Une note ou une tâche, et l'espace où la déplacer.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MoveToSpace {
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
                space: None,
            };
            to_value(api::task_update(state, user, edit.id, changes).await?)
        }
        "deplacer_tache" => {
            let edit: MoveToSpace = args(arguments)?;
            let changes = api::TaskChanges {
                title: None,
                done: None,
                due_date: None,
                space: Some(edit.space),
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
            let edit: MoveToSpace = args(arguments)?;
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
        "formats_possibles" => {
            let ById { id } = args(arguments)?;
            to_value(api::formats(state, user, id).await?)
        }
        "convertir" => {
            let convert: Convert = args(arguments)?;
            to_value(
                api::start(
                    state,
                    user,
                    convert.id,
                    &convert.to,
                    convert.pages.as_deref(),
                )
                .await?,
            )
        }
        "lister_conversions" => to_value(api::jobs(state, user, args(arguments)?).await?),
        "suivre_conversion" => {
            let ByJob { job } = args(arguments)?;
            to_value(api::fetch_job(state, user, job).await?)
        }
        "remplacer_original" => {
            let ByJob { job } = args(arguments)?;
            api::replace_original(state, user, job).await
        }
        "lire_agenda" => to_value(api::calendar_events(state, user, args(arguments)?).await?),
        "rafraichir_agenda" => to_value(api::calendar_refresh(state, user).await?),
        "creer_evenement" => to_value(api::event_create(state, user, args(arguments)?).await?),
        "modifier_evenement" => {
            let edit: WithId<api::EventBody> = args(arguments)?;
            to_value(api::event_update(state, user, edit.id, edit.rest).await?)
        }
        "supprimer_evenement" => {
            let ById { id } = args(arguments)?;
            api::event_delete(state, user, id).await?;
            Ok(json!({ "supprime": id }))
        }
        "lister_sources_agenda" => to_value(api::sources(state, user).await?),
        "ajouter_source_agenda" => {
            to_value(api::source_add(state, user, args::<api::NewSource>(arguments)?).await?)
        }
        "retirer_source_agenda" => {
            let ById { id } = args(arguments)?;
            api::source_remove(state, user, id).await?;
            Ok(json!({ "retire": id }))
        }
        "creer_espace" => {
            to_value(api::space_create(state, user, args::<api::NewSpace>(arguments)?).await?)
        }
        "modifier_espace" => {
            let edit: InSpace<api::SpaceChanges> = args(arguments)?;
            to_value(api::space_update(state, user, &edit.space, edit.rest).await?)
        }
        "archiver_espace" => {
            let BySpace { space } = args(arguments)?;
            to_value(api::space_archive(state, user, &space).await?)
        }
        "desarchiver_espace" => {
            let BySpace { space } = args(arguments)?;
            to_value(api::space_unarchive(state, user, &space).await?)
        }
        "supprimer_espace" => {
            let removal: InSpace<api::SpaceRemoval> = args(arguments)?;
            api::space_delete(state, user, &removal.space, removal.rest).await?;
            Ok(json!({ "dans_la_corbeille": removal.space }))
        }
        "lister_membres" => {
            let BySpace { space } = args(arguments)?;
            to_value(api::members_list(state, user, &space).await?)
        }
        "inviter_membre" => {
            let invite: InSpace<api::NewInvitation> = args(arguments)?;
            to_value(api::member_invite(state, user, &invite.space, &invite.rest.email).await?)
        }
        "annuler_invitation" => {
            let SpaceItem { space, id } = args(arguments)?;
            api::invitation_cancel(state, user, &space, id).await?;
            Ok(json!({ "annulee": id }))
        }
        "retirer_membre" => {
            let SpaceItem { space, id } = args(arguments)?;
            api::member_remove(state, user, &space, id).await?;
            Ok(json!({ "retire": id }))
        }
        "quitter_espace" => {
            let BySpace { space } = args(arguments)?;
            api::space_leave(state, user, &space).await?;
            Ok(json!({ "quitte": space }))
        }
        "lister_invitations" => to_value(api::invitations_list(state, user).await?),
        "accepter_invitation" => {
            let ById { id } = args(arguments)?;
            to_value(api::invitation_accept(state, user, id).await?)
        }
        "refuser_invitation" => {
            let ById { id } = args(arguments)?;
            crate::spaces::decline_for(&state.db, user.id, id).await?;
            Ok(json!({ "refusee": id }))
        }
        "lire_tableau_de_bord" => {
            let BySpace { space } = args(arguments)?;
            to_value(api::dashboard_read(state, user, &space).await?)
        }
        "modifier_tableau_de_bord" => {
            let edit: InSpace<api::DashboardBody> = args(arguments)?;
            to_value(api::dashboard_write(state, user, &edit.space, edit.rest).await?)
        }
        "lire_outils_espace" => {
            let BySpace { space } = args(arguments)?;
            to_value(api::tools_read(state, user, &space).await?)
        }
        "modifier_outils_espace" => {
            let edit: InSpace<api::ToolsBody> = args(arguments)?;
            to_value(api::tools_write(state, user, &edit.space, edit.rest).await?)
        }
        "lire_reglages" => to_value(api::settings_read(state, user).await?),
        "modifier_reglages" => to_value(api::settings_update(state, user, args(arguments)?).await?),
        "changer_avatar" => {
            let NewAvatar { content_base64 } = args(arguments)?;
            if content_base64.len() > AVATAR_MAX / 3 * 4 + 4 {
                return Err(ApiError::bad_request("image de plus de 2 Mo"));
            }
            let bytes = BASE64
                .decode(content_base64.trim())
                .map_err(|_| ApiError::bad_request("content_base64 n'est pas du base64 valide"))?;
            crate::settings::profile::store_avatar(&state.db, user.id, bytes.into()).await?;
            Ok(json!({ "avatar": true }))
        }
        "retirer_avatar" => {
            crate::settings::profile::remove_avatar(&state.db, user.id).await?;
            Ok(json!({ "avatar": false }))
        }
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
