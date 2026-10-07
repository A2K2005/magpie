//! The engine: one SQLite database, one background indexer thread, and search on the caller's thread.
//! At rest it holds a database connection and a sleeping thread. CLIP, PaddleOCR and the vector cache
//! load on demand and are dropped when unused.

pub mod clip;
pub mod colors;
pub mod db;
mod fetch;
pub mod image;
mod indexer;
pub use indexer::is_image;
pub mod ocr;
mod onnx;
pub mod query;
pub mod search;
pub mod text;

use crate::types::*;
use clip::Clip;
use rusqlite::Connection;
use search::{Img, Semantic, Vectors, Vocab};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub enum Event {
    Status(IndexStatus),
    /// New or changed shots are searchable; the UI refreshes quietly.
    Indexed,
}

pub(crate) enum Msg {
    Configure,
    Kick,
    Fs(PathBuf),
    Reindex,
    Remove(Vec<i64>),
}

#[derive(Clone)]
pub struct Dirs {
    pub data: PathBuf,
    pub thumbs: PathBuf,
    pub models: PathBuf,
}

pub struct Shared {
    pub folders: Mutex<Vec<PathBuf>>,
    pub semantic: AtomicBool,
    /// Re-read text with PaddleOCR (Windows).
    pub sharp: AtomicBool,
    pub paused: AtomicBool,
    pub on_battery: AtomicBool,
    pub user_idle: AtomicBool,
    pub busy: AtomicBool,
    pub scanning: AtomicBool,
    pub model: Mutex<ModelInfo>,
    /// PaddleOCR model files.
    pub text_model: Mutex<ModelInfo>,
    /// Text encoder for queries (and image queries). The indexer has its own for images.
    pub query_clip: Mutex<Clip>,
    pub vectors: Mutex<Option<(Vectors, Instant)>>,
    /// Words for spelling corrections, rebuilt after new text is read and dropped when idle.
    pub vocab: Mutex<Option<(Vocab, Instant)>>,
    pub emit: Box<dyn Fn(Event) + Send + Sync>,
}

impl Shared {
    pub fn semantic_ready(&self) -> bool {
        self.semantic.load(Relaxed) && self.model.lock().unwrap().state == "ready"
    }

    pub fn sharp_ready(&self) -> bool {
        self.sharp.load(Relaxed) && self.text_model.lock().unwrap().state == "ready"
    }

    pub fn invalidate_vectors(&self) {
        *self.vectors.lock().unwrap() = None;
    }
}

pub struct Engine {
    pub dirs: Dirs,
    read: Mutex<Connection>,
    pub shared: Arc<Shared>,
    tx: Sender<Msg>,
}

impl Engine {
    pub fn start(data: &Path, emit: Box<dyn Fn(Event) + Send + Sync>) -> anyhow::Result<Self> {
        let dirs = Dirs { data: data.to_path_buf(), thumbs: data.join("thumbs"), models: data.join("models") };
        std::fs::create_dir_all(&dirs.thumbs)?;
        std::fs::create_dir_all(&dirs.models)?;
        let read = db::open(&dirs.data.join("magpie.db"))?;
        // Shots hidden by a trash that never committed (the app quit first) come back.
        read.execute("UPDATE shots SET hidden = 0 WHERE hidden = 1", [])?;
        let shared = Arc::new(Shared {
            folders: Mutex::new(vec![]),
            semantic: AtomicBool::new(false),
            sharp: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            on_battery: AtomicBool::new(false),
            user_idle: AtomicBool::new(true),
            busy: AtomicBool::new(false),
            scanning: AtomicBool::new(false),
            model: Mutex::new(ModelInfo { state: "off", progress: None, error: None }),
            text_model: Mutex::new(ModelInfo { state: "off", progress: None, error: None }),
            query_clip: Mutex::new(Clip::new(dirs.models.clone(), false)),
            vectors: Mutex::new(None),
            vocab: Mutex::new(None),
            emit,
        });
        let (tx, rx) = channel();
        indexer::spawn(dirs.clone(), shared.clone(), tx.clone(), rx);
        Ok(Self { dirs, read: Mutex::new(read), shared, tx })
    }

    /// `everywhere` adds every fixed drive (Windows) or the home folder (macOS) to `folders`.
    /// `sharp` only takes effect on Windows: Apple Vision is already as accurate as PaddleOCR.
    pub fn configure(&self, folders: &[String], semantic: bool, sharp: bool, everywhere: bool) {
        let mut norm: Vec<PathBuf> = folders.iter().map(PathBuf::from).collect();
        if everywhere {
            norm.extend(crate::platform::everywhere_roots());
        }
        norm.sort();
        norm.dedup();
        *self.shared.folders.lock().unwrap() = norm;
        if semantic && !self.shared.semantic.swap(true, Relaxed) {
            self.fetch(|s| &s.model, &clip::FILES);
        } else if !semantic {
            self.shared.semantic.store(false, Relaxed);
            *self.shared.model.lock().unwrap() = ModelInfo { state: "off", progress: None, error: None };
        }
        let sharp = sharp && cfg!(windows);
        if sharp && !self.shared.sharp.swap(true, Relaxed) {
            self.fetch(|s| &s.text_model, &ocr::paddle::FILES);
        } else if !sharp {
            self.shared.sharp.store(false, Relaxed);
            *self.shared.text_model.lock().unwrap() = ModelInfo { state: "off", progress: None, error: None };
        }
        let _ = self.tx.send(Msg::Configure);
    }

