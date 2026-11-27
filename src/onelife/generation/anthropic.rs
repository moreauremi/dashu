//! L'appel à l'API d'Anthropic (Messages API), par reqwest : il n'existe pas
//! de SDK officiel en Rust. Une seule requête, sans flux ; du texte
//! seulement, aucun outil pour l'IA.
//!
//! La clé part dans l'en-tête `x-api-key` et nulle part ailleurs : elle
//! n'apparaît ni dans les logs ni dans les messages. Les messages d'erreur de
//! l'API vont dans les logs du serveur, pas dans la page.

use serde::Deserialize;
use serde_json::json;

use crate::config::AiConfig;

use super::MAX_TOKENS;

/// Version de l'API envoyée dans l'en-tête `anthropic-version`.
pub const API_VERSION: &str = "2026-10-21";

/// Ce qui part chez Anthropic : le modèle d'IA, la consigne système et un
/// seul message.
pub struct Request {
    pub model: String,
    pub system: String,
    pub user: String,
}

/// Une réponse complète.
#[derive(Debug)]
pub struct Reply {
    pub text: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// Pourquoi rien n'a été créé. Chaque cas a son message, lisible.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    NoKey,
    Timeout,
    Unreachable,
    /// 401 : clé refusée.
    Auth,
    /// 402, ou 400 « crédit épuisé » : facturation.
    Billing,
    /// 403 : la clé n'a pas accès.
    Permission,
    /// 404 : modèle inconnu de l'API.
    UnknownModel(String),
    /// 413 : demande trop grosse.
    TooLarge,
    /// 429 : trop de demandes, ou plafond de dépenses.
    RateLimited,
    /// 500, 529… : l'API est en panne ou surchargée.
    Overloaded,
    /// 400 et le reste : demande refusée.
    Rejected(u16),
    /// `stop_reason: max_tokens` : la réponse a été coupée. Les tokens sont
    /// facturés quand même.
    Cut {
        input: i64,
        output: i64,
    },
    /// `stop_reason: refusal`.
    Refused,
    /// Réponse vide ou illisible.
    Unreadable,
}

impl Failure {
    pub fn message(&self, timeout_secs: u64) -> String {
        let nothing = "Rien n'est créé.";
        match self {
            Self::NoKey => {
                "IA non configurée : ANTHROPIC_API_KEY manque sur le serveur. Rien n'est envoyé."
                    .into()
            }
            Self::Timeout => {
                let minutes = timeout_secs.div_ceil(60);
                format!(
                    "Pas de réponse de l'IA en {}. {nothing} Réessaie, ou raccourcis les notes.",
                    if timeout_secs < 60 {
                        format!("{timeout_secs} secondes")
                    } else if minutes == 1 {
                        "1 minute".into()
                    } else {
                        format!("{minutes} minutes")
                    }
                )
            }
            Self::Unreachable => {
                format!("L'API d'Anthropic est injoignable. {nothing} Réessaie dans un moment.")
            }
            Self::Auth => format!(
                "Clé refusée par Anthropic : vérifie ANTHROPIC_API_KEY sur le serveur (README). {nothing}"
            ),
            Self::Billing => format!(
                "Le compte Anthropic n'a plus de crédit, ou sa facturation est à régler (console d'Anthropic). {nothing}"
            ),
            Self::Permission => {
                format!("Cette clé n'a pas accès à ce modèle d'IA. {nothing}")
            }
            Self::UnknownModel(model) => format!(
                "Modèle d'IA inconnu de l'API (« {model} ») : vérifie ONELIFE_AI_MODELS. {nothing}"
            ),
            Self::TooLarge => format!(
                "Demande trop grosse pour l'API : raccourcis les notes ou le modèle. {nothing}"
            ),
            Self::RateLimited => format!(
                "Quota de l'API atteint (trop de demandes, ou plafond de dépenses du compte) : réessaie plus tard. {nothing}"
            ),
            Self::Overloaded => format!(
                "L'API d'Anthropic est surchargée ou en panne : réessaie dans quelques minutes. {nothing}"
            ),
            Self::Rejected(status) => format!(
                "Demande refusée par l'API (erreur {status}) : {nothing} Le détail est dans les logs du serveur."
            ),
            Self::Cut { .. } => format!(
                "La réponse a été coupée : elle dépassait {} tokens. {nothing} Raccourcis le modèle ou les notes, ou essaie un autre modèle d'IA.",
                super::thousands(i64::from(MAX_TOKENS))
            ),
            Self::Refused => format!("Le modèle d'IA a refusé de répondre. {nothing}"),
            Self::Unreadable => format!("Réponse de l'IA illisible. {nothing}"),
        }
    }
}

