//! API, lot « espaces » (SPEC, « API complète et corbeille ») : les espaces,
//! leurs membres et invitations, le tableau de bord de chaque espace et les
//! réglages du compte. Mêmes fonctions que le site, donc mêmes règles : seul
//! le propriétaire gère un espace (403 pour un membre), 8 espaces au plus,
//! Perso ne s'archive, ne se supprime ni ne se partage.
//!
//! Un espace se désigne par son slug. Dans l'adresse, un slug inconnu (ou
//! l'espace d'un autre compte) répond 404.
//!
//! Rien ici ne touche à ce qui protège le compte : ni mot de passe, ni
//! liaison Google, ni jetons (`SettingsChanges` refuse tout champ inconnu).

use axum::body::Bytes;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use super::{ApiError, ApiResult, ApiUser, contenu};
use crate::AppState;
use crate::dashboard::layout;
use crate::settings::{self, profile};
use crate::spaces::{self, Person, Space, SpaceForm};
use crate::tabbar;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/spaces", get(list_spaces).post(create_space))
        .route(
            "/spaces/{slug}",
            get(read_space).patch(update_space).delete(delete_space),
        )
        .route("/spaces/{slug}/archive", post(archive_space))
        .route("/spaces/{slug}/unarchive", post(unarchive_space))
        .route(
            "/spaces/{slug}/dashboard",
            get(read_dashboard).put(write_dashboard),
        )
        .route("/spaces/{slug}/members", get(list_members))
        .route("/spaces/{slug}/members/{user}", delete(remove_member))
        .route("/spaces/{slug}/invitations", post(invite))
        .route("/spaces/{slug}/invitations/{id}", delete(cancel_invitation))
        .route("/spaces/{slug}/leave", post(leave))
        .route("/invitations", get(list_invitations))
        .route("/invitations/{id}/accept", post(accept_invitation))
        .route("/invitations/{id}/decline", post(decline_invitation))
        .route("/settings", get(read_settings).patch(update_settings))
        .route(
            "/settings/avatar",
            get(read_avatar).put(put_avatar).delete(delete_avatar),
        )
}

impl ApiUser {
    /// Espace désigné dans l'adresse : inconnu ou d'un autre compte, 404.
    fn space_at(&self, slug: &str) -> ApiResult<&Space> {
        self.space(slug).map_err(|_| ApiError::not_found("espace"))
    }
}

// ---------------------------------------------------------------------
// Espaces
// ---------------------------------------------------------------------

#[derive(Serialize)]
pub(crate) struct SpaceOut {
    slug: String,
    name: String,
    color: String,
    icon: String,
    archived: bool,
    /// Vrai si le compte en est le propriétaire ; faux pour un espace rejoint.
    mine: bool,
    /// Règle : une vidéo qui arrive est convertie en MP4.
    auto_mp4: bool,
    /// Règle : une photo HEIC qui arrive est doublée d'un JPEG.
    auto_heic_jpeg: bool,
}

fn space_out(space: &Space) -> SpaceOut {
    SpaceOut {
        slug: space.slug.clone(),
        name: space.name.clone(),
        color: space.color.clone(),
        icon: space.icon().to_string(),
        archived: space.archived,
        mine: space.mine,
        auto_mp4: space.auto_mp4,
        auto_heic_jpeg: space.auto_heic_jpeg,
    }
}

pub(crate) fn spaces_out(user: &ApiUser) -> Vec<SpaceOut> {
    user.spaces.iter().map(space_out).collect()
}

/// L'espace tel qu'il est après une modification.
async fn reread(
    state: &AppState,
    user: &ApiUser,
    wanted: impl Fn(&Space) -> bool,
) -> ApiResult<SpaceOut> {
    spaces::list(&state.db, user.id)
        .await?
        .iter()
        .find(|s| wanted(s))
        .map(space_out)
        .ok_or_else(|| ApiError::not_found("espace"))
}

/// `GET /api/v1/spaces`
async fn list_spaces(user: ApiUser) -> Json<Vec<SpaceOut>> {
    Json(spaces_out(&user))
}

/// `GET /api/v1/spaces/{slug}`
async fn read_space(user: ApiUser, Path(slug): Path<String>) -> ApiResult<Json<SpaceOut>> {
    Ok(Json(space_out(user.space_at(&slug)?)))
}

