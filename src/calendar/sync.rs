//! Synchronisation des sources iCal : téléchargement, analyse, remplacement
//! du cache local. Tourne toutes les heures en tâche de fond, et à la demande
//! (bouton « Rafraîchir »).
//!
//! L'URL d'un flux Pronote contient un jeton d'accès : elle n'apparaît ni
//! dans les logs, ni dans les messages d'erreur, ni en entier dans l'interface.

use std::net::IpAddr;
use std::time::Duration;

use sqlx::SqlitePool;
use url::{Host, Url};

use super::ical::{self, ParsedEvent};
use crate::AppState;
use crate::time;

/// Taille maximale d'un flux (un an d'emploi du temps pèse quelques centaines de Ko).
const MAX_BYTES: usize = 5 * 1024 * 1024;
const HOUR: Duration = Duration::from_secs(3600);

/// Lance la synchro horaire de toutes les sources.
pub fn spawn_hourly(state: AppState) {
    tokio::spawn(async move {
        // Laisse le serveur démarrer avant la première passe.
        tokio::time::sleep(Duration::from_secs(10)).await;
        let mut tick = tokio::time::interval(HOUR);
        loop {
            tick.tick().await;
            sync_all(&state).await;
        }
    });
}

async fn sync_all(state: &AppState) {
    let sources = match sqlx::query!(r#"SELECT id AS "id!", ical_url FROM calendar_sources"#)
        .fetch_all(&state.db)
        .await
    {
        Ok(sources) => sources,
        Err(error) => {
            tracing::error!(%error, "lecture des sources iCal impossible");
            return;
        }
    };
    for source in sources {
        // Les erreurs sont enregistrées sur la source par sync_source.
        let _ = sync_source(state, source.id, &source.ical_url).await;
    }
}

/// Synchronise une source. En cas d'échec, le cache existant est conservé et
/// l'erreur est enregistrée sur la source pour être affichée.
pub async fn sync_source(state: &AppState, source_id: i64, url: &str) -> Result<usize, String> {
    let now = time::now();
    let result = match fetch(&state.http, url).await {
        Ok(text) => ical::parse(&text),
        Err(error) => Err(error),
    };

    match result {
        Ok(events) => {
            let count = events.len();
            store(&state.db, source_id, &events, now)
                .await
                .map_err(|error| {
                    tracing::error!(source_id, %error, "enregistrement des événements impossible");
                    "erreur d'enregistrement".to_string()
                })?;
            tracing::info!(source_id, events = count, "agenda synchronisé");
            Ok(count)
        }
        Err(message) => {
            tracing::warn!(source_id, error = %message, "échec de synchro de l'agenda");
            let _ = sqlx::query!(
                "UPDATE calendar_sources SET last_attempt_at = ?, last_error = ? WHERE id = ?",
                now,
                message,
                source_id
            )
            .execute(&state.db)
            .await;
            Err(message)
        }
    }
}

/// Remplace tous les événements d'une source, dans une transaction : un
/// lecteur voit soit l'ancienne version complète, soit la nouvelle.
pub async fn store(
    db: &SqlitePool,
    source_id: i64,
    events: &[ParsedEvent],
    now: i64,
) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    sqlx::query!("DELETE FROM calendar_events WHERE source_id = ?", source_id)
        .execute(&mut *tx)
        .await?;
    for event in events {
        sqlx::query!(
            "INSERT INTO calendar_events (source_id, uid, title, location, starts_at, ends_at, all_day)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            source_id,
            event.uid,
            event.title,
            event.location,
            event.starts_at,
            event.ends_at,
            event.all_day
        )
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query!(
        "UPDATE calendar_sources SET last_fetched_at = ?, last_attempt_at = ?, last_error = NULL WHERE id = ?",
        now,
        now,
        source_id
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

/// Télécharge un flux. Les messages d'erreur ne contiennent jamais l'URL.
async fn fetch(http: &reqwest::Client, url: &str) -> Result<String, String> {
    let mut response = http
        .get(url)
        .header(
            reqwest::header::ACCEPT,
            "text/calendar, text/plain;q=0.9, */*;q=0.5",
        )
        .send()
        .await
        .map_err(describe)?;

    if !response.status().is_success() {
        return Err(format!(
            "le serveur a répondu {}",
            response.status().as_u16()
        ));
    }

    // Lecture par morceaux pour pouvoir s'arrêter dès que la limite est dépassée.
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(describe)? {
        if body.len() + chunk.len() > MAX_BYTES {
            return Err("flux trop volumineux (plus de 5 Mo)".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

fn describe(error: reqwest::Error) -> String {
    if error.is_timeout() {
        "le serveur n'a pas répondu à temps".into()
    } else if error.is_connect() {
        "serveur injoignable".into()
    } else if error.is_redirect() {
        "trop de redirections".into()
    } else {
        "erreur réseau pendant le téléchargement".into()
    }
}

/// Valide l'URL saisie pour une source iCal et la renvoie normalisée.
///
/// Le serveur va télécharger cette URL : on refuse donc tout ce qui pointe
/// vers lui-même ou vers le réseau local (attaque dite SSRF). La vérification
/// porte sur le nom saisi ; un nom public qui résoudrait vers une adresse
/// privée n'est pas détecté ici.
pub fn validate_url(input: &str) -> Result<String, &'static str> {
    let input = input.trim();
    // Certains outils donnent le lien en webcal:// : c'est du HTTPS.
    let normalized = match input.strip_prefix("webcal://") {
        Some(rest) => format!("https://{rest}"),
        None => input.to_string(),
    };
    let url = Url::parse(&normalized).map_err(|_| "Cette URL n'est pas valide.")?;

    let dev = std::env::var_os("DASHU_ALLOW_LOCAL_ICAL").is_some();
    if dev && matches!(url.scheme(), "http" | "https") {
        return Ok(url.to_string());
    }

    if url.scheme() != "https" {
        return Err("L'URL doit commencer par https://");
    }
    let local = match url.host() {
        Some(Host::Domain(domain)) => {
            let domain = domain.to_lowercase();
            domain == "localhost"
                || !domain.contains('.')
                || [".localhost", ".local", ".internal", ".lan", ".home"]
                    .iter()
                    .any(|s| domain.ends_with(s))
        }
        Some(Host::Ipv4(ip)) => !is_public(IpAddr::V4(ip)),
        Some(Host::Ipv6(ip)) => !is_public(IpAddr::V6(ip)),
        None => true,
    };
    if local {
        return Err("Cette adresse pointe vers le réseau local : elle n'est pas acceptée.");
    }
    Ok(url.to_string())
}

fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_documentation()
                // 100.64.0.0/10 : plage CGNAT, utilisée notamment par Tailscale.
                || (ip.octets()[0] == 100 && (ip.octets()[1] & 0xc0) == 64))
        }
        IpAddr::V6(ip) => {
            !(ip.is_loopback()
                || ip.is_unspecified()
                // fc00::/7 (adresses locales uniques) et fe80::/10 (lien local).
                || (ip.segments()[0] & 0xfe00) == 0xfc00
                || (ip.segments()[0] & 0xffc0) == 0xfe80
                || ip.to_ipv4_mapped().is_some_and(|v4| !is_public(IpAddr::V4(v4))))
        }
    }
}

/// Version affichable d'une URL de flux : le domaine seulement.
/// Le chemin et les paramètres, qui portent le jeton d'accès, sont masqués.
pub fn mask_url(url: &str) -> String {
    match Url::parse(url) {
        Ok(parsed) => format!(
            "{}://{}/…",
            parsed.scheme(),
            parsed.host_str().unwrap_or("?")
        ),
        Err(_) => "…".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_https_publique_acceptee() {
        assert!(
            validate_url("https://campus.example.fr/pronote/ical/Edt.ics?icalsecurise=ABC").is_ok()
        );
        assert_eq!(
            validate_url("webcal://campus.example.fr/a.ics").unwrap(),
            "https://campus.example.fr/a.ics"
        );
    }

    #[test]
    fn url_locale_ou_non_chiffree_refusee() {
        for url in [
            "http://campus.example.fr/a.ics",
            "https://localhost/a.ics",
            "https://127.0.0.1/a.ics",
            "https://192.168.1.10/a.ics",
            "https://10.0.0.5/a.ics",
            "https://100.100.1.1/a.ics",
            "https://[::1]/a.ics",
            "https://[fd00::1]/a.ics",
            "https://nas/a.ics",
            "https://proxmox.local/a.ics",
            "file:///etc/passwd",
            "pas une url",
        ] {
            assert!(validate_url(url).is_err(), "{url} aurait dû être refusée");
        }
    }

    #[test]
    fn url_masquee_sans_jeton() {
        let masked =
            mask_url("https://campus.example.fr/pronote/ical/Edt.ics?icalsecurise=SECRET123");
        assert_eq!(masked, "https://campus.example.fr/…");
        assert!(!masked.contains("SECRET"));
    }
}
