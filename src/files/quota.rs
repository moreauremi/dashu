//! Quota de stockage par compte (SPEC v2, « Quota de stockage »).
//!
//! * Chaque compte a un plafond : le sien (`users.quota_bytes`) ou, sans
//!   plafond à lui, la valeur par défaut du serveur (`FILES_QUOTA`, 30 Gio).
//! * L'usage est la somme des tailles enregistrées en base de ses fichiers,
//!   tous espaces confondus, corbeille comprise. Les fichiers de l'onglet
//!   Outils pas encore rangés (`expires_at`) ne comptent pas. Le disque n'est
//!   jamais parcouru.
//! * Le plafond ne se change que par la commande `dashu quota`, sur le
//!   serveur : aucune page, aucune route de l'API ni du connecteur Claude ne
//!   l'écrit, même pour l'administrateur. La seule écriture est [`write_cap`].
//! * Un envoi est vérifié trois fois : à l'ouverture (taille annoncée),
//!   pendant la réception (octets comptés) et dans la transaction qui
//!   l'enregistre, pour que deux envois en parallèle ne dépassent pas le
//!   plafond à eux deux.

use std::fmt::Write as _;
use std::path::Path;

use sqlx::{SqliteExecutor, SqlitePool};

use crate::time;
use crate::web::AppError;

/// Ce qu'un compte occupe, et son plafond, en octets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub used: i64,
    pub cap: i64,
}

impl Usage {
    /// Place restante (négative si le plafond a été baissé sous l'usage).
    pub fn room(&self) -> i64 {
        self.cap - self.used
    }

    /// Vrai si `extra` octets de plus tiennent sous le plafond.
    pub fn fits(&self, extra: i64) -> bool {
        self.used.saturating_add(extra) <= self.cap
    }

    /// Valeur de la barre de la jauge : l'usage, ramené au plafond.
    pub fn bar(&self) -> i64 {
        self.used.clamp(0, self.cap.max(0))
    }

    /// Occupation en pour cent, arrondie vers le bas (peut dépasser 100).
    pub fn percent(&self) -> i64 {
        if self.cap <= 0 {
            return 100;
        }
        i64::try_from(i128::from(self.used) * 100 / i128::from(self.cap)).unwrap_or(i64::MAX)
    }

    /// À partir de 90 % : la jauge passe en couleur d'alerte.
    pub fn alert(&self) -> bool {
        i128::from(self.used) * 10 >= i128::from(self.cap) * 9
    }

    /// « 12,4 Go / 30 Go »
    pub fn label(&self) -> String {
        format!("{} / {}", size_label(self.used), size_label(self.cap))
    }

    /// Message du refus d'un envoi (HTTP 413).
    pub fn full_message(&self) -> String {
        format!(
            "Espace plein : {} utilisés sur {}. Supprime des fichiers ou demande à l'administrateur d'augmenter ton plafond.",
            size_label(self.used),
            size_label(self.cap)
        )
    }

    /// Le refus d'un envoi, pour les handlers.
    pub fn full(&self) -> AppError {
        AppError::TooLarge(self.full_message())
    }
}

/// Usage et plafond d'un compte. `default` : la valeur par défaut du serveur
/// (`FilesConfig::default_quota_bytes`). À appeler dans la transaction qui
/// enregistre un fichier, pour que la vérification et l'écriture se tiennent.
pub async fn usage(db: impl SqliteExecutor<'_>, user: i64, default: u64) -> sqlx::Result<Usage> {
    let default = to_i64(default);
    let row = sqlx::query!(
        r#"SELECT
             (SELECT coalesce(sum(size), 0) FROM files
               WHERE user_id = ?1 AND expires_at IS NULL) AS "used!: i64",
             coalesce((SELECT quota_bytes FROM users WHERE id = ?1), ?2) AS "cap!: i64""#,
        user,
        default
    )
    .fetch_one(db)
    .await?;
    Ok(Usage {
        used: row.used,
        cap: row.cap,
    })
}

/// Ouvre une transaction qui prend tout de suite le verrou d'écriture
/// (`BEGIN IMMEDIATE`) : entre la lecture de l'usage et l'écriture du fichier,
/// aucun autre envoi ne peut s'enregistrer. Sans cela, en mode WAL, deux
/// envois liraient le même usage et passeraient tous les deux.
pub async fn begin(db: &SqlitePool) -> sqlx::Result<sqlx::Transaction<'static, sqlx::Sqlite>> {
    db.begin_with("BEGIN IMMEDIATE").await
}

