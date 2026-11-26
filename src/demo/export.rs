//! Export de la démo : parcours des pages et écriture des fichiers.
//!
//! Le parcours part des pages de chaque espace et suit les liens (`href`,
//! `src`, `hx-get`, chemins des attributs `data-*`), comme un visiteur qui
//! cliquerait partout. Chaque adresse est demandée avec le préfixe de son
//! espace (`/e/ecole/notes/12`) : le serveur l'accepte pour toutes les pages,
//! et une page statique ne peut pas lire l'en-tête `X-Espace`.
//!
//! Correspondance adresse → fichier (même règle dans `static/js/demo.js`) :
//!
//!   /e/ecole/notes            e/ecole/notes/index.html
//!   /e/ecole/notes?tag=Cours  e/ecole/notes/_q/tag-Cours/index.html
//!   /e/ecole/files/3/raw      e/ecole/files/3/raw.pdf (extension du type)
//!
//! Les paramètres sont triés par nom, les vides retirés, puis tout ce qui
//! n'est ni lettre ASCII, ni chiffre, ni « . _ - » devient « - ».
//!
//! Une fois tout lu, chaque page est réécrite : adresses préfixées par
//! `/demo`, liens vers une page non exportée remplacés par `#` avec
//! `data-demo-off`, jeton CSRF remplacé par `demo`, notes en direct coupées.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use anyhow::Context;
use axum::http::{StatusCode, header};
use chrono::{Days, NaiveDate};

use super::{Browser, Mode};
use crate::{AppState, time};

/// Ce que l'export a écrit.
pub struct Report {
    pub pages: usize,
    pub assets: usize,
    pub disabled: usize,
}

/// Plafond du parcours : au-delà, les liens restants sont désactivés.
const MAX_URLS: usize = 3000;

/// Adresses jamais exportées : connexion, API, partage public, temps réel,
/// outils réservés, pages sans intérêt hors serveur. Comparées au chemin
/// sans le préfixe de l'espace.
const SKIP: &[&str] = &[
    "/api",
    "/mcp",
    "/oauth",
    "/.well-known",
    "/auth/",
    "/login",
    "/logout",
    "/setup",
    "/s/",
    "/settings/google-calendar",
    "/1life",
    "/styleguide",
    // Choix d'un dossier où déplacer : ne sert qu'à écrire.
    "/destinations",
    "/health",
    "/cmd/suggest",
];

/// Attributs `data-*` qui portent une adresse d'écriture : jamais demandés.
const WRITE_DATA: &[&str] = &["data-drag-move", "data-save-url", "data-live-url"];

/// Fenêtre de l'agenda exportée, en jours autour d'aujourd'hui.
const AGENDA_BEFORE: u64 = 35;
const AGENDA_AFTER: u64 = 56;

/// Une page (ou un fichier) du site à exporter.
#[derive(Clone, Debug)]
struct Req {
    /// Adresse demandée au serveur : `/e/ecole/notes?tag=Cours`.
    key: String,
    /// Dossier du fichier exporté, sans `/` final : `e/ecole/notes/_q/tag-Cours`.
    dir: String,
}

/// Ce que devient une adresse trouvée dans une page.
#[derive(Debug)]
enum Link {
    /// Hors du site (`https:`, `#ancre`, `mailto:`…) : inchangée.
    Keep,
    /// Fichier statique : seulement préfixé.
    Static(String),
    /// Jamais exportée.
    Off,
    /// Page ou fichier à exporter ; le reste (`#ancre`) est recollé.
    Page(Req, String),
}

/// Ce que le parcours a obtenu pour une adresse.
enum Outcome {
    /// Écrite dans `file` (relatif au dossier d'export), servie à `url`.
    Saved {
        file: String,
        url: String,
        body: Vec<u8>,
        /// Espace de la page, pour réécrire ses liens.
        html: Option<String>,
    },
    /// Redirigée vers une autre adresse du parcours.
    Alias(String),
    /// Absente (erreur, refus, plafond) : les liens qui y mènent sont coupés.
    Missing,
}

