//! The indexer thread. Sleeps on a channel until there is work: a folder change, a settings change,
//! or a queued shot. Runs at background priority, newest first: OS text (several images at once),
//! then CLIP, then PaddleOCR. On battery it works at most half the time.

use super::clip::Clip;
use super::colors::color_tags;
use super::ocr::paddle::Paddle;
use super::{Dirs, Event, Msg, Shared, db, image, ocr, text};
use crate::types::{IndexStatus, ModelInfo, OcrLine};
use notify::{RecursiveMode, Watcher};
use rusqlite::{Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, UNIX_EPOCH};

pub const IMAGE_EXT: [&str; 8] = ["png", "jpg", "jpeg", "webp", "gif", "tif", "tiff", "bmp"];
// ponytail: burst grouping knobs, tuned by eye. Raise LINE_SHARE_MIN if different pages of one app get stacked.
const BURST_MS: i64 = 10 * 60_000;
const HAMMING_MAX: u32 = 6;
const MIN_LINES: usize = 4;
const LINE_SHARE_MIN: f32 = 0.7;
const EMBED_BATCH: usize = 8;
/// New screenshots embed right away; a bigger backlog waits for the user to step away.
const SMALL_BACKLOG: i64 = 40;
/// Screenshot tools write in several steps.
const SETTLE: Duration = Duration::from_millis(400);
/// Models and caches unused this long are dropped.
const IDLE_RELEASE: Duration = Duration::from_secs(60);
/// Shots the OS read text in, not yet re-read by PaddleOCR. Shots without text are not re-read:
/// detection alone costs ~0.5 s, and a photo library would take hours for little gain.
const SHARP_WHERE: &str = "ocr = 0 AND stage >= 1 AND hidden = 0 AND text != ''";
/// Below this size (px, both sides) an image is an icon: no OCR.
const ICON: u32 = 40;

pub fn is_image(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXT.contains(&e.to_ascii_lowercase().as_str()))
}

