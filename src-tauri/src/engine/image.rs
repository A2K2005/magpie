//! Decoding, thumbnails, perceptual hash, dominant colors and CLIP input.

use super::colors::Color;
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageDecoder, ImageReader, RgbImage};
use std::collections::HashMap;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

pub struct Analysis {
    /// Upright full-resolution image, for OCR.
    pub img: DynamicImage,
    pub dhash: String,
    pub colors: Vec<Color>,
}

/// Larger images are not decoded (a 100-megapixel panorama needs 400 MB); they stay findable by name.
const MAX_DECODE_BYTES: u64 = 200 << 20;

/// Decodes a file upright (EXIF orientation applied).
pub fn decode(path: &Path) -> anyhow::Result<DynamicImage> {
    let mut decoder = ImageReader::open(path)?.with_guessed_format()?.into_decoder()?;
    let (w, h) = decoder.dimensions();
    if u64::from(w) * u64::from(h) * 4 > MAX_DECODE_BYTES {
        anyhow::bail!("Too large to read ({w}x{h})");
    }
    let orientation = decoder.orientation()?;
    let mut img = DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    Ok(img)
}

pub fn decode_bytes(bytes: &[u8]) -> anyhow::Result<DynamicImage> {
    Ok(ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?.decode()?)
}

/// Writes the 480 px-wide JPEG thumbnail and computes the hash and colors from it.
pub fn analyze(path: &Path, thumb: &Path) -> anyhow::Result<Analysis> {
    let img = decode(path)?;
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        anyhow::bail!("Empty image");
    }
    let t = write_thumb(&img, thumb)?;
    Ok(Analysis { dhash: dhash(&t), colors: dominant_colors(&t), img })
}

/// The 480 px-wide JPEG thumbnail, written whole or not at all: the indexer and the asset server
/// can both make one, and a reader must never see half a file.
pub fn write_thumb(img: &DynamicImage, thumb: &Path) -> anyhow::Result<RgbImage> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let t = img.thumbnail(480, 2400).to_rgb8();
    let tmp = thumb.with_extension(format!("{}.part", SEQ.fetch_add(1, Relaxed)));
    let mut out = BufWriter::new(File::create(&tmp)?);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80).encode_image(&t)?;
    drop(out);
    std::fs::rename(&tmp, thumb)?;
    Ok(t)
}

/// 64-bit difference hash as 16 hex chars. Near-identical screenshots differ in a few bits.
fn dhash(t: &RgbImage) -> String {
    let g = image::imageops::resize(&DynamicImage::ImageRgb8(t.clone()).to_luma8(), 9, 8, FilterType::Triangle);
    let mut bits: u64 = 0;
    for y in 0..8 {
        for x in 0..8 {
            bits = (bits << 1) | u64::from(g.get_pixel(x, y)[0] > g.get_pixel(x + 1, y)[0]);
        }
    }
    format!("{bits:016x}")
}

pub fn hamming(a: &str, b: &str) -> u32 {
    match (u64::from_str_radix(a, 16), u64::from_str_radix(b, 16)) {
        (Ok(a), Ok(b)) => (a ^ b).count_ones(),
        _ => 64,
    }
}

fn dominant_colors(t: &RgbImage) -> Vec<Color> {
    let small = image::imageops::resize(t, 48, 48, FilterType::Nearest);
    // 4 bits per channel → 4096 bins; each bin keeps sums so its color is the true average.
    let mut bins: HashMap<u16, [u32; 4]> = HashMap::new();
    for p in small.pixels() {
        let key = (u16::from(p[0] >> 4) << 8) | (u16::from(p[1] >> 4) << 4) | u16::from(p[2] >> 4);
        let b = bins.entry(key).or_default();
        b[0] += u32::from(p[0]);
        b[1] += u32::from(p[1]);
        b[2] += u32::from(p[2]);
        b[3] += 1;
    }
    let total = (small.width() * small.height()) as f32;
    let mut v: Vec<[u32; 4]> = bins.into_values().collect();
    v.sort_by(|a, b| b[3].cmp(&a[3]));
    v.into_iter()
        .take(5)
        .filter(|b| b[3] as f32 / total >= 0.03)
        .map(|[r, g, b, n]| Color {
            hex: format!("#{:02x}{:02x}{:02x}", r / n, g / n, b / n),
            share: (b_share(n, total) * 1000.0).round() / 1000.0,
        })
        .collect()
}

fn b_share(n: u32, total: f32) -> f32 {
    n as f32 / total
}

const MEAN: [f32; 3] = [0.481_454_66, 0.457_827_5, 0.408_210_73];
const STD: [f32; 3] = [0.268_629_54, 0.261_302_58, 0.275_777_1];

/// CLIP input: letterboxed to 224×224 (wide screenshots are not center-cropped), normalized, CHW.
pub fn clip_pixels(img: &DynamicImage) -> Vec<f32> {
    let fit = img.resize(224, 224, FilterType::Triangle).to_rgb8();
    let mut canvas = RgbImage::from_pixel(224, 224, image::Rgb([127, 127, 127]));
    let (x, y) = ((224 - fit.width()) / 2, (224 - fit.height()) / 2);
    image::imageops::overlay(&mut canvas, &fit, i64::from(x), i64::from(y));
    let mut out = vec![0f32; 3 * 224 * 224];
    for (i, p) in canvas.pixels().enumerate() {
        for c in 0..3 {
            out[c * 224 * 224 + i] = (f32::from(p[c]) / 255.0 - MEAN[c]) / STD[c];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_distance() {
        assert_eq!(hamming("ffffffffffffffff", "fffffffffffffff0"), 4);
        assert_eq!(hamming("0", "zz"), 64);
    }
}