fn to_i64(bytes: u64) -> i64 {
    i64::try_from(bytes).unwrap_or(i64::MAX)
}

// ---------------------------------------------------------------------
// Tailles
// ---------------------------------------------------------------------

const UNITS: [(&str, u32); 4] = [("K", 1), ("M", 2), ("G", 3), ("T", 4)];

/// Lit une taille comme « 50G », « 500M », « 1,5T » ou « 30 Gio » (unités
/// binaires : 1G = 1024³ octets). L'unité est obligatoire : « 50 » seul est
/// refusé, plutôt que lu comme 50 octets.
pub fn parse_size(text: &str) -> Result<u64, String> {
    let compact = text.trim();
    let invalid = || {
        format!(
            "taille invalide « {} » : un nombre suivi de K, M, G ou T, par exemple 50G ou 500M",
            text.trim()
        )
    };
    if compact.starts_with('-') {
        return Err(format!(
            "taille invalide « {} » : une taille est positive",
            text.trim()
        ));
    }
    let split = compact
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ','))
        .ok_or_else(invalid)?;
    let (number, unit) = compact.split_at(split);
    // Une espace est permise entre le nombre et l'unité : « 30 Gio ».
    let unit = unit.trim_start().to_ascii_uppercase();
    let power = UNITS
        .iter()
        .find(|(letter, _)| {
            let rest = unit.strip_prefix(letter);
            matches!(rest, Some("" | "O" | "IO" | "B" | "IB"))
        })
        .map(|(_, power)| *power)
        .ok_or_else(invalid)?;
    let multiplier = 1024u64.pow(power);
    let number = number.replace(',', ".");
    if number.is_empty() || number.matches('.').count() > 1 || number.ends_with('.') {
        return Err(invalid());
    }
    let too_big = || format!("taille trop grande « {} »", text.trim());
    let bytes = match number.split_once('.') {
        None => number
            .parse::<u64>()
            .map_err(|_| too_big())?
            .checked_mul(multiplier)
            .ok_or_else(too_big)?,
        Some((whole, fraction)) => {
            // Au plus trois décimales : 1,5T, 0,25G. Calcul en entiers.
            if fraction.len() > 3 {
                return Err(invalid());
            }
            let scale = 10u64.pow(fraction.len() as u32);
            let whole: u64 = if whole.is_empty() {
                0
            } else {
                whole.parse().map_err(|_| too_big())?
            };
            let fraction: u64 = fraction.parse().map_err(|_| invalid())?;
            let scaled = whole
                .checked_mul(scale)
                .and_then(|w| w.checked_add(fraction))
                .ok_or_else(too_big)?;
            u64::try_from(u128::from(scaled) * u128::from(multiplier) / u128::from(scale))
                .map_err(|_| too_big())?
        }
    };
    if bytes == 0 {
        return Err(format!(
            "taille invalide « {} » : un plafond ne peut pas être nul",
            text.trim()
        ));
    }
    if bytes > i64::MAX as u64 {
        return Err(too_big());
    }
    Ok(bytes)
}

/// Taille lisible, à une décimale arrondie vers le bas : « 12,4 Go »,
/// « 30 Go », « 512 o ». Vers le bas pour qu'un compte presque plein
/// n'affiche jamais « 30 Go utilisés sur 30 Go ». Unités binaires, comme
/// le reste de dashu (`human_size`).
pub fn size_label(bytes: i64) -> String {
    const NAMES: [&str; 4] = ["Ko", "Mo", "Go", "To"];
    if bytes < 1024 {
        return format!("{} o", bytes.max(0));
    }
    let mut unit = 0;
    let mut divisor: i128 = 1024;
    while i128::from(bytes) >= divisor * 1024 && unit < NAMES.len() - 1 {
        divisor *= 1024;
        unit += 1;
    }
    let tenths = i128::from(bytes) * 10 / divisor;
    let (whole, tenth) = (tenths / 10, tenths % 10);
    if tenth == 0 {
        format!("{whole} {}", NAMES[unit])
    } else {
        format!("{whole},{tenth} {}", NAMES[unit])
    }
}

// ---------------------------------------------------------------------
// Commande `dashu quota`
// ---------------------------------------------------------------------

pub const USAGE: &str = "usage :\n  dashu quota list                       usage et plafond de chaque compte\n  dashu quota show <email>               usage, plafond et changements d'un compte\n  dashu quota set <email> <taille>       plafond personnel : 50G, 500M, 1,5T…\n  dashu quota reset <email>              retour à la valeur par défaut";