#[derive(Deserialize)]
pub(crate) struct NewSpace {
    name: String,
    /// Absentes : les premières qu'aucun espace n'utilise, comme sur le site.
    color: Option<String>,
    icon: Option<String>,
}

/// `POST /api/v1/spaces` `{"name", "color"?, "icon"?}`
async fn create_space(
    user: ApiUser,
    State(state): State<AppState>,
    payload: Result<Json<NewSpace>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<SpaceOut>)> {
    let Json(new) = payload?;
    Ok((
        StatusCode::CREATED,
        Json(space_create(&state, &user, new).await?),
    ))
}

pub(crate) async fn space_create(
    state: &AppState,
    user: &ApiUser,
    new: NewSpace,
) -> ApiResult<SpaceOut> {
    let in_service: Vec<Space> = user
        .spaces
        .iter()
        .filter(|s| !s.archived)
        .cloned()
        .collect();
    let (color, icon) = spaces::suggestion(&in_service);
    let form = SpaceForm {
        name: new.name,
        color: new.color.unwrap_or_else(|| color.to_string()),
        icon: new.icon.unwrap_or_else(|| icon.to_string()),
        rules: None,
        auto_mp4: None,
        auto_heic_jpeg: None,
    };
    let (slug, _) = spaces::create_space(&state.db, user.id, &user.spaces, &form)
        .await?
        .map_err(ApiError::bad_request)?;
    reread(state, user, |s| s.slug == slug).await
}

#[derive(Deserialize, Default)]
pub(crate) struct SpaceChanges {
    name: Option<String>,
    color: Option<String>,
    icon: Option<String>,
    auto_mp4: Option<bool>,
    auto_heic_jpeg: Option<bool>,
}

/// `PATCH /api/v1/spaces/{slug}` : les champs donnés changent, les autres
/// restent. Le slug ne change pas quand on renomme.
async fn update_space(
    user: ApiUser,
    State(state): State<AppState>,
    Path(slug): Path<String>,
    payload: Result<Json<SpaceChanges>, JsonRejection>,
) -> ApiResult<Json<SpaceOut>> {
    let Json(changes) = payload?;
    Ok(Json(space_update(&state, &user, &slug, changes).await?))
}

pub(crate) async fn space_update(
    state: &AppState,
    user: &ApiUser,
    slug: &str,
    changes: SpaceChanges,
) -> ApiResult<SpaceOut> {
    let space = user.space_at(slug)?;
    let rules = changes.auto_mp4.is_some() || changes.auto_heic_jpeg.is_some();
    let on = |rule: bool| rule.then(|| "on".to_string());
    let form = SpaceForm {
        name: changes.name.unwrap_or_else(|| space.name.clone()),
        color: changes.color.unwrap_or_else(|| space.color.clone()),
        icon: changes.icon.unwrap_or_else(|| space.icon().to_string()),
        rules: rules.then(|| "1".to_string()),
        auto_mp4: on(changes.auto_mp4.unwrap_or(space.auto_mp4)),
        auto_heic_jpeg: on(changes.auto_heic_jpeg.unwrap_or(space.auto_heic_jpeg)),
    };
    spaces::update_space(&state.db, user.id, &user.spaces, space, &form)
        .await?
        .map_err(ApiError::bad_request)?;
    reread(state, user, |s| s.id == space.id).await
}

