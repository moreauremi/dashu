//! 1Life, encart « Identité » d'une fiche client (SPEC, « 1Life — Fiche
//! client et réglages »).
//!
//! Un registre déclare les champs : l'encart, l'enregistrement et les
//! variables des modèles d'audit (`{{client.<clé>}}`) en sont tirés. Ajouter
//! un champ = ajouter une entrée de [`FIELDS`].
//!
//! Les valeurs vivent dans `onelife_client_fields`, une ligne par champ,
//! reliée au dossier du client : elles suivent sa corbeille, sa restauration
//! et son effacement. Une clé inconnue en base (champ retiré) est ignorée.

use std::collections::HashMap;

use axum::extract::rejection::FormRejection;
use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use sqlx::SqlitePool;

use crate::AppState;
use crate::auth::session::Ctx;
use crate::time;
use crate::web::{self, AppResult};

/// Sorte de champ : le type de la saisie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    /// Plusieurs lignes (l'adresse).
    Long,
    Email,
    Phone,
}

/// Un champ de l'encart Identité.
#[derive(Debug, PartialEq, Eq)]
pub struct Field {
    /// Clé en base et nom de la variable : `{{client.<key>}}`.
    pub key: &'static str,
    /// Libellé, lisible seul : il sert aussi dans « [À compléter : …] ».
    pub label: &'static str,
    pub kind: Kind,
    /// Longueur maximale, en caractères.
    pub max: usize,
}

/// La clé de la raison sociale : vide, elle vaut le nom du dossier.
pub const COMPANY: &str = "raison_sociale";

/// Les champs, dans l'ordre de l'encart. En ajouter un = une entrée.
pub const FIELDS: &[Field] = &[
    Field {
        key: COMPANY,
        label: "Raison sociale",
        kind: Kind::Text,
        max: 500,
    },
    Field {
        key: "adresse",
        label: "Adresse",
        kind: Kind::Long,
        max: 1000,
    },
    Field {
        key: "contact.nom",
        label: "Nom du contact",
        kind: Kind::Text,
        max: 500,
    },
    Field {
        key: "contact.fonction",
        label: "Fonction du contact",
        kind: Kind::Text,
        max: 500,
    },
    Field {
        key: "contact.email",
        label: "E-mail du contact",
        kind: Kind::Email,
        max: 500,
    },
    Field {
        key: "contact.telephone",
        label: "Téléphone du contact",
        kind: Kind::Phone,
        max: 500,
    },
    Field {
        key: "secteur",
        label: "Secteur d'activité",
        kind: Kind::Text,
        max: 500,
    },
    Field {
        key: "effectif",
        label: "Effectif",
        kind: Kind::Text,
        max: 500,
    },
    Field {
        key: "version_openprod",
        label: "Version d'Open-Prod",
        kind: Kind::Text,
        max: 500,
    },
];

/// Les valeurs enregistrées d'un client, par clé (champs connus seulement).
pub async fn stored(db: &SqlitePool, folder: i64) -> AppResult<HashMap<String, String>> {
    let rows = sqlx::query!(
        "SELECT key, value FROM onelife_client_fields WHERE folder_id = ?",
        folder
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .filter(|r| FIELDS.iter().any(|f| f.key == r.key))
        .map(|r| (r.key, r.value))
        .collect())
}

/// La valeur d'un champ pour un modèle d'audit : la raison sociale vide vaut
/// le nom du dossier.
pub fn value(values: &HashMap<String, String>, key: &str, folder_name: &str) -> String {
    let value = values.get(key).map(|v| v.trim()).unwrap_or_default();
    if value.is_empty() && key == COMPANY {
        folder_name.to_string()
    } else {
        value.to_string()
    }
}

/// Un champ de l'encart, prêt pour le gabarit.
pub struct Input {
    pub key: &'static str,
    pub label: &'static str,
    /// `text`, `email`, `tel`, ou vide pour une zone de texte.
    pub input_type: &'static str,
    pub autocomplete: &'static str,
    pub value: String,
    pub placeholder: String,
    pub max: usize,
}

