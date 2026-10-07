//! Exercise fresh native scanning, OS OCR, embedding, thumbnails and search in an isolated profile.
use anyhow::{Context, ensure};
use magpie_lib::{
    engine::{Engine, clip},
    types::SearchRequest,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::{
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    ensure!(
        args.len() == 4,
        "Usage: engine_smoke <models_dir> <fixture_image_folder> <new_profile_dir> <report.json>"
    );
    let models = Path::new(&args[0]).canonicalize()?;
    let folder = Path::new(&args[1]).canonicalize()?;
    let profile = PathBuf::from(&args[2]);
    ensure!(folder.is_dir(), "Fixture image folder does not exist");
    ensure!(
        !profile.exists(),
        "Choose a new profile directory; existing profiles are never reused"
    );
    ensure!(
        clip::cached(&models),
        "Models must already exist locally; no downloads are intended"
    );
    // Copy model files so model/indexer writes cannot affect the installed app's cache.
    std::fs::create_dir_all(profile.join("models/onnx"))?;
    for (file, _, _) in clip::FILES {
        std::fs::copy(models.join(file), profile.join("models").join(file))?;
    }
    for (file, expected) in clip::SHA256 {
        let mut input = std::fs::File::open(profile.join("models").join(file))?;
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 65536];
        loop {
            let n = input.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
        }
        ensure!(
            format!("{:x}", hash.finalize()) == expected,
            "Cached model {file} does not match the pinned release; refusing network repair during a smoke test"
        );
    }
    let engine = Engine::start(&profile, Box::new(|_| {}))?;
    let started = Instant::now();
    engine.configure(
        &[folder.to_string_lossy().into_owned()],
        true,
        false,
        false,
        &[],
    );
    engine.finish_indexing(true);
    let status = loop {
        let status = engine.status();
        if status.total > 0
            && status.ocr_done == status.total
            && status.embedded == status.total
            && status.errors == 0
        {
            break status;
        }
        if status.model.state == "error" {
            anyhow::bail!("Model initialization failed: {:?}", status.model.error);
        }
        ensure!(
            started.elapsed() < Duration::from_secs(180),
            "Native indexing timed out: {}",
            serde_json::to_string(&status)?
        );
        thread::sleep(Duration::from_millis(250));
    };
    let indexing_ms = started.elapsed().as_millis();
    let recent = engine.search(
        &SearchRequest {
            limit: Some(500),
            ..Default::default()
        },
        None,
    )?;
    let mut indexed = vec![];
    for hit in recent.hits {
        let detail = engine
            .get_shot(hit.shot.id)?
            .context("Indexed shot missing")?;
        ensure!(
            profile
                .join("thumbs")
                .join(format!("{}.jpg", detail.shot.id))
                .exists(),
            "Missing thumbnail for {}",
            detail.shot.name
        );
        indexed.push(json!({"name":detail.shot.name,"text":detail.text,"width":detail.shot.width,"height":detail.shot.height}));
    }
    let visual = engine.search(
        &SearchRequest {
            q: "dog".into(),
            mode: Some("visual".into()),
            ..Default::default()
        },
        None,
    )?;
    let text = engine.search(
        &SearchRequest {
            q: "invoice".into(),
            mode: Some("text".into()),
            ..Default::default()
        },
        None,
    )?;
    let dog_found = visual
        .hits
        .iter()
        .any(|h| matches!(h.shot.name.as_str(), "001.jpg" | "002.jpg"));
    let invoice_found = text.hits.iter().any(|h| h.shot.name == "007.png");
    let conn = rusqlite::Connection::open_with_flags(
        profile.join("magpie.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let vectors = magpie_lib::engine::search::Vectors::load(&conn)?;
    let eligible = vectors.ids.iter().copied().collect();
    let mut diagnostics = vec![];
    for prompt in ["dog", "a photo of a dog.", "a white fluffy dog"] {
        let q = magpie_lib::engine::search::Semantic::embed_text(&engine, prompt)
            .context("Query model did not return an embedding")?;
        let scored = vectors.nearest(&q, -1.0, 500, &eligible);
        let mut named = vec![];
        for (id, score) in scored {
            named.push(json!({"name":engine.get_shot(id)?.context("Missing shot")?.shot.name,"cosine":score}));
        }
        diagnostics.push(json!({"prompt":prompt,"scores":named}));
    }
    let report = json!({"profile":profile,"fixture_folder":folder,"indexing_ms":indexing_ms,"total_ms":started.elapsed().as_millis(),"embedding_version":magpie_lib::engine::clip::EMBEDDING_VERSION,
        "status":status,"indexed":indexed,"dog_visual":visual,"invoice_text":text,"dog_found":dog_found,"invoice_found":invoice_found,"diagnostics":diagnostics,
        "limitations":["Small public/synthetic fixture corpus, not production relevance", "Actual OS OCR, native scan/index/embed/search executed", "Fresh isolated profile; installed library database untouched", "Profile intentionally kept for inspection"]});
    std::fs::write(&args[3], serde_json::to_vec_pretty(&report)?)?;
    ensure!(
        dog_found,
        "Dog query missed both real dog fixtures; diagnostic report: {}",
        args[3]
    );
    ensure!(
        invoice_found,
        "OS OCR did not retrieve the invoice fixture; report: {}",
        args[3]
    );
    println!("Native Engine smoke passed. Report: {}", args[3]);
    Ok(())
}