/// `POST /api/v1/spaces/{slug}/archive`
async fn archive_space(
    user: ApiUser,
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> ApiResult<Json<SpaceOut>> {
    Ok(Json(space_archive(&state, &user, &slug).await?))
}

pub(crate) async fn space_archive(
    state: &AppState,
    user: &ApiUser,
    slug: &str,
) -> ApiResult<SpaceOut> {
    let space = user.space_at(slug)?;
    spaces::archive_space(&state.db, user.id, &user.spaces, space)
        .await?
        .map_err(ApiError::bad_request)?;
    reread(state, user, |s| s.id == space.id).await
}

/// `POST /api/v1/spaces/{slug}/unarchive`
async fn unarchive_space(
    user: ApiUser,
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> ApiResult<Json<SpaceOut>> {
    Ok(Json(space_unarchive(&state, &user, &slug).await?))
}

pub(crate) async fn space_unarchive(
    state: &AppState,
    user: &ApiUser,
    slug: &str,
) -> ApiResult<SpaceOut> {
    let space = user.space_at(slug)?;
    spaces::unarchive_space(&state.db, user.id, &user.spaces, space)
        .await?
        .map_err(ApiError::bad_request)?;
    reread(state, user, |s| s.id == space.id).await
}

/// Que faire du contenu d'un espace supprimé : l'un ou l'autre, jamais par
/// défaut.
#[derive(Deserialize, Default)]
pub(crate) struct SpaceRemoval {
    /// `trash` : le contenu part dans la corbeille avec l'espace.
    content: Option<String>,
    /// Le contenu passe dans cet espace du propriétaire.
    transfer_to: Option<String>,
}

/// `DELETE /api/v1/spaces/{slug}?content=trash` ou `?transfer_to=taff`
async fn delete_space(
    user: ApiUser,
    State(state): State<AppState>,
    Path(slug): Path<String>,
    query: Result<Query<SpaceRemoval>, QueryRejection>,
) -> ApiResult<StatusCode> {
    let Query(how) = query?;
    space_delete(&state, &user, &slug, how).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn space_delete(
    state: &AppState,
    user: &ApiUser,
    slug: &str,
    how: SpaceRemoval,
) -> ApiResult<()> {
    let space = user.space_at(slug)?;
    // Avant de lire les choix : un membre reçoit 403, quels qu'ils soient.
    spaces::owner_only(space)?;
    let transfer = how
        .transfer_to
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let (content, target) = match (how.content.as_deref(), transfer) {
        (Some("trash"), None) => ("delete", None),
        (None, Some(to)) => ("move", Some(user.space(to)?.id)),
        _ => {
            return Err(ApiError::bad_request(
                "dis quoi faire du contenu : content=trash (il part dans la corbeille) ou transfer_to=<espace>",
            ));
        }
    };
    spaces::delete_space(&state.db, user.who(), &user.spaces, space, content, target)
        .await?
        .map_err(ApiError::bad_request)?;
    Ok(())
}

// ---------------------------------------------------------------------
// Tableau de bord
// ---------------------------------------------------------------------

#[derive(Serialize)]
pub(crate) struct WidgetOut {
    widget: &'static str,
    title: &'static str,
    w: u8,
    h: u8,
}

#[derive(Serialize)]
pub(crate) struct DashboardOut {
    space: String,
    /// La disposition en une ligne : `calendar:2x3,tasks:2x2`.
    layout: String,
    widgets: Vec<WidgetOut>,
    /// Les widgets qui existent.
    available: Vec<&'static str>,
}

/// `GET /api/v1/spaces/{slug}/dashboard`
async fn read_dashboard(
    user: ApiUser,
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> ApiResult<Json<DashboardOut>> {
    Ok(Json(dashboard_read(&state, &user, &slug).await?))
}

pub(crate) async fn dashboard_read(
    state: &AppState,
    user: &ApiUser,
    slug: &str,
) -> ApiResult<DashboardOut> {
    let space = user.space_at(slug)?;
    let files = state.config.files.is_some();
    let widgets = layout::load(&state.db, space.id, files).await?;
    Ok(DashboardOut {
        space: space.slug.clone(),
        layout: layout::encode(&widgets),
        widgets: widgets
            .iter()
            .map(|w| WidgetOut {
                widget: w.kind.key,
                title: w.kind.title,
                w: w.w,
                h: w.h,
            })
            .collect(),
        available: layout::KINDS
            .iter()
            .map(|k| k.key)
            .filter(|key| files || *key != "files")
            .collect(),
    })
}

#[derive(Deserialize)]
pub(crate) struct DashboardBody {
    /// `calendar:2x3,tasks:2x2` ; vide : aucun widget ; `null` : la
    /// disposition par défaut. Obligatoire : un corps vide n'efface rien.
    #[serde(default, deserialize_with = "contenu::some")]
    layout: Option<Option<String>>,
}

/// `PUT /api/v1/spaces/{slug}/dashboard` `{"layout": "calendar:2x3,tasks:2x2"}`
async fn write_dashboard(
    user: ApiUser,
    State(state): State<AppState>,
    Path(slug): Path<String>,
    payload: Result<Json<DashboardBody>, JsonRejection>,
) -> ApiResult<Json<DashboardOut>> {
    let Json(body) = payload?;
    Ok(Json(dashboard_write(&state, &user, &slug, body).await?))
}

pub(crate) async fn dashboard_write(
    state: &AppState,
    user: &ApiUser,
    slug: &str,
    body: DashboardBody,
) -> ApiResult<DashboardOut> {
    let space = user.space_at(slug)?;
    let wanted = body
        .layout
        .ok_or_else(|| ApiError::bad_request("layout est obligatoire (null : par défaut)"))?;
    match wanted {
        Some(text) => {
            let widgets = layout::parse(&text).map_err(ApiError::bad_request)?;
            layout::save(&state.db, user.id, space.id, Some(&widgets)).await?;
        }
        None => layout::save(&state.db, user.id, space.id, None).await?,
    }
    dashboard_read(state, user, slug).await
}

// ---------------------------------------------------------------------
// Membres et invitations
// ---------------------------------------------------------------------

#[derive(Serialize)]
pub(crate) struct PendingOut {
    id: i64,
    person: Person,
}

#[derive(Serialize)]
pub(crate) struct MembersOut {
    space: String,
    owner: Person,
    members: Vec<Person>,
    /// Invitations en attente : vide pour un membre, elles ne regardent que
    /// le propriétaire.
    invitations: Vec<PendingOut>,
}

/// `GET /api/v1/spaces/{slug}/members`
async fn list_members(
    user: ApiUser,
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> ApiResult<Json<MembersOut>> {
    Ok(Json(members_list(&state, &user, &slug).await?))
}

pub(crate) async fn members_list(
    state: &AppState,
    user: &ApiUser,
    slug: &str,
) -> ApiResult<MembersOut> {
    let space = user.space_at(slug)?;
    let roster = spaces::roster(&state.db, space).await?;
    Ok(MembersOut {
        space: space.slug.clone(),
        owner: roster.owner,
        members: roster.members,
        invitations: roster
            .pending
            .into_iter()
            .map(|(id, person)| PendingOut { id, person })
            .collect(),
    })
}

#[derive(Deserialize)]
pub(crate) struct NewInvitation {
    pub(crate) email: String,
}

#[derive(Serialize)]
pub(crate) struct InvitationOut {
    id: i64,
    email: String,
}

/// `POST /api/v1/spaces/{slug}/invitations` `{"email"}` : un compte existant.
async fn invite(
    user: ApiUser,
    State(state): State<AppState>,
    Path(slug): Path<String>,
    payload: Result<Json<NewInvitation>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<InvitationOut>)> {
    let Json(new) = payload?;
    Ok((
        StatusCode::CREATED,
        Json(member_invite(&state, &user, &slug, &new.email).await?),
    ))
}

pub(crate) async fn member_invite(
    state: &AppState,
    user: &ApiUser,
    slug: &str,
    email: &str,
) -> ApiResult<InvitationOut> {
    let space = user.space_at(slug)?;
    let (id, email) = spaces::invite_to(state, user.id, space, email)
        .await?
        .map_err(ApiError::bad_request)?;
    tracing::info!(
        user = user.id,
        via = user.via,
        space = space.id,
        invitation = id,
        "invitation envoyée"
    );
    Ok(InvitationOut { id, email })
}

/// `DELETE /api/v1/spaces/{slug}/invitations/{id}`
async fn cancel_invitation(
    user: ApiUser,
    State(state): State<AppState>,
    Path((slug, id)): Path<(String, i64)>,
) -> ApiResult<StatusCode> {
    invitation_cancel(&state, &user, &slug, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn invitation_cancel(
    state: &AppState,
    user: &ApiUser,
    slug: &str,
    id: i64,
) -> ApiResult<()> {
    let space = user.space_at(slug)?;
    Ok(spaces::cancel_invite(&state.db, space, id).await?)
}

/// `DELETE /api/v1/spaces/{slug}/members/{user}`
async fn remove_member(
    user: ApiUser,
    State(state): State<AppState>,
    Path((slug, member)): Path<(String, i64)>,
) -> ApiResult<StatusCode> {
    member_remove(&state, &user, &slug, member).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn member_remove(
    state: &AppState,
    user: &ApiUser,
    slug: &str,
    member: i64,
) -> ApiResult<()> {
    let space = user.space_at(slug)?;
    spaces::remove_from(&state.db, space, member).await?;
    tracing::info!(
        user = user.id,
        via = user.via,
        space = space.id,
        member,
        "membre retiré"
    );
    Ok(())
}

/// `POST /api/v1/spaces/{slug}/leave` : un membre s'en va (403 au propriétaire).
async fn leave(
    user: ApiUser,
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> ApiResult<StatusCode> {
    space_leave(&state, &user, &slug).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn space_leave(state: &AppState, user: &ApiUser, slug: &str) -> ApiResult<()> {
    let space = user.space_at(slug)?;
    Ok(spaces::leave_space(&state.db, user.id, space).await?)
}

#[derive(Serialize)]
pub(crate) struct ReceivedOut {
    id: i64,
    space: String,
    color: String,
    icon: String,
    /// Qui invite.
    from: String,
}

/// `GET /api/v1/invitations` : les invitations reçues, en attente.
async fn list_invitations(
    user: ApiUser,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<ReceivedOut>>> {
    Ok(Json(invitations_list(&state, &user).await?))
}

pub(crate) async fn invitations_list(
    state: &AppState,
    user: &ApiUser,
) -> ApiResult<Vec<ReceivedOut>> {
    Ok(settings::received_invitations(&state.db, user.id)
        .await?
        .into_iter()
        .map(|i| ReceivedOut {
            id: i.id,
            space: i.space_name,
            color: i.space_color,
            icon: i.space_icon,
            from: i.from,
        })
        .collect())
}

/// `POST /api/v1/invitations/{id}/accept` : rend l'espace rejoint.
async fn accept_invitation(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<SpaceOut>> {
    Ok(Json(invitation_accept(&state, &user, id).await?))
}

pub(crate) async fn invitation_accept(
    state: &AppState,
    user: &ApiUser,
    id: i64,
) -> ApiResult<SpaceOut> {
    let (space, _) = spaces::accept_for(&state.db, user.id, &user.spaces, id)
        .await?
        .map_err(ApiError::bad_request)?;
    reread(state, user, |s| s.id == space).await
}

/// `POST /api/v1/invitations/{id}/decline`
async fn decline_invitation(
    user: ApiUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    spaces::decline_for(&state.db, user.id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------
// Réglages
// ---------------------------------------------------------------------

#[derive(Serialize)]
pub(crate) struct SettingsOut {
    email: String,
    first_name: String,
    last_name: String,
    has_avatar: bool,
    /// `dark`, `light` ou `system`.
    theme: String,
    /// `small`, `normal` ou `large`.
    ui_size: String,
    sidebar_floating: bool,
    /// Raccourcis de la barre du téléphone.
    mobile_tabs: Vec<&'static str>,
    /// Espace des jours sans cours ; `null` : le dernier utilisé.
    moment_space: Option<String>,
}

/// `GET /api/v1/settings`
async fn read_settings(
    user: ApiUser,
    State(state): State<AppState>,
) -> ApiResult<Json<SettingsOut>> {
    Ok(Json(settings_read(&state, &user).await?))
}

pub(crate) async fn settings_read(state: &AppState, user: &ApiUser) -> ApiResult<SettingsOut> {
    let row = sqlx::query!(
        r#"SELECT email, first_name, last_name, theme, ui_size, mobile_tabs, moment_space,
                  sidebar_floating AS "sidebar_floating!: bool",
                  avatar IS NOT NULL AS "has_avatar!: bool"
           FROM users WHERE id = ?"#,
        user.id
    )
    .fetch_one(&state.db)
    .await?;
    Ok(SettingsOut {
        email: row.email,
        first_name: row.first_name,
        last_name: row.last_name,
        has_avatar: row.has_avatar,
        theme: row.theme.unwrap_or_else(|| "system".into()),
        ui_size: row.ui_size.unwrap_or_else(|| "normal".into()),
        sidebar_floating: row.sidebar_floating,
        mobile_tabs: tabbar::selected(row.mobile_tabs.as_deref())
            .iter()
            .map(|tab| tab.key)
            .collect(),
        moment_space: row.moment_space.and_then(|id| {
            user.spaces
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.slug.clone())
        }),
    })
}

/// Ce qu'on peut changer. Un champ inconnu est refusé : une demande de
/// changer le mot de passe ne doit pas répondre « OK » sans rien faire.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct SettingsChanges {
    first_name: Option<String>,
    last_name: Option<String>,
    theme: Option<String>,
    ui_size: Option<String>,
    sidebar_floating: Option<bool>,
    mobile_tabs: Option<Vec<String>>,
    /// Slug d'un espace en service ; `null` : le dernier utilisé.
    #[serde(default, deserialize_with = "contenu::some")]
    moment_space: Option<Option<String>>,
}

/// `PATCH /api/v1/settings` : les champs donnés changent, les autres restent.
async fn update_settings(
    user: ApiUser,
    State(state): State<AppState>,
    payload: Result<Json<SettingsChanges>, JsonRejection>,
) -> ApiResult<Json<SettingsOut>> {
    let Json(changes) = payload?;
    Ok(Json(settings_update(&state, &user, changes).await?))
}

pub(crate) async fn settings_update(
    state: &AppState,
    user: &ApiUser,
    changes: SettingsChanges,
) -> ApiResult<SettingsOut> {
    // Tout est vérifié avant la première écriture : une saisie refusée ne
    // laisse pas la moitié des réglages changés.
    let names = if changes.first_name.is_some() || changes.last_name.is_some() {
        let current = settings_read(state, user).await?;
        let name = |given: Option<&str>, kept: String| match given {
            Some(raw) => profile::clean(raw).map_err(ApiError::bad_request),
            None => Ok(kept),
        };
        Some((
            name(changes.first_name.as_deref(), current.first_name)?,
            name(changes.last_name.as_deref(), current.last_name)?,
        ))
    } else {
        None
    };
    let theme = changes
        .theme
        .as_deref()
        .map(settings::parse_theme)
        .transpose()
        .map_err(ApiError::bad_request)?;
    let size = changes
        .ui_size
        .as_deref()
        .map(profile::parse_size)
        .transpose()
        .map_err(ApiError::bad_request)?;
    let tabs = match &changes.mobile_tabs {
        Some(keys) => {
            // `encode` ignore un raccourci inconnu ; ici, on le dit.
            if let Some(unknown) = keys
                .iter()
                .find(|key| !tabbar::ALL.iter().any(|tab| tab.key == key.as_str()))
            {
                let known: Vec<&str> = tabbar::ALL.iter().map(|tab| tab.key).collect();
                return Err(ApiError::bad_request(format!(
                    "raccourci inconnu « {unknown} » : valeurs possibles {}",
                    known.join(", ")
                )));
            }
            let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
            Some(tabbar::encode(&keys).map_err(ApiError::bad_request)?)
        }
        None => None,
    };
    let moment = match &changes.moment_space {
        Some(Some(slug)) => {
            let space = user.space(slug)?;
            if space.archived {
                return Err(ApiError::bad_request(format!(
                    "{} est archivé : il ne peut pas être l'espace du moment",
                    space.name
                )));
            }
            Some(Some(space.id))
        }
        Some(None) => Some(None),
        None => None,
    };

    if let Some((first, last)) = names {
        profile::store_names(&state.db, user.id, &first, &last).await?;
    }
    if let Some(theme) = theme {
        settings::store_theme(&state.db, user.id, theme).await?;
    }
    if let Some(size) = size {
        profile::store_size(&state.db, user.id, size).await?;
    }
    if let Some(floating) = changes.sidebar_floating {
        profile::store_sidebar(&state.db, user.id, floating).await?;
    }
    if let Some(tabs) = tabs {
        settings::store_tabs(&state.db, user.id, &tabs).await?;
    }
    if let Some(moment) = moment {
        settings::store_moment(&state.db, user.id, moment).await?;
    }
    settings_read(state, user).await
}

/// `GET /api/v1/settings/avatar` : la photo du compte, en JPEG.
async fn read_avatar(user: ApiUser, State(state): State<AppState>) -> ApiResult<Response> {
    let jpeg = sqlx::query_scalar!("SELECT avatar FROM users WHERE id = ?", user.id)
        .fetch_one(&state.db)
        .await?
        .ok_or_else(|| ApiError::not_found("photo"))?;
    Ok(([(header::CONTENT_TYPE, "image/jpeg")], jpeg).into_response())
}

/// `PUT /api/v1/settings/avatar` : l'image en corps brut (JPEG, PNG ou WebP,
/// 2 Mo au plus), recadrée et réenregistrée comme sur le site.
async fn put_avatar(
    user: ApiUser,
    State(state): State<AppState>,
    body: Bytes,
) -> ApiResult<StatusCode> {
    profile::store_avatar(&state.db, user.id, body).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/v1/settings/avatar` : retour aux initiales.
async fn delete_avatar(user: ApiUser, State(state): State<AppState>) -> ApiResult<StatusCode> {
    profile::remove_avatar(&state.db, user.id).await?;
    Ok(StatusCode::NO_CONTENT)
}
