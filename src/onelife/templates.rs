//! 1Life, modèles d'audit (SPEC, « 1Life — Audit », « Modèles d'audit »).
//!
//! Un modèle est une note ou un fichier `.md` du dossier des modèles choisi
//! dans les réglages (son contenu direct, pas ses sous-dossiers). Le contenu
//! de 1Life reste dans la base : le seul modèle du dépôt est le modèle
//! générique [`BUILTIN`], qui sert tant qu'aucun dossier n'est choisi.
//!
//! Le même dossier peut contenir un `.docx` : le plus récent sert de document
//! de référence à l'export Word des notes d'audit (SPEC, « 1Life — Audit en
//! Word »).

use crate::AppState;
use crate::auth::session::Ctx;
use crate::folders;
use crate::web::AppResult;

use super::settings::Settings;

/// Identifiant du modèle intégré.
pub const BUILTIN_KEY: &str = "integre";

/// Le modèle générique, intégré au code : rien de propre à 1Life.
pub const BUILTIN: &str = "# {{audit.titre}}

- **Client :** {{client.raison_sociale}}
- **Date de l'audit :** {{audit.date}}
- **Consultant :** {{consultant.nom}}, {{consultant.fonction}}
- **Approbateur :** {{audit.approbateur}}

## Contexte

> Consigne : présenter le client (activité, secteur, effectif) et la raison de l'audit, en un ou deux paragraphes.

## Périmètre

> Consigne : lister les domaines et les services couverts par l'audit, puis ce qui en est exclu s'il y a lieu.

## Constats

> Consigne : un titre de niveau 3 par domaine. Dans chaque domaine, les constats en liste, les points bloquants en premier.

## Recommandations

> Consigne : une recommandation par constat important, avec sa priorité (haute, moyenne ou basse), dans un tableau « Recommandation | Priorité | Domaine ».

## Prochaines étapes

> Consigne : les actions à mener, en cases à cocher, avec le responsable quand il est connu.
";

/// Taille maximale d'un modèle : 200 Ko.
pub const MAX_BYTES: i64 = 200 * 1024;

/// Taille maximale du document de référence Word : 10 Mo.
pub const REFERENCE_MAX_BYTES: i64 = 10 * 1024 * 1024;

/// Un modèle proposé dans une liste.
pub struct Choice {
    /// `integre`, `note:<id>` ou `fichier:<id>`.
    pub key: String,
    pub label: String,
}

/// Le dossier des modèles du compte, s'il est choisi et accessible : son
/// identifiant et son chemin (« Taff / Modèles »).
pub async fn folder(
    state: &AppState,
    ctx: &Ctx,
    settings: &Settings,
) -> AppResult<Option<(i64, i64, String)>> {
    let Some(id) = settings.templates_folder_id else {
        return Ok(None);
    };
    let all = folders::all(&state.db, ctx.user.id).await?;
    let Some(folder) = all.iter().find(|f| f.id == id) else {
        return Ok(None);
    };
    let Some(space) = ctx.space_by_id(folder.space_id) else {
        return Ok(None);
    };
    Ok(Some((
        folder.id,
        folder.space_id,
        folders::label(&space.name, &all, Some(folder.id)),
    )))
}

/// Les modèles proposés : le modèle intégré, puis les notes et les fichiers
/// `.md` du dossier des modèles, par nom.
pub async fn choices(state: &AppState, ctx: &Ctx, settings: &Settings) -> AppResult<Vec<Choice>> {
    let mut out = vec![Choice {
        key: BUILTIN_KEY.into(),
        label: "Modèle intégré".into(),
    }];
    let Some((folder, _, _)) = folder(state, ctx, settings).await? else {
        return Ok(out);
    };
    let user = ctx.user.id;
    let notes = sqlx::query!(
        r#"SELECT id AS "id!", title FROM notes
           WHERE folder_id = ?2 AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?1)
           ORDER BY title COLLATE NOCASE, id"#,
        user,
        folder
    )
    .fetch_all(&state.db)
    .await?;
    for note in notes {
        out.push(Choice {
            key: format!("note:{}", note.id),
            label: crate::notes::display_title(&note.title),
        });
    }
    let files = sqlx::query!(
        r#"SELECT id AS "id!", name FROM files
           WHERE folder_id = ?2 AND expires_at IS NULL AND lower(name) LIKE '%.md'
             AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?1)
           ORDER BY name COLLATE NOCASE, id"#,
        user,
        folder
    )
    .fetch_all(&state.db)
    .await?;
    for file in files {
        out.push(Choice {
            key: format!("fichier:{}", file.id),
            label: file.name,
        });
    }
    Ok(out)
}

