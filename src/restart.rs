//! Relance de dashu après un changement des réglages de l'instance (SPEC,
//! « Installation simple », étapes 4 et 5).
//!
//! La configuration est lue une fois, au démarrage : la changer à chaud
//! demanderait de reconstruire le client Google, la couche des sessions et
//! tout ce qui dépend de l'adresse publique. dashu termine donc les requêtes
//! en cours, puis se remplace par une nouvelle copie de lui-même (même
//! binaire, mêmes arguments, même numéro de processus pour systemd ou
//! Docker) : quelques secondes de coupure.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::Notify;

#[derive(Default)]
pub struct Restart {
    asked: AtomicBool,
    notify: Notify,
}

impl Restart {
    /// Demande la relance. L'arrêt commence une demi-seconde plus tard :
    /// la réponse en cours part d'abord.
    pub fn ask(self: &std::sync::Arc<Self>) {
        self.asked.store(true, Ordering::SeqCst);
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            me.notify.notify_one();
        });
    }

    /// Une relance a-t-elle été demandée ?
    pub fn asked(&self) -> bool {
        self.asked.load(Ordering::SeqCst)
    }

    /// Attend la demande de relance (signal d'arrêt du serveur).
    pub async fn wait(&self) {
        self.notify.notified().await;
    }
}

/// Remplace le processus par une nouvelle copie de lui-même. Ne revient
/// qu'en cas d'échec.
#[cfg(unix)]
pub fn exec() -> anyhow::Error {
    use std::os::unix::process::CommandExt;
    let mut args = std::env::args_os();
    // Le chemin donné au lancement (systemd, Docker) plutôt que
    // /proc/self/exe : après un dashu-update, l'ancien fichier n'existe plus.
    let program = match args.next() {
        Some(first) if std::path::Path::new(&first).is_absolute() => first.into(),
        _ => match std::env::current_exe() {
            Ok(path) => path,
            Err(e) => return e.into(),
        },
    };
    tracing::info!("relance de dashu pour prendre la nouvelle configuration");
    std::process::Command::new(program).args(args).exec().into()
}

/// Hors Unix (développement sous Windows), pas de remplacement : dashu
/// s'arrête et demande d'être relancé.
#[cfg(not(unix))]
pub fn exec() -> anyhow::Error {
    anyhow::anyhow!("nouvelle configuration enregistrée : relance dashu pour la prendre")
}
