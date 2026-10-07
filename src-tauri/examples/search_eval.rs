//! Real local-model evaluation. Never opens the user's library database or uploads images.
use anyhow::{ensure, Context};
use magpie_lib::engine::{
    clip::Clip,
    db, image,
    search::{self, Img, Semantic, Vectors, Vocab},
};
use magpie_lib::types::SearchRequest;
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Deserialize)]
struct Manifest {
    /// State the provenance and whether images are synthetic, public or private.
    description: String,
    images: Vec<ExampleImage>,
    queries: Vec<Query>,
}
#[derive(Deserialize)]
struct ExampleImage {
    id: String,
    path: PathBuf,
    #[serde(default)]
    text: String,
    #[serde(default)]
    note: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    collections: Vec<String>,
}
#[derive(Deserialize)]
struct Query {
    query: String,
    #[serde(default = "visual")]
    mode: String,
    relevant: Vec<String>,
    #[serde(default = "ten")]
    k: usize,
}
fn visual() -> String {
    "visual".into()
}
fn ten() -> usize {
    10
}

struct LocalModel {
    clip: RefCell<Clip>,
    error: RefCell<Option<String>>,
}
impl Semantic for LocalModel {
    fn state(&self) -> &'static str {
        "ready"
    }
    fn embed_text(&self, q: &str) -> Option<Vec<f32>> {
        match self.clip.borrow_mut().embed_text(q) {
            Ok(v) => Some(v),
            Err(e) => {
                *self.error.borrow_mut() = Some(e.to_string());
                None
            }
        }
    }
    fn embed_image(&self, img: Img) -> Option<Vec<f32>> {
        let result = (|| -> anyhow::Result<Vec<f32>> {
            let img = match img {
                Img::Path(path) => image::decode(Path::new(path))?,
                Img::Bytes(bytes) => image::decode_bytes(bytes)?,
            };
            self.clip
                .borrow_mut()
                .embed_images(&[image::clip_pixels(&img)])?
                .pop()
                .context("Empty image embedding")
        })();
        match result {
            Ok(v) => Some(v),
            Err(e) => {
                *self.error.borrow_mut() = Some(e.to_string());
                None
            }
        }
    }
    fn with_vectors<R>(&self, conn: &Connection, f: impl FnOnce(&Vectors) -> R) -> Option<R> {
        match Vectors::load(conn) {
            Ok(v) => Some(f(&v)),
            Err(e) => {
                *self.error.borrow_mut() = Some(e.to_string());
                None
            }
        }
    }
    fn with_vocab<R>(&self, conn: &Connection, f: impl FnOnce(&Vocab) -> R) -> Option<R> {
        match Vocab::load(conn) {
            Ok(v) => Some(f(&v)),
            Err(e) => {
                *self.error.borrow_mut() = Some(e.to_string());
                None
            }
        }
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    ensure!(
        (3..=4).contains(&args.len()),
        "Usage: search_eval <models_dir> <manifest.json> <report.json> [batch_size:1..8]"
    );
    let batch_size = args
        .get(3)
        .map(|v| v.parse::<usize>())
        .transpose()?
        .unwrap_or(8);
    ensure!(
        (1..=8).contains(&batch_size),
        "Batch size must be 1..8 (production maximum 8)"
    );
    let models = PathBuf::from(&args[0]);
    ensure!(
        magpie_lib::engine::clip::cached(&models),
        "Models missing: use the app to download CLIP first"
    );
    let manifest_path = Path::new(&args[1]).canonicalize()?;
    let manifest: Manifest = serde_json::from_slice(&std::fs::read(&manifest_path)?)?;
    ensure!(
        !manifest.images.is_empty() && !manifest.queries.is_empty(),
        "Manifest needs images and queries"
    );
    ensure!(
        !manifest.description.trim().is_empty(),
        "Describe corpus provenance"
    );
    let base = manifest_path.parent().context("Manifest has no parent")?;
    let conn = db::open(Path::new(":memory:"))?;
    let model = LocalModel {
        clip: RefCell::new(Clip::new(models.clone(), false)),
        error: RefCell::new(None),
    };
    let mut external: HashMap<i64, String> = HashMap::new();
    let mut known = HashSet::new();
    let mut indexing_ms = 0;
    let mut embedding_sha256 = std::collections::BTreeMap::new();
    for (chunk_index, chunk) in manifest.images.chunks(batch_size).enumerate() {
        let start = Instant::now();
        let mut prepared = vec![];
        for example in chunk {
            ensure!(
                !example.id.trim().is_empty() && known.insert(example.id.clone()),
                "Empty or duplicate image ID: {}",
                example.id
            );
            let path = base
                .join(&example.path)
                .canonicalize()
                .with_context(|| format!("Image {}", example.path.display()))?;
            let img = image::decode(&path)?;
            prepared.push((path, img.width(), img.height(), image::clip_pixels(&img)));
        }
        let pixels: Vec<_> = prepared
            .iter()
            .map(|(_, _, _, pixels)| pixels.clone())
            .collect();
        let embeddings = model.clip.borrow_mut().embed_images(&pixels)?;
        ensure!(
            embeddings.len() == chunk.len(),
            "Incorrect embedding batch length"
        );
        indexing_ms += start.elapsed().as_millis();
        for (index, ((example, (path, width, height, _)), embedding)) in
            chunk.iter().zip(prepared).zip(embeddings).enumerate()
        {
            let id = (chunk_index * batch_size + index) as i64 + 1;
            embedding_sha256.insert(
                example.id.clone(),
                format!("{:x}", Sha256::digest(db::to_blob(&embedding))),
            );
            let path_text = path.to_string_lossy();
            let folder = path
                .parent()
                .context("Image has no parent")?
                .to_string_lossy();
            let name = path
                .file_name()
                .context("Image has no filename")?
                .to_string_lossy();
            conn.execute("INSERT INTO shots(id,path,folder,name,mtime,size,width,height,stage,text,note,user_tags,collections,embedding,embedding_version)
                VALUES (?,?,?,?,0,0,?,?,2,?,?,?,?,?,?)", rusqlite::params![id, path_text, folder, name,
                    i64::from(width), i64::from(height), example.text, example.note,
                    serde_json::to_string(&example.tags)?, serde_json::to_string(&example.collections)?, db::to_blob(&embedding), magpie_lib::engine::clip::EMBEDDING_VERSION])?;
            external.insert(id, example.id.clone());
        }
    }
    let mut results = vec![];
    for case in &manifest.queries {
        ensure!(case.k > 0 && case.k <= 500, "k must be 1..500");
        ensure!(
            ["all", "text", "visual"].contains(&case.mode.as_str()),
            "Unknown mode {}",
            case.mode
        );
        let relevant: HashSet<_> = case.relevant.iter().collect();
        ensure!(
            relevant.len() == case.relevant.len(),
            "Duplicate relevance labels for {}",
            case.query
        );
        ensure!(
            relevant.iter().all(|id| known.contains(*id)),
            "Unknown relevance label for {}",
            case.query
        );
        let response = search::run(
            &conn,
            &SearchRequest {
                q: case.query.clone(),
                mode: Some(case.mode.clone()),
                limit: Some(case.k),
                ..Default::default()
            },
            None,
            &model,
        )?;
        ensure!(
            model.error.borrow().is_none(),
            "Model failed: {:?}",
            model.error.borrow()
        );
        let ids: Vec<_> = response
            .hits
            .iter()
            .map(|h| external.get(&h.shot.id).expect("known row"))
            .collect();
        let matched = ids.iter().filter(|id| relevant.contains(**id)).count();
        let reciprocal_rank = ids
            .iter()
            .position(|id| relevant.contains(*id))
            .map_or(0.0, |rank| 1.0 / (rank + 1) as f64);
        results.push(json!({"query":case.query,"mode":case.mode,"k":case.k,"relevant":case.relevant,
            "precision_at_k":matched as f64 / case.k as f64,
            "recall_at_k":if relevant.is_empty() { None } else { Some(matched as f64 / relevant.len() as f64) },
            "reciprocal_rank":if relevant.is_empty() { None } else { Some(reciprocal_rank) },
            "no_match_correct":if relevant.is_empty() { Some(ids.is_empty()) } else { None },
            "returned":ids,"scores":response.hits.iter().map(|h| h.score).collect::<Vec<_>>(),
            "evidence":response.hits.iter().map(|h| &h.evidence).collect::<Vec<_>>(),"took_ms":response.took_ms,"has_more":response.has_more}));
    }
    let model_files: Vec<_> = magpie_lib::engine::clip::FILES.iter().map(|(file, _, _)| -> anyhow::Result<_> {
        let meta = std::fs::metadata(models.join(file))?;
        let mut input = std::fs::File::open(models.join(file))?;
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 65536];
        loop { let n = input.read(&mut buffer)?; if n == 0 { break; } hash.update(&buffer[..n]); }
        let sha256 = format!("{:x}", hash.finalize());
        Ok(json!({"file":file,"bytes":meta.len(),"sha256":sha256,"modified_unix_seconds":meta.modified()?.duration_since(std::time::UNIX_EPOCH)?.as_secs()}))
    }).collect::<anyhow::Result<_>>()?;
    let report = json!({"description":manifest.description,"images":manifest.images.len(),
        "queries":results,"batch_size":batch_size,"embedding_version":magpie_lib::engine::clip::EMBEDDING_VERSION,"embedding_sha256":embedding_sha256,"model_files":model_files,"indexing_ms":indexing_ms,
        "limitations":["Corpus labels are supplied by the evaluator, not independently validated", "Precision@k uses k as denominator even when fewer results return", "OCR text is manifest ground truth; OCR pipeline accuracy is not measured", "All-mode scores are reciprocal-rank fusion; visual-mode scores are cosine similarity", "No private library database is opened and no image is uploaded"]});
    std::fs::write(&args[2], serde_json::to_vec_pretty(&report)?)?;
    println!(
        "Evaluated {} images and {} queries with the local model. Report: {}",
        manifest.images.len(),
        manifest.queries.len(),
        args[2]
    );
    Ok(())
}