pub async fn crawl(
    state: &AppState,
    browser: &mut Browser,
    out: &Path,
    base: &str,
) -> anyhow::Result<Report> {
    let today = time::paris_date(time::now());
    let window = (
        today
            .checked_sub_days(Days::new(AGENDA_BEFORE))
            .context("date")?,
        today
            .checked_add_days(Days::new(AGENDA_AFTER))
            .context("date")?,
    );
    let slugs: Vec<String> = sqlx::query_scalar(
        "SELECT slug FROM spaces WHERE archived_at IS NULL AND deleted_at IS NULL ORDER BY position",
    )
    .fetch_all(&state.db)
    .await?;

    let mut queue: VecDeque<(Req, Mode)> = VecDeque::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut push = |queue: &mut VecDeque<(Req, Mode)>, req: Req, mode: Mode| {
        if seen.insert(req.key.clone()) {
            queue.push_back((req, mode));
        }
    };
    for slug in &slugs {
        for page in [
            "/",
            "/notes",
            "/files",
            "/calendar",
            "/tools",
            "/settings",
            "/settings/trash",
        ] {
            if let Link::Page(req, _) = resolve(page, slug, window) {
                push(&mut queue, req, Mode::Page);
            }
        }
    }

    let mut outcomes: HashMap<String, Outcome> = HashMap::new();
    while let Some((req, mode)) = queue.pop_front() {
        if outcomes.len() >= MAX_URLS {
            outcomes.insert(req.key, Outcome::Missing);
            continue;
        }
        let reply = browser.get(&req.key, mode, None).await?;
        let slug = space_of(&req.key).to_string();
        if reply.status.is_redirection() {
            let target = reply
                .headers
                .get(header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .map(|l| resolve(l, &slug, window));
            let outcome = match target {
                Some(Link::Page(target, _)) if target.key != req.key => {
                    let key = target.key.clone();
                    push(&mut queue, target, mode);
                    Outcome::Alias(key)
                }
                _ => Outcome::Missing,
            };
            outcomes.insert(req.key, outcome);
            continue;
        }
        if reply.status != StatusCode::OK {
            if reply.status.is_server_error() {
                tracing::warn!(url = %req.key, status = %reply.status, "page de la démo en erreur");
            }
            outcomes.insert(req.key, Outcome::Missing);
            continue;
        }
        let mime = reply
            .headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let disposition = reply
            .headers
            .get(header::CONTENT_DISPOSITION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let (file, url, html) = match mime.as_str() {
            "text/html" => {
                let text = reply.text();
                for (found, found_mode) in links(&text) {
                    if let Link::Page(target, _) = resolve(&found, &slug, window) {
                        push(&mut queue, target, found_mode);
                    }
                }
                // La barre de commandes demande son catalogue pour la page
                // affichée (`#cmd-page`).
                if let Some(page) = super::between(
                    &text,
                    "id=\"cmd-page\" type=\"hidden\" name=\"page\" value=\"",
                    "\"",
                ) {
                    let page = decode_entities(page);
                    let query = form_urlencoded::Serializer::new(String::new())
                        .append_pair("page", &page)
                        .finish();
                    if let Link::Page(target, _) =
                        resolve(&format!("/cmd/catalogue?{query}"), &slug, window)
                    {
                        push(&mut queue, target, Mode::Fragment);
                    }
                }
                (
                    format!("{}/index.html", req.dir),
                    format!("{base}/{}/", req.dir),
                    Some(slug.clone()),
                )
            }
            "application/json" => (
                format!("{}/index.json", req.dir),
                format!("{base}/{}/index.json", req.dir),
                None,
            ),
            _ => {
                let last = req.dir.rsplit('/').next().unwrap_or_default();
                let file = if last.contains('.') {
                    req.dir.clone()
                } else {
                    let ext = disposition
                        .as_deref()
                        .and_then(extension_of_disposition)
                        .unwrap_or_else(|| extension_of_mime(&mime).to_string());
                    format!("{}.{ext}", req.dir)
                };
                let url = format!("{base}/{file}");
                (file, url, None)
            }
        };
        outcomes.insert(
            req.key,
            Outcome::Saved {
                file,
                url,
                body: reply.body.to_vec(),
                html,
            },
        );
    }

    // --- Écriture -----------------------------------------------------------
    let finals = final_urls(&outcomes);
    let mut report = Report {
        pages: 0,
        assets: 0,
        disabled: 0,
    };
    let mut disabled: HashSet<String> = HashSet::new();
    for outcome in outcomes.values() {
        let Outcome::Saved {
            file, body, html, ..
        } = outcome
        else {
            continue;
        };
        let bytes = match html {
            Some(slug) => {
                let text = String::from_utf8_lossy(body);
                let page = rewrite(&text, slug, base, window, &finals, &mut disabled);
                report.pages += 1;
                page.into_bytes()
            }
            None => body.clone(),
        };
        write(out, file, &bytes)?;
    }
    report.disabled = disabled.len();

    // --- Fichiers statiques, tels que le serveur les sert --------------------
    for asset in crate::assets::ASSETS {
        let plain = format!("/static/{}", asset.path);
        for url in [asset.url, plain.as_str()] {
            let reply = browser.get(url, Mode::Plain, None).await?;
            if reply.status != StatusCode::OK {
                continue;
            }
            let mut bytes = reply.body.to_vec();
            let text_like = asset.mime.starts_with("text/")
                || asset.mime.contains("javascript")
                || asset.mime.contains("json")
                || asset.path.ends_with(".webmanifest");
            if text_like && let Ok(text) = std::str::from_utf8(&bytes) {
                let mut text = text.replace("/static/", &format!("{base}/static/"));
                if asset.path.ends_with(".webmanifest") {
                    text = text
                        .replace("\"id\": \"/\"", &format!("\"id\": \"{base}/\""))
                        .replace(
                            "\"start_url\": \"/\"",
                            &format!("\"start_url\": \"{base}/e/perso/\""),
                        )
                        .replace("\"scope\": \"/\"", &format!("\"scope\": \"{base}/\""));
                }
                bytes = text.into_bytes();
            }
            write(out, url.trim_start_matches('/'), &bytes)?;
            report.assets += 1;
        }
    }

    // L'entrée de la démo : /demo/ mène au dashboard de Perso.
    let first = slugs.first().map(String::as_str).unwrap_or("perso");
    write(out, "index.html", entry_page(base, first).as_bytes())?;
    Ok(report)
}

/// Page d'entrée de `/demo/` : un lien et une redirection sans script (la
/// CSP des pages servies par GitHub n'interdit pas `<meta refresh>`).
fn entry_page(base: &str, slug: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"fr\"><head><meta charset=\"utf-8\">\
         <meta http-equiv=\"refresh\" content=\"0; url={base}/e/{slug}/\">\
         <meta name=\"robots\" content=\"noindex\"><title>Démo</title></head>\
         <body><p><a href=\"{base}/e/{slug}/\">Ouvrir la démo</a></p></body></html>\n"
    )
}

fn write(out: &Path, file: &str, bytes: &[u8]) -> anyhow::Result<()> {
    let path = out.join(file);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("dossier {}", dir.display()))?;
    }
    std::fs::write(&path, bytes).with_context(|| format!("écriture de {}", path.display()))
}

