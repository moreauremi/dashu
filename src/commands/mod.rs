//! Barre de commandes « / » (SPEC, « Barre de commandes »).
//!
//! On tape `/` hors d'un champ, puis une commande : `/go agenda demain`,
//! `/add task Réviser @ecole`. Le navigateur complète la saisie (Tab) sans
//! attendre le serveur : il reçoit la liste des commandes à l'ouverture de la
//! barre (`GET /cmd/catalogue`). Cette liste est définie une seule fois, ici
//! ([`catalogue::tree`]), avec ce qui dépend de la page ouverte (espaces,
//! conversions d'un fichier…).
//!
//! Une ligne validée part dans `POST /cmd` : [`resolve`] la relit avec les
//! mêmes règles que le navigateur (un début de mot suffit s'il n'y a pas
//! d'ambiguïté, accents et majuscules ignorés), puis [`run::run`] appelle le
//! code des boutons existants. Seules les suggestions de `/open` (notes,
//! fichiers) se cherchent sur le serveur, pendant la frappe (`GET /cmd/suggest`).

mod catalogue;
mod run;

use axum::Json;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use chrono::{Datelike, Duration, NaiveDate, Weekday};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::auth::session::Ctx;
use crate::calendar::ical;
use crate::notes::{self, markdown};
use crate::spaces::Space;
use crate::time;
use crate::web::{AppResult, local_path};

pub use run::run;

/// Ce que fait une commande, une fois la ligne lue. Jamais envoyé au navigateur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Simple groupe de sous-commandes (`/add`, `/go`).
    Group,
    /// Liste des commandes, affichée par le navigateur.
    Help,
    /// Page fixe : son adresse (sans l'espace).
    GoPage(&'static str),
    GoAgenda,
    Open,
    Search,
    Space,
    AddTask,
    AddNote,
    /// Sélecteur de fichiers, ouvert par le navigateur.
    AddFile,
    Move,
    Convert(crate::tools::Conversion),
    Compress,
    AddSpace,
    AddAgenda,
    Done,
    Undone,
    /// Supprime la note ou le fichier ouvert.
    Delete,
    DeleteTask,
    Tag,
    Untag,
    Pin,
    Keep,
    Download,
    /// Thème : `dark`, `light` ou `system`.
    Theme(&'static str),
    /// Taille de l'interface : `small`, `normal` ou `large`.
    Size(&'static str),
    Sidebar,
    Refresh,
    Logout,
}

impl Action {
    /// Action faite par le navigateur seul, sans `POST /cmd`.
    fn client(self) -> Option<&'static str> {
        match self {
            Self::Help => Some("help"),
            Self::AddFile => Some("file"),
            _ => None,
        }
    }
}

/// Une commande ou une sous-commande.
#[derive(Debug, Clone, Serialize)]
pub struct Node {
    /// Mot à taper : `add`, `task`.
    pub name: &'static str,
    /// Autres mots acceptés : `fichiers` pour `files`…
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub aliases: &'static [&'static str],
    /// Ce que fait la commande, affiché à côté de la suggestion.
    pub help: String,
    #[serde(skip)]
    pub action: Action,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arg: Option<Arg>,
    /// Se lance aussi sans sous-commande, bien qu'elle en ait.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub run: bool,
    /// Commande qui n'a pas de sens sur cette page : le message à montrer.
    /// Elle n'est pas suggérée, mais on dit pourquoi si on la tape.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub off: Option<&'static str>,
    /// Action faite par le navigateur seul (`help`, `file`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<&'static str>,
    /// Suppression : le navigateur montre ce texte et attend un second Entrée.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirm: Option<String>,
}

impl Node {
    fn new(name: &'static str, help: impl Into<String>, action: Action) -> Self {
        Self {
            name,
            aliases: &[],
            help: help.into(),
            action,
            children: Vec::new(),
            arg: None,
            run: false,
            off: None,
            client: action.client(),
            confirm: None,
        }
    }

    fn group(name: &'static str, help: &str, children: Vec<Node>) -> Self {
        Self {
            children,
            ..Self::new(name, help, Action::Group)
        }
    }

