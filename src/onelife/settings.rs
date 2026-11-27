//! 1Life, page « Réglages » (SPEC, « 1Life — Fiche client et réglages ») :
//! le consultant, l'approbateur par défaut, le dossier des modèles d'audit,
//! le modèle par défaut et le modèle d'IA par défaut. Un compte, une ligne
//! de `onelife_settings` (la même que le dossier des clients).

use askama::Template;
use axum::extract::rejection::FormRejection;
use axum::extract::{Form, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::AppState;
use crate::auth::session::{Ctx, Shell, User};
use crate::folders;
use crate::time;
use crate::web::{self, AppError, AppResult, see_other};

use super::templates::{self, BUILTIN_KEY, Choice};

/// Longueur maximale d'un réglage en texte.
pub const TEXT_MAX: usize = 200;

/// Les réglages de 1Life d'un compte. Un texte vide : la valeur par défaut.
#[derive(Debug, Default, Clone)]
pub struct Settings {
    pub consultant_name: String,
    pub consultant_role: String,
    pub consultant_email: String,
    pub approver: String,
    pub templates_folder_id: Option<i64>,
    /// `integre`, `note:<id>` ou `fichier:<id>` ; vide : le modèle intégré.
    pub default_template: String,
    /// Un modèle de `ONELIFE_AI_MODELS` ; vide : le premier.
    pub ai_model: String,
}

impl Settings {
    /// Le nom du consultant : celui des réglages, sinon le prénom et le nom
    /// du profil.
    pub fn consultant(&self, user: &User) -> String {
        let name = self.consultant_name.trim();
        if name.is_empty() {
            format!("{} {}", user.first_name.trim(), user.last_name.trim())
                .trim()
                .to_string()
        } else {
            name.to_string()
        }
    }

    /// Le modèle par défaut.
    pub fn template(&self) -> &str {
        if self.default_template.is_empty() {
            BUILTIN_KEY
        } else {
            &self.default_template
        }
    }

    /// Le modèle d'IA par défaut, s'il est proposé ; sinon le premier.
    pub fn model<'a>(&self, models: &'a [String]) -> &'a str {
        models
            .iter()
            .find(|m| **m == self.ai_model)
            .or(models.first())
            .map_or("", String::as_str)
    }
}

/// Les réglages d'un compte (ceux par défaut s'il n'en a pas).
pub async fn load(db: &SqlitePool, user: i64) -> AppResult<Settings> {
    let row = sqlx::query!(
        "SELECT consultant_name, consultant_role, consultant_email, approver,
                templates_folder_id, default_template, ai_model
         FROM onelife_settings WHERE user_id = ?",
        user
    )
    .fetch_optional(db)
    .await?;
    Ok(row
        .map(|r| Settings {
            consultant_name: r.consultant_name,
            consultant_role: r.consultant_role,
            consultant_email: r.consultant_email,
            approver: r.approver,
            templates_folder_id: r.templates_folder_id,
            default_template: r.default_template,
            ai_model: r.ai_model,
        })
        .unwrap_or_default())
}

/// Adresse d'une page de 1Life dans l'espace actif.
fn url(ctx: &Ctx, rest: &str) -> String {
    format!("/e/{}/1life{rest}", ctx.active.slug)
}

/// Un modèle d'IA proposé.
pub struct ModelChoice {
    pub id: String,
    pub label: String,
    pub selected: bool,
}

#[derive(Template)]
#[template(path = "onelife/settings.html")]
struct SettingsPage {
    shell: Shell,
    home: String,
    action: String,
    settings: Settings,
    /// Le nom du profil, montré quand le nom du consultant est vide.
    profile_name: String,
    /// « Taff / Modèles », si le dossier des modèles est choisi et accessible.
    folder_label: Option<String>,
    /// Choisi, mais plus accessible (corbeille, espace quitté).
    folder_gone: bool,
    picker: String,
    templates: Vec<Choice>,
    template: String,
    models: Vec<ModelChoice>,
    error: Option<String>,
    max: usize,
}

/// La page, pour [`page`] et pour un enregistrement refusé.
async fn render(
    state: &AppState,
    ctx: &Ctx,
    settings: Settings,
    error: Option<String>,
) -> AppResult<Response> {
    let folder = templates::folder(state, ctx, &settings).await?;
    let models = &state.config.ai.models;
    let chosen = settings.model(models).to_string();
    let templates = templates::choices(state, ctx, &settings).await?;
    let template = settings.template().to_string();
    let page = SettingsPage {
        shell: ctx.shell(super::TOOL).await?,
        home: url(ctx, ""),
        action: url(ctx, "/reglages"),
        profile_name: Settings::default().consultant(&ctx.user),
        folder_gone: settings.templates_folder_id.is_some() && folder.is_none(),
        folder_label: folder.map(|(_, _, label)| label),
        picker: PICKER.into(),
        templates,
        template,
        models: models
            .iter()
            .map(|m| ModelChoice {
                id: m.clone(),
                label: super::generation::model_label(m),
                selected: *m == chosen,
            })
            .collect(),
        settings,
        max: TEXT_MAX,
        error: error.clone(),
    };
    let status = if error.is_some() {
        StatusCode::UNPROCESSABLE_ENTITY
    } else {
        StatusCode::OK
    };
    Ok((status, web::html(page)).into_response())
}

/// La fenêtre « Ranger » en mode « dossier des modèles ».
const PICKER: &str = "/destinations?modeles=1";

/// `GET /1life/reglages`
pub async fn page(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
) -> AppResult<Response> {
    let ctx = match super::enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let settings = load(&state.db, ctx.user.id).await?;
    render(&state, &ctx, settings, None).await
}

