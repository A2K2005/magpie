//! OCR through the operating system: Windows.Media.Ocr on Windows, Vision on macOS. No models to
//! download, and both engines run outside our heap. On Windows, PaddleOCR later re-reads each image
//! more accurately (see `paddle`); QR codes are decoded on every platform.

use crate::types::OcrLine;
use image::{DynamicImage, GenericImageView};

pub mod paddle;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;
#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    pub fn recognize(_: &image::DynamicImage) -> anyhow::Result<Vec<crate::types::OcrLine>> {
        anyhow::bail!("OCR is not available on this OS")
    }
    pub fn init_thread() {}
}

pub use platform::init_thread;

const OVERLAP: u32 = 120;
/// Both engines lose small text when a huge image is scaled down internally; keep inputs modest.
const MAX_SIDE: u32 = 4000;

type Engine<'a> = &'a mut dyn FnMut(&DynamicImage) -> anyhow::Result<Vec<OcrLine>>;

/// Lines with normalized boxes, read by the OS engine.
pub fn recognize(img: &DynamicImage) -> anyhow::Result<Vec<OcrLine>> {
    recognize_with(img, &mut platform::recognize)
}

/// Lines with normalized boxes. Tall scrolling screenshots are read in overlapping tiles.
pub fn recognize_with(img: &DynamicImage, engine: Engine) -> anyhow::Result<Vec<OcrLine>> {
    let (w, h) = img.dimensions();
    let lines = if h <= 2400 || f64::from(h) <= f64::from(w) * 2.2 {
        if w.max(h) > MAX_SIDE {
            engine(&img.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Triangle))?
        } else {
            engine(img)?
        }
    } else {
        tiled(img, w, h, engine)?
    };
    // One-character lines are almost always icons read as a letter.
    Ok(lines.into_iter().filter(|l| l.t.trim().chars().count() > 1).collect())
}

/// QR codes as lines reading "QR <content>", boxed where the code is.
pub fn qr_lines(img: &DynamicImage) -> Vec<OcrLine> {
    let (w, h) = img.dimensions();
    // Codes in screenshots are large; finding them on a smaller copy is much cheaper.
    let scale = (1600.0 / w.max(h) as f32).min(1.0);
    let gray = if scale < 1.0 {
        img.resize((w as f32 * scale) as u32, (h as f32 * scale) as u32, image::imageops::FilterType::Triangle).to_luma8()
    } else {
        img.to_luma8()
    };
    let (gw, gh) = (gray.width() as f32, gray.height() as f32);
    let mut prepared = rqrr::PreparedImage::prepare(gray);
    prepared
        .detect_grids()
        .into_iter()
        .filter_map(|g| {
            let (_, content) = g.decode().ok()?;
            let xs = g.bounds.iter().map(|p| p.x as f32);
            let ys = g.bounds.iter().map(|p| p.y as f32);
            let (x0, x1) = (xs.clone().fold(f32::MAX, f32::min), xs.fold(0.0, f32::max));
            let (y0, y1) = (ys.clone().fold(f32::MAX, f32::min), ys.fold(0.0, f32::max));
            Some(OcrLine {
                t: format!("QR {}", content.trim()),
                x: (x0 / gw).max(0.0),
                y: (y0 / gh).max(0.0),
                w: ((x1 - x0) / gw).min(1.0),
                h: ((y1 - y0) / gh).min(1.0),
            })
        })
        .collect()
}

/// `primary` plus the lines of `extra` that cover a region `primary` did not read, each placed
/// before the first primary line below it.
pub fn merge(mut primary: Vec<OcrLine>, extra: &[OcrLine]) -> Vec<OcrLine> {
    let overlap = |a: &OcrLine, b: &OcrLine| {
        let ix = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
        let iy = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
        if ix > 0.0 && iy > 0.0 { ix * iy } else { 0.0 }
    };
    for e in extra {
        let covered: f32 = primary.iter().map(|p| overlap(e, p)).sum();
        if covered < 0.3 * e.w * e.h {
            let at = primary.iter().position(|p| p.y > e.y).unwrap_or(primary.len());
            primary.insert(at, e.clone());
        }
    }
    primary
}

fn tiled(img: &DynamicImage, w: u32, h: u32, engine: Engine) -> anyhow::Result<Vec<OcrLine>> {
    let tile_h = 1400.max((f64::from(w) * 1.25) as u32);
    let mut out = Vec::new();
    let mut top = 0u32;
    loop {
        let th = tile_h.min(h - top);
        let last = top + th >= h;
        let tile = img.crop_imm(0, top, w, th);
        // Keep each line once: drop lines whose centre falls in the half of an overlap owned by the neighbour.
        let lo = if top == 0 { 0.0 } else { f64::from(top + OVERLAP / 2) };
        let hi = if last { f64::from(h) } else { f64::from(top + th - OVERLAP / 2) };
        for l in recognize_with(&tile, &mut *engine)? {
            let cy = f64::from(top) + f64::from(l.y + l.h / 2.0) * f64::from(th);
            if cy < lo || cy >= hi {
                continue;
            }
            out.push(OcrLine {
                y: ((f64::from(top) + f64::from(l.y) * f64::from(th)) / f64::from(h)) as f32,
                h: (f64::from(l.h) * f64::from(th) / f64::from(h)) as f32,
                ..l
            });
        }
        if last {
            break;
        }
        top += tile_h - OVERLAP;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(t: &str, x: f32, y: f32, w: f32, h: f32) -> OcrLine {
        OcrLine { t: t.into(), x, y, w, h }
    }

    #[test]
    fn merge_keeps_only_unread_regions() {
        let paddle = vec![line("Home Code", 0.0, 0.1, 0.5, 0.05), line("Settings", 0.0, 0.5, 0.3, 0.05)];
        let os = [line("Home coae", 0.0, 0.1, 0.5, 0.05), line("QR https://x.io", 0.6, 0.3, 0.2, 0.2)];
        let m = merge(paddle, &os);
        let texts: Vec<&str> = m.iter().map(|l| l.t.as_str()).collect();
        assert_eq!(texts, ["Home Code", "QR https://x.io", "Settings"]);
    }
}
