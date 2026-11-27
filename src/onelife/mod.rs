//! 1Life : outil réservé (SPEC, « Outils par espace », PR 2).
//!
//! Réservé aux comptes autorisés sur le serveur (`dashu tools grant <email>
//! 1life`, table `tool_grants`) : pour tout autre compte, et sans session,
//! toutes ses adresses répondent 404, comme une adresse qui n'existe pas.
//! Une fois autorisé, il s'affiche ou se masque par espace comme les autres
//! outils (masqué par défaut).
//!
//! Une page d'accueil, une carte par module. Clients est fait (SPEC,
//! « 1Life — Clients », sous-module [`clients`], avec l'encart Identité de
//! [`fields`]), Audit aussi (« 1Life — Audit », [`audit`], qui appelle l'IA
//! d'Anthropic par [`generation`]) ; les autres modules ont une page « Module
//! à venir ». Les réglages de 1Life sont dans [`settings`]. Le dépôt est
//! public : rien de propre à 1Life (vrai client, document, information
//! interne) ne va ici ni dans les tests.

pub mod audit;
pub mod clients;
pub mod fields;
pub mod generation;
pub mod settings;
pub mod templates;

use askama::Template;
use axum::extract::Path;
use axum::response::Response;

use crate::auth::session::{Ctx, Shell};
use crate::registry;
use crate::web::{self, AppError, AppResult};

/// Identifiant de l'outil dans le registre.
pub const TOOL: &str = "1life";

/// Un module de 1Life : une carte sur l'accueil, une page à son adresse.
#[derive(Debug, PartialEq, Eq)]
pub struct Module {
    /// Dernier mot de l'adresse : `/1life/<id>`.
    pub id: &'static str,
    pub name: &'static str,
    /// Icône du sprite (`static/icons.svg`).
    pub icon: &'static str,
    /// Une ligne sous le nom, sur la carte.
    pub summary: &'static str,
    /// Pas encore fait : badge « Bientôt » et page « Module à venir ».
    pub soon: bool,
}

/// Les modules, dans l'ordre de l'accueil. En ajouter un = une entrée.
pub const MODULES: &[Module] = &[
    Module {
        id: "cr",
        name: "Compte rendu",
        icon: "notes",
        summary: "Rédiger un compte rendu.",
        soon: true,
    },
    Module {
        id: "audit",
        name: "Audit",
        icon: "check",
        summary: "Des notes d'audit au rapport, par l'IA.",
        soon: false,
    },
    Module {
        id: "clients",
        name: "Clients",
        icon: "users",
        summary: "Ses clients : dossiers, infos et journées.",
        soon: false,
    },
    Module {
        id: "taches",
        name: "Tâches",
        icon: "tasks",
        summary: "Suivre ses tâches de travail.",
        soon: true,
    },
];

#[derive(Template)]
#[template(path = "onelife/home.html")]
struct HomePage {
    shell: Shell,
    modules: &'static [Module],
    /// `/e/<espace>/1life/reglages`
    settings_url: String,
}

#[derive(Template)]
#[template(path = "onelife/module.html")]
struct ModulePage {
    shell: Shell,
    module: &'static Module,
}

/// Le compte peut entrer : connecté et autorisé, sinon 404 (pas la page de
/// connexion, ni 403 : rien ne dit que l'outil existe). Masqué dans
/// l'espace actif : retour à l'accueil de l'espace, comme les autres outils.
async fn enter(ctx: Result<Ctx, Response>) -> AppResult<Result<Ctx, Response>> {
    let ctx = ctx.map_err(|_| AppError::NotFound)?;
    if ctx.tools.tool(TOOL).is_none() {
        return Err(AppError::NotFound);
    }
    match registry::guard(&ctx, TOOL).await? {
        Some(back) => Ok(Err(back)),
        None => Ok(Ok(ctx)),
    }
}

/// `GET /1life` : l'accueil, une carte par module.
pub async fn home(ctx: Result<Ctx, Response>) -> AppResult<Response> {
    let ctx = match enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    Ok(web::html(HomePage {
        settings_url: format!("/e/{}/1life/reglages", ctx.active.slug),
        shell: ctx.shell(TOOL).await?,
        modules: MODULES,
    }))
}

/// `GET /1life/{module}` : la page d'un module (« Module à venir »).
pub async fn module(ctx: Result<Ctx, Response>, Path(id): Path<String>) -> AppResult<Response> {
    let ctx = match enter(ctx).await? {
        Ok(ctx) => ctx,
        Err(back) => return Ok(back),
    };
    let module = MODULES
        .iter()
        .find(|m| m.id == id)
        .ok_or(AppError::NotFound)?;
    Ok(web::html(ModulePage {
        shell: ctx.shell(TOOL).await?,
        module,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modules_coherents() {
        for (i, module) in MODULES.iter().enumerate() {
            assert!(MODULES[..i].iter().all(|m| m.id != module.id));
            assert!(
                module
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b == b'-')
            );
        }
        assert_eq!(
            MODULES.iter().map(|m| m.id).collect::<Vec<_>>(),
            ["cr", "audit", "clients", "taches"]
        );
    }
}