/// Erreur de la commande : message en français, et code de sortie (2 pour
/// une commande mal formée, 1 sinon).
#[derive(Debug)]
pub struct CommandError {
    pub message: String,
    pub code: i32,
}

impl CommandError {
    fn usage() -> Self {
        Self {
            message: USAGE.into(),
            code: 2,
        }
    }

    fn refused(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: 1,
        }
    }
}

impl From<sqlx::Error> for CommandError {
    fn from(error: sqlx::Error) -> Self {
        Self::refused(format!("base de données : {error}"))
    }
}

/// Un compte, son usage et son plafond personnel (`None` : la valeur par
/// défaut).
struct Account {
    id: i64,
    email: String,
    own_cap: Option<i64>,
    usage: Usage,
}

impl Account {
    fn cap_label(&self) -> String {
        match self.own_cap {
            Some(_) => size_label(self.usage.cap),
            None => format!("{} (défaut)", size_label(self.usage.cap)),
        }
    }
}

/// `dashu quota …` : rend le texte à afficher. `files_dir` : le disque des
/// fichiers (`FILES_DIR`), pour comparer les plafonds à sa place réelle.
pub async fn command(
    db: &SqlitePool,
    default: u64,
    files_dir: Option<&Path>,
    args: &[&str],
) -> Result<String, CommandError> {
    match args {
        ["list"] => list(db, default, files_dir).await,
        ["show", email] => show(db, default, email).await,
        ["set", email, size] => {
            // La taille d'abord : rien n'est lu ni écrit pour une taille invalide.
            let bytes = parse_size(size).map_err(CommandError::refused)?;
            change(db, default, files_dir, email, Some(to_i64(bytes))).await
        }
        ["reset", email] => change(db, default, files_dir, email, None).await,
        _ => Err(CommandError::usage()),
    }
}

async fn accounts(db: &SqlitePool, default: u64) -> sqlx::Result<Vec<Account>> {
    let rows = sqlx::query!(
        r#"SELECT id AS "id!", email, quota_bytes FROM users ORDER BY email COLLATE NOCASE"#
    )
    .fetch_all(db)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(Account {
            usage: usage(db, row.id, default).await?,
            id: row.id,
            email: row.email,
            own_cap: row.quota_bytes,
        });
    }
    Ok(out)
}

async fn account(db: &SqlitePool, default: u64, email: &str) -> Result<Account, CommandError> {
    let email = email.trim();
    let row = sqlx::query!(
        r#"SELECT id AS "id!", email, quota_bytes FROM users WHERE email = ? COLLATE NOCASE"#,
        email
    )
    .fetch_optional(db)
    .await?
    .ok_or_else(|| {
        CommandError::refused(format!(
            "aucun compte {email} : dashu quota list donne les comptes existants"
        ))
    })?;
    Ok(Account {
        usage: usage(db, row.id, default).await?,
        id: row.id,
        email: row.email,
        own_cap: row.quota_bytes,
    })
}

async fn list(
    db: &SqlitePool,
    default: u64,
    files_dir: Option<&Path>,
) -> Result<String, CommandError> {
    let accounts = accounts(db, default).await?;
    let mut out = String::new();
    if accounts.is_empty() {
        out.push_str("Aucun compte.\n");
    } else {
        let width = accounts
            .iter()
            .map(|a| a.email.chars().count())
            .max()
            .unwrap_or(0)
            .max(6);
        let _ = writeln!(
            out,
            "{:<width$}  {:>10}  {:>16}  {:>5}",
            "Compte", "Utilisé", "Plafond", "%"
        );
        for account in &accounts {
            let _ = writeln!(
                out,
                "{:<width$}  {:>10}  {:>16}  {:>3} %",
                account.email,
                size_label(account.usage.used),
                account.cap_label(),
                account.usage.percent()
            );
        }
    }
    out.push('\n');
    out.push_str(&disk_summary(db, default, files_dir).await?);
    Ok(out)
}

async fn show(db: &SqlitePool, default: u64, email: &str) -> Result<String, CommandError> {
    let account = account(db, default, email).await?;
    let mut out = String::new();
    let _ = writeln!(out, "{}", account.email);
    let _ = writeln!(
        out,
        "  Utilisé  : {} ({} octets)",
        size_label(account.usage.used),
        account.usage.used
    );
    let _ = writeln!(
        out,
        "  Plafond  : {} ({} octets)",
        account.cap_label(),
        account.usage.cap
    );
    let _ = writeln!(out, "  Occupé   : {} %", account.usage.percent());
    if account.usage.room() < 0 {
        out.push_str("  Au-dessus du plafond : les nouveaux envois sont refusés.\n");
    }
    let changes = sqlx::query!(
        "SELECT before_bytes, after_bytes, changed_at FROM quota_changes
         WHERE email = ? COLLATE NOCASE ORDER BY changed_at DESC, id DESC LIMIT 10",
        account.email
    )
    .fetch_all(db)
    .await?;
    if !changes.is_empty() {
        out.push_str("  Derniers changements :\n");
        for change in changes {
            let _ = writeln!(
                out,
                "    {}  {} → {}",
                time::paris(change.changed_at).format("%d/%m/%Y %H:%M"),
                cap_or_default(change.before_bytes),
                cap_or_default(change.after_bytes)
            );
        }
    }
    Ok(out)
}