/// Adresse servie de chaque page exportée, redirections suivies.
fn final_urls(outcomes: &HashMap<String, Outcome>) -> HashMap<String, String> {
    let mut finals = HashMap::new();
    for key in outcomes.keys() {
        let mut current = key;
        for _ in 0..8 {
            match outcomes.get(current) {
                Some(Outcome::Saved { url, .. }) => {
                    finals.insert(key.clone(), url.clone());
                    break;
                }
                Some(Outcome::Alias(next)) => current = next,
                _ => break,
            }
        }
    }
    finals
}

/// `/e/ecole/notes` → `ecole`.
fn space_of(key: &str) -> &str {
    crate::spaces::url::split(key)
        .map(|(slug, _)| slug)
        .unwrap_or("perso")
}

/// Paramètres d'une adresse, sous leur forme canonique : vides retirés,
/// triés par nom (même règle dans demo.js).
fn canonical_pairs(query: &str) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .filter(|(_, value)| !value.is_empty())
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    pairs
}

/// Nom de dossier des paramètres : `tag=Cours&q=x` → `q-x-tag-Cours`.
pub(crate) fn query_slug(pairs: &[(String, String)]) -> String {
    let joined = pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let mut slug: String = joined
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .take(150)
        .collect();
    if slug.starts_with('.') {
        slug.replace_range(..1, "-");
    }
    slug
}

