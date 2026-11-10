//! Markdown -> HTML sûr.
//!
//! Trois étapes :
//!   1. pulldown-cmark transforme le Markdown en HTML (tableaux, cases à
//!      cocher, texte barré) ;
//!   2. au passage, syntect colore les blocs de code, sous forme de
//!      `<span class="hl-…">` (des classes, pas de styles en ligne) ;
//!   3. ammonia nettoie le résultat : il ne garde qu'une liste blanche de
//!      balises et d'attributs. Scripts, gestionnaires `onclick`, liens
//!      `javascript:`, iframes… tout le reste disparaît.
//!
//! L'étape 3 passe sur *tout* le HTML, y compris celui produit par la
//! coloration : rien n'arrive au navigateur sans avoir été nettoyé.

use std::borrow::Cow;
use std::sync::LazyLock;

use ammonia::Builder;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd, html};
use syntect::html::{ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use crate::web::escape;

/// Préfixe des classes de coloration. C'est le seul motif de classe
/// qu'ammonia laisse passer : une note ne peut pas emprunter les classes de
/// l'interface pour se déguiser en bouton ou en bandeau.
const HL_PREFIX: &str = "hl-";

/// Grammaires de coloration, chargées à la première note contenant du code.
static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);

static SANITIZER: LazyLock<Builder<'static>> = LazyLock::new(|| {
    let mut builder = Builder::default();
    builder
        // Cases à cocher des listes de tâches : `- [ ] à faire`. Le type et
        // l'état désactivé sont imposés : quoi qu'écrive la note, un <input>
        // ne peut être qu'une case à cocher inerte, jamais un champ de saisie.
        .add_tags(["input"])
        .add_tag_attributes("input", ["checked"])
        .set_tag_attribute_value("input", "type", "checkbox")
        .set_tag_attribute_value("input", "disabled", "")
        // Classes de coloration sur les blocs de code.
        .add_tag_attributes("span", ["class"])
        .add_tag_attributes("code", ["class"])
        .add_tag_attributes("pre", ["class"])
        .attribute_filter(|_element, attribute, value| match attribute {
            "class" => {
                let kept: Vec<&str> = value
                    .split_whitespace()
                    .filter(|c| c.starts_with(HL_PREFIX))
                    .collect();
                if kept.is_empty() {
                    None
                } else {
                    Some(Cow::Owned(kept.join(" ")))
                }
            }
            _ => Some(Cow::Borrowed(value)),
        })
        // Les liens sortants ne transmettent ni l'adresse de la note ni
        // l'accès à la fenêtre d'origine.
        .link_rel(Some("noopener noreferrer nofollow"));
    builder
});

/// Extensions Markdown reconnues, au rendu comme dans les extraits.
fn options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS | Options::ENABLE_STRIKETHROUGH
}

/// Rend une note en HTML prêt à être inséré dans la page.
pub fn render(markdown: &str) -> String {
    let options = options();

    let mut events = Vec::new();
    // Bloc de code en cours : (langage, contenu accumulé).
    let mut code: Option<(String, String)> = None;

    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((lang, String::new()));
            }
            Event::Text(text) if code.is_some() => {
                if let Some((_, buffer)) = code.as_mut() {
                    buffer.push_str(&text);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((lang, source)) = code.take() {
                    events.push(Event::Html(highlight(&lang, &source).into()));
                }
            }
            other => events.push(other),
        }
    }

    let mut unsafe_html = String::with_capacity(markdown.len() * 3 / 2);
    html::push_html(&mut unsafe_html, events.into_iter());
    SANITIZER.clean(&unsafe_html).to_string()
}

/// Colore un bloc de code. Langage inconnu ou absent : texte brut échappé.
fn highlight(lang: &str, source: &str) -> String {
    let syntax = if lang.is_empty() {
        None
    } else {
        SYNTAXES.find_syntax_by_token(lang)
    };
    let Some(syntax) = syntax else {
        return format!("<pre><code>{}</code></pre>", escape(source));
    };

    let mut generator = ClassedHTMLGenerator::new_with_class_style(
        syntax,
        &SYNTAXES,
        ClassStyle::SpacedPrefixed { prefix: HL_PREFIX },
    );
    for line in LinesWithEndings::from(source) {
        if generator
            .parse_html_for_line_which_includes_newline(line)
            .is_err()
        {
            return format!("<pre><code>{}</code></pre>", escape(source));
        }
    }
    format!("<pre><code>{}</code></pre>", generator.finalize())
}

/// Texte brut d'un morceau de Markdown, sur une ligne : ni `#`, ni `**`, ni
/// cases `[x]`. Sert aussi aux extraits de la recherche plein texte, qui sont
/// des morceaux du Markdown d'origine : les marqueurs de FTS5 (caractères de
/// contrôle) sont du texte pour le parseur et traversent intacts.
pub fn plain(markdown: &str) -> String {
    plain_until(markdown, usize::MAX)
}

