//! PaddleOCR (PP-OCRv5 mobile, Apache-2.0) on ONNX Runtime: text detection (DB) then line
//! recognition (CTC). Tuned for screenshots the way gyotaku found works: detect on a downscaled
//! copy (never upscale), recognise crops from the full-resolution image, axis-aligned boxes.

use crate::types::OcrLine;
use image::{DynamicImage, GenericImageView, RgbImage, imageops::FilterType};
use ort::session::Session;
use ort::value::Tensor;
use std::path::Path;

/// (file in the models dir, URL, approximate size for progress)
pub const FILES: [(&str, &str, u64); 3] = [
    ("paddle/det.onnx", DET_URL, 4_826_518),
    ("paddle/rec.onnx", REC_URL, 7_848_423),
    ("paddle/dict.txt", DICT_URL, 1_416),
];
// Official PaddlePaddle exports. The English recogniser (438 classes) instead of the multilingual one
// (18,385 classes): its output tensor is 40× smaller per character, which is most of the memory.
const DET_URL: &str = "https://huggingface.co/PaddlePaddle/PP-OCRv5_mobile_det_onnx/resolve/main/inference.onnx";
const REC_URL: &str = "https://huggingface.co/PaddlePaddle/en_PP-OCRv5_mobile_rec_onnx/resolve/main/inference.onnx";
// Byte-identical to PaddleOCR's ppocr/utils/dict/en_dict.txt (the official repo embeds it in YAML only).
const DICT_URL: &str = "https://huggingface.co/monkt/paddleocr-onnx/resolve/main/languages/english/dict.txt";

/// Detection input is capped at about this many pixels: screen text is legible at native size,
/// and upscaling only costs memory.
const DET_MAX_PIXELS: f32 = 1_200_000.0;
const THRESH: f32 = 0.3;
const BOX_THRESH: f32 = 0.6;
const UNCLIP: f32 = 1.5;
const MIN_SIDE: u32 = 3;
const REC_H: u32 = 48;
const REC_MAX_W: u32 = 3200;
const REC_BATCH: usize = 8;
const MIN_SCORE: f32 = 0.5;

/// Width of a crop once scaled to the recogniser's height.
fn rec_width(w: u32, h: u32) -> u32 {
    ((REC_H as f32 * w as f32 / h.max(1) as f32).ceil() as u32).clamp(8, REC_MAX_W)
}

pub fn cached(models: &Path) -> bool {
    FILES.iter().all(|(f, _, _)| models.join(f).exists())
}

pub struct Paddle {
    det: Session,
    rec: Session,
    /// Class i (1-based) → dict[i - 1]; class 0 is the CTC blank; the last class is a space.
    dict: Vec<String>,
}

impl Paddle {
    pub fn load(models: &Path) -> anyhow::Result<Self> {
        let mut dict: Vec<String> =
            std::fs::read_to_string(models.join(FILES[2].0))?.lines().map(|l| l.trim_end_matches('\r').to_string()).collect();
        dict.push(" ".into());
        let session = |f: &str, arena| crate::engine::onnx::session(&models.join(f), true, arena);
        // Detection runs once per image on a large input: without an arena its ~250 MB of
        // activations are freed after each run. Recognition runs many times on small inputs, where
        // an arena makes it about 6x faster for little memory.
        Ok(Self { det: session(FILES[0].0, false)?, rec: session(FILES[1].0, true)?, dict })
    }