pub fn status(conn: &Connection, shared: &Shared) -> IndexStatus {
    let (total, ocr, emb, err, sharp): (i64, i64, i64, i64, i64) = conn
        .query_row(
            "SELECT count(*), coalesce(sum(stage >= 1), 0), coalesce(sum(stage = 2), 0), coalesce(sum(stage = -1), 0),
               coalesce(sum(stage >= 1 AND (ocr != 0 OR text = '')), 0)
             FROM shots WHERE hidden = 0",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap_or((0, 0, 0, 0, 0));
    let model = if shared.semantic.load(Relaxed) {
        shared.model.lock().unwrap().clone()
    } else {
        ModelInfo { state: "off", progress: None, error: None }
    };
    let state = if shared.paused.load(Relaxed) {
        "paused"
    } else if shared.scanning.load(Relaxed) {
        "scanning"
    } else if shared.busy.load(Relaxed) {
        "indexing"
    } else {
        "idle"
    };
    let text_model = if shared.sharp.load(Relaxed) {
        shared.text_model.lock().unwrap().clone()
    } else {
        ModelInfo { state: "off", progress: None, error: None }
    };
    IndexStatus { state, total, ocr_done: ocr, embedded: emb, errors: err, model, sharp, text_model }
}

pub(super) fn spawn(dirs: Dirs, shared: Arc<Shared>, tx: Sender<Msg>, rx: Receiver<Msg>) {
    std::thread::Builder::new()
        .name("magpie-indexer".into())
        .spawn(move || {
            crate::platform::background_thread();
            ocr::init_thread();
            let conn = db::open(&dirs.data.join("magpie.db")).expect("open database");
            Indexer {
                conn,
                dirs,
                shared,
                tx,
                watchers: vec![],
                roots: vec![],
                pending: HashMap::new(),
                clip: None,
                paddle: None,
                last_step: Duration::ZERO,
                status_at: Instant::now(),
                last_status: None,
                status_pending: false,
                indexed_dirty: false,
                indexed_at: Instant::now(),
            }
            .run(rx)
        })
        .expect("spawn indexer");
}

struct Indexer {
    conn: Connection,
    dirs: Dirs,
    shared: Arc<Shared>,
    tx: Sender<Msg>,
    watchers: Vec<notify::RecommendedWatcher>,
    roots: Vec<PathBuf>,
    /// Changed paths waiting for SETTLE.
    pending: HashMap<PathBuf, Instant>,
    /// Image encoder, loaded only while there are images to embed.
    clip: Option<Clip>,
    /// PaddleOCR, loaded only while there are shots to re-read.
    paddle: Option<Paddle>,
    /// How long the last unit of work took; on battery the indexer rests at least as long.
    last_step: Duration,
    status_at: Instant,
    last_status: Option<IndexStatus>,
    status_pending: bool,
    indexed_dirty: bool,
    indexed_at: Instant,
}

impl Indexer {
    fn run(mut self, rx: Receiver<Msg>) {
        loop {
            let work = !self.shared.paused.load(Relaxed) && self.has_work();
            self.shared.busy.store(work, Relaxed);
            let wait = if work {
                if self.shared.on_battery.load(Relaxed) { self.last_step.max(Duration::from_millis(400)) } else { Duration::ZERO }
            } else if !self.pending.is_empty() {
                SETTLE
            } else if self.status_pending || self.indexed_dirty {
                Duration::from_millis(250)
            } else {
                // Asleep: wake for housekeeping only.
                Duration::from_secs(30)
            };
            match rx.recv_timeout(wait) {
                Ok(msg) => self.handle(msg),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            while let Ok(msg) = rx.try_recv() {
                self.handle(msg);
            }
            self.flush_pending();
            if work {
                let t = Instant::now();
                self.step();
                self.last_step = t.elapsed();
            } else {
                self.last_step = Duration::ZERO;
                self.release_idle();
            }
            self.emit();
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Configure => {
                let roots = self.shared.folders.lock().unwrap().clone();
                if roots != self.roots {
                    self.roots = roots;
                    self.watch();
                    self.scan();
                }
            }
            Msg::Kick => {}
            Msg::Fs(p) => {
                self.pending.insert(p, Instant::now());
            }
            Msg::Reindex => {
                let _ = self.conn.execute_batch(
                    "UPDATE shots SET stage = 0, ocr = 0, error = NULL, embedding = NULL, group_id = NULL, text = '', norm = ''",
                );
                self.shared.invalidate_vectors();
                self.scan();
            }
            Msg::Remove(ids) => self.remove(&ids),
        }
    }

    fn has_work(&self) -> bool {
        let q = |sql: &str| self.conn.query_row(sql, [], |_| Ok(())).optional().ok().flatten().is_some();
        q("SELECT 1 FROM shots WHERE stage = 0 AND hidden = 0 LIMIT 1")
            || (self.shared.semantic_ready() && self.may_embed() && q("SELECT 1 FROM shots WHERE stage = 1 AND hidden = 0 LIMIT 1"))
            || (self.shared.sharp_ready() && self.may_sharpen() && q(&format!("SELECT 1 FROM shots WHERE {SHARP_WHERE} LIMIT 1")))
    }

    fn may_embed(&self) -> bool {
        self.may_backlog("stage = 1 AND hidden = 0")
    }

    fn may_sharpen(&self) -> bool {
        self.may_backlog(SHARP_WHERE)
    }

    /// New screenshots go right away; a large backlog waits until the user is away and on mains power.
    fn may_backlog(&self, wh: &str) -> bool {
        if self.shared.on_battery.load(Relaxed) {
            return false;
        }
        if self.shared.user_idle.load(Relaxed) {
            return true;
        }
        let sql = format!("SELECT count(*) FROM (SELECT 1 FROM shots WHERE {wh} LIMIT {})", SMALL_BACKLOG + 1);
        self.conn.query_row(&sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0) <= SMALL_BACKLOG
    }

    fn root_of(&self, p: &Path) -> Option<PathBuf> {
        // Longest root wins when folders nest.
        self.roots.iter().filter(|r| p.starts_with(r)).max_by_key(|r| r.as_os_str().len()).cloned()
    }

    fn watch(&mut self) {
        self.watchers.clear();
        for root in &self.roots {
            let tx = self.tx.clone();
            let r = root.clone();
            let w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(ev) = res {
                    for p in ev.paths.into_iter().filter(|p| is_image(p) && !skipped(&r, p)) {
                        let _ = tx.send(Msg::Fs(p));
                    }
                }
            });
            if let Ok(mut w) = w {
                if w.watch(root, RecursiveMode::Recursive).is_ok() {
                    self.watchers.push(w);
                }
            }
        }
    }

    /// Syncs the database with the folders: adds new files, requeues changed ones, drops missing ones.
    fn scan(&mut self) {
        self.shared.scanning.store(true, Relaxed);
        self.emit_now();
        let mut found: HashMap<PathBuf, (i64, i64, PathBuf)> = HashMap::new();
        for root in self.roots.clone() {
            walk(&root, &mut |p, mtime, size| {
                if let Some(r) = self.root_of(p) {
                    found.insert(p.to_path_buf(), (mtime, size, r));
                }
            });
        }
        let known: Vec<(i64, String, i64, i64)> = self
            .conn
            .prepare("SELECT id, path, mtime, size FROM shots")
            .and_then(|mut st| st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect())
            .unwrap_or_default();
        let mut gone = vec![];
        if let Ok(tx) = self.conn.transaction() {
            let mut seen = HashSet::new();
            for (id, path, mtime, size) in known {
                let key = PathBuf::from(&path);
                match found.get(&key) {
                    None => {
                        let _ = tx.execute("DELETE FROM shots WHERE id = ?", [id]);
                        gone.push(id);
                    }
                    Some((m, s, root)) if *m != mtime || *s != size => {
                        let _ = tx.execute(
                            "UPDATE shots SET stage = 0, mtime = ?, size = ?, folder = ?, error = NULL, embedding = NULL WHERE id = ?",
                            (m, s, root.to_string_lossy(), id),
                        );
                    }
                    _ => {}
                }
                seen.insert(key);
            }
            for (p, (m, s, root)) in &found {
                if !seen.contains(p) {
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let _ = tx.execute(
                        "INSERT OR IGNORE INTO shots(path, folder, name, mtime, size) VALUES (?, ?, ?, ?, ?)",
                        (p.to_string_lossy(), root.to_string_lossy(), name, m, s),
                    );
                }
            }
            let _ = tx.commit();
        }
        for id in &gone {
            let _ = std::fs::remove_file(self.thumb(*id));
        }
        if !gone.is_empty() {
            self.shared.invalidate_vectors();
        }
        self.shared.scanning.store(false, Relaxed);
        self.indexed_dirty = true;
    }

    fn thumb(&self, id: i64) -> PathBuf {
        self.dirs.thumbs.join(format!("{id}.jpg"))
    }

    fn flush_pending(&mut self) {
        let due: Vec<PathBuf> =
            self.pending.iter().filter(|(_, t)| t.elapsed() >= SETTLE).map(|(p, _)| p.clone()).collect();
        for p in due {
            self.pending.remove(&p);
            self.upsert(&p);
        }
    }

    fn upsert(&mut self, p: &Path) {
        let path = p.to_string_lossy();
        let row: Option<(i64, i64, i64)> = self
            .conn
            .query_row("SELECT id, mtime, size FROM shots WHERE path = ?", [&path], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()
            .ok()
            .flatten();
        let meta = std::fs::metadata(p).ok().filter(|m| m.is_file() && is_local(m));
        let root = self.root_of(p);
        let (Some(meta), Some(root)) = (meta, root) else {
            if let Some((id, _, _)) = row {
                self.remove(&[id]);
            }
            return;
        };
        let (mtime, size) = (mtime_ms(&meta), meta.len() as i64);
        match row {
            None => {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let _ = self.conn.execute(
                    "INSERT OR IGNORE INTO shots(path, folder, name, mtime, size) VALUES (?, ?, ?, ?, ?)",
                    (&path, root.to_string_lossy(), name, mtime, size),
                );
            }
            Some((id, m, s)) if m != mtime || s != size => {
                let _ = self.conn.execute(
                    "UPDATE shots SET stage = 0, mtime = ?, size = ?, error = NULL, embedding = NULL WHERE id = ?",
                    (mtime, size, id),
                );
            }
            _ => {}
        }
    }

    fn remove(&mut self, ids: &[i64]) {
        for id in ids {
            let _ = self.conn.execute("DELETE FROM shots WHERE id = ?", [id]);
            let _ = std::fs::remove_file(self.thumb(*id));
        }
        self.shared.invalidate_vectors();
        self.indexed_dirty = true;
    }

    /// One unit of work: OCR one shot, or embed one batch.
    fn step(&mut self) {
        // Several images are read at once on background-priority threads; one per thread on battery.
        let workers = if self.shared.on_battery.load(Relaxed) { 1 } else { read_workers() };
        let batch: Vec<(i64, String, String, i64)> = self
            .conn
            .prepare("SELECT id, path, folder, mtime FROM shots WHERE stage = 0 AND hidden = 0 ORDER BY mtime DESC LIMIT ?")
            .and_then(|mut st| st.query_map([workers * READS_PER_WORKER], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect())
            .unwrap_or_default();
        if !batch.is_empty() {
            let jobs: Vec<(String, PathBuf)> = batch.iter().map(|(id, path, _, _)| (path.clone(), self.thumb(*id))).collect();
            let results = read_all(&jobs, workers);
            for ((id, _, folder, mtime), result) in batch.iter().zip(results) {
                if let Err(e) = result.and_then(|r| self.store(*id, folder, *mtime, r)) {
                    let _ = self.conn.execute("UPDATE shots SET stage = -1, error = ? WHERE id = ?", (e.to_string(), id));
                }
            }
            self.indexed_dirty = true;
            return;
        }
        if self.shared.semantic_ready() && self.may_embed() && self.embed_batch() {
            return;
        }
        if self.shared.sharp_ready() && self.may_sharpen() {
            self.sharpen();
        }
    }

    fn store(&mut self, id: i64, folder: &str, mtime: i64, a: Read) -> anyhow::Result<()> {
        let Read { lines, w, h, dhash, colors } = a;
        let a = (dhash, colors);
        let txt = lines.iter().map(|l| l.t.as_str()).collect::<Vec<_>>().join("\n");
        let mut tags = color_tags(&a.1);
        tags.extend(text::action_tags(&text::detect_actions(&txt)));
        if !txt.is_empty() {
            tags.push("h:text".into());
        }
        let line_texts: Vec<&str> = lines.iter().map(|l| l.t.as_str()).collect();
        let group = self.find_group(id, folder, mtime, &a.0, &line_texts);
        let colors: Vec<&str> = a.1.iter().map(|c| c.hex.as_str()).collect();
        self.conn.execute(
            "UPDATE shots SET stage = 1, ocr = 0, error = NULL, width = ?, height = ?, text = ?, norm = ?, lines = ?,
               dhash = ?, colors = ?, tags = ?, group_id = ? WHERE id = ?",
            rusqlite::params![
                w,
                h,
                txt,
                text::fold(&txt),
                serde_json::to_string(&lines)?,
                a.0,
                serde_json::to_string(&colors)?,
                tags.join(" "),
                group,
                id
            ],
        )?;
        Ok(())
    }

    /// Joins a burst: same folder, taken within BURST_MS, showing the same thing. Shots with text compare
    /// their lines (different pages of one app share a sidebar, not the rest); others a perceptual hash.
    fn find_group(&self, id: i64, folder: &str, mtime: i64, dhash: &str, lines: &[&str]) -> Option<i64> {
        let mut st = self
            .conn
            .prepare(
                "SELECT id, dhash, group_id, text FROM shots WHERE folder = ? AND id != ? AND dhash IS NOT NULL
                 AND hidden = 0 AND stage >= 1 AND mtime BETWEEN ? AND ?",
            )
            .ok()?;
        let near: Vec<(i64, String, Option<i64>, String)> = st
            .query_map((folder, id, mtime - BURST_MS, mtime + BURST_MS), |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .ok()?
            .filter_map(Result::ok)
            .collect();
        let mine: HashSet<&str> = lines.iter().copied().collect();
        let mut best: Option<(i64, Option<i64>, f32)> = None;
        for (nid, nhash, ngroup, ntext) in &near {
            let theirs: HashSet<&str> = ntext.lines().collect();
            let score = if mine.len() >= MIN_LINES && theirs.len() >= MIN_LINES {
                let shared = mine.intersection(&theirs).count() as f32;
                let s = shared / mine.len().max(theirs.len()) as f32;
                if s < LINE_SHARE_MIN {
                    continue;
                }
                s
            } else {
                let d = image::hamming(dhash, nhash);
                if d > HAMMING_MAX {
                    continue;
                }
                1.0 - d as f32 / 64.0
            };
            if best.is_none_or(|b| score > b.2) {
                best = Some((*nid, *ngroup, score));
            }
        }
        let (nid, ngroup, _) = best?;
        let gid = ngroup.unwrap_or(nid);
        if ngroup.is_none() {
            let _ = self.conn.execute("UPDATE shots SET group_id = ? WHERE id = ?", (gid, nid));
        }
        Some(gid)
    }

    /// Embeds the next batch. False when there was nothing to embed.
    fn embed_batch(&mut self) -> bool {
        let batch: Vec<(i64, String)> = self
            .conn
            .prepare("SELECT id, path FROM shots WHERE stage = 1 AND hidden = 0 ORDER BY mtime DESC LIMIT ?")
            .and_then(|mut st| st.query_map([EMBED_BATCH], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
            .unwrap_or_default();
        if batch.is_empty() {
            return false;
        }
        let mut ok: Vec<(i64, Vec<f32>)> = Vec::new();
        for (id, path) in &batch {
            let px = std::panic::catch_unwind(|| image::decode(Path::new(path)).map(|img| image::clip_pixels(&img)));
            match px.unwrap_or_else(|_| Err(anyhow::anyhow!("Could not read this file"))) {
                Ok(px) => ok.push((*id, px)),
                Err(e) => {
                    let _ = self.conn.execute("UPDATE shots SET stage = -1, error = ? WHERE id = ?", (e.to_string(), id));
                }
            }
        }
        let clip = self.clip.get_or_insert_with(|| Clip::new(self.dirs.models.clone(), true));
        let pixels: Vec<Vec<f32>> = ok.iter().map(|(_, p)| p.clone()).collect();
        match clip.embed_images(&pixels) {
            Ok(vecs) => {
                for ((id, _), v) in ok.iter().zip(vecs) {
                    let _ = self.conn.execute("UPDATE shots SET stage = 2, embedding = ? WHERE id = ?", (db::to_blob(&v), id));
                }
                self.shared.invalidate_vectors();
            }
            Err(e) => {
                *self.shared.model.lock().unwrap() = ModelInfo { state: "error", progress: None, error: Some(e.to_string()) };
            }
        }
        true
    }

    /// Re-reads the newest OS-read shot with PaddleOCR. Lines the OS read where PaddleOCR found
    /// nothing (and QR codes) are kept.
    fn sharpen(&mut self) {
        let sql = format!("SELECT id, path, lines, tags FROM shots WHERE {SHARP_WHERE} ORDER BY mtime DESC LIMIT 1");
        let next: Option<(i64, String, String, String)> = self
            .conn
            .query_row(&sql, [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .optional()
            .ok()
            .flatten();
        let Some((id, path, os_lines, tags)) = next else { return };
        if self.paddle.is_none() {
            match Paddle::load(&self.dirs.models) {
                Ok(p) => self.paddle = Some(p),
                Err(e) => {
                    *self.shared.text_model.lock().unwrap() =
                        ModelInfo { state: "error", progress: None, error: Some(e.to_string()) };
                    return;
                }
            }
        }
        let paddle = self.paddle.as_mut().unwrap();
        let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            image::decode(Path::new(&path)).and_then(|img| ocr::recognize_with(&img, &mut |i| paddle.recognize(i)))
        }));
        if read.is_err() {
            self.paddle = None; // its state after a panic is unknown
        }
        let lines = match read.unwrap_or_else(|_| Err(anyhow::anyhow!("PaddleOCR failed"))) {
            Ok(lines) => ocr::merge(lines, &serde_json::from_str::<Vec<OcrLine>>(&os_lines).unwrap_or_default()),
            Err(_) => {
                let _ = self.conn.execute("UPDATE shots SET ocr = -1 WHERE id = ?", [id]);
                return;
            }
        };
        let txt = lines.iter().map(|l| l.t.as_str()).collect::<Vec<_>>().join("\n");
        let mut tags: Vec<String> = tags.split(' ').filter(|t| t.starts_with("c:")).map(String::from).collect();
        tags.extend(text::action_tags(&text::detect_actions(&txt)));
        if !txt.is_empty() {
            tags.push("h:text".into());
        }
        let _ = self.conn.execute(
            "UPDATE shots SET ocr = 1, text = ?, norm = ?, lines = ?, tags = ? WHERE id = ?",
            rusqlite::params![txt, text::fold(&txt), serde_json::to_string(&lines).unwrap_or_default(), tags.join(" "), id],
        );
        self.indexed_dirty = true;
    }

    /// Frees what was loaded for work that has finished. The image encoder goes as soon as the backlog
    /// is done; the query encoder and vector cache after a minute without searches.
    fn release_idle(&mut self) {
        self.clip = None;
        self.paddle = None;
        if let Ok(mut q) = self.shared.query_clip.try_lock() {
            q.release_idle(IDLE_RELEASE);
        }
        if let Ok(mut v) = self.shared.vectors.try_lock() {
            if v.as_ref().is_some_and(|(_, used)| used.elapsed() > IDLE_RELEASE) {
                *v = None;
            }
        }
        if let Ok(mut v) = self.shared.vocab.try_lock() {
            if v.as_ref().is_some_and(|(_, used)| used.elapsed() > IDLE_RELEASE) {
                *v = None;
            }
        }
    }

    /// Status at most 4×/s and only when it changed; `indexed` at most 1×/s.
    fn emit(&mut self) {
        let s = status(&self.conn, &self.shared);
        self.status_pending = self.last_status.as_ref() != Some(&s);
        if self.status_pending && self.status_at.elapsed() >= Duration::from_millis(250) {
            self.send_status(s);
        }
        if self.indexed_dirty && self.indexed_at.elapsed() >= Duration::from_secs(1) {
            self.indexed_dirty = false;
            self.indexed_at = Instant::now();
            *self.shared.vocab.lock().unwrap() = None; // new words to correct towards
            (self.shared.emit)(Event::Indexed);
        }
    }

    fn emit_now(&mut self) {
        let s = status(&self.conn, &self.shared);
        if self.last_status.as_ref() != Some(&s) {
            self.send_status(s);
        }
    }

    fn send_status(&mut self, s: IndexStatus) {
        self.status_at = Instant::now();
        self.status_pending = false;
        self.last_status = Some(s.clone());
        (self.shared.emit)(Event::Status(s));
    }
}

const READS_PER_WORKER: usize = 4;

/// Threads reading images at once: a quarter of the cores, at most 3 (each holds a decoded image).
fn read_workers() -> usize {
    std::thread::available_parallelism().map_or(1, |n| (n.get() / 4).clamp(1, 3))
}

/// What reading an image yields, before it is stored.
struct Read {
    lines: Vec<OcrLine>,
    w: u32,
    h: u32,
    dhash: String,
    colors: Vec<super::colors::Color>,
}

/// Thumbnail, hash, colors, OS text and QR codes for one image.
fn read(path: &str, thumb: &Path) -> anyhow::Result<Read> {
    let a = image::analyze(Path::new(path), thumb)?;
    let (w, h) = (a.img.width(), a.img.height());
    let mut lines = if w.max(h) < ICON { vec![] } else { ocr::recognize(&a.img)? };
    lines.extend(ocr::qr_lines(&a.img));
    Ok(Read { lines, w, h, dhash: a.dhash, colors: a.colors })
}

/// Reads `(path, thumbnail)` jobs on `workers` background threads, results in job order. A file that
/// makes a decoder panic fails alone instead of taking the app down.
fn read_all(jobs: &[(String, PathBuf)], workers: usize) -> Vec<anyhow::Result<Read>> {
    let per = jobs.len().div_ceil(workers.max(1));
    std::thread::scope(|s| {
        let handles: Vec<_> = jobs
            .chunks(per)
            .map(|chunk| {
                s.spawn(move || {
                    crate::platform::background_thread();
                    ocr::init_thread();
                    chunk
                        .iter()
                        .map(|(path, thumb)| {
                            std::panic::catch_unwind(|| read(path, thumb))
                                .unwrap_or_else(|_| Err(anyhow::anyhow!("Could not read this file")))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    })
}

fn mtime_ms(m: &std::fs::Metadata) -> i64 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64)
}

/// Recursive walk, skipping the folders in [`skip_dir`] and (Windows) hidden or system folders such
/// as AppData. Links are not followed.
fn walk(dir: &Path, f: &mut dyn FnMut(&Path, i64, i64)) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            if !skip_dir(&e.file_name().to_string_lossy(), dir) && !e.metadata().is_ok_and(|m| hidden(&m)) {
                walk(&p, f);
            }
        } else if ft.is_file() && is_image(&p) {
            if let Some(m) = e.metadata().ok().filter(is_local) {
                f(&p, mtime_ms(&m), m.len() as i64);
            }
        }
    }
}

/// Folders never worth reading: hidden ones, the OS, installed programs, app data, caches and code
/// dependencies. They hold thousands of icons and cached images nobody searches for.
fn skip_dir(name: &str, parent: &Path) -> bool {
    let n = name.to_lowercase();
    if n.starts_with('.') || n.starts_with('$') || n.ends_with(".app") || n.ends_with(".photoslibrary") {
        return true;
    }
    if matches!(
        n.as_str(),
        "appdata" | "node_modules" | "bower_components" | "__pycache__" | "__macosx" | "site-packages" | "steamapps"
            | "system volume information"
    ) {
        return true;
    }
    // Directly under a drive root (C:\Windows), or ~/Library on macOS.
    let top = parent.parent().is_none();
    (top && matches!(
        n.as_str(),
        "windows" | "program files" | "program files (x86)" | "programdata" | "recovery" | "perflogs" | "msocache" | "windows.old"
    )) || (cfg!(target_os = "macos") && n == "library" && std::env::var_os("HOME").is_some_and(|h| Path::new(&h) == parent))
}

/// True if a folder between `root` and the file `p` is one [`walk`] skips.
fn skipped(root: &Path, p: &Path) -> bool {
    let Ok(rel) = p.strip_prefix(root) else { return true };
    let mut parent = root.to_path_buf();
    let dirs: Vec<_> = rel.components().collect();
    for c in &dirs[..dirs.len().saturating_sub(1)] {
        if skip_dir(&c.as_os_str().to_string_lossy(), &parent) {
            return true;
        }
        parent.push(c);
    }
    false
}

fn hidden(m: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const HIDDEN: u32 = 0x2;
        const SYSTEM: u32 = 0x4;
        m.file_attributes() & (HIDDEN | SYSTEM) != 0
    }
    #[cfg(not(windows))]
    {
        let _ = m;
        false
    }
}

/// False for cloud placeholders (OneDrive "online-only" files): reading one would download it,
/// and a big library can mean gigabytes of traffic. They are indexed once they are on disk.
fn is_local(m: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const OFFLINE: u32 = 0x1000;
        const RECALL_ON_OPEN: u32 = 0x40000;
        const RECALL_ON_DATA_ACCESS: u32 = 0x400000;
        m.file_attributes() & (OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS) == 0
    }
    #[cfg(not(windows))]
    {
        let _ = m;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_system_and_app_folders() {
        let root = if cfg!(windows) { Path::new("C:\\") } else { Path::new("/") };
        assert!(skip_dir("Windows", root));
        assert!(skip_dir("Program Files", root));
        assert!(!skip_dir("Windows", &root.join("wallpapers")));
        assert!(skip_dir("node_modules", &root.join("code")));
        assert!(skip_dir(".git", &root.join("code")));
        assert!(!skip_dir("Screenshots", &root.join("Pictures")));
        let home = root.join("Users").join("me");
        assert!(skipped(root, &home.join("AppData").join("Local").join("x.png")));
        assert!(!skipped(root, &home.join("Pictures").join("x.png")));
        // A chosen folder inside AppData is a root, so nothing above it is checked.
        let chosen = home.join("AppData").join("Roaming").join("Tool");
        assert!(!skipped(&chosen, &chosen.join("shot.png")));
    }
}