/// L'encart Identité (`onelife/_identity.html`).
pub struct Identity {
    /// Adresse du formulaire : `…/{id}/identite`.
    pub action: String,
    pub inputs: Vec<Input>,
}

impl Identity {
    pub fn new(fiche: &str, values: &HashMap<String, String>, folder_name: &str) -> Self {
        let inputs = FIELDS
            .iter()
            .map(|f| Input {
                key: f.key,
                label: f.label,
                input_type: match f.kind {
                    Kind::Text => "text",
                    Kind::Long => "",
                    Kind::Email => "email",
                    Kind::Phone => "tel",
                },
                autocomplete: match f.kind {
                    Kind::Email => "email",
                    Kind::Phone => "tel",
                    _ => "off",
                },
                value: values.get(f.key).cloned().unwrap_or_default(),
                placeholder: if f.key == COMPANY {
                    folder_name.to_string()
                } else {
                    String::new()
                },
                max: f.max,
            })
            .collect();
        Self {
            action: format!("{fiche}/identite"),
            inputs,
        }
    }
}

/// Une saisie : retours à la ligne de Windows ramenés à `\n`, sans espaces
/// autour.
fn clean(raw: &str) -> String {
    raw.replace("\r\n", "\n").trim().to_string()
}

/// `POST /1life/clients/{id}/identite` : enregistre l'encart (htmx, une
/// seconde après la dernière frappe et à la sortie d'un champ). Répond
/// « Enregistré à 14:32 » ; une valeur trop longue est refusée (422) sans
/// rien écrire.
pub async fn save(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    form: Result<Form<HashMap<String, String>>, FormRejection>,
) -> AppResult<Response> {
    let ctx = match super::enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let (_, client) = super::clients::client_of(&state, &ctx, &id).await?;
    let form = form.map(|Form(f)| f).unwrap_or_default();
    let mut values = Vec::new();
    for field in FIELDS {
        let Some(raw) = form.get(field.key) else {
            continue;
        };
        let value = clean(raw);
        if value.chars().count() > field.max {
            let error = format!(
                "{} : {} caractères au plus. Rien n'est enregistré.",
                field.label, field.max
            );
            return Ok(
                (StatusCode::UNPROCESSABLE_ENTITY, Html(web::escape(&error))).into_response(),
            );
        }
        values.push((field.key, value));
    }
    let user = ctx.user.id;
    let now = time::now();
    let mut tx = state.db.begin().await?;
    for (key, value) in values {
        sqlx::query!(
            "INSERT INTO onelife_client_fields (folder_id, key, value, updated_at, updated_by)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (folder_id, key) DO UPDATE SET value = excluded.value,
                                                        updated_at = excluded.updated_at,
                                                        updated_by = excluded.updated_by",
            client.id,
            key,
            value,
            now,
            user
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    // Sans htmx (formulaire envoyé à la main) : retour à la fiche.
    if !web::is_fragment_request(&headers) {
        return Ok(web::see_other(&super::clients::url(
            &ctx,
            &format!("/{}", client.id),
        )));
    }
    Ok(Html(format!("Enregistré à {}", time::hm(now))).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registre_coherent() {
        for (i, field) in FIELDS.iter().enumerate() {
            assert!(FIELDS[..i].iter().all(|f| f.key != field.key));
            // Une clé tient dans une variable : `{{client.<clé>}}`.
            assert!(
                field
                    .key
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b == b'_' || b == b'.')
            );
            assert!(field.max > 0 && !field.label.is_empty());
        }
        assert_eq!(FIELDS[0].key, COMPANY);
    }

    #[test]
    fn raison_sociale_par_defaut() {
        let mut values = HashMap::new();
        assert_eq!(value(&values, COMPANY, "Atelier Test"), "Atelier Test");
        assert_eq!(value(&values, "secteur", "Atelier Test"), "");
        values.insert(COMPANY.to_string(), " Atelier Test SAS ".to_string());
        assert_eq!(value(&values, COMPANY, "Atelier Test"), "Atelier Test SAS");
    }
}