    /// Marks a model ready, downloading its files first if any are missing.
    fn fetch(&self, slot: fn(&Shared) -> &Mutex<ModelInfo>, files: &'static [(&'static str, &'static str, u64)]) {
        let set = move |shared: &Shared, state: &'static str, progress: Option<f64>, error: Option<String>| {
            *slot(shared).lock().unwrap() = ModelInfo { state, progress, error };
        };
        if files.iter().all(|(f, _, _)| self.dirs.models.join(f).exists()) {
            set(&self.shared, "ready", None, None);
            return;
        }
        set(&self.shared, "downloading", Some(0.0), None);
        let (shared, dir, tx) = (self.shared.clone(), self.dirs.models.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let report = |p: f64| {
                set(&shared, "downloading", Some(p), None);
                let _ = tx.send(Msg::Kick); // the indexer emits throttled status
            };
            match fetch::download(&dir, files, &report) {
                Ok(()) => set(&shared, "ready", None, None),
                Err(e) => set(&shared, "error", None, Some(e.to_string())),
            }
            let _ = tx.send(Msg::Kick);
        });
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.read.lock().unwrap()
    }

    pub fn search(&self, req: &SearchRequest, bytes: Option<&[u8]>) -> anyhow::Result<SearchResponse> {
        let conn = self.conn();
        Ok(search::run(&conn, req, bytes.map(Img::Bytes), self)?)
    }

    pub fn get_shot(&self, id: i64) -> anyhow::Result<Option<ShotDetail>> {
        Ok(search::get_shot(&self.conn(), id)?)
    }

    pub fn stats(&self) -> anyhow::Result<Stats> {
        Ok(search::stats(&self.conn())?)
    }

    pub fn status(&self) -> IndexStatus {
        indexer::status(&self.conn(), &self.shared)
    }

    pub fn paths(&self, ids: &[i64]) -> Vec<Option<String>> {
        let conn = self.conn();
        ids.iter()
            .map(|id| conn.query_row("SELECT path FROM shots WHERE id = ?", [id], |r| r.get(0)).ok())
            .collect()
    }

    /// Path of the most recently saved screenshot.
    pub fn latest_path(&self) -> Option<String> {
        self.conn().query_row("SELECT path FROM shots WHERE hidden = 0 ORDER BY mtime DESC LIMIT 1", [], |r| r.get(0)).ok()
    }

    pub fn set_hidden(&self, ids: &[i64], hidden: bool) {
        let conn = self.conn();
        for id in ids {
            let _ = conn.execute("UPDATE shots SET hidden = ? WHERE id = ?", (i64::from(hidden), id));
        }
        self.shared.invalidate_vectors();
        let _ = self.tx.send(Msg::Kick);
    }

    pub fn pin(&self, id: i64, pinned: bool) {
        let _ = self.conn().execute("UPDATE shots SET pinned = ? WHERE id = ?", (i64::from(pinned), id));
    }

    pub fn remove(&self, ids: Vec<i64>) {
        let _ = self.tx.send(Msg::Remove(ids));
    }

    pub fn reindex(&self) {
        let _ = self.tx.send(Msg::Reindex);
    }

    pub fn pause(&self, paused: bool) {
        self.shared.paused.store(paused, Relaxed);
        let _ = self.tx.send(Msg::Kick);
    }

    pub fn power(&self, on_battery: bool, user_idle: bool) {
        self.shared.on_battery.store(on_battery, Relaxed);
        self.shared.user_idle.store(user_idle, Relaxed);
        let _ = self.tx.send(Msg::Kick);
    }

    /// Window shown: load the text encoder ahead of the first visual query.
    pub fn warm(&self) {
        if self.shared.semantic_ready() {
            let shared = self.shared.clone();
            std::thread::spawn(move || {
                let _ = shared.query_clip.lock().unwrap().embed_text("");
            });
        }
    }
}

impl Semantic for Engine {
    fn state(&self) -> &'static str {
        if self.shared.semantic.load(Relaxed) { self.shared.model.lock().unwrap().state } else { "off" }
    }

    fn embed_text(&self, q: &str) -> Option<Vec<f32>> {
        self.shared.query_clip.lock().unwrap().embed_text(q).ok()
    }

    fn embed_image(&self, img: Img) -> Option<Vec<f32>> {
        let decoded = match img {
            Img::Path(p) => image::decode(Path::new(p)).ok()?,
            Img::Bytes(b) => image::decode_bytes(b).ok()?,
        };
        let px = image::clip_pixels(&decoded);
        self.shared.query_clip.lock().unwrap().embed_images(&[px]).ok()?.pop()
    }

    fn with_vectors<R>(&self, conn: &Connection, f: impl FnOnce(&Vectors) -> R) -> Option<R> {
        let mut cache = self.shared.vectors.lock().unwrap();
        if cache.is_none() {
            *cache = Some((Vectors::load(conn).ok()?, Instant::now()));
        }
        let (v, used) = cache.as_mut()?;
        *used = Instant::now();
        Some(f(v))
    }

    fn with_vocab<R>(&self, conn: &Connection, f: impl FnOnce(&Vocab) -> R) -> Option<R> {
        let mut cache = self.shared.vocab.lock().unwrap();
        if cache.is_none() {
            *cache = Some((Vocab::load(conn).ok()?, Instant::now()));
        }
        let (v, used) = cache.as_mut()?;
        *used = Instant::now();
        Some(f(v))
    }
}