fn cap_or_default(bytes: Option<i64>) -> String {
    match bytes {
        Some(bytes) => size_label(bytes),
        None => "défaut".into(),
    }
}

/// `set` et `reset` : écrit le plafond, le journalise, et dit ce qui change.
async fn change(
    db: &SqlitePool,
    default: u64,
    files_dir: Option<&Path>,
    email: &str,
    cap: Option<i64>,
) -> Result<String, CommandError> {
    let before = account(db, default, email).await?;
    write_cap(db, before.id, &before.email, before.own_cap, cap).await?;
    let after = account(db, default, email).await?;
    tracing::info!(
        email = %after.email,
        avant = ?before.own_cap,
        apres = ?after.own_cap,
        "plafond de stockage changé"
    );

    let mut out = String::new();
    let _ = writeln!(
        out,
        "Plafond de {} : {} → {}",
        after.email,
        before.cap_label(),
        after.cap_label()
    );
    let _ = writeln!(
        out,
        "Utilisé : {} ({} %).",
        size_label(after.usage.used),
        after.usage.percent()
    );
    if after.usage.room() < 0 {
        out.push_str(
            "Attention : l'usage dépasse déjà ce plafond. Rien n'est supprimé, mais les nouveaux envois sont refusés.\n",
        );
    }
    if let Some(warning) = overcommit(db, default, files_dir).await? {
        out.push_str(&warning);
    }
    Ok(out)
}