    pub fn recognize(&mut self, img: &DynamicImage) -> anyhow::Result<Vec<OcrLine>> {
        let (w, h) = img.dimensions();
        let t0 = std::time::Instant::now();
        let boxes = self.detect(img)?;
        let t_det = t0.elapsed().as_millis();
        let rgb = img.to_rgb8();
        let mut lines = Vec::with_capacity(boxes.len());
        // Batch by similar width so padding stays small.
        let mut order: Vec<usize> = (0..boxes.len()).collect();
        order.sort_by(|&a, &b| {
            let ra = boxes[a].2 as f32 / boxes[a].3 as f32;
            let rb = boxes[b].2 as f32 / boxes[b].3 as f32;
            ra.total_cmp(&rb)
        });
        let mut texts: Vec<Option<(String, f32)>> = vec![None; boxes.len()];
        // Batches of 8 similar widths. Smaller batches use less memory but read worse: the model reads
        // a long line that fills its input exactly with letters missing ("checkd").
        for chunk in order.chunks(REC_BATCH) {
            let crops: Vec<RgbImage> = chunk
                .iter()
                .map(|&i| {
                    let (x, y, bw, bh) = boxes[i];
                    image::imageops::crop_imm(&rgb, x, y, bw, bh).to_image()
                })
                .collect();
            for (k, r) in self.read(&crops)?.into_iter().enumerate() {
                texts[chunk[k]] = r;
            }
        }
        if std::env::var_os("MAGPIE_OCR_DEBUG").is_some() {
            eprintln!("    paddle: detect {t_det} ms, {} boxes, recognise {} ms", boxes.len(), t0.elapsed().as_millis() - t_det);
        }
        for (i, (x, y, bw, bh)) in boxes.into_iter().enumerate() {
            if let Some((t, score)) = &texts[i] {
                if *score >= MIN_SCORE && !t.trim().is_empty() {
                    lines.push(OcrLine {
                        t: t.trim().to_string(),
                        x: x as f32 / w as f32,
                        y: y as f32 / h as f32,
                        w: bw as f32 / w as f32,
                        h: bh as f32 / h as f32,
                    });
                }
            }
        }
        Ok(lines)
    }

    /// Text boxes (x, y, w, h) in full-resolution pixels, in reading order.
    fn detect(&mut self, img: &DynamicImage) -> anyhow::Result<Vec<(u32, u32, u32, u32)>> {
        let (w, h) = img.dimensions();
        let scale = (DET_MAX_PIXELS / (w * h) as f32).sqrt().min(1.0);
        let round32 = |v: f32| ((v / 32.0).round() as u32).max(1) * 32;
        let (dw, dh) = (round32(w as f32 * scale), round32(h as f32 * scale));
        let small = img.resize_exact(dw, dh, FilterType::Triangle).to_rgb8();
        // ImageNet normalization on BGR, as PaddleOCR trains (OpenCV channel order).
        const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
        const STD: [f32; 3] = [0.229, 0.224, 0.225];
        let plane = (dw * dh) as usize;
        let mut input = vec![0f32; 3 * plane];
        for (i, p) in small.pixels().enumerate() {
            let bgr = [p[2], p[1], p[0]];
            for c in 0..3 {
                input[c * plane + i] = (f32::from(bgr[c]) / 255.0 - MEAN[c]) / STD[c];
            }
        }
        let name = self.det.inputs()[0].name().to_string();
        let tensor = Tensor::from_array(([1usize, 3, dh as usize, dw as usize], input))?;
        let outputs = self.det.run(ort::inputs![name.as_str() => tensor])?;
        let (_, prob) = outputs[0].try_extract_tensor::<f32>()?;

        // Connected components of the thresholded map → axis-aligned boxes (screen text is horizontal).
        let (mw, mh) = (dw as usize, dh as usize);
        let mut seen = vec![false; mw * mh];
        let mut stack = Vec::new();
        let mut boxes = Vec::new();
        for start in 0..mw * mh {
            if seen[start] || prob[start] <= THRESH {
                continue;
            }
            let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
            let (mut sum, mut n) = (0f32, 0u32);
            seen[start] = true;
            stack.push(start);
            while let Some(i) = stack.pop() {
                let (x, y) = (i % mw, i / mw);
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
                sum += prob[i];
                n += 1;
                let mut visit = |j: usize| {
                    if !seen[j] && prob[j] > THRESH {
                        seen[j] = true;
                        stack.push(j);
                    }
                };
                if x > 0 {
                    visit(i - 1);
                }
                if x + 1 < mw {
                    visit(i + 1);
                }
                if y > 0 {
                    visit(i - mw);
                }
                if y + 1 < mh {
                    visit(i + mw);
                }
            }
            let (bw, bh) = ((x1 - x0 + 1) as f32, (y1 - y0 + 1) as f32);
            if sum / (n as f32) < BOX_THRESH || bw.min(bh) < (MIN_SIDE as f32) {
                continue;
            }
            // DB "unclip": the map marks a shrunken text core; grow it back by area × ratio / perimeter.
            let d = bw * bh * UNCLIP / (2.0 * (bw + bh));
            let (sx, sy) = (w as f32 / dw as f32, h as f32 / dh as f32);
            let bx0 = ((x0 as f32 - d) * sx).max(0.0);
            let by0 = ((y0 as f32 - d) * sy).max(0.0);
            let bx1 = ((x1 as f32 + 1.0 + d) * sx).min(w as f32);
            let by1 = ((y1 as f32 + 1.0 + d) * sy).min(h as f32);
            if bx1 - bx0 >= MIN_SIDE as f32 && by1 - by0 >= MIN_SIDE as f32 {
                boxes.push((bx0 as u32, by0 as u32, (bx1 - bx0) as u32, (by1 - by0) as u32));
            }
        }
        // Reading order: group into rows (a box whose vertical centre falls inside the row's first box
        // joins that row), rows top to bottom, boxes left to right.
        boxes.sort_by_key(|b| (b.1, b.0));
        let mut rows: Vec<Vec<(u32, u32, u32, u32)>> = Vec::new();
        for b in boxes {
            let centre = b.1 + b.3 / 2;
            match rows.last_mut() {
                Some(row) if centre < row[0].1 + row[0].3 => row.push(b),
                _ => rows.push(vec![b]),
            }
        }
        Ok(rows
            .into_iter()
            .flat_map(|mut r| {
                r.sort_by_key(|b| b.0);
                r
            })
            .collect())
    }