#[derive(Deserialize)]
struct Body {
    #[serde(default)]
    content: Vec<Block>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Usage,
}

#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}

#[derive(Deserialize, Default)]
struct Usage {
    #[serde(default)]
    input_tokens: i64,
    #[serde(default)]
    output_tokens: i64,
}

#[derive(Deserialize, Default)]
struct ErrorBody {
    #[serde(default)]
    error: ErrorDetail,
}

#[derive(Deserialize, Default)]
struct ErrorDetail {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    message: String,
}

/// Envoie la demande et attend la réponse complète, dans le délai de la
/// configuration.
pub async fn call(
    http: &reqwest::Client,
    ai: &AiConfig,
    request: &Request,
) -> Result<Reply, Failure> {
    let Some(key) = ai.key.as_deref() else {
        return Err(Failure::NoKey);
    };
    let body = json!({
        "model": request.model,
        "max_tokens": MAX_TOKENS,
        "system": request.system,
        "messages": [{ "role": "user", "content": request.user }],
        // Rédiger d'après un modèle demande peu de réflexion, et la
        // réflexion se paie en tokens de sortie, sur les 16 000.
        "output_config": { "effort": "low" },
    });
    let sent = http
        .post(format!("{}/v1/messages", ai.base_url))
        .header("x-api-key", key)
        .header("anthropic-version", API_VERSION)
        .json(&body)
        .timeout(ai.timeout)
        .send()
        .await;
    let response = match sent {
        Ok(response) => response,
        Err(error) if error.is_timeout() => return Err(Failure::Timeout),
        Err(error) => {
            tracing::warn!(error = %error.without_url(), "1life : API d'Anthropic injoignable");
            return Err(Failure::Unreachable);
        }
    };
    let status = response.status();
    let text = match response.text().await {
        Ok(text) => text,
        Err(error) if error.is_timeout() => return Err(Failure::Timeout),
        Err(error) => {
            tracing::warn!(error = %error.without_url(), "1life : réponse de l'API coupée");
            return Err(Failure::Unreachable);
        }
    };
    if !status.is_success() {
        let detail: ErrorBody = serde_json::from_str(&text).unwrap_or_default();
        tracing::warn!(
            status = status.as_u16(),
            kind = %detail.error.kind,
            message = %detail.error.message,
            "1life : l'API d'Anthropic refuse la demande"
        );
        return Err(match status.as_u16() {
            401 => Failure::Auth,
            402 => Failure::Billing,
            400 if detail.error.message.to_lowercase().contains("credit") => Failure::Billing,
            403 => Failure::Permission,
            404 => Failure::UnknownModel(request.model.clone()),
            413 => Failure::TooLarge,
            429 => Failure::RateLimited,
            500..=599 => Failure::Overloaded,
            other => Failure::Rejected(other),
        });
    }
    let Ok(body) = serde_json::from_str::<Body>(&text) else {
        tracing::warn!("1life : réponse de l'API illisible");
        return Err(Failure::Unreadable);
    };
    let (input, output) = (body.usage.input_tokens, body.usage.output_tokens);
    match body.stop_reason.as_deref() {
        Some("max_tokens") => return Err(Failure::Cut { input, output }),
        Some("refusal") => return Err(Failure::Refused),
        _ => {}
    }
    // Seulement le texte : les blocs de réflexion (vides) sont ignorés.
    let reply: String = body
        .content
        .iter()
        .filter(|b| b.kind == "text")
        .map(|b| b.text.as_str())
        .collect();
    if reply.trim().is_empty() {
        return Err(Failure::Unreadable);
    }
    Ok(Reply {
        text: reply,
        input_tokens: input,
        output_tokens: output,
    })
}