/// Ce que devient l'adresse `raw`, trouvée dans une page de l'espace `slug`.
fn resolve(raw: &str, slug: &str, window: (NaiveDate, NaiveDate)) -> Link {
    let raw = raw.trim();
    if !raw.starts_with('/') || raw.starts_with("//") {
        return Link::Keep;
    }
    let (raw, anchor) = match raw.find('#') {
        Some(at) => raw.split_at(at),
        None => (raw, ""),
    };
    let (path, query) = raw.split_once('?').unwrap_or((raw, ""));
    if path.starts_with("/static/") {
        return Link::Static(format!(
            "{path}{}{anchor}",
            if query.is_empty() {
                String::new()
            } else {
                format!("?{query}")
            }
        ));
    }
    let (space, rest) = crate::spaces::url::split(path).unwrap_or((slug, path));
    let skipped = SKIP.iter().any(|p| rest.starts_with(p))
        || rest.ends_with("/live")
        || rest.ends_with("download.pdf");
    // Un chemin qui ne tient pas dans un nom de fichier sûr n'est pas exporté.
    let safe = rest
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-'))
        && !rest.contains("..")
        && !rest.contains("/_q/");
    if skipped || !safe {
        return Link::Off;
    }
    let mut pairs = canonical_pairs(query);
    // Le formulaire d'un nouvel événement existe pour chaque jour affiché :
    // un seul suffit (aujourd'hui), l'enregistrer est refusé de toute façon.
    if rest == "/calendar/events/new" {
        pairs.clear();
    }
    if rest == "/calendar"
        && let Some((_, date)) = pairs.iter().find(|(k, _)| k == "date")
    {
        match NaiveDate::parse_from_str(date, "%Y-%m-%d") {
            Ok(day) if day >= window.0 && day <= window.1 => {}
            _ => return Link::Off,
        }
    }
    let mut key = format!("/e/{space}{rest}");
    let mut dir = format!("e/{space}{}", rest.trim_end_matches('/'));
    if !pairs.is_empty() {
        let mut query = form_urlencoded::Serializer::new(String::new());
        for (k, v) in &pairs {
            query.append_pair(k, v);
        }
        key = format!("{key}?{}", query.finish());
        dir = format!("{dir}/_q/{}", query_slug(&pairs));
    }
    Link::Page(Req { key, dir }, anchor.to_string())
}

fn extension_of_mime(mime: &str) -> &'static str {
    match mime {
        "application/pdf" => "pdf",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        "text/plain" => "txt",
        "text/csv" => "csv",
        "text/markdown" => "md",
        "text/css" => "css",
        "application/json" => "json",
        "text/javascript" | "application/javascript" => "js",
        _ => "bin",
    }
}