/// Un modèle lu, prêt à remplir.
pub struct Loaded {
    pub label: String,
    pub body: String,
    /// Le modèle demandé n'existe plus (ou est illisible) : le modèle intégré
    /// sert à sa place, et la page de l'audit le dit.
    pub replaced: bool,
}

fn builtin(replaced: bool) -> Loaded {
    Loaded {
        label: "Modèle intégré".into(),
        body: BUILTIN.into(),
        replaced,
    }
}

/// Lit le modèle `key` (`integre`, `note:<id>`, `fichier:<id>`), s'il est
/// accessible au compte ; sinon, le modèle intégré.
pub async fn load(state: &AppState, ctx: &Ctx, key: &str) -> AppResult<Loaded> {
    let user = ctx.user.id;
    if let Some(id) = key
        .strip_prefix("note:")
        .and_then(|n| n.parse::<i64>().ok())
    {
        let note = sqlx::query!(
            "SELECT title, body_md FROM notes WHERE id = ?2
               AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?1)",
            user,
            id
        )
        .fetch_optional(&state.db)
        .await?;
        return Ok(match note {
            Some(note) if note.body_md.len() as i64 <= MAX_BYTES => Loaded {
                label: crate::notes::display_title(&note.title),
                body: note.body_md,
                replaced: false,
            },
            _ => builtin(true),
        });
    }
    if let Some(id) = key
        .strip_prefix("fichier:")
        .and_then(|n| n.parse::<i64>().ok())
    {
        let Some(files) = state.config.files.as_ref() else {
            return Ok(builtin(true));
        };
        let file = sqlx::query!(
            "SELECT name, size, storage_key FROM files WHERE id = ?2 AND expires_at IS NULL
               AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?1)",
            user,
            id
        )
        .fetch_optional(&state.db)
        .await?;
        let Some(file) = file.filter(|f| f.size <= MAX_BYTES) else {
            return Ok(builtin(true));
        };
        let path = crate::files::stored_path(&files.dir, &file.storage_key);
        return Ok(match tokio::fs::read(&path).await {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                Loaded {
                    label: file.name,
                    body: text.trim_start_matches('\u{feff}').replace("\r\n", "\n"),
                    replaced: false,
                }
            }
            Err(error) => {
                tracing::warn!(%error, file = id, "modèle d'audit illisible sur le disque");
                builtin(true)
            }
        });
    }
    Ok(builtin(false))
}

/// Le document de référence Word du compte : le `.docx` le plus récent du
/// dossier des modèles, s'il est lisible, de 10 Mo au plus, et un vrai
/// fichier ZIP. `None` sinon : l'export se fait avec le Word de pandoc.
pub async fn reference_docx(
    state: &AppState,
    ctx: &Ctx,
    settings: &Settings,
) -> AppResult<Option<Vec<u8>>> {
    let Some(files) = state.config.files.as_ref() else {
        return Ok(None);
    };
    let Some((folder, _, _)) = folder(state, ctx, settings).await? else {
        return Ok(None);
    };
    let user = ctx.user.id;
    let file = sqlx::query!(
        r#"SELECT id AS "id!", size, storage_key FROM files
           WHERE folder_id = ?2 AND expires_at IS NULL AND lower(name) LIKE '%.docx'
             AND space_id IN (SELECT space_id FROM space_access WHERE user_id = ?1)
           ORDER BY updated_at DESC, id DESC LIMIT 1"#,
        user,
        folder
    )
    .fetch_optional(&state.db)
    .await?;
    let Some(file) = file.filter(|f| f.size <= REFERENCE_MAX_BYTES) else {
        return Ok(None);
    };
    let path = crate::files::stored_path(&files.dir, &file.storage_key);
    match tokio::fs::read(&path).await {
        Ok(bytes) if bytes.starts_with(b"PK\x03\x04") => Ok(Some(bytes)),
        Ok(_) => {
            tracing::warn!(
                file = file.id,
                "document de référence Word : pas un fichier ZIP"
            );
            Ok(None)
        }
        Err(error) => {
            tracing::warn!(%error, file = file.id, "document de référence Word illisible");
            Ok(None)
        }
    }
}
