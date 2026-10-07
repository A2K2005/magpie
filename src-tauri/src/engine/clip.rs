//! CLIP ViT-B/32 (MIT; int8 ONNX export by Xenova) on ONNX Runtime. Each encoder loads on first
//! use and is dropped after a minute idle, so visual search costs memory only while it is used.

use ort::session::Session;
use ort::value::Tensor;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokenizers::Tokenizer;

pub const DIM: usize = 512;
/// Pinned model, tokenizer and preprocessing contract. Unknown legacy vectors are rebuilt.
pub const EMBEDDING_VERSION: &str = "clip-vit-b32-int8-d15189d7-single-image-v1";
// Verified against Hugging Face model metadata and the pinned tokenizer on 2026-10-07.
pub const SHA256: [(&str, &str); 3] = [
    (
        "tokenizer.json",
        "f7f3b7af117d467b58374797691a6438d3e6b9e9cef800dfd5dced7f697a90cd",
    ),
    (
        "onnx/text_model_quantized.onnx",
        "73baab855d406190da9faa498cfedf65f15cf309f4cc7385b7b032e6d08e5c3a",
    ),
    (
        "onnx/vision_model_quantized.onnx",
        "583fd1110a514667812fee7d684952aaf82a99b959760c8d7dca7e0ab9839299",
    ),
];
/// (file in the models dir, URL, approximate size for progress)
pub const FILES: [(&str, &str, u64); 3] = [
    (
        "tokenizer.json",
        "https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/d15189d7028b43f1d3e65039190477f6af591c2a/tokenizer.json",
        2_224_119,
    ),
    (
        "onnx/text_model_quantized.onnx",
        "https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/d15189d7028b43f1d3e65039190477f6af591c2a/onnx/text_model_quantized.onnx",
        64_504_507,
    ),
    (
        "onnx/vision_model_quantized.onnx",
        "https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/d15189d7028b43f1d3e65039190477f6af591c2a/onnx/vision_model_quantized.onnx",
        89_117_001,
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
        Self {
            dir,
            background,
            text: None,
            vision: None,
        }
    }

    pub fn embed_text(&mut self, q: &str) -> anyhow::Result<Vec<f32>> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.embed_text_inner(q))) {
            Ok(result) => result,
            Err(_) => {
                self.text = None;
                Err(anyhow::anyhow!(
                    "Visual text model failed. Repair models and retry."
                ))
            }
        }
    }

    fn embed_text_inner(&mut self, q: &str) -> anyhow::Result<Vec<f32>> {
        if self.text.is_none() {
            let tok =
                Tokenizer::from_file(self.dir.join(FILES[0].0)).map_err(|e| anyhow::anyhow!(e))?;
            self.text = Some((
                super::onnx::session(&self.dir.join(FILES[1].0), self.background, true)?,
                tok,
                Instant::now(),
            ));
        }
        let (sess, tok, used) = self.text.as_mut().unwrap();
        *used = Instant::now();
        let enc = tok.encode(q, true).map_err(|e| anyhow::anyhow!(e))?;
        let n = enc.get_ids().len().min(MAX_TOKENS);
        let ids: Vec<i64> = enc.get_ids()[..n].iter().map(|&i| i64::from(i)).collect();
        let mask: Vec<i64> = enc.get_attention_mask()[..n]
            .iter()
            .map(|&i| i64::from(i))
            .collect();
        let wants_mask = sess.inputs().iter().any(|i| i.name() == "attention_mask");
        let ids = Tensor::from_array(([1usize, n], ids))?;
        let outputs = if wants_mask {
            let mask = Tensor::from_array(([1usize, n], mask))?;
            sess.run(ort::inputs!["input_ids" => ids, "attention_mask" => mask])?
        } else {
            sess.run(ort::inputs!["input_ids" => ids])?
        };
        let (_, data) = outputs["text_embeds"].try_extract_tensor::<f32>()?;
        anyhow::ensure!(
            data.len() == DIM && data.iter().all(|v| v.is_finite()),
            "Invalid text embedding output"
        );
        let mut v = data.to_vec();
        normalize(&mut v);
        Ok(v)
    }

    /// `pixels`: CHW 3×224×224 each (see `image::clip_pixels`).
    pub fn embed_images(&mut self, pixels: &[Vec<f32>]) -> anyhow::Result<Vec<Vec<f32>>> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.embed_images_inner(pixels)
        })) {
            Ok(result) => result,
            Err(_) => {
                self.vision = None;
                Err(anyhow::anyhow!(
                    "Visual image model failed. Repair models and retry."
                ))
            }
        }
    }

    fn embed_images_inner(&mut self, pixels: &[Vec<f32>]) -> anyhow::Result<Vec<Vec<f32>>> {
        if pixels.is_empty() {
            return Ok(vec![]);
        }
        anyhow::ensure!(
            pixels
                .iter()
                .all(|p| p.len() == 3 * 224 * 224 && p.iter().all(|v| v.is_finite())),
            "Invalid image embedding input"
        );
        if self.vision.is_none() {
            self.vision = Some((
                super::onnx::session(&self.dir.join(FILES[2].0), self.background, true)?,
                Instant::now(),
            ));
        }
        let (sess, used) = self.vision.as_mut().unwrap();
        *used = Instant::now();
        // This int8 export produces batch-dependent outputs. A picture's
        // representation must not change with unrelated neighbors or queue timing.
        // Keep the loaded session and caller's scheduling batch, but infer one at a time.
        let mut embeddings = Vec::with_capacity(pixels.len());
        for image in pixels {
            let input = Tensor::from_array(([1usize, 3, 224, 224], image.clone()))?;
            let outputs = sess.run(ort::inputs!["pixel_values" => input])?;
            let (_, data) = outputs["image_embeds"].try_extract_tensor::<f32>()?;
            anyhow::ensure!(
                data.len() == DIM && data.iter().all(|v| v.is_finite()),
                "Invalid image embedding output"
            );
            let mut embedding = data.to_vec();
            normalize(&mut embedding);
            embeddings.push(embedding);
        }
        Ok(embeddings)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_inputs_are_validated_before_loading_models() {
        let mut clip = Clip::new(PathBuf::from("missing-test-models"), false);
        assert!(clip.embed_images(&[]).unwrap().is_empty());
        assert!(clip
            .embed_images(&[vec![0.0; 12]])
            .unwrap_err()
            .to_string()
            .contains("Invalid image embedding input"));
        let mut invalid = vec![0.0; 3 * 224 * 224];
        invalid[42] = f32::NAN;
        assert!(clip
            .embed_images(&[invalid])
            .unwrap_err()
            .to_string()
            .contains("Invalid image embedding input"));
        assert!(!clip.loaded());
    }
}
