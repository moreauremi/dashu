//! 1Life, moteur de génération (SPEC, « 1Life — Audit », « La génération ») :
//! un modèle rempli par le serveur, du texte, l'API d'Anthropic, et le
//! retour découpé. Partagé par les modules : Audit s'en sert, Compte rendu
//! s'en servira.
//!
//! * Les variables `{{…}}` sont remplacées ici, avant l'envoi, jamais par
//!   l'IA ([`fill`]).
//! * Les consignes (`> Consigne : …`) guident l'IA, puis sont retirées du
//!   résultat, même si l'IA les a recopiées ([`strip_consignes`]).
//! * La réponse se coupe à une ligne séparatrice fixe : avant, le document ;
//!   après, la liste des points non repris ([`split_points`]).
//! * L'appel lui-même est dans [`anthropic`] : du texte seulement, aucun
//!   outil pour l'IA.

pub mod anthropic;

use crate::config::{AiPrice, ai_price};

/// Tokens de sortie au plus, par génération.
pub const MAX_TOKENS: u32 = 16_000;
/// Tokens d'entrée estimés au plus : sous le seuil de 100 000 où Haiku 5.5
/// coûte cinq fois plus. Au-delà, rien n'est envoyé.
pub const MAX_INPUT_TOKENS: u64 = 80_000;
/// Générations par compte et par jour (heure de Paris), au plus.
pub const DAILY_LIMIT: i64 = 20;
/// La ligne qui sépare le document des points non repris.
pub const SEPARATOR: &str = "=== POINTS NON REPRIS ===";

/// Estimation prudente du nombre de tokens d'un texte : un token pour trois
/// caractères (le français en compte plutôt quatre). La vraie valeur arrive
/// avec la réponse.
pub fn estimate_tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(3)
}

/// Coût d'une demande, en millionièmes de dollar ; `None` pour un modèle
/// dont le tarif n'est pas connu.
pub fn cost_micro(model: &str, input: u64, output: u64) -> Option<i64> {
    let price = ai_price(model)?;
    Some(cost_with(price, input, output))
}

fn cost_with(price: AiPrice, input: u64, output: u64) -> i64 {
    let (pin, pout) = match price.long {
        Some((threshold, pin, pout)) if input > threshold => (pin, pout),
        _ => (price.input, price.output),
    };
    // Prix par million de tokens : en millionièmes de dollar, c'est le prix
    // par token.
    (input as f64 * pin + output as f64 * pout).round() as i64
}

/// « 0,01 $ », « 0,184 $ » : au moins deux décimales, trois sous un cent.
pub fn money(micro: i64) -> String {
    let dollars = micro as f64 / 1_000_000.0;
    let text = if micro > 0 && dollars < 0.01 {
        format!("{dollars:.4}")
    } else if dollars < 1.0 {
        format!("{dollars:.3}")
    } else {
        format!("{dollars:.2}")
    };
    // Pas de zéros inutiles au-delà de deux décimales.
    let (int, frac) = text.split_once('.').unwrap_or((&text, "00"));
    let mut frac = frac.trim_end_matches('0').to_string();
    while frac.len() < 2 {
        frac.push('0');
    }
    format!("{int},{frac} $")
}

/// Coût affiché avant l'envoi : « au plus », avec l'entrée estimée et toute
/// la sortie possible.
pub fn cost_ceiling(model: &str, estimated_input: u64) -> String {
    match cost_micro(model, estimated_input, u64::from(MAX_TOKENS)) {
        Some(micro) => format!("{} au plus", money(micro)),
        None => "coût inconnu".into(),
    }
}

/// « claude-haiku-5-5 » → « Haiku 5.5 ».
pub fn model_label(id: &str) -> String {
    let rest = id.strip_prefix("claude-").unwrap_or(id);
    let mut parts = rest.split('-');
    let Some(name) = parts.next().filter(|n| !n.is_empty()) else {
        return id.to_string();
    };
    let version: Vec<&str> = parts.collect();
    let mut chars = name.chars();
    let name: String = chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default();
    if version.is_empty()
        || !version
            .iter()
            .all(|p| p.chars().all(|c| c.is_ascii_digit()))
    {
        return id.to_string();
    }
    format!("{name} {}", version.join("."))
}

/// « 12 345 »
pub fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('\u{202f}');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

// ---------------------------------------------------------------------
// Variables
// ---------------------------------------------------------------------

/// Une variable connue : son nom (`client.adresse`), son nom lisible
/// (« Adresse ») et sa valeur.
pub struct Var {
    pub name: String,
    pub label: String,
    pub value: String,
}

/// Le modèle rempli, et ce qui reste à compléter.
#[derive(Debug, PartialEq, Eq)]
pub struct Filled {
    pub text: String,
    /// Les noms lisibles des variables vides ou inconnues, sans doublon,
    /// dans l'ordre du modèle.
    pub missing: Vec<String>,
}