#[derive(Deserialize, Default)]
pub struct SettingsForm {
    #[serde(default)]
    consultant_name: String,
    #[serde(default)]
    consultant_role: String,
    #[serde(default)]
    consultant_email: String,
    #[serde(default)]
    approver: String,
    #[serde(default)]
    template: String,
    #[serde(default)]
    ai_model: String,
}

/// `POST /1life/reglages` : enregistre les réglages (le dossier des modèles
/// se choisit à part, par la fenêtre « Ranger »).
pub async fn save(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    form: Result<Form<SettingsForm>, FormRejection>,
) -> AppResult<Response> {
    let ctx = match super::enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let form = form.map(|Form(f)| f).unwrap_or_default();
    let user = ctx.user.id;
    let mut settings = load(&state.db, user).await?;
    settings.consultant_name = form.consultant_name.trim().to_string();
    settings.consultant_role = form.consultant_role.trim().to_string();
    settings.consultant_email = form.consultant_email.trim().to_string();
    settings.approver = form.approver.trim().to_string();
    let too_long = [
        &settings.consultant_name,
        &settings.consultant_role,
        &settings.consultant_email,
        &settings.approver,
    ]
    .iter()
    .any(|v| v.chars().count() > TEXT_MAX);
    if too_long {
        let error = format!("{TEXT_MAX} caractères au plus par champ : rien n'est enregistré.");
        return render(&state, &ctx, settings, Some(error)).await;
    }
    // Seulement un modèle proposé, et un modèle d'IA de la liste.
    let template = form.template.trim();
    settings.default_template = if templates::choices(&state, &ctx, &settings)
        .await?
        .iter()
        .any(|c| c.key == template && c.key != BUILTIN_KEY)
    {
        template.to_string()
    } else {
        String::new()
    };
    let model = form.ai_model.trim();
    settings.ai_model = if state.config.ai.models.iter().any(|m| m == model) {
        model.to_string()
    } else {
        String::new()
    };
    let now = time::now();
    sqlx::query!(
        "INSERT INTO onelife_settings (user_id, updated_at, consultant_name, consultant_role,
                                       consultant_email, approver, default_template, ai_model)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (user_id) DO UPDATE SET updated_at = excluded.updated_at,
             consultant_name = excluded.consultant_name,
             consultant_role = excluded.consultant_role,
             consultant_email = excluded.consultant_email,
             approver = excluded.approver,
             default_template = excluded.default_template,
             ai_model = excluded.ai_model",
        user,
        now,
        settings.consultant_name,
        settings.consultant_role,
        settings.consultant_email,
        settings.approver,
        settings.default_template,
        settings.ai_model
    )
    .execute(&state.db)
    .await?;
    ctx.flash_ok("Réglages de 1Life enregistrés").await?;
    Ok(see_other(&url(&ctx, "/reglages")))
}

#[derive(Deserialize, Default)]
pub struct FolderForm {
    #[serde(default)]
    space_id: String,
    #[serde(default)]
    folder_id: String,
}

/// `POST /1life/reglages/modeles` : enregistre le dossier des modèles choisi
/// dans la fenêtre « Ranger » (un dossier, à tout niveau ; un espace ne se
/// choisit pas).
pub async fn choose_folder(
    ctx: Result<Ctx, Response>,
    State(state): State<AppState>,
    form: Result<Form<FolderForm>, FormRejection>,
) -> AppResult<Response> {
    let ctx = match super::enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let form = form.map(|Form(f)| f).unwrap_or_default();
    let user = ctx.user.id;
    let back = see_other(&url(&ctx, "/reglages"));
    let Ok(id) = form.folder_id.trim().parse::<i64>() else {
        ctx.flash_error("Choisis un dossier : un espace ne peut pas être le dossier des modèles.")
            .await?;
        return Ok(back);
    };
    let all = folders::all(&state.db, user).await?;
    let space: Option<i64> = form.space_id.trim().parse().ok();
    let folder = all
        .iter()
        .find(|f| f.id == id)
        .filter(|f| space.is_none_or(|s| s == f.space_id))
        .ok_or(AppError::NotFound)?;
    let now = time::now();
    sqlx::query!(
        "INSERT INTO onelife_settings (user_id, updated_at, templates_folder_id) VALUES (?, ?, ?)
         ON CONFLICT (user_id) DO UPDATE SET templates_folder_id = excluded.templates_folder_id,
                                             updated_at = excluded.updated_at",
        user,
        now,
        folder.id
    )
    .execute(&state.db)
    .await?;
    let space = ctx
        .space_by_id(folder.space_id)
        .map(|s| s.name.clone())
        .unwrap_or_default();
    ctx.flash_ok(format!(
        "Dossier des modèles : {}",
        folders::label(&space, &all, Some(folder.id))
    ))
    .await?;
    Ok(back)
}

/// Le choix actuel du dossier des modèles, pour la fenêtre « Ranger ».
pub(crate) async fn chosen_folder(db: &SqlitePool, user: i64) -> AppResult<Option<i64>> {
    Ok(load(db, user).await?.templates_folder_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modele_d_ia_par_defaut() {
        let models = vec![
            "claude-haiku-5-5".to_string(),
            "claude-sonnet-5-5".to_string(),
        ];
        let mut settings = Settings::default();
        assert_eq!(settings.model(&models), "claude-haiku-5-5");
        settings.ai_model = "claude-sonnet-5-5".into();
        assert_eq!(settings.model(&models), "claude-sonnet-5-5");
        // Un modèle retiré de ONELIFE_AI_MODELS : le premier.
        settings.ai_model = "claude-retire".into();
        assert_eq!(settings.model(&models), "claude-haiku-5-5");
        assert_eq!(settings.template(), BUILTIN_KEY);
    }
}