/// Extension du nom donné par `Content-Disposition` (`filename="a.pdf"`).
fn extension_of_disposition(value: &str) -> Option<String> {
    let name = value
        .split("filename=")
        .nth(1)?
        .trim_matches(|c| c == '"' || c == ' ');
    let ext = name.rsplit_once('.')?.1.trim_end_matches('"');
    (!ext.is_empty() && ext.len() <= 8 && ext.bytes().all(|b| b.is_ascii_alphanumeric()))
        .then(|| ext.to_ascii_lowercase())
}

// --- Lecture et réécriture du HTML -------------------------------------------

/// Un attribut d'une balise ; `value` est décodée (`&amp;` → `&`).
struct Attr {
    name: String,
    value: Option<String>,
}

/// Attributs dont la valeur est une adresse lue en GET, et comment la lire.
fn get_mode(tag: &str, attrs: &[Attr], name: &str) -> Option<Mode> {
    match name {
        "href" => Some(if tag == "a" { Mode::Page } else { Mode::Plain }),
        "src" | "poster" => Some(Mode::Plain),
        "hx-get" => Some(Mode::Fragment),
        "action" => {
            let method = attr(attrs, "method").unwrap_or("get").to_ascii_lowercase();
            (method == "get").then_some(Mode::Page)
        }
        n if n.starts_with("data-") && !WRITE_DATA.contains(&n) => Some(Mode::Plain),
        _ => None,
    }
}

fn attr<'a>(attrs: &'a [Attr], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|a| a.name == name)
        .and_then(|a| a.value.as_deref())
}

/// Adresses lues en GET dans une page, avec la façon de les demander.
fn links(html: &str) -> Vec<(String, Mode)> {
    let mut found = Vec::new();
    walk(html, &mut |tag, attrs| {
        for a in attrs.iter() {
            if let (Some(value), Some(mode)) = (&a.value, get_mode(tag, attrs, &a.name))
                && value.starts_with('/')
            {
                found.push((value.clone(), mode));
            }
        }
    });
    found
}

/// Réécrit une page de l'espace `slug` pour la démo statique.
fn rewrite(
    html: &str,
    slug: &str,
    base: &str,
    window: (NaiveDate, NaiveDate),
    finals: &HashMap<String, String>,
    disabled: &mut HashSet<String>,
) -> String {
    // Le jeton CSRF de la session d'export ne doit pas partir dans la démo.
    let html = match super::between(html, "\"X-CSRF-Token\":\"", "\"") {
        Some(token) if token.len() >= 16 => html.replace(token, "demo"),
        _ => html.to_string(),
    };
    walk(&html, &mut |tag, attrs| {
        attrs.retain(|a| a.name != "data-live-url");
        let mut off = false;
        let mut renamed = None;
        // Seules les adresses lues en GET changent ; celles d'écriture
        // (formulaire POST, hx-post…) restent : demo.js les arrête.
        let reads: Vec<bool> = attrs
            .iter()
            .map(|a| get_mode(tag, attrs, &a.name).is_some())
            .collect();
        for (i, a) in attrs.iter_mut().enumerate() {
            let Some(value) = a.value.clone() else {
                continue;
            };
            if !value.starts_with('/') || !reads[i] {
                continue;
            }
            match resolve(&value, slug, window) {
                Link::Keep => {}
                Link::Static(path) => a.value = Some(format!("{base}{path}")),
                Link::Page(req, anchor) => match finals.get(&req.key) {
                    Some(url) => a.value = Some(format!("{url}{anchor}")),
                    None => {
                        disabled.insert(req.key);
                        off = true;
                        cut(a, &mut renamed, i, base, &value);
                    }
                },
                Link::Off => {
                    disabled.insert(value.clone());
                    off = true;
                    cut(a, &mut renamed, i, base, &value);
                }
            }
        }
        if let Some(i) = renamed {
            attrs[i].name = "data-demo-hx-get".into();
        }
        // Un clic sur l'élément (ou dans un formulaire coupé) affiche le
        // message au lieu d'ouvrir une page absente.
        if off && !attrs.iter().any(|a| a.name == "data-demo-off") {
            attrs.push(Attr {
                name: "data-demo-off".into(),
                value: None,
            });
        }
    })
}