/// Remplace chaque `{{nom}}` du modèle par sa valeur. Une variable vide ou
/// inconnue devient « [À compléter : <nom lisible>] ». Un seul passage : une
/// valeur qui contient `{{` reste telle quelle.
pub fn fill(template: &str, vars: &[Var]) -> Filled {
    let mut text = String::with_capacity(template.len());
    let mut missing: Vec<String> = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let Some(len) = rest[start + 2..].find("}}") else {
            break;
        };
        let inner = &rest[start + 2..start + 2 + len];
        // Pas une variable (retour à la ligne, accolade) : on recopie.
        if inner.contains(['\n', '{', '}']) {
            text.push_str(&rest[..start + 2]);
            rest = &rest[start + 2..];
            continue;
        }
        text.push_str(&rest[..start]);
        let name = inner.trim();
        let known = vars.iter().find(|v| v.name == name);
        match known {
            Some(var) if !var.value.trim().is_empty() => text.push_str(var.value.trim()),
            _ => {
                let label = known.map_or_else(|| name.to_string(), |v| v.label.clone());
                text.push_str(&format!("[À compléter : {label}]"));
                if !missing.contains(&label) {
                    missing.push(label);
                }
            }
        }
        rest = &rest[start + 2 + len + 2..];
    }
    text.push_str(rest);
    Filled { text, missing }
}

// ---------------------------------------------------------------------
// Consignes et réponse
// ---------------------------------------------------------------------

/// Le texte d'une ligne de citation, sans le `>` ni la mise en forme du
/// début : « **Consigne :** … » → « Consigne : … ».
fn quote_text(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix('>')?;
    Some(rest.trim_start_matches(|c: char| c.is_whitespace() || c == '>' || c == '*' || c == '_'))
}

/// Une ligne qui ouvre une consigne : « > Consigne : … », majuscules et
/// gras mis à part.
fn is_consigne(line: &str) -> bool {
    let Some(text) = quote_text(line) else {
        return false;
    };
    let lower = text.to_lowercase();
    let Some(after) = lower.strip_prefix("consigne") else {
        return false;
    };
    after
        .trim_start_matches(|c: char| c.is_whitespace() || c == '*' || c == '_')
        .starts_with(':')
}

/// Retire les consignes : chaque citation dont la première ligne commence
/// par « Consigne : », avec ses lignes de suite. Plus de deux lignes vides
/// d'affilée se resserrent.
pub fn strip_consignes(markdown: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut lines = markdown.lines().peekable();
    while let Some(line) = lines.next() {
        if is_consigne(line) {
            while lines
                .peek()
                .is_some_and(|next| next.trim_start().starts_with('>'))
            {
                lines.next();
            }
            continue;
        }
        out.push(line);
    }
    let mut text = String::with_capacity(markdown.len());
    let mut blanks = 0;
    for line in out {
        if line.trim().is_empty() {
            blanks += 1;
            if blanks > 1 {
                continue;
            }
        } else {
            blanks = 0;
        }
        text.push_str(line.trim_end());
        text.push('\n');
    }
    text.trim().to_string()
}

/// La réponse découpée.
#[derive(Debug, PartialEq, Eq)]
pub struct Split {
    /// Le document, consignes retirées.
    pub document: String,
    /// Les points non repris ; `None` si la ligne séparatrice manque.
    pub points: Option<Vec<String>>,
}

/// Points non repris d'une version, au plus.
const POINTS_MAX: usize = 200;
/// Longueur d'un point, au plus.
const POINT_MAX_CHARS: usize = 1000;

/// Coupe la réponse à la ligne séparatrice : avant, le document ; après, la
/// liste des points non repris (« - Aucun » : liste vide). Un bloc de code
/// qui entourerait toute la réponse est retiré.
pub fn split_points(reply: &str) -> Split {
    let reply = unfence(reply.trim());
    let mut document: Vec<&str> = Vec::new();
    let mut points: Option<Vec<String>> = None;
    for line in reply.lines() {
        if let Some(list) = points.as_mut() {
            if let Some(point) = list_item(line)
                && list.len() < POINTS_MAX
            {
                list.push(point.chars().take(POINT_MAX_CHARS).collect());
            }
            continue;
        }
        let bare = line
            .trim()
            .trim_matches(|c: char| c == '*' || c == '_' || c == '#');
        if bare.trim() == SEPARATOR {
            points = Some(Vec::new());
            continue;
        }
        document.push(line);
    }
    // Un trait horizontal laissé juste avant la ligne séparatrice.
    while document
        .last()
        .is_some_and(|l| l.trim().is_empty() || l.trim() == "---" || l.trim() == "***")
    {
        document.pop();
    }
    if let Some(list) = points.as_mut() {
        list.retain(|p| {
            let lower = p.to_lowercase();
            let lower = lower.trim_end_matches('.');
            lower != "aucun" && lower != "aucune" && lower != "néant"
        });
    }
    Split {
        document: strip_consignes(&document.join("\n")),
        points,
    }
}

