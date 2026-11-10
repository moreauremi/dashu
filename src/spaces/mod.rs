//! Espaces : Perso, École, Taff. Tout contenu appartient à un espace ;
//! l'espace actif est gardé en session et filtre toutes les pages.

use axum::response::Response;
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::auth::session::{ACTIVE_SPACE, Ctx};
use crate::web::{AppError, AppResult, local_path, see_other};

#[derive(Debug, Clone)]
pub struct Space {
    pub id: i64,
    /// `perso`, `ecole` ou `taff`.
    pub slug: String,
    pub name: String,
    /// Nom d'un accent du design system (`orange`, `blue`, `green`), appliqué
    /// par l'attribut `data-accent` du conteneur de la page.
    pub color: String,
}

impl Space {
    /// Icône du sprite associée à l'espace.
    pub fn icon(&self) -> &'static str {
        match self.slug.as_str() {
            "ecole" => "school",
            "taff" => "work",
            _ => "home",
        }
    }
}

/// (slug, nom, couleur) dans l'ordre d'affichage.
const DEFAULTS: [(&str, &str, &str); 3] = [
    ("perso", "Perso", "orange"),
    ("ecole", "École", "blue"),
    ("taff", "Taff", "green"),
];

/// Crée les trois espaces d'un utilisateur s'ils n'existent pas encore.
pub async fn ensure_defaults(db: &SqlitePool, user_id: i64) -> AppResult<()> {
    for (position, (slug, name, color)) in (0_i64..).zip(DEFAULTS) {
        sqlx::query!(
            "INSERT OR IGNORE INTO spaces (user_id, slug, name, color, position) VALUES (?, ?, ?, ?, ?)",
            user_id,
            slug,
            name,
            color,
            position
        )
        .execute(db)
        .await?;
    }
    Ok(())
}

pub async fn list(db: &SqlitePool, user_id: i64) -> AppResult<Vec<Space>> {
    let spaces = sqlx::query_as!(
        Space,
        r#"SELECT id AS "id!", slug, name, color FROM spaces WHERE user_id = ? ORDER BY position"#,
        user_id
    )
    .fetch_all(db)
    .await?;
    Ok(spaces)
}

#[derive(Deserialize)]
pub struct SwitchForm {
    slug: String,
    /// Page d'où vient la demande, pour y revenir.
    next: Option<String>,
}

/// `POST /space` : change l'espace actif puis revient sur la page courante.
pub async fn switch(ctx: Ctx, axum::Form(form): axum::Form<SwitchForm>) -> AppResult<Response> {
    if !ctx.spaces.iter().any(|s| s.slug == form.slug) {
        return Err(AppError::NotFound);
    }
    ctx.session.insert(ACTIVE_SPACE, &form.slug).await?;

    let next = local_path(form.next.as_deref());
    // Une note ouverte appartient à l'ancien espace : on revient à la liste.
    let next = if next.starts_with("/notes/") {
        "/notes"
    } else {
        next
    };
    Ok(see_other(next))
}
