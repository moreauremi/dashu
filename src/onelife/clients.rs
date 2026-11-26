//! 1Life, module Clients (SPEC, « 1Life — Clients ») : le carnet de clients
//! d'un compte autorisé à 1Life.
//!
//! * Toutes les adresses passent par [`super::enter`] : 404 pour un compte
//!   non autorisé ou sans session, retour à l'accueil de l'espace si 1Life y
//!   est masqué. Les extracteurs qui pourraient échouer (numéro, formulaire)
//!   sont lus après ce contrôle : une erreur 400 ou 422 dirait que l'outil
//!   existe.
//! * Les clients sont au compte (`user_id`), pas à un espace. Celui d'un autre
//!   compte répond 404, même si ce compte est autorisé à 1Life.
//! * Tout est du texte brut, échappé par askama : ni Markdown ni HTML.
//! * Hors de l'API, du connecteur et d'Hermès, comme le reste de 1Life.
//!
//! Dépôt public : aucun vrai nom de client ici ni dans les tests.

use askama::Template;
use axum::extract::rejection::{FormRejection, QueryRejection};
use axum::extract::{Form, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::AppState;
use crate::auth::session::{Ctx, Shell};
use crate::commands::fold;
use crate::time;
use crate::web::{self, AppError, AppResult, see_other};

/// Longueurs maximales, en caractères.
pub const NAME_MAX: usize = 80;
pub const CONTACT_MAX: usize = 80;
pub const EMAIL_MAX: usize = 254;
pub const PHONE_MAX: usize = 30;
pub const NOTES_MAX: usize = 4000;

/// Une fiche client.
#[derive(Debug, Clone)]
pub struct Client {
    pub id: i64,
    pub fields: Fields,
    pub archived: bool,
}

/// Ce qu'on saisit dans une fiche, nettoyé et vérifié par [`Fields::check`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fields {
    pub name: String,
    pub contact: String,
    pub email: String,
    pub phone: String,
    pub notes: String,
}

