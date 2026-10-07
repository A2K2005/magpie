//! Runs the engine without a window: indexes, then times searches. Nothing on screen, so it can run
//! while someone is using the computer.
//! Run: cargo run --release --example engine_bench -- <data-dir> [folder ...] [--everywhere] [--sharp]
//!        [--semantic] [--active] [-- query ...]
//! `--active` behaves as if the user is at the keyboard (the indexer works at most half the time).
//! Models are read from <data-dir>/models; copy them there first to skip downloads.

use magpie_lib::engine::{Engine, Event};
use magpie_lib::types::SearchRequest;
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (opts, queries) = match args.iter().position(|a| a == "--") {
        Some(i) => (&args[..i], &args[i + 1..]),
        None => (&args[..], &[][..]),
    };
    let has = |f: &str| opts.iter().any(|a| a == f);
    let data = std::path::PathBuf::from(&opts[0]);
    let folders: Vec<String> = opts[1..].iter().filter(|a| !a.starts_with("--")).cloned().collect();
    std::fs::create_dir_all(&data)?;

    let t0 = Instant::now();
    let engine = Engine::start(&data, Box::new(|_: Event| {}))?;
    engine.power(false, !has("--active"));
    engine.configure(&folders, has("--semantic"), has("--sharp"), has("--everywhere"), &[]);
    let mut scanned = None;
    let mut read = None;
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let s = engine.status();
        if scanned.is_none() && s.state != "scanning" && s.total > 0 {
            scanned = Some(t0.elapsed());
            println!("catalogued {} images in {:.1} s", s.total, t0.elapsed().as_secs_f32());
            if has("--catalog-only") {
                break;
            }
        }
        if read.is_none() && s.total > 0 && s.ocr_done + s.errors >= s.total {
            read = Some(t0.elapsed());
            println!("OS text read for all in {:.1} s ({} errors)", t0.elapsed().as_secs_f32(), s.errors);
        }
        if s.total > 0 && s.state == "idle" {
            println!(
                "idle after {:.1} s: total {} read {} sharp {} embedded {} errors {} text model {}",
                t0.elapsed().as_secs_f32(),
                s.total,
                s.ocr_done,
                s.sharp,
                s.embedded,
                s.errors,
                s.text_model.state
            );
            break;
        }
    }
    for q in queries {
        let req = SearchRequest { q: q.clone(), ..Default::default() };
        let _ = engine.search(&req, None)?; // warm the page cache
        let res = engine.search(&req, None)?;
        let first = res.hits.first().map_or(String::new(), |h| format!(" | first: {} [{}]", h.shot.name, h.kind));
        println!("search {q:?}: {} ms, {} hits{first}", res.took_ms, res.hits.len());
    }
    println!("peak memory: {} MB", magpie_lib::platform::peak_memory_mb());
    if has("--wait") {
        // Leaves time to measure memory once models are released.
        std::thread::sleep(Duration::from_secs(75));
    }
    Ok(())
}