    fn aliases(mut self, aliases: &'static [&'static str]) -> Self {
        self.aliases = aliases;
        self
    }

    fn arg(mut self, arg: Arg) -> Self {
        self.arg = Some(arg);
        self
    }

    /// Indisponible ici, avec la raison.
    fn off(mut self, why: &'static str) -> Self {
        self.off = Some(why);
        self
    }

    /// Se lance aussi sans sous-commande (`/delete`, qui a `/delete task`).
    fn runnable(mut self) -> Self {
        self.run = true;
        self
    }

    /// Demande un second Entrée, avec ce texte.
    fn confirm(mut self, text: impl Into<String>) -> Self {
        self.confirm = Some(text.into());
        self
    }

    /// Mots qui désignent la commande.
    fn words(&self) -> impl Iterator<Item = &'static str> + '_ {
        std::iter::once(self.name).chain(self.aliases.iter().copied())
    }
}

/// L'argument d'une commande.
#[derive(Debug, Clone, Serialize)]
pub struct Arg {
    /// Aide affichée pendant la saisie : `<titre> [@espace]`.
    pub usage: &'static str,
    pub required: bool,
    pub mode: Mode,
    /// Valeurs proposées (voir [`Mode`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<Choice>,
}

impl Arg {
    fn new(mode: Mode, usage: &'static str, required: bool) -> Self {
        Self {
            usage,
            required,
            mode,
            values: Vec::new(),
        }
    }

    fn values(mut self, values: Vec<Choice>) -> Self {
        self.values = values;
        self
    }
}

/// Façon de compléter un argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Texte libre ; `values` complètent le mot en cours (`@ecole`, `demain`).
    Free,
    /// Un mot parmi `values`.
    Choice,
    /// Toute la fin de la ligne, parmi `values` (le titre d'une tâche).
    Line,
    /// Suggestions cherchées sur le serveur (`GET /cmd/suggest`).
    Remote,
}

/// Une valeur proposée pour un argument.
#[derive(Debug, Clone, Serialize)]
pub struct Choice {
    pub v: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub help: String,
    /// Autres façons de l'écrire (le nom d'un espace renommé…).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub alias: Vec<String>,
}