    /// CTC greedy decode of a batch of line crops: (text, mean confidence) per crop.
    fn read(&mut self, crops: &[RgbImage]) -> anyhow::Result<Vec<Option<(String, f32)>>> {
        let widths: Vec<u32> = crops.iter().map(|c| rec_width(c.width(), c.height())).collect();
        let bw = *widths.iter().max().unwrap_or(&8);
        let n = crops.len();
        let plane = (REC_H * bw) as usize;
        // Padding stays 0, which is mid-grey after normalization, as in PaddleOCR.
        let mut input = vec![0f32; n * 3 * plane];
        for (k, c) in crops.iter().enumerate() {
            let r = image::imageops::resize(c, widths[k], REC_H, FilterType::Triangle);
            for (x, y, p) in r.enumerate_pixels() {
                let bgr = [p[2], p[1], p[0]];
                for ch in 0..3 {
                    input[k * 3 * plane + ch * plane + (y * bw + x) as usize] = f32::from(bgr[ch]) / 127.5 - 1.0;
                }
            }
        }
        let name = self.rec.inputs()[0].name().to_string();
        let tensor = Tensor::from_array(([n, 3usize, REC_H as usize, bw as usize], input))?;
        let outputs = self.rec.run(ort::inputs![name.as_str() => tensor])?;
        let (shape, probs) = outputs[0].try_extract_tensor::<f32>()?;
        let (t, classes) = (shape[1] as usize, shape[2] as usize);
        let mut out = Vec::with_capacity(n);
        for k in 0..n {
            let mut text = String::new();
            let (mut conf, mut kept, mut prev) = (0f32, 0u32, 0usize);
            // Steps past this crop's own width only see padding.
            let steps = ((widths[k] as f32 / bw as f32) * t as f32).ceil() as usize;
            for s in 0..steps.min(t) {
                let row = &probs[(k * t + s) * classes..(k * t + s + 1) * classes];
                let (best, p) = row.iter().enumerate().fold((0, f32::MIN), |m, (i, &v)| if v > m.1 { (i, v) } else { m });
                if best != 0 && best != prev {
                    if let Some(ch) = self.dict.get(best - 1) {
                        text.push_str(ch);
                        conf += p;
                        kept += 1;
                    }
                }
                prev = best;
            }
            out.push((kept > 0).then(|| (text, conf / kept as f32)));
        }
        Ok(out)
    }
}
