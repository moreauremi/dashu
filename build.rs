//! Prépare les fichiers statiques embarqués dans le binaire.
//!
//! Pour chaque fichier de `static/` :
//!   1. on calcule une empreinte de son contenu,
//!   2. on l'insère dans le nom (`css/app.css` -> `css/app.3fa91c20d4.css`),
//!   3. on génère `assets.rs`, une table que `src/assets.rs` inclut.
//!
//! Le nom changeant dès que le contenu change, le navigateur peut garder ces
//! fichiers en cache un an sans jamais servir une vieille version.
//!
//! On génère aussi `grid.css`, les classes de placement de l'emploi du temps :
//! la Content-Security-Policy interdit les attributs `style`, donc la position
//! d'un cours dans la grille passe par des classes (`gs24` = commence à la
//! 24e tranche de 5 minutes) plutôt que par un `style="top: …"`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let static_dir = manifest.join("static");

    println!("cargo:rerun-if-changed=static");
    // sqlx::migrate!() lit ce dossier à la compilation.
    println!("cargo:rerun-if-changed=migrations");

    let grid = grid_css();
    fs::write(out.join("grid.css"), &grid).unwrap();

    let mut files = Vec::new();
    walk(&static_dir, &static_dir, &mut files);
    files.sort();

    // Les CSS et le manifeste de l'app passent en dernier : ils référencent
    // polices et images, dont il faut d'abord connaître le nom avec empreinte.
    let (css, others): (Vec<_>, Vec<_>) = files
        .into_iter()
        .partition(|(p, _)| p.ends_with(".css") || p.ends_with(".webmanifest"));

    let mut entries: Vec<(String, String, PathBuf)> = Vec::new();
    for (logical, path) in others {
        let bytes = fs::read(&path).unwrap();
        entries.push((logical.clone(), hashed_name(&logical, &bytes), path));
    }
    for (logical, path) in css {
        let mut text = fs::read_to_string(&path).unwrap();
        for (other, hashed, _) in &entries {
            text = text.replace(&format!("/static/{other}"), &format!("/static/{hashed}"));
        }
        if logical == "css/app.css" {
            text.push_str(&grid);
        }
        let processed = out.join(logical.replace('/', "_"));
        fs::write(&processed, &text).unwrap();
        entries.push((
            logical.clone(),
            hashed_name(&logical, text.as_bytes()),
            processed,
        ));
    }
    entries.sort();

    let mut code = String::from("pub static ASSETS: &[Asset] = &[\n");
    for (logical, hashed, path) in &entries {
        writeln!(
            code,
            "    Asset {{ path: {logical:?}, url: {url:?}, mime: {mime:?}, bytes: include_bytes!({file:?}) }},",
            url = format!("/static/{hashed}"),
            mime = mime(logical),
            file = path.to_str().unwrap(),
        )
        .unwrap();
    }
    code.push_str("];\n");
    fs::write(out.join("assets.rs"), code).unwrap();
}

fn walk(root: &Path, dir: &Path, files: &mut Vec<(String, PathBuf)>) {
    let Ok(read) = fs::read_dir(dir) else { return };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(root, &path, files);
        } else {
            let logical = path
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .unwrap()
                .replace('\\', "/");
            files.push((logical, path));
        }
    }
}

/// `css/app.css` + contenu -> `css/app.3fa91c20d4.css`
fn hashed_name(logical: &str, bytes: &[u8]) -> String {
    // FNV-1a 64 bits : pas besoin d'un hachage cryptographique pour une empreinte de cache.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let fingerprint = &format!("{hash:016x}")[..10];
    match logical.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}.{fingerprint}.{ext}"),
        None => format!("{logical}.{fingerprint}"),
    }
}

fn mime(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "svg" => "image/svg+xml",
        "woff2" => "font/woff2",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "webmanifest" => "application/manifest+json",
        _ => "application/octet-stream",
    }
}

/// Classes de placement de l'emploi du temps. Une journée est une grille CSS
/// dont chaque rangée vaut 5 minutes (288 rangées pour 24 h).
fn grid_css() -> String {
    let mut css =
        String::from("\n/* --- Grille de l'emploi du temps : généré par build.rs --- */\n");
    // Nombre d'heures affichées -> nombre de rangées.
    for hours in 1..=24 {
        writeln!(
            css,
            ".tgh{hours}{{grid-template-rows:repeat({},var(--slot))}}",
            hours * 12
        )
        .unwrap();
    }
    // Début et fin d'un événement, en tranches de 5 minutes depuis le haut de la grille.
    for slot in 0..=288 {
        writeln!(css, ".gs{slot}{{grid-row-start:{}}}", slot + 1).unwrap();
        writeln!(css, ".ge{slot}{{grid-row-end:{}}}", slot + 1).unwrap();
    }
    css
}