impl Choice {
    fn new(v: impl Into<String>, help: impl Into<String>) -> Self {
        Self {
            v: v.into(),
            help: help.into(),
            alias: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------
// Lecture d'une ligne
// ---------------------------------------------------------------------

/// Commande trouvée dans une ligne, avec le texte de son argument.
pub struct Resolved<'a> {
    pub node: &'a Node,
    pub arg: &'a str,
}

/// Mot qui ne désigne aucune commande, ou plusieurs.
enum Miss {
    Unknown,
    Ambiguous(Vec<&'static str>),
}

/// La commande de ce niveau que désigne `word` : son nom ou un alias en
/// entier, sinon le seul qui commence par `word`. Une commande indisponible
/// ici ne compte que si aucune autre ne convient (on dira alors pourquoi).
fn find<'a>(level: &'a [Node], word: &str) -> Result<&'a Node, Miss> {
    let word = fold(word);
    if let Some(node) = level.iter().find(|n| n.words().any(|w| fold(w) == word)) {
        return Ok(node);
    }
    let matching: Vec<&Node> = level
        .iter()
        .filter(|n| n.words().any(|w| fold(w).starts_with(&word)))
        .collect();
    let open: Vec<&Node> = matching
        .iter()
        .copied()
        .filter(|n| n.off.is_none())
        .collect();
    match (open.as_slice(), matching.as_slice()) {
        ([node], _) | ([], [node]) => Ok(node),
        ([], []) => Err(Miss::Unknown),
        ([], many) | (many, _) => Err(Miss::Ambiguous(many.iter().map(|n| n.name).collect())),
    }
}

/// Noms des commandes disponibles d'un niveau.
fn names(level: &[Node]) -> String {
    level
        .iter()
        .filter(|n| n.off.is_none())
        .map(|n| n.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Relit une ligne tapée dans la barre : la commande, puis le texte de son
/// argument. Mêmes règles que la complétion du navigateur (`static/js/app.js`).
pub fn resolve<'a>(tree: &'a [Node], line: &'a str) -> Result<Resolved<'a>, String> {
    let Some(mut rest) = line.trim_start().strip_prefix('/') else {
        return Err("Une commande commence par « / ».".into());
    };
    let mut level = tree;
    let mut current: Option<&Node> = None;
    let mut path = String::new();
    while current.is_none_or(|node| !node.children.is_empty()) {
        rest = rest.trim_start();
        let Some(word) = rest.split_whitespace().next() else {
            break;
        };
        let node = find(level, word).map_err(|miss| match miss {
            Miss::Unknown if path.is_empty() => {
                format!("Commande inconnue : /{word}. /help les liste toutes.")
            }
            Miss::Unknown => format!("« {word} » n'existe pas après /{path} : {}.", names(level)),
            Miss::Ambiguous(names) => {
                format!("« {word} » peut être : {}.", names.join(", "))
            }
        })?;
        if let Some(why) = node.off {
            return Err(why.into());
        }
        rest = &rest[word.len()..];
        if !path.is_empty() {
            path.push(' ');
        }
        path.push_str(node.name);
        current = Some(node);
        level = &node.children;
    }
    let Some(node) = current else {
        return Err("Tape une commande après « / », ou /help pour les voir toutes.".into());
    };
    let arg = rest.trim();
    if !node.children.is_empty() && !node.run {
        let next: Vec<String> = node
            .children
            .iter()
            .filter(|c| c.off.is_none())
            .map(|c| format!("/{path} {}", c.name))
            .collect();
        return Err(format!("Il manque la suite : {}.", next.join(", ")));
    }
    match &node.arg {
        Some(spec) if spec.required && arg.is_empty() => {
            Err(format!("Il manque la suite : /{path} {}.", spec.usage))
        }
        None if !arg.is_empty() => Err(format!("Rien n'est attendu après /{path}.")),
        _ => Ok(Resolved { node, arg }),
    }
}

/// Texte libre d'un argument, séparé de ses options : `@ecole` (un espace)
/// et, si `days`, `+demain` (un jour). « Réviser @ecole » -> (« Réviser »,
/// [ecole], []).
fn split_options(arg: &str, days: bool) -> (String, Vec<&str>, Vec<&str>) {
    let (mut words, mut at, mut plus) = (Vec::new(), Vec::new(), Vec::new());
    for word in arg.split_whitespace() {
        match (word.strip_prefix('@'), word.strip_prefix('+')) {
            (Some(space), _) if !space.is_empty() => at.push(space),
            (_, Some(day)) if days && !day.is_empty() => plus.push(day),
            _ => words.push(word),
        }
    }
    (words.join(" "), at, plus)
}

/// Le mot qui désigne chaque espace dans la barre : son nom actuel, écrit
/// comme un mot de commande (« Vie de famille » -> `vie-de-famille`).
///
/// Le slug, fixé à la création et gardé dans les adresses (`/e/perso/`), n'y
/// sert pas : Perso renommé « Général » se tape `general`, et un nouvel
/// espace « Perso » se tape `perso`, même si son slug est `perso-2`. Deux noms
/// qui donneraient le même mot (« École » et « Ecole ») : le second prend son
/// slug, qui est unique.
fn space_words(spaces: &[Space]) -> Vec<(String, &Space)> {
    let mut out: Vec<(String, &Space)> = Vec::with_capacity(spaces.len());
    for space in spaces {
        let word = space_word(&space.name);
        let word = if word.is_empty() || out.iter().any(|(w, _)| *w == word) {
            space.slug.clone()
        } else {
            word
        };
        out.push((word, space));
    }
    out
}

/// L'espace que désigne un mot : son nom en entier (accents, majuscules et
/// espaces ignorés), sinon le seul dont le nom commence par ce mot.
fn find_space<'a>(spaces: &'a [Space], typed: &str) -> Result<&'a Space, String> {
    let typed = space_word(typed);
    let words = space_words(spaces);
    if let Some((_, space)) = words.iter().find(|(w, _)| *w == typed) {
        return Ok(space);
    }
    let matching: Vec<&(String, &Space)> = words
        .iter()
        .filter(|(w, _)| w.starts_with(&typed))
        .collect();
    let all = || {
        words
            .iter()
            .map(|(w, _)| w.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match matching.as_slice() {
        [only] => Ok(only.1),
        [] => Err(format!("Espace inconnu : {typed}. Possible : {}.", all())),
        many => Err(format!(
            "« {typed} » peut être : {}.",
            many.iter()
                .map(|(w, _)| w.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Nom d'un espace écrit comme un mot de commande : « Vie de famille » ->
/// `vie-de-famille`.
fn space_word(name: &str) -> String {
    fold(name).split_whitespace().collect::<Vec<_>>().join("-")
}

/// Valeurs proposées pour choisir un espace (ceux que garde `keep`),
/// préfixées (`@` pour une option), avec leur vrai nom en aide.
fn space_choices(spaces: &[Space], keep: impl Fn(&Space) -> bool, prefix: &str) -> Vec<Choice> {
    space_words(spaces)
        .into_iter()
        .filter(|(_, space)| keep(space))
        .map(|(word, space)| Choice::new(format!("{prefix}{word}"), space.name.clone()))
        .collect()
}

// ---------------------------------------------------------------------
// Jours
// ---------------------------------------------------------------------

const WEEKDAYS: [(&str, Weekday); 7] = [
    ("lundi", Weekday::Mon),
    ("mardi", Weekday::Tue),
    ("mercredi", Weekday::Wed),
    ("jeudi", Weekday::Thu),
    ("vendredi", Weekday::Fri),
    ("samedi", Weekday::Sat),
    ("dimanche", Weekday::Sun),
];

/// Mots de jour proposés par la complétion, préfixés (`+` pour une option).
fn day_choices(prefix: &str) -> Vec<Choice> {
    let mut out = vec![
        Choice::new(format!("{prefix}aujourdhui"), "Aujourd'hui"),
        Choice::new(format!("{prefix}demain"), "Demain"),
    ];
    if prefix.is_empty() {
        out.push(Choice::new("hier", "Hier"));
    }
    out.extend(
        WEEKDAYS
            .iter()
            .map(|(name, _)| Choice::new(format!("{prefix}{name}"), format!("Le prochain {name}"))),
    );
    out
}

/// L'élément que désigne un texte, par son titre : en entier, sinon le seul
/// qui commence par ce texte, sinon le seul qui le contient (accents et
/// majuscules ignorés). Des homonymes ne sont pas une ambiguïté : le premier
/// est pris. `none` commence le message d'échec : « Aucune tâche du jour ».
fn find_by_title<'a, T>(
    items: &'a [T],
    title: impl Fn(&T) -> &str,
    text: &str,
    none: &str,
) -> Result<&'a T, String> {
    let wanted = fold(text.trim());
    let rules: [&dyn Fn(&str) -> bool; 3] = [
        &|t: &str| t == wanted,
        &|t: &str| t.starts_with(&wanted),
        &|t: &str| t.contains(&wanted),
    ];
    for rule in rules {
        let found: Vec<&T> = items.iter().filter(|i| rule(&fold(title(i)))).collect();
        let mut names: Vec<&str> = Vec::new();
        for item in &found {
            let name = title(item);
            if !names.iter().any(|n| fold(n) == fold(name)) {
                names.push(name);
            }
        }
        match names.len() {
            0 => continue,
            1 => return Ok(found[0]),
            _ => {
                return Err(format!(
                    "« {} » peut être : {}.",
                    text.trim(),
                    names.join(", ")
                ));
            }
        }
    }
    Err(format!("{none} ne correspond à « {} ».", text.trim()))
}

/// Mots de jour, avec leur écart à aujourd'hui.
const RELATIVE_DAYS: [(&str, i64); 4] = [
    ("aujourdhui", 0),
    ("demain", 1),
    ("apres-demain", 2),
    ("hier", -1),
];

/// Le jour que désigne un mot : `aujourdhui`, `demain`, `apres-demain`,
/// `hier`, un jour de la semaine (le prochain, jamais aujourd'hui) ; trois
/// lettres suffisent s'il n'y a pas d'ambiguïté. Ou une date : `14/11`
/// (cette année), `14/11/2026`, `2026-11-14`.
pub fn parse_day(word: &str, today: NaiveDate) -> Option<NaiveDate> {
    let word = fold(word).replace('\'', "");
    if word.len() >= 3 {
        let relative: Vec<i64> = RELATIVE_DAYS
            .iter()
            .filter(|(name, _)| name.starts_with(&word) || name.replace('-', "") == word)
            .map(|(_, offset)| *offset)
            .collect();
        let weekday: Vec<Weekday> = WEEKDAYS
            .iter()
            .filter(|(name, _)| name.starts_with(&word))
            .map(|(_, day)| *day)
            .collect();
        match (relative.as_slice(), weekday.as_slice()) {
            ([offset], []) => return today.checked_add_signed(Duration::days(*offset)),
            ([], [day]) => {
                let ahead =
                    (day.num_days_from_monday() + 7 - today.weekday().num_days_from_monday()) % 7;
                let ahead = if ahead == 0 { 7 } else { ahead };
                return today.checked_add_signed(Duration::days(i64::from(ahead)));
            }
            _ => {}
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(&word, "%Y-%m-%d") {
        return Some(date);
    }
    let parts: Vec<&str> = word.split(['/', '.', '-']).collect();
    let number = |s: &str| s.parse::<u32>().ok();
    match parts.as_slice() {
        [d, m] => NaiveDate::from_ymd_opt(today.year(), number(m)?, number(d)?),
        [d, m, y] => {
            let year = i32::try_from(number(y)?).ok()?;
            let year = if year < 100 { 2000 + year } else { year };
            NaiveDate::from_ymd_opt(year, number(m)?, number(d)?)
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------
// Variables : #date, #cours
// ---------------------------------------------------------------------

/// Ce que valent les variables d'un texte au moment de la commande : on tape
/// `/add note #cours du #date`, la note s'appelle « MATHS du 13/11/2026 ».
pub struct Vars {
    /// `#date` : la date du jour, à Paris.
    pub date: String,
    /// `#cours` : la matière du cours qui a lieu maintenant, comme l'écrit le
    /// widget « Prochains cours » (dans Perso, celui de n'importe quel espace).
    pub course: Option<String>,
}

impl Vars {
    pub async fn now(ctx: &Ctx, state: &AppState) -> AppResult<Self> {
        let now = time::now();
        let overview = ctx.active.is_overview();
        let title = sqlx::query_scalar!(
            r#"SELECT e.title FROM calendar_events e
               JOIN calendar_sources s ON s.id = e.source_id
               WHERE s.user_id = ?1 AND (?3 AND s.space_id NOT IN (SELECT id FROM spaces WHERE archived_at IS NOT NULL) OR s.space_id = ?2)
                 AND e.all_day = 0 AND e.starts_at <= ?4 AND e.ends_at > ?4
               ORDER BY e.starts_at
               LIMIT 1"#,
            ctx.user.id,
            ctx.active.id,
            overview,
            now
        )
        .fetch_optional(&state.db)
        .await?;
        Ok(Self {
            date: time::paris_date(now).format("%d/%m/%Y").to_string(),
            course: title
                .map(|t| ical::subject(&t).to_string())
                .filter(|s| !s.is_empty()),
        })
    }

    /// Valeurs proposées par la complétion, avec ce qu'elles donneront.
    fn choices(&self) -> Vec<Choice> {
        vec![
            Choice::new("#date", format!("Date du jour : {}", self.date)),
            Choice::new(
                "#cours",
                match &self.course {
                    Some(course) => format!("Cours en cours : {course}"),
                    None => "Aucun cours en ce moment".into(),
                },
            ),
        ]
    }
}

/// Remplace `#date` et `#cours` dans un texte (majuscules et accents
/// ignorés, ponctuation autour permise). Tout autre `#mot` reste tel quel :
/// « Exercice #3 », un tag. Refusé si `#cours` est demandé hors d'un cours.
pub fn expand(text: &str, vars: &Vars) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('#') {
        let (before, after) = rest.split_at(at);
        out.push_str(before);
        let word: String = after[1..]
            .chars()
            .take_while(|c| c.is_alphanumeric())
            .collect();
        // « a#date » n'est pas une variable : le # doit commencer un mot.
        let starts_word = out.chars().last().is_none_or(|c| !c.is_alphanumeric());
        let value = match fold(&word).as_str() {
            "date" if starts_word => Some(vars.date.clone()),
            "cours" if starts_word => Some(vars.course.clone().ok_or_else(|| {
                "Aucun cours en ce moment : #cours n'a rien à mettre.".to_string()
            })?),
            _ => None,
        };
        match value {
            Some(value) => {
                out.push_str(&value);
                rest = &after[1 + word.len()..];
            }
            None => {
                out.push('#');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    Ok(out)
}

// ---------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CatalogueQuery {
    /// Page ouverte (sans l'espace de l'adresse) : `/files/12`, `/notes/3`…
    page: Option<String>,
}

/// `GET /cmd/catalogue?page=…` : les commandes, pour la complétion.
pub async fn catalogue(
    ctx: Ctx,
    State(state): State<AppState>,
    Query(query): Query<CatalogueQuery>,
) -> AppResult<Response> {
    let page = local_path(query.page.as_deref());
    let opened = catalogue::opened(&ctx, &state, page).await?;
    let commands = catalogue::tree(&ctx, &state, &opened).await?;
    Ok(Json(serde_json::json!({ "commands": commands })).into_response())
}

/// Une note ou un fichier proposé par `/open`.
#[derive(Debug, Serialize)]
pub struct Hit {
    /// Titre (ou nom du fichier), inséré par Tab.
    pub v: String,
    /// « Note · École ».
    pub help: String,
    /// Extrait du texte qui correspond, en texte brut.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sub: String,
    pub href: String,
    /// Espace de l'élément (pour l'ouvrir dans le bon onglet).
    #[serde(skip)]
    pub slug: String,
}

#[derive(Deserialize)]
pub struct SuggestQuery {
    q: Option<String>,
}

/// `GET /cmd/suggest?q=…` : notes et fichiers pour `/open`.
pub async fn suggest(
    ctx: Ctx,
    State(state): State<AppState>,
    Query(query): Query<SuggestQuery>,
) -> AppResult<Response> {
    let items = hits(&ctx, &state, query.q.as_deref().unwrap_or_default()).await?;
    Ok(Json(serde_json::json!({ "items": items })).into_response())
}

/// Notes (recherche plein texte) puis fichiers (nom) qui correspondent, dans
/// tous les espaces de l'utilisateur, archivés compris. Sans texte : les
/// dernières notes modifiées.
pub async fn hits(ctx: &Ctx, state: &AppState, q: &str) -> AppResult<Vec<Hit>> {
    let space_of = |id: i64| {
        ctx.space_by_id(id)
            .map(|s| (s.slug.clone(), s.name.clone()))
            .unwrap_or_default()
    };
    let title = |t: &str| {
        if t.trim().is_empty() {
            "Sans titre".to_string()
        } else {
            t.to_string()
        }
    };
    let mut out = Vec::new();
    let Some(fts) = notes::fts_query(q) else {
        let rows = sqlx::query!(
            r#"SELECT id AS "id!", title, space_id, substr(body_md, 1, 200) AS "head!: String"
               FROM notes WHERE user_id = ? ORDER BY updated_at DESC LIMIT 5"#,
            ctx.user.id
        )
        .fetch_all(&state.db)
        .await?;
        for row in rows {
            let (slug, name) = space_of(row.space_id);
            out.push(Hit {
                v: title(&row.title),
                help: format!("Note · {name}"),
                sub: markdown::excerpt(&row.head, 70),
                href: format!("/notes/{}", row.id),
                slug,
            });
        }
        return Ok(out);
    };

    let rows = sqlx::query!(
        r#"SELECT n.id AS "id!", n.title AS "title!", n.space_id AS "space_id!",
                  snippet(notes_fts, 1, '', '', '…', 10) AS "snippet!: String"
           FROM notes_fts
           JOIN notes n ON n.id = notes_fts.rowid
           WHERE notes_fts MATCH ?1 AND n.user_id = ?2
           ORDER BY bm25(notes_fts, 5.0, 1.0)
           LIMIT 6"#,
        fts,
        ctx.user.id
    )
    .fetch_all(&state.db)
    .await?;
    for row in rows {
        let (slug, name) = space_of(row.space_id);
        out.push(Hit {
            v: title(&row.title),
            help: format!("Note · {name}"),
            sub: markdown::plain(&row.snippet),
            href: format!("/notes/{}", row.id),
            slug,
        });
    }

    // Fichiers : chaque mot tapé doit se trouver dans le nom, accents et
    // majuscules ignorés. Les fichiers de l'onglet Outils (pas encore rangés)
    // n'y sont pas.
    let words: Vec<String> = fold(q).split_whitespace().map(str::to_string).collect();
    let files = sqlx::query!(
        r#"SELECT id AS "id!", name, space_id FROM files
           WHERE user_id = ? AND expires_at IS NULL
           ORDER BY updated_at DESC LIMIT 1000"#,
        ctx.user.id
    )
    .fetch_all(&state.db)
    .await?;
    for file in files
        .into_iter()
        .filter(|f| {
            let name = fold(&f.name);
            words.iter().all(|w| name.contains(w.as_str()))
        })
        .take(4)
    {
        let (slug, name) = space_of(file.space_id);
        out.push(Hit {
            v: file.name,
            help: format!("Fichier · {name}"),
            sub: String::new(),
            href: format!("/files/{}", file.id),
            slug,
        });
    }
    Ok(out)
}

/// Minuscules sans accents, pour comparer « reglages » et « Réglages ».
pub fn fold(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| match c {
            'à' | 'â' | 'ä' | 'á' => 'a',
            'ç' => 'c',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'î' | 'ï' | 'í' => 'i',
            'ô' | 'ö' | 'ó' => 'o',
            'ù' | 'û' | 'ü' | 'ú' => 'u',
            'ÿ' => 'y',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Vec<Node> {
        vec![
            Node::group(
                "go",
                "Aller à une page",
                vec![
                    Node::new("agenda", "Agenda", Action::GoAgenda)
                        .aliases(&["calendar"])
                        .arg(Arg::new(Mode::Free, "[quand]", false)),
                    Node::new("notes", "Notes", Action::GoPage("/notes")),
                ],
            ),
            Node::group(
                "add",
                "Créer",
                vec![
                    Node::new("task", "Tâche", Action::AddTask).arg(Arg::new(
                        Mode::Free,
                        "<titre>",
                        true,
                    )),
                    Node::new("file", "Fichier", Action::AddFile),
                ],
            ),
            Node::new("compress", "Compresser", Action::Compress).off("Sur un fichier."),
            Node::new("convert", "Convertir", Action::Help),
            Node::new("download", "Télécharger", Action::Help).off("Sur un fichier."),
            Node::new("delete", "Supprimer", Action::Help).off("Sur un fichier."),
            Node::new("open", "Ouvrir", Action::Open).arg(Arg::new(Mode::Remote, "<titre>", true)),
            Node::new("space", "Espace", Action::Space),
            Node::new("search", "Chercher", Action::Search),
        ]
    }

    fn action(line: &str) -> Result<(Action, String), String> {
        let tree = tree();
        resolve(&tree, line).map(|r| (r.node.action, r.arg.to_string()))
    }

    #[test]
    fn lecture_des_lignes() {
        assert_eq!(
            action("/go agenda demain").unwrap(),
            (Action::GoAgenda, "demain".into())
        );
        // Un début de mot suffit, majuscules et alias compris.
        assert_eq!(action("/g ag").unwrap().0, Action::GoAgenda);
        assert_eq!(action("/GO Calendar").unwrap().0, Action::GoAgenda);
        assert_eq!(
            action("/add task  Réviser   le DM ").unwrap(),
            (Action::AddTask, "Réviser   le DM".into())
        );
        assert_eq!(action("/o x").unwrap(), (Action::Open, "x".into()));
        // Une commande indisponible ne rend pas l'autre ambiguë…
        assert_eq!(action("/co").unwrap().0, Action::Help);
        // … mais on dit pourquoi si on la tape.
        assert_eq!(action("/compress").unwrap_err(), "Sur un fichier.");
        assert_eq!(
            action("/d").unwrap_err(),
            "« d » peut être : download, delete."
        );
        assert_eq!(
            action("/s").unwrap_err(),
            "« s » peut être : space, search."
        );
        assert!(action("/zz").unwrap_err().contains("inconnue"));
        assert!(action("/go zz").unwrap_err().contains("agenda, notes"));
        assert_eq!(
            action("/add").unwrap_err(),
            "Il manque la suite : /add task, /add file."
        );
        assert!(action("/add task").unwrap_err().contains("<titre>"));
        assert!(
            action("/add file x")
                .unwrap_err()
                .contains("Rien n'est attendu")
        );
        assert!(action("go").is_err());
        assert!(action("/").is_err());
    }

    #[test]
    fn options_d_un_argument() {
        assert_eq!(
            split_options("Réviser  @ecole le DM +demain", true),
            ("Réviser le DM".to_string(), vec!["ecole"], vec!["demain"])
        );
        assert_eq!(split_options("1 +1 @ecole", false).0, "1 +1");
        assert_eq!(split_options("a @ + b", true).0, "a @ + b");
    }

    #[test]
    fn jours() {
        // Vendredi 13 novembre 2026.
        let today = NaiveDate::from_ymd_opt(2026, 11, 13).unwrap();
        let day = |w| parse_day(w, today).map(|d| d.format("%Y-%m-%d").to_string());
        assert_eq!(day("demain").as_deref(), Some("2026-11-14"));
        assert_eq!(day("Aujourd'hui").as_deref(), Some("2026-11-13"));
        assert_eq!(day("hier").as_deref(), Some("2026-11-12"));
        assert_eq!(day("dem").as_deref(), Some("2026-11-14"));
        assert_eq!(day("apresdemain").as_deref(), Some("2026-11-15"));
        assert_eq!(day("mar").as_deref(), Some("2026-11-17"));
        assert_eq!(day("lundi").as_deref(), Some("2026-11-16"));
        // Le prochain vendredi, pas aujourd'hui.
        assert_eq!(day("ven").as_deref(), Some("2026-11-20"));
        assert_eq!(day("25/12").as_deref(), Some("2026-12-25"));
        assert_eq!(day("2/1/27").as_deref(), Some("2027-01-02"));
        assert_eq!(day("2027-03-28").as_deref(), Some("2027-03-28"));
        assert_eq!(day("31/02"), None);
        assert_eq!(day("lu"), None);
        assert_eq!(day("bientot"), None);
    }

    #[test]
    fn comparaison_sans_accents() {
        assert_eq!(fold("Réglages"), "reglages");
        assert_eq!(fold("École"), "ecole");
        assert_eq!(space_word("Vie de  Famille"), "vie-de-famille");
    }

    fn space(id: i64, slug: &str, name: &str) -> Space {
        Space {
            id,
            slug: slug.into(),
            name: name.into(),
            color: "orange".into(),
            icon: "home".into(),
            archived: false,
            auto_mp4: false,
            auto_heic_jpeg: false,
        }
    }

    #[test]
    fn les_espaces_par_leur_nom() {
        // Perso renommé « Général », puis un nouvel espace « Perso » (slug
        // perso-2) : la barre suit les noms, pas les slugs.
        let spaces = [
            space(1, "perso", "Général"),
            space(2, "ecole", "École"),
            space(3, "perso-2", "Perso"),
            space(4, "vie", "Vie de famille"),
        ];
        let id = |typed| find_space(&spaces, typed).map(|s| s.id);
        assert_eq!(id("perso"), Ok(3));
        assert_eq!(id("general"), Ok(1));
        assert_eq!(id("Gén"), Ok(1));
        assert_eq!(id("ECOLE"), Ok(2));
        assert_eq!(id("vie de famille"), Ok(4));
        assert_eq!(id("vie"), Ok(4));
        assert!(id("perso-2").unwrap_err().contains("Espace inconnu"));
        let words: Vec<String> = space_choices(&spaces, |s| s.id != 2, "@")
            .into_iter()
            .map(|c| c.v)
            .collect();
        assert_eq!(words, ["@general", "@perso", "@vie-de-famille"]);
        // Deux noms qui donnent le même mot : le second garde son slug.
        let twins = [space(1, "ecole", "École"), space(2, "ecole-2", "Ecole")];
        assert_eq!(find_space(&twins, "ecole").map(|s| s.id), Ok(1));
        assert_eq!(find_space(&twins, "ecole-2").map(|s| s.id), Ok(2));
    }

    #[test]
    fn variables() {
        let vars = Vars {
            date: "13/11/2026".into(),
            course: Some("MATHS".into()),
        };
        let x = |text| expand(text, &vars);
        assert_eq!(x("#cours du #date").unwrap(), "MATHS du 13/11/2026");
        assert_eq!(
            x("Notes (#Cours), #DATE.").unwrap(),
            "Notes (MATHS), 13/11/2026."
        );
        // Les autres # restent : numéros, tags, mots collés.
        assert_eq!(
            x("Exercice #3 #projet a#date").unwrap(),
            "Exercice #3 #projet a#date"
        );
        assert_eq!(x("#dates # ##date").unwrap(), "#dates # #13/11/2026");
        let nothing = Vars {
            date: "13/11/2026".into(),
            course: None,
        };
        assert!(
            expand("#cours", &nothing)
                .unwrap_err()
                .contains("Aucun cours")
        );
        assert_eq!(expand("sans variable", &nothing).unwrap(), "sans variable");
    }

    #[test]
    fn retrouver_par_le_titre() {
        let tasks = [
            "Réviser le bac",
            "Rendre le DM",
            "Appeler mamie",
            "Rendre le DM",
        ];
        let find = |text| find_by_title(&tasks, |t| t, text, "Aucune tâche").copied();
        assert_eq!(find("appeler mamie"), Ok("Appeler mamie"));
        assert_eq!(find("rev"), Ok("Réviser le bac"));
        assert_eq!(find("mamie"), Ok("Appeler mamie"));
        // Deux tâches du même nom : la première.
        assert_eq!(find("rendre"), Ok("Rendre le DM"));
        assert_eq!(
            find("re"),
            Err("« re » peut être : Réviser le bac, Rendre le DM.".into())
        );
        assert_eq!(
            find("zz"),
            Err("Aucune tâche ne correspond à « zz ».".into())
        );
    }
}