/// La seule écriture de `users.quota_bytes`, avec sa ligne de journal, dans
/// une même transaction.
async fn write_cap(
    db: &SqlitePool,
    user: i64,
    email: &str,
    before: Option<i64>,
    after: Option<i64>,
) -> sqlx::Result<()> {
    let now = time::now();
    let mut tx = db.begin().await?;
    sqlx::query!("UPDATE users SET quota_bytes = ? WHERE id = ?", after, user)
        .execute(&mut *tx)
        .await?;
    sqlx::query!(
        "INSERT INTO quota_changes (email, before_bytes, after_bytes, changed_at) VALUES (?, ?, ?, ?)",
        email,
        before,
        after,
        now
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

/// Ce que les plafonds promettent, face à ce que le disque peut tenir.
struct Disk {
    /// Somme des plafonds de tous les comptes.
    promised: i64,
    /// Fichiers déjà stockés (tous les comptes, ceux de l'onglet Outils compris).
    stored: i64,
    /// Place libre sur le disque des fichiers, si on sait la lire.
    free: Option<i64>,
}

impl Disk {
    /// Place réelle : ce qui est déjà stocké, plus ce qui reste libre.
    fn real(&self) -> Option<i64> {
        self.free.map(|free| free.saturating_add(self.stored))
    }
}

async fn disk(db: &SqlitePool, default: u64, files_dir: Option<&Path>) -> sqlx::Result<Disk> {
    let default = to_i64(default);
    let promised = sqlx::query_scalar!(
        r#"SELECT coalesce(sum(coalesce(quota_bytes, ?)), 0) AS "n!: i64" FROM users"#,
        default
    )
    .fetch_one(db)
    .await?;
    let stored = sqlx::query_scalar!(r#"SELECT coalesce(sum(size), 0) AS "n!: i64" FROM files"#)
        .fetch_one(db)
        .await?;
    Ok(Disk {
        promised,
        stored,
        free: files_dir.and_then(free_space),
    })
}

async fn disk_summary(
    db: &SqlitePool,
    default: u64,
    files_dir: Option<&Path>,
) -> sqlx::Result<String> {
    let disk = disk(db, default, files_dir).await?;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Plafonds attribués : {} au total (défaut : {}).",
        size_label(disk.promised),
        size_label(to_i64(default))
    );
    let _ = writeln!(out, "Fichiers stockés   : {}.", size_label(disk.stored));
    match (files_dir, disk.free) {
        (Some(dir), Some(free)) => {
            let _ = writeln!(
                out,
                "Libre sur le disque des fichiers ({}) : {}.",
                dir.display(),
                size_label(free)
            );
        }
        (Some(dir), None) => {
            let _ = writeln!(
                out,
                "Place libre sur le disque des fichiers ({}) : illisible.",
                dir.display()
            );
        }
        (None, _) => out.push_str("FILES_DIR absent : place du disque inconnue.\n"),
    }
    if let Some(warning) = overcommit_text(&disk) {
        out.push_str(&warning);
    }
    Ok(out)
}

async fn overcommit(
    db: &SqlitePool,
    default: u64,
    files_dir: Option<&Path>,
) -> sqlx::Result<Option<String>> {
    Ok(overcommit_text(&disk(db, default, files_dir).await?))
}

/// Avertissement, sans rien bloquer, si les plafonds dépassent la place
/// réelle du disque (stocké + libre).
fn overcommit_text(disk: &Disk) -> Option<String> {
    let real = disk.real()?;
    (disk.promised > real).then(|| {
        format!(
            "Attention : les plafonds ({}) dépassent la place réelle du disque ({}, fichiers stockés compris). Le disque peut être plein avant qu'un compte n'atteigne son plafond.\n",
            size_label(disk.promised),
            size_label(real)
        )
    })
}

/// Place libre sur le disque d'un dossier, lue par `df` (POSIX) : pas de
/// dépendance de plus pour une commande lancée à la main. `None` si `df`
/// manque ou répond autre chose.
fn free_space(dir: &Path) -> Option<i64> {
    let output = std::process::Command::new("df")
        .arg("-Pk")
        .arg(dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    // Deuxième ligne, quatrième colonne : les blocs de 1 Kio disponibles.
    let kib: i64 = text
        .lines()
        .nth(1)?
        .split_whitespace()
        .nth(3)?
        .parse()
        .ok()?;
    kib.checked_mul(1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: u64 = 1024 * 1024 * 1024;

    #[test]
    fn tailles_lues() {
        assert_eq!(parse_size("50G"), Ok(50 * G));
        assert_eq!(parse_size("500M"), Ok(500 * 1024 * 1024));
        assert_eq!(parse_size("30 Gio"), Ok(30 * G));
        assert_eq!(parse_size("2go"), Ok(2 * G));
        assert_eq!(parse_size("1,5T"), Ok(1536 * G));
        assert_eq!(parse_size("0.5G"), Ok(G / 2));
        assert_eq!(parse_size("64K"), Ok(64 * 1024));
        assert_eq!(parse_size("8GiB"), Ok(8 * G));
    }

    #[test]
    fn tailles_refusees() {
        for bad in [
            "",
            "50",
            "-5G",
            "0G",
            "0,0M",
            "G",
            "50X",
            "5 0G",
            "1.2.3G",
            "1,G",
            "50GG",
            "abc",
            "9999999999999T",
            "1,23456G",
        ] {
            assert!(parse_size(bad).is_err(), "{bad:?} accepté");
        }
        assert!(parse_size("-5G").unwrap_err().contains("positive"));
        assert!(parse_size("0G").unwrap_err().contains("nul"));
    }

    #[test]
    fn tailles_affichees() {
        assert_eq!(size_label(512), "512 o");
        assert_eq!(size_label(30 * G as i64), "30 Go");
        assert_eq!(size_label((12.45 * G as f64) as i64), "12,4 Go");
        // Vers le bas : presque plein ne s'affiche jamais « plein ».
        assert_eq!(size_label(30 * G as i64 - 1), "29,9 Go");
        assert_eq!(size_label(1536 * 1024 * 1024), "1,5 Go");
    }

    #[test]
    fn jauge_et_message() {
        let usage = Usage {
            // 29,6 Gio et quelques octets (affichage arrondi vers le bas).
            used: (29 * G + G * 6 / 10) as i64 + 1_000,
            cap: 30 * G as i64,
        };
        assert!(usage.alert());
        assert_eq!(usage.percent(), 98);
        assert_eq!(
            usage.full_message(),
            "Espace plein : 29,6 Go utilisés sur 30 Go. Supprime des fichiers ou demande à l'administrateur d'augmenter ton plafond."
        );
        let calm = Usage {
            used: (12 * G + G * 4 / 10) as i64 + 1_000,
            cap: 30 * G as i64,
        };
        assert!(!calm.alert());
        assert_eq!(calm.label(), "12,4 Go / 30 Go");
        assert!(calm.fits(calm.room()) && !calm.fits(calm.room() + 1));
    }
}