/// Coupe une adresse non exportée : lien vers `#`, `hx-get` rendu inerte,
/// le reste préfixé (la requête échouera sans rien toucher).
fn cut(a: &mut Attr, renamed: &mut Option<usize>, i: usize, base: &str, value: &str) {
    match a.name.as_str() {
        "href" => a.value = Some("#".into()),
        "hx-get" => *renamed = Some(i),
        _ => a.value = Some(format!("{base}{value}")),
    }
}

/// Décode les entités d'une valeur d'attribut.
fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest.find(';').filter(|&end| end <= 12) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn encode_attr(value: &str, quote: char) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '"' if quote == '"' => out.push_str("&#34;"),
            '\'' if quote == '\'' => out.push_str("&#39;"),
            '<' => out.push_str("&lt;"),
            c => out.push(c),
        }
    }
    out
}

/// Parcourt les balises d'une page : `visit` reçoit le nom de chaque balise
/// ouvrante et ses attributs, qu'il peut changer. Le reste (texte,
/// commentaires, contenu des scripts) est recopié tel quel.
fn walk(html: &str, visit: &mut dyn FnMut(&str, &mut Vec<Attr>)) -> String {
    let mut out = String::with_capacity(html.len() + html.len() / 8);
    let bytes = html.as_bytes();
    let mut i = 0;
    while let Some(lt) = html[i..].find('<').map(|p| p + i) {
        out.push_str(&html[i..lt]);
        let rest = &html[lt..];
        if rest.starts_with("<!--") {
            let end = rest.find("-->").map(|p| lt + p + 3).unwrap_or(html.len());
            out.push_str(&html[lt..end]);
            i = end;
            continue;
        }
        let name_len = rest[1..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == ':'))
            .unwrap_or(rest.len() - 1);
        if name_len == 0 {
            // `</…`, `<!doctype>`, ou un « < » isolé : recopié.
            let end = rest.find('>').map(|p| lt + p + 1).unwrap_or(html.len());
            out.push_str(&html[lt..end]);
            i = end;
            continue;
        }
        let tag = rest[1..1 + name_len].to_ascii_lowercase();
        // Attributs, jusqu'au « > » hors guillemets.
        let mut j = lt + 1 + name_len;
        let mut attrs = Vec::new();
        let mut quotes = Vec::new();
        let mut self_closing = false;
        loop {
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j >= bytes.len() {
                break;
            }
            match bytes[j] {
                b'>' => {
                    j += 1;
                    break;
                }
                b'/' => {
                    self_closing = true;
                    j += 1;
                    continue;
                }
                _ => {}
            }
            self_closing = false;
            let start = j;
            while j < bytes.len()
                && !bytes[j].is_ascii_whitespace()
                && !matches!(bytes[j], b'=' | b'>' | b'/')
            {
                j += 1;
            }
            let name = html[start..j].to_ascii_lowercase();
            if j < bytes.len() && bytes[j] == b'=' {
                j += 1;
                let (value, quote) = match bytes.get(j) {
                    Some(&q @ (b'"' | b'\'')) => {
                        let close = html[j + 1..]
                            .find(q as char)
                            .map(|p| j + 1 + p)
                            .unwrap_or(html.len());
                        let value = &html[j + 1..close.min(html.len())];
                        j = (close + 1).min(html.len());
                        (value, q as char)
                    }
                    _ => {
                        let start = j;
                        while j < bytes.len() && !bytes[j].is_ascii_whitespace() && bytes[j] != b'>'
                        {
                            j += 1;
                        }
                        (&html[start..j], '"')
                    }
                };
                attrs.push(Attr {
                    name,
                    value: Some(decode_entities(value)),
                });
                quotes.push(quote);
            } else if !name.is_empty() {
                attrs.push(Attr { name, value: None });
                quotes.push('"');
            } else {
                j += 1;
            }
        }
        let before: Vec<String> = attrs.iter().map(|a| a.name.clone()).collect();
        visit(&tag, &mut attrs);
        out.push('<');
        out.push_str(&tag);
        for a in &attrs {
            out.push(' ');
            out.push_str(&a.name);
            if let Some(value) = &a.value {
                // Même guillemet qu'à l'origine (hx-headers en JSON garde « ' »).
                let quote = before
                    .iter()
                    .position(|n| *n == a.name)
                    .and_then(|p| quotes.get(p).copied())
                    .unwrap_or('"');
                out.push('=');
                out.push(quote);
                out.push_str(&encode_attr(value, quote));
                out.push(quote);
            }
        }
        out.push_str(if self_closing { "/>" } else { ">" });
        i = j;
        // Le contenu d'un script ou d'un style n'est pas du HTML.
        if matches!(tag.as_str(), "script" | "style" | "textarea") && !self_closing {
            let close = format!("</{tag}");
            let end = html[i..]
                .to_ascii_lowercase()
                .find(&close)
                .map(|p| i + p)
                .unwrap_or(html.len());
            out.push_str(&html[i..end]);
            i = end;
        }
    }
    out.push_str(&html[i..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> (NaiveDate, NaiveDate) {
        (
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            NaiveDate::from_ymd_opt(2026, 12, 31).unwrap(),
        )
    }

    #[test]
    fn correspondance_des_adresses() {
        let Link::Page(req, anchor) = resolve("/notes?tag=Cours&q=&x=1#haut", "ecole", window())
        else {
            panic!("page attendue");
        };
        assert_eq!(req.key, "/e/ecole/notes?tag=Cours&x=1");
        assert_eq!(req.dir, "e/ecole/notes/_q/tag-Cours-x-1");
        assert_eq!(anchor, "#haut");
        let Link::Page(req, _) = resolve("/e/taff/", "ecole", window()) else {
            panic!("page attendue");
        };
        assert_eq!((req.key.as_str(), req.dir.as_str()), ("/e/taff/", "e/taff"));
        assert!(matches!(resolve("/login", "perso", window()), Link::Off));
        assert!(matches!(
            resolve("/e/perso/1life/clients", "perso", window()),
            Link::Off
        ));
        assert!(matches!(
            resolve("/notes/3/live", "perso", window()),
            Link::Off
        ));
        assert!(matches!(
            resolve("/calendar?date=2030-01-01", "perso", window()),
            Link::Off
        ));
        assert!(matches!(
            resolve("https://dashu.dev", "perso", window()),
            Link::Keep
        ));
        assert!(matches!(
            resolve("/static/a.css", "perso", window()),
            Link::Static(_)
        ));
    }

    #[test]
    fn reecriture_des_balises() {
        let html = "<a href=\"/notes?tag=A&amp;q=b\">x</a><div hx-get=\"/login\" hx-headers='{\"a\":\"b\"}'></div><script>if (a < b) {}</script><textarea><b>\"</textarea>";
        let mut finals = HashMap::new();
        finals.insert(
            "/e/perso/notes?q=b&tag=A".to_string(),
            "/demo/e/perso/notes/_q/q-b-tag-A/".to_string(),
        );
        let mut disabled = HashSet::new();
        let out = rewrite(html, "perso", "/demo", window(), &finals, &mut disabled);
        assert!(
            out.contains("href=\"/demo/e/perso/notes/_q/q-b-tag-A/\""),
            "{out}"
        );
        assert!(out.contains("data-demo-hx-get=\"/login\""), "{out}");
        assert!(out.contains("data-demo-off"), "{out}");
        assert!(out.contains("hx-headers='{\"a\":\"b\"}'"), "{out}");
        assert!(out.contains("<script>if (a < b) {}</script>"), "{out}");
        assert!(out.contains("<textarea><b>\"</textarea>"), "{out}");
    }
}