/// Le texte d'un élément de liste (« - », « * », « + », « 1. »), case à
/// cocher retirée.
fn list_item(line: &str) -> Option<String> {
    let line = line.trim();
    let rest = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("+ "))
        .or_else(|| {
            let digits = line.find(|c: char| !c.is_ascii_digit())?;
            (digits > 0)
                .then(|| line[digits..].strip_prefix(". "))
                .flatten()
        })?;
    let rest = rest
        .strip_prefix("[ ] ")
        .or_else(|| rest.strip_prefix("[x] "))
        .or_else(|| rest.strip_prefix("[X] "))
        .unwrap_or(rest)
        .trim();
    (!rest.is_empty()).then(|| rest.to_string())
}

/// Retire un bloc de code qui entourerait toute la réponse
/// (« ```markdown … ``` »).
fn unfence(text: &str) -> &str {
    let Some(first) = text.lines().next() else {
        return text;
    };
    if !first.trim_start().starts_with("```") || !text.trim_end().ends_with("```") {
        return text;
    }
    let inner = &text[first.len()..];
    let inner = inner.trim_end();
    inner[..inner.len() - 3].trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(name: &str, label: &str, value: &str) -> Var {
        Var {
            name: name.into(),
            label: label.into(),
            value: value.into(),
        }
    }

    #[test]
    fn variables_remplies_par_le_serveur() {
        let vars = [
            var("client.raison_sociale", "Raison sociale", "Atelier Test"),
            var("client.adresse", "Adresse", "  "),
        ];
        let filled = fill(
            "# {{ client.raison_sociale }}\n{{client.adresse}} {{client.siret}} {{client.adresse}}\n{{ pas fermée",
            &vars,
        );
        assert_eq!(
            filled.text,
            "# Atelier Test\n[À compléter : Adresse] [À compléter : client.siret] [À compléter : Adresse]\n{{ pas fermée"
        );
        assert_eq!(filled.missing, ["Adresse", "client.siret"]);
        // Une valeur qui contient des accolades n'est pas relue.
        let tricky = [var("audit.titre", "Titre", "{{client.adresse}}")];
        assert_eq!(fill("{{audit.titre}}", &tricky).text, "{{client.adresse}}");
    }

    #[test]
    fn consignes_retirees_meme_recopiees() {
        let text = "## Contexte\n\n> Consigne : présenter le client.\n> sur deux lignes\n\nLe client fabrique des meubles.\n\n> **Consigne :** recopiée par l'IA\n\n> Une vraie citation reste.\n\n>consigne: sans espace";
        assert_eq!(
            strip_consignes(text),
            "## Contexte\n\nLe client fabrique des meubles.\n\n> Une vraie citation reste."
        );
        assert!(!is_consigne("> Consignes générales"));
    }

    #[test]
    fn points_non_repris_separes() {
        let reply = "# Rapport\n\nTexte.\n\n---\n\n=== POINTS NON REPRIS ===\n- Le stock du dépôt 2\n* [ ] Une remarque\n1. Une autre\n";
        let split = split_points(reply);
        assert_eq!(split.document, "# Rapport\n\nTexte.");
        assert_eq!(
            split.points.unwrap(),
            ["Le stock du dépôt 2", "Une remarque", "Une autre"]
        );
        // « Aucun » : une liste vide, pas un point.
        let none = split_points("Texte.\n\n**=== POINTS NON REPRIS ===**\n\n- Aucun.");
        assert_eq!(none.points, Some(Vec::new()));
        // Sans la ligne : tout est le document.
        let missing = split_points("```markdown\n# Rapport\n```");
        assert_eq!(missing.document, "# Rapport");
        assert_eq!(missing.points, None);
    }

    #[test]
    fn couts_et_libelles() {
        // Haiku 5.5 : 0,10 $ et 0,50 $ par million de tokens.
        assert_eq!(cost_micro("claude-haiku-5-5", 10_000, 2_000), Some(2_000));
        // Au-delà de 100 000 tokens d'entrée, cinq fois plus.
        assert_eq!(cost_micro("claude-haiku-5-5", 120_000, 0), Some(60_000));
        assert_eq!(cost_micro("claude-sonnet-5-5", 10_000, 2_000), Some(40_000));
        assert_eq!(cost_micro("claude-inconnu", 10, 10), None);
        assert_eq!(money(2_000), "0,002 $");
        assert_eq!(money(40_000), "0,04 $");
        assert_eq!(money(184_000), "0,184 $");
        assert_eq!(money(1_250_000), "1,25 $");
        assert_eq!(money(0), "0,00 $");
        assert_eq!(model_label("claude-haiku-5-5"), "Haiku 5.5");
        assert_eq!(model_label("claude-sonnet-5-5"), "Sonnet 5.5");
        assert_eq!(model_label("autre"), "autre");
        assert_eq!(thousands(12_345), "12\u{202f}345");
        assert_eq!(estimate_tokens("abcd"), 2);
        assert_eq!(cost_ceiling("claude-haiku-5-5", 12_000), "0,0092 $ au plus");
    }
}