/// Une ligne de texte : sans caractère de contrôle ni espaces en trop.
fn clean_line(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Un texte libre : retours à la ligne gardés, autres caractères de
/// contrôle retirés, sans espaces en fin.
fn clean_text(raw: &str) -> String {
    raw.replace("\r\n", "\n")
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect::<String>()
        .trim_end()
        .trim_start_matches('\n')
        .to_string()
}

/// Une adresse plausible : une seule `@`, quelque chose de chaque côté, un
/// point dans le domaine, ni espace ni caractère qui casserait un lien.
pub fn valid_email(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains("..")
        && email
            .chars()
            .all(|c| c.is_alphanumeric() || "._%+-@!#$&*/=?^`{|}~".contains(c))
}

/// Un numéro plausible : chiffres, espaces et `+ . - ( )`, six chiffres au
/// moins.
pub fn valid_phone(phone: &str) -> bool {
    phone
        .chars()
        .all(|c| c.is_ascii_digit() || " +.-()".contains(c))
        && phone.chars().filter(char::is_ascii_digit).count() >= 6
}

impl Fields {
    /// Nettoie une saisie et la vérifie : les champs nettoyés (à réafficher
    /// si la saisie est refusée), et le refus à montrer s'il y en a un. Le
    /// nom déjà pris se vérifie à part ([`name_taken`]), contre la base.
    pub fn check(form: &ClientForm) -> (Self, Option<String>) {
        let fields = Self {
            name: clean_line(&form.name),
            contact: clean_line(&form.contact),
            email: clean_line(&form.email),
            phone: clean_line(&form.phone),
            notes: clean_text(&form.notes),
        };
        let long = |text: &str, max: usize| text.chars().count() > max;
        let refusal = if fields.name.is_empty() {
            Some("Donne un nom au client.".to_string())
        } else if long(&fields.name, NAME_MAX) {
            Some(format!("Le nom fait {NAME_MAX} caractères au plus."))
        } else if long(&fields.contact, CONTACT_MAX) {
            Some(format!("Le contact fait {CONTACT_MAX} caractères au plus."))
        } else if !fields.email.is_empty()
            && (long(&fields.email, EMAIL_MAX) || !valid_email(&fields.email))
        {
            Some("Cette adresse e-mail n'a pas l'air valide.".to_string())
        } else if !fields.phone.is_empty()
            && (long(&fields.phone, PHONE_MAX) || !valid_phone(&fields.phone))
        {
            Some(
                "Ce numéro de téléphone n'a pas l'air valide : des chiffres, des espaces et + . - ( ) seulement."
                    .to_string(),
            )
        } else if long(&fields.notes, NOTES_MAX) {
            Some(format!("Les notes font {NOTES_MAX} caractères au plus."))
        } else {
            None
        };
        (fields, refusal)
    }

    /// Lien `mailto:` de l'adresse, si elle est valide.
    pub fn mailto(&self) -> Option<String> {
        valid_email(&self.email).then(|| format!("mailto:{}", self.email))
    }

    /// Lien `tel:` du numéro, si il est valide (chiffres et `+` seulement).
    pub fn tel(&self) -> Option<String> {
        valid_phone(&self.phone).then(|| {
            let digits: String = self
                .phone
                .chars()
                .filter(|c| c.is_ascii_digit() || *c == '+')
                .collect();
            format!("tel:{digits}")
        })
    }
}

// ---------------------------------------------------------------------
// Base
// ---------------------------------------------------------------------

/// Les clients d'un compte, archivés compris, par nom (majuscules et accents
/// mis à part).
pub async fn all(db: &SqlitePool, user: i64) -> AppResult<Vec<Client>> {
    let mut clients: Vec<Client> = sqlx::query!(
        r#"SELECT id AS "id!", name, contact, email, phone, notes, archived AS "archived!: bool"
           FROM onelife_clients WHERE user_id = ?"#,
        user
    )
    .fetch_all(db)
    .await?
    .into_iter()
    .map(|row| Client {
        id: row.id,
        fields: Fields {
            name: row.name,
            contact: row.contact,
            email: row.email,
            phone: row.phone,
            notes: row.notes,
        },
        archived: row.archived,
    })
    .collect();
    clients.sort_by_cached_key(|c| (fold(&c.fields.name), c.id));
    Ok(clients)
}

/// Un client du compte, ou rien (celui d'un autre compte non plus).
pub async fn find(db: &SqlitePool, user: i64, id: i64) -> AppResult<Option<Client>> {
    Ok(all(db, user).await?.into_iter().find(|c| c.id == id))
}

/// Le nom est déjà pris par un autre client du compte, majuscules et accents
/// compris (« Dupont » et « dupont », « Élan » et « Elan »).
pub async fn name_taken(
    db: &SqlitePool,
    user: i64,
    name: &str,
    except: Option<i64>,
) -> AppResult<bool> {
    let wanted = fold(name);
    Ok(all(db, user)
        .await?
        .iter()
        .any(|c| Some(c.id) != except && fold(&c.fields.name) == wanted))
}

/// Le client correspond à une recherche (dans le nom, le contact, l'adresse
/// et le numéro ; majuscules et accents ignorés).
pub fn matches(client: &Client, q: &str) -> bool {
    let q = fold(q.trim());
    if q.is_empty() {
        return true;
    }
    let f = &client.fields;
    [&f.name, &f.contact, &f.email, &f.phone]
        .iter()
        .any(|text| fold(text).contains(&q))
}

// ---------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------

/// Adresse d'une page de Clients dans l'espace actif.
fn url(ctx: &Ctx, rest: &str) -> String {
    format!("/e/{}/1life/clients{rest}", ctx.active.slug)
}

/// Le compte peut entrer (voir [`super::enter`]) ; sinon la réponse à
/// rendre telle quelle.
async fn enter(ctx: Result<Ctx, Response>) -> AppResult<Result<Ctx, Response>> {
    super::enter(ctx).await
}

/// Une ligne de la liste.
pub struct Row {
    pub id: i64,
    pub name: String,
    pub contact: String,
    pub email: String,
    pub phone: String,
}

#[derive(Template)]
#[template(path = "onelife/clients.html")]
struct ListPage {
    shell: Shell,
    rows: Vec<Row>,
    q: String,
    /// La liste des archivés plutôt que celle des actifs.
    archives: bool,
    /// Combien de clients en tout dans l'autre liste (actifs ou archivés).
    other_count: usize,
    /// Combien dans cette liste, recherche mise à part.
    total: usize,
}

#[derive(Deserialize, Default)]
pub struct ListQuery {
    q: Option<String>,
    archives: Option<String>,
}

/// `GET /1life/clients?q=…&archives=1` : la liste, par nom.
pub async fn list(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> AppResult<Response> {
    let ctx = match enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let query = query.map(|Query(q)| q).unwrap_or_default();
    let archives = query.archives.is_some_and(|a| a == "1");
    let q = query.q.unwrap_or_default().trim().to_string();
    let clients = all(&state.db, ctx.user.id).await?;
    let (here, other): (Vec<Client>, Vec<Client>) =
        clients.into_iter().partition(|c| c.archived == archives);
    let total = here.len();
    let rows = here
        .into_iter()
        .filter(|c| matches(c, &q))
        .map(|c| Row {
            id: c.id,
            name: c.fields.name,
            contact: c.fields.contact,
            email: c.fields.email,
            phone: c.fields.phone,
        })
        .collect();
    Ok(web::html(ListPage {
        shell: ctx.shell(super::TOOL).await?,
        rows,
        q,
        archives,
        other_count: other.len(),
        total,
    }))
}

#[derive(Template)]
#[template(path = "onelife/client.html")]
struct ClientPage {
    shell: Shell,
    /// `None` : un nouveau client.
    id: Option<i64>,
    fields: Fields,
    archived: bool,
    error: Option<String>,
    mailto: Option<String>,
    tel: Option<String>,
}

impl ClientPage {
    async fn render(
        ctx: &Ctx,
        id: Option<i64>,
        fields: Fields,
        archived: bool,
        error: Option<String>,
    ) -> AppResult<Response> {
        let status = if error.is_some() {
            StatusCode::UNPROCESSABLE_ENTITY
        } else {
            StatusCode::OK
        };
        let page = Self {
            shell: ctx.shell(super::TOOL).await?,
            id,
            mailto: fields.mailto(),
            tel: fields.tel(),
            fields,
            archived,
            error,
        };
        Ok((status, web::html(page)).into_response())
    }
}

/// `GET /1life/clients/new` : le formulaire d'un nouveau client.
pub async fn new_page(ctx: Result<Ctx, Response>) -> AppResult<Response> {
    let ctx = match enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    ClientPage::render(&ctx, None, Fields::default(), false, None).await
}

/// Champs du formulaire d'une fiche ; tous facultatifs à la lecture, le
/// nom est vérifié ensuite.
#[derive(Deserialize, Default)]
pub struct ClientForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    contact: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    phone: String,
    #[serde(default)]
    notes: String,
}

/// Le formulaire reçu, ou un formulaire vide s'il n'a pas pu être lu (le
/// nom manquant le fera refuser).
fn read_form(form: Result<Form<ClientForm>, FormRejection>) -> ClientForm {
    form.map(|Form(f)| f).unwrap_or_default()
}

/// Un client d'après le numéro de l'adresse ; 404 sinon (numéro illisible,
/// client d'un autre compte ou effacé).
async fn owned(state: &AppState, ctx: &Ctx, id: &str) -> AppResult<Client> {
    let id: i64 = id.parse().map_err(|_| AppError::NotFound)?;
    find(&state.db, ctx.user.id, id)
        .await?
        .ok_or(AppError::NotFound)
}

/// `POST /1life/clients` : crée un client, puis ouvre sa fiche.
pub async fn create(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    form: Result<Form<ClientForm>, FormRejection>,
) -> AppResult<Response> {
    let ctx = match enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let form = read_form(form);
    let (fields, refusal) = Fields::check(&form);
    if let Some(error) = refusal {
        return ClientPage::render(&ctx, None, fields, false, Some(error)).await;
    }
    let user = ctx.user.id;
    if name_taken(&state.db, user, &fields.name, None).await? {
        let error = format!("Un client « {} » existe déjà.", fields.name);
        return ClientPage::render(&ctx, None, fields, false, Some(error)).await;
    }
    let now = time::now();
    let id = sqlx::query!(
        "INSERT INTO onelife_clients (user_id, name, contact, email, phone, notes, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        user,
        fields.name,
        fields.contact,
        fields.email,
        fields.phone,
        fields.notes,
        now,
        now
    )
    .execute(&state.db)
    .await?
    .last_insert_rowid();
    tracing::info!(user, client = id, "1life : client créé");
    ctx.flash_ok(format!("Client « {} » créé", fields.name))
        .await?;
    Ok(see_other(&url(&ctx, &format!("/{id}"))))
}

/// `GET /1life/clients/{id}` : la fiche, en formulaire.
pub async fn show(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Response> {
    let ctx = match enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let client = owned(&state, &ctx, &id).await?;
    ClientPage::render(&ctx, Some(client.id), client.fields, client.archived, None).await
}

/// `POST /1life/clients/{id}` : enregistre la fiche.
pub async fn update(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    form: Result<Form<ClientForm>, FormRejection>,
) -> AppResult<Response> {
    let ctx = match enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let client = owned(&state, &ctx, &id).await?;
    let form = read_form(form);
    let (fields, refusal) = Fields::check(&form);
    if let Some(error) = refusal {
        return ClientPage::render(&ctx, Some(client.id), fields, client.archived, Some(error))
            .await;
    }
    let user = ctx.user.id;
    if name_taken(&state.db, user, &fields.name, Some(client.id)).await? {
        let error = format!("Un client « {} » existe déjà.", fields.name);
        return ClientPage::render(&ctx, Some(client.id), fields, client.archived, Some(error))
            .await;
    }
    let now = time::now();
    sqlx::query!(
        "UPDATE onelife_clients SET name = ?, contact = ?, email = ?, phone = ?, notes = ?, updated_at = ?
         WHERE id = ? AND user_id = ?",
        fields.name,
        fields.contact,
        fields.email,
        fields.phone,
        fields.notes,
        now,
        client.id,
        user
    )
    .execute(&state.db)
    .await?;
    ctx.flash_ok("Fiche enregistrée").await?;
    Ok(see_other(&url(&ctx, &format!("/{}", client.id))))
}

#[derive(Deserialize, Default)]
pub struct ArchiveForm {
    /// `1` : archiver ; `0` : remettre dans la liste.
    #[serde(default)]
    archived: String,
}

/// `POST /1life/clients/{id}/archive` : archive le client (il quitte la
/// liste sans être effacé) ou le remet dans la liste.
pub async fn archive(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    form: Result<Form<ArchiveForm>, FormRejection>,
) -> AppResult<Response> {
    let ctx = match enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let client = owned(&state, &ctx, &id).await?;
    let archived = form.map(|Form(f)| f).unwrap_or_default().archived == "1";
    let user = ctx.user.id;
    let now = time::now();
    sqlx::query!(
        "UPDATE onelife_clients SET archived = ?, updated_at = ? WHERE id = ? AND user_id = ?",
        archived,
        now,
        client.id,
        user
    )
    .execute(&state.db)
    .await?;
    let name = &client.fields.name;
    if archived {
        ctx.flash_ok(format!("Client « {name} » archivé")).await?;
        Ok(see_other(&url(&ctx, "")))
    } else {
        ctx.flash_ok(format!("Client « {name} » remis dans la liste"))
            .await?;
        Ok(see_other(&url(&ctx, &format!("/{}", client.id))))
    }
}

/// `POST /1life/clients/{id}/delete` : efface la fiche pour de bon (le
/// formulaire demande d'abord une confirmation).
pub async fn delete(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> AppResult<Response> {
    let ctx = match enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let client = owned(&state, &ctx, &id).await?;
    let user = ctx.user.id;
    sqlx::query!(
        "DELETE FROM onelife_clients WHERE id = ? AND user_id = ?",
        client.id,
        user
    )
    .execute(&state.db)
    .await?;
    tracing::info!(user, client = client.id, "1life : client supprimé");
    ctx.flash_ok(format!("Client « {} » supprimé", client.fields.name))
        .await?;
    Ok(see_other(&url(&ctx, "")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(name: &str, email: &str, phone: &str) -> ClientForm {
        ClientForm {
            name: name.into(),
            email: email.into(),
            phone: phone.into(),
            ..ClientForm::default()
        }
    }

    #[test]
    fn adresses_et_numeros() {
        for ok in ["contact@exemple.test", "a.b+c@sous.exemple.test"] {
            assert!(valid_email(ok), "{ok}");
        }
        for bad in [
            "exemple.test",
            "@exemple.test",
            "a@b",
            "a@.test",
            "a@test.",
            "a@@b.test",
            "a b@c.test",
            "a\"@b.test",
            "<a@b.test>",
        ] {
            assert!(!valid_email(bad), "{bad}");
        }
        assert!(valid_phone("+33 6 12 34 56 78"));
        assert!(valid_phone("(01) 23.45-67.89"));
        assert!(!valid_phone("12345"));
        assert!(!valid_phone("06 12 34 56 7a"));
    }

    #[test]
    fn saisie_nettoyee_et_verifiee() {
        let mut f = form("  Société   Exemple \t", "", "");
        f.notes = "\r\nLigne 1\r\n\r\nLigne 2\u{7}  \n".into();
        let (fields, refusal) = Fields::check(&f);
        assert_eq!(refusal, None);
        assert_eq!(fields.name, "Société Exemple");
        assert_eq!(fields.notes, "Ligne 1\n\nLigne 2");
        for (bad, message) in [
            (form(" ", "", ""), "Donne un nom"),
            (form(&"x".repeat(NAME_MAX + 1), "", ""), "80 caractères"),
            (form("A", "pas-une-adresse", ""), "e-mail"),
            (form("A", "", "abc"), "téléphone"),
        ] {
            let (_, refusal) = Fields::check(&bad);
            let refusal = refusal.expect(message);
            assert!(refusal.contains(message), "{refusal}");
        }
    }

    #[test]
    fn liens_et_recherche() {
        let fields = Fields {
            name: "Atelier Élan".into(),
            email: "bonjour@exemple.test".into(),
            phone: "+33 1 23 45 67 89".into(),
            ..Fields::default()
        };
        assert_eq!(
            fields.mailto().as_deref(),
            Some("mailto:bonjour@exemple.test")
        );
        assert_eq!(fields.tel().as_deref(), Some("tel:+33123456789"));
        let client = Client {
            id: 1,
            fields,
            archived: false,
        };
        assert!(matches(&client, "elan"));
        assert!(matches(&client, "ATELIER"));
        assert!(matches(&client, "45 67"));
        assert!(!matches(&client, "dupont"));
        assert!(Fields::default().mailto().is_none() && Fields::default().tel().is_none());
    }
}
