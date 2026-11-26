//! Contenu des fichiers fictifs de la démo, fabriqué à la volée : images
//! dessinées pixel par pixel, PDF d'une page, textes. Rien ne vient d'un vrai
//! fichier, donc rien de réel ne peut se glisser dans la démo.

use std::io::Cursor;

use image::{ImageFormat, Rgb, RgbImage};

/// Octets du fichier `name` d'après sa sorte (`kind` des fixtures).
pub fn render(name: &str, kind: &str, lines: &[String], text: &str) -> anyhow::Result<Vec<u8>> {
    let image = match kind {
        "landscape" => landscape(),
        "diagram" => diagram(),
        "rack" => rack(),
        "pdf" => return Ok(pdf(lines)),
        "text" => return Ok(text.as_bytes().to_vec()),
        other => anyhow::bail!("sorte de fichier inconnue dans les fixtures : {other}"),
    };
    let format = if name.ends_with(".png") {
        ImageFormat::Png
    } else {
        ImageFormat::Jpeg
    };
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, format)?;
    Ok(out.into_inner())
}

const W: u32 = 960;
const H: u32 = 640;

fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> Rgb<u8> {
    let c = |i: usize| (f32::from(a[i]) + (f32::from(b[i]) - f32::from(a[i])) * t).round() as u8;
    Rgb([c(0), c(1), c(2)])
}

fn fill_rect(img: &mut RgbImage, x: u32, y: u32, w: u32, h: u32, color: [u8; 3]) {
    for py in y..(y + h).min(img.height()) {
        for px in x..(x + w).min(img.width()) {
            img.put_pixel(px, py, Rgb(color));
        }
    }
}

fn disc(img: &mut RgbImage, cx: i32, cy: i32, r: i32, color: [u8; 3]) {
    for y in (cy - r).max(0)..(cy + r).min(img.height() as i32) {
        for x in (cx - r).max(0)..(cx + r).min(img.width() as i32) {
            if (x - cx).pow(2) + (y - cy).pow(2) <= r * r {
                img.put_pixel(x as u32, y as u32, Rgb(color));
            }
        }
    }
}

fn line(img: &mut RgbImage, (x0, y0): (i32, i32), (x1, y1): (i32, i32), color: [u8; 3]) {
    let steps = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
    for i in 0..=steps {
        let x = x0 + (x1 - x0) * i / steps;
        let y = y0 + (y1 - y0) * i / steps;
        disc(img, x, y, 2, color);
    }
}

/// Coucher de soleil sur des collines.
fn landscape() -> RgbImage {
    let mut img = RgbImage::new(W, H);
    for y in 0..H {
        let color = mix([246, 168, 92], [92, 64, 140], y as f32 / H as f32);
        for x in 0..W {
            img.put_pixel(x, y, color);
        }
    }
    disc(&mut img, 640, 330, 70, [255, 226, 160]);
    let layers: [([u8; 3], f32, f32, f32); 3] = [
        ([120, 76, 120], 380.0, 40.0, 0.006),
        ([74, 48, 92], 450.0, 30.0, 0.011),
        ([36, 28, 52], 530.0, 22.0, 0.017),
    ];
    for (color, base, amp, freq) in layers {
        for x in 0..W {
            let top =
                base + amp * (x as f32 * freq).sin() + amp / 2.0 * (x as f32 * freq * 2.7).cos();
            for y in (top.max(0.0) as u32)..H {
                img.put_pixel(x, y, Rgb(color));
            }
        }
    }
    img
}

/// Schéma de réseau : un routeur, un switch, des postes.
fn diagram() -> RgbImage {
    let mut img = RgbImage::from_pixel(W, H, Rgb([250, 250, 247]));
    let ink = [60, 64, 72];
    let router = (480, 110);
    let switch = (480, 300);
    line(&mut img, router, switch, ink);
    let hosts = [(160, 500), (373, 500), (586, 500), (800, 500)];
    for host in hosts {
        line(&mut img, switch, host, ink);
    }
    let boxes = [(router, [37, 99, 235]), (switch, [22, 163, 74])];
    for ((x, y), color) in boxes {
        fill_rect(&mut img, (x - 90) as u32, (y - 40) as u32, 180, 80, color);
    }
    for (x, y) in hosts {
        fill_rect(
            &mut img,
            (x - 60) as u32,
            (y - 40) as u32,
            120,
            80,
            [234, 88, 12],
        );
        fill_rect(
            &mut img,
            (x - 48) as u32,
            (y - 30) as u32,
            96,
            50,
            [255, 237, 213],
        );
    }
    img
}

/// Baie informatique : des équipements empilés et leurs voyants.
fn rack() -> RgbImage {
    let mut img = RgbImage::from_pixel(W, H, Rgb([38, 40, 46]));
    fill_rect(&mut img, 300, 40, 360, 560, [22, 23, 27]);
    for unit in 0..9 {
        let y = 60 + unit * 60;
        fill_rect(&mut img, 320, y, 320, 48, [70, 74, 84]);
        for led in 0..6 {
            let color = if (unit + led) % 4 == 0 {
                [250, 190, 40]
            } else {
                [60, 220, 110]
            };
            disc(&mut img, 340 + led as i32 * 18, y as i32 + 24, 4, color);
        }
        fill_rect(&mut img, 470, y + 14, 150, 20, [44, 47, 54]);
    }
    img
}

/// PDF d'une page : les lignes en Helvetica, sans accent (police standard).
fn pdf(lines: &[String]) -> Vec<u8> {
    let mut content = String::from("BT /F1 13 Tf 16 TL 60 780 Td\n");
    for (i, line) in lines.iter().enumerate() {
        let safe: String = line
            .chars()
            .map(|c| {
                if c.is_ascii() && !c.is_ascii_control() {
                    c
                } else {
                    '?'
                }
            })
            .collect::<String>()
            .replace('\\', "\\\\")
            .replace('(', "\\(")
            .replace(')', "\\)");
        if i == 0 {
            content.push_str(&format!("/F1 20 Tf ({safe}) Tj /F1 13 Tf T* T*\n"));
        } else {
            content.push_str(&format!("({safe}) Tj T*\n"));
        }
    }
    content.push_str("ET\n");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
    ];
    let mut out = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.push_str(&format!("{} 0 obj\n{object}\nendobj\n", i + 1));
    }
    let xref = out.len();
    out.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for offset in offsets {
        out.push_str(&format!("{offset:010} 00000 n \n"));
    }
    out.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    ));
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fichiers_fabriques() {
        let png = render("a.png", "diagram", &[], "").unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        let jpg = render("a.jpg", "landscape", &[], "").unwrap();
        assert!(jpg.starts_with(&[0xFF, 0xD8]));
        let pdf = render("a.pdf", "pdf", &["Titre".into(), "(ligne)".into()], "").unwrap();
        assert!(pdf.starts_with(b"%PDF-1.4") && pdf.ends_with(b"%%EOF\n"));
        assert!(render("a.bin", "inconnu", &[], "").is_err());
    }
}
