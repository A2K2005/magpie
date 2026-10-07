//! CLIP ViT-B/32 (MIT; int8 ONNX export by Xenova) on ONNX Runtime. Each encoder loads on first
//! use and is dropped after a minute idle, so visual search costs memory only while it is used.

use ort::session::Session;
use ort::value::Tensor;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokenizers::Tokenizer;

pub const DIM: usize = 512;
/// (file in the models dir, URL, approximate size for progress)
pub const FILES: [(&str, &str, u64); 3] = [
    ("tokenizer.json", "https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/main/tokenizer.json", 2_200_000),
    (
        "onnx/text_model_quantized.onnx",
        "https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/main/onnx/text_model_quantized.onnx",
        64_200_000,
    ),
    (
        "onnx/vision_model_quantized.onnx",
        "https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/main/onnx/vision_model_quantized.onnx",
        88_200_000,
    ),
];
const MAX_TOKENS: usize = 77;

pub fn cached(dir: &Path) -> bool {
    FILES.iter().all(|(f, _, _)| dir.join(f).exists())
}

fn normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
    v.iter_mut().for_each(|x| *x /= n);
}

pub struct Clip {
    dir: PathBuf,
    /// Indexing (background priority) rather than a query the user is waiting on.
    background: bool,
    text: Option<(Session, Tokenizer, Instant)>,
    vision: Option<(Session, Instant)>,
}

impl Clip {
    pub fn new(dir: PathBuf, background: bool) -> Self {
        Self { dir, background, text: None, vision: None }
    }

    pub fn embed_text(&mut self, q: &str) -> anyhow::Result<Vec<f32>> {
        if self.text.is_none() {
            let tok = Tokenizer::from_file(self.dir.join(FILES[0].0)).map_err(|e| anyhow::anyhow!(e))?;
            self.text = Some((super::onnx::session(&self.dir.join(FILES[1].0), self.background, true)?, tok, Instant::now()));
        }
        let (sess, tok, used) = self.text.as_mut().unwrap();
        *used = Instant::now();
        let enc = tok.encode(q, true).map_err(|e| anyhow::anyhow!(e))?;
        let n = enc.get_ids().len().min(MAX_TOKENS);
        let ids: Vec<i64> = enc.get_ids()[..n].iter().map(|&i| i64::from(i)).collect();
        let mask: Vec<i64> = enc.get_attention_mask()[..n].iter().map(|&i| i64::from(i)).collect();
        let wants_mask = sess.inputs().iter().any(|i| i.name() == "attention_mask");
        let ids = Tensor::from_array(([1usize, n], ids))?;
        let outputs = if wants_mask {
            let mask = Tensor::from_array(([1usize, n], mask))?;
            sess.run(ort::inputs!["input_ids" => ids, "attention_mask" => mask])?
        } else {
            sess.run(ort::inputs!["input_ids" => ids])?
        };
        let (_, data) = outputs["text_embeds"].try_extract_tensor::<f32>()?;
        let mut v = data[..DIM].to_vec();
        normalize(&mut v);
        Ok(v)
    }

    /// `pixels`: CHW 3×224×224 each (see `image::clip_pixels`).
    pub fn embed_images(&mut self, pixels: &[Vec<f32>]) -> anyhow::Result<Vec<Vec<f32>>> {
        if pixels.is_empty() {
            return Ok(vec![]);
        }
        if self.vision.is_none() {
            self.vision = Some((super::onnx::session(&self.dir.join(FILES[2].0), self.background, true)?, Instant::now()));
        }
        let (sess, used) = self.vision.as_mut().unwrap();
        *used = Instant::now();
        let flat: Vec<f32> = pixels.concat();
        let input = Tensor::from_array(([pixels.len(), 3usize, 224, 224], flat))?;
        let outputs = sess.run(ort::inputs!["pixel_values" => input])?;
        let (_, data) = outputs["image_embeds"].try_extract_tensor::<f32>()?;
        Ok(data
            .chunks_exact(DIM)
            .map(|c| {
                let mut v = c.to_vec();
                normalize(&mut v);
                v
            })
            .collect())
    }

    /// Drops encoders unused for `idle`. Returns true if anything was released.
    pub fn release_idle(&mut self, idle: Duration) -> bool {
        let mut released = false;
        if self.text.as_ref().is_some_and(|t| t.2.elapsed() > idle) {
            self.text = None;
            released = true;
        }
        if self.vision.as_ref().is_some_and(|v| v.1.elapsed() > idle) {
            self.vision = None;
            released = true;
        }
        released
    }

    pub fn loaded(&self) -> bool {
        self.text.is_some() || self.vision.is_some()
    }
}