fn plain_until(markdown: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for event in Parser::new_ext(markdown, options()) {
        let text = match event {
            Event::Text(text) | Event::Code(text) => text,
            Event::SoftBreak | Event::HardBreak | Event::End(_) => " ".into(),
            _ => continue,
        };
        out.push_str(&text);
        if out.chars().count() >= max_chars {
            break;
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Début du texte d'une note, sans la syntaxe Markdown, pour les listes.
pub fn excerpt(markdown: &str, max_chars: usize) -> String {
    let collapsed = plain_until(markdown, max_chars);
    if collapsed.chars().count() > max_chars {
        let cut: String = collapsed.chars().take(max_chars).collect();
        format!("{}…", cut.trim_end())
    } else {
        collapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_de_base() {
        let html = render("# Titre\n\nUn **gras** et un [lien](https://example.com).");
        assert!(html.contains("<h1>Titre</h1>"));
        assert!(html.contains("<strong>gras</strong>"));
        assert!(html.contains(r#"href="https://example.com""#));
        assert!(html.contains("noopener"));
    }

    #[test]
    fn tableaux_et_cases_a_cocher() {
        let html = render("| a | b |\n|---|---|\n| 1 | 2 |\n\n- [x] fait\n- [ ] à faire\n");
        assert!(html.contains("<table>"));
        assert!(html.contains("<td>1</td>"));
        assert_eq!(html.matches(r#"type="checkbox""#).count(), 2);
        assert!(html.contains("checked"));
    }

    #[test]
    fn code_colore_avec_des_classes() {
        let html = render("```rust\nfn main() { let x = 1; }\n```");
        assert!(html.contains("<pre><code>"));
        assert!(html.contains(r#"<span class="hl-"#));
        // Jamais de style en ligne : la CSP les interdit.
        assert!(!html.contains("style="));
    }

    #[test]
    fn code_sans_langage_echappe() {
        let html = render("```\n<script>alert(1)</script>\n```");
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
    }

    // --- Critère d'acceptation : aucune injection de script via une note ---

    #[test]
    fn balise_script_supprimee() {
        let html = render("Bonjour <script>alert('xss')</script> monde");
        assert!(!html.contains("<script"));
        assert!(!html.contains("alert("));
    }

    #[test]
    fn gestionnaires_d_evenements_supprimes() {
        let html = render(r#"<img src="x" onerror="alert(1)"> <b onclick="alert(2)">gras</b>"#);
        assert!(!html.contains("onerror"));
        assert!(!html.contains("onclick"));
        assert!(html.contains("<b>gras</b>"));
    }

    #[test]
    fn liens_javascript_neutralises() {
        let html = render("[clic](javascript:alert(1)) <a href=\"javascript:alert(2)\">x</a>");
        assert!(!html.contains("javascript:"));
    }

    #[test]
    fn balises_dangereuses_supprimees() {
        let html = render(
            "<iframe src=\"https://evil.example\"></iframe>\
             <object data=\"x\"></object>\
             <form action=\"https://evil.example\"><input name=\"mdp\"></form>\
             <style>body{display:none}</style>\
             <svg><script>alert(1)</script></svg>\
             <math><mi xlink:href=\"javascript:alert(1)\">x</mi></math>",
        );
        for forbidden in [
            "<iframe",
            "<object",
            "<form",
            "<style",
            "<svg",
            "<script",
            "javascript:",
            "display:none",
        ] {
            assert!(
                !html.contains(forbidden),
                "{forbidden} ne devrait pas survivre : {html}"
            );
        }
    }

    #[test]
    fn styles_et_classes_de_l_interface_refuses() {
        let html = render(r#"<span class="btn hl-keyword" style="position:fixed">x</span>"#);
        assert!(!html.contains("style="));
        assert!(!html.contains("btn"));
        assert!(html.contains(r#"class="hl-keyword""#));
    }

    #[test]
    fn seules_les_cases_a_cocher_passent() {
        let html = render(r#"<input type="password" name="mdp"><input type="text">"#);
        assert!(!html.contains("password"));
        assert!(!html.contains(r#"type="text""#));
        assert!(!html.contains("name="));
        assert_eq!(html.matches(r#"type="checkbox""#).count(), 2);
        assert_eq!(html.matches("disabled").count(), 2);
    }

    #[test]
    fn extrait_sans_syntaxe() {
        assert_eq!(
            excerpt("# Titre\n\nUn **gras** et `du code`.", 100),
            "Titre Un gras et du code."
        );
        assert_eq!(excerpt("abcdefghij klm", 5), "abcde…");
        // Les cases d'une liste de tâches ne passent pas dans l'extrait.
        assert_eq!(
            excerpt("## Objectif\n\n- [x] Créer les VLAN\n- [ ] Tester", 100),
            "Objectif Créer les VLAN Tester"
        );
    }

    #[test]
    fn extrait_fts_sans_syntaxe_et_marqueurs_gardes() {
        // Morceau tel que le renvoie snippet() de FTS5 : du Markdown brut,
        // avec \u{1} et \u{2} autour du mot trouvé.
        assert_eq!(
            plain("## Objectif\n\nFiltrer le \u{1}VLAN\u{2} 10 :\n\n- [x] Règle **deny**"),
            "Objectif Filtrer le \u{1}VLAN\u{2} 10 : Règle deny"
        );
    }
}
