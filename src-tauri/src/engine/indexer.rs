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
const RECONCILE: Duration = Duration::from_secs(5 * 60);
/// Screenshot tools write in several steps.
const SETTLE: Duration = Duration::from_millis(400);
/// Models and caches unused this long are dropped.
const IDLE_RELEASE: Duration = Duration::from_secs(60);
/// Re-read OS text, plus large landscape images where OS OCR may have missed screenshot text.
// ponytail: landscape size is a screenshot heuristic; use capture provenance if photo-library OCR cost grows.
const SHARP_WHERE: &str = "ocr = 0 AND stage >= 1 AND hidden = 0 AND (text != '' OR (width >= 640 AND height >= 360 AND width >= height))";
fn embed_where() -> String {
    format!("stage >= 1 AND hidden = 0 AND error IS NULL AND (embedding IS NULL OR embedding_version IS NULL OR embedding_version != '{}')", super::clip::EMBEDDING_VERSION)
}
/// Below this size (px, both sides) an image is an icon: no OCR.
const ICON: u32 = 40;

pub fn is_image(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXT.contains(&e.to_ascii_lowercase().as_str()))
}

pub fn status(conn: &Connection, shared: &Shared) -> IndexStatus {
    let (total, ocr, emb, err, sharp): (i64, i64, i64, i64, i64) = conn
        .query_row(
            "SELECT count(*), coalesce(sum(stage >= 1), 0), coalesce(sum(embedding IS NOT NULL AND embedding_version = ?), 0), coalesce(sum(stage = -1 OR ocr = -1 OR error IS NOT NULL), 0),
               coalesce(sum(stage >= 1 AND (ocr = 1 OR (text = '' AND NOT (width >= 640 AND height >= 360 AND width >= height)))), 0)
             FROM shots WHERE hidden = 0",
            [super::clip::EMBEDDING_VERSION],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap_or((0, 0, 0, 0, 0));
    let model = if shared.semantic.load(Relaxed) {
        shared.model.lock().unwrap().clone()
    } else {
        ModelInfo {
            state: "off",
            progress: None,
            error: None,
        }
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
        ModelInfo {
            state: "off",
            progress: None,
            error: None,
        }
    };
    let force_indexing = shared.force_indexing.load(Relaxed);
    let queued = |wh: &str| {
        conn.query_row(
            &format!("SELECT 1 FROM shots WHERE {wh} LIMIT 1"),
            [],
            |_| Ok(()),
        )
        .optional()
        .ok()
        .flatten()
        .is_some()
    };
    let visual_pending = shared.semantic.load(Relaxed) && queued(&embed_where());
    let sharp_pending = shared.sharp.load(Relaxed) && queued(SHARP_WHERE);
    let waiting_reason = if shared.paused.load(Relaxed) {
        Some("Paused by you".into())
    } else if model.state == "error" {
        Some("Visual model needs repair".into())
    } else if model.state == "downloading" {
        Some("Preparing visual model".into())
    } else if (visual_pending || sharp_pending)
        && shared.on_battery.load(Relaxed)
        && !force_indexing
    {
        Some("AI indexing waits for power. Finish indexing now to continue on battery.".into())
    } else if !shared.busy.load(Relaxed)
        && (visual_pending || sharp_pending)
        && !shared.user_idle.load(Relaxed)
        && !force_indexing
    {
        Some("Older images wait while you work. Finish indexing now to process the backlog.".into())
    } else {
        None
    };
    IndexStatus {
        state,
        total,
        ocr_done: ocr,
        embedded: emb,
        errors: err,
        model,
        sharp,
        text_model,
        waiting_reason,
        force_indexing,
        warnings: shared.warnings.lock().unwrap().clone(),
    }
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
                excluded: vec![],
                pending: HashMap::new(),
                clip: None,
                paddle: None,
                last_step: Duration::ZERO,
                status_at: Instant::now(),
                last_status: None,
                status_pending: false,
                indexed_dirty: false,
                indexed_at: Instant::now(),
                scanned_at: Instant::now(),
                rescan_at: None,
                next_lane: 0,
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
    excluded: Vec<PathBuf>,
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
    scanned_at: Instant,
    rescan_at: Option<Instant>,
    next_lane: u8,
}

impl Indexer {
    fn run(mut self, rx: Receiver<Msg>) {
        loop {
            let work = !self.shared.paused.load(Relaxed) && self.has_work();
            self.shared.busy.store(work, Relaxed);
            let wait = if work {
                if self.shared.on_battery.load(Relaxed) {
                    self.last_step.max(Duration::from_millis(400))
                } else {
                    Duration::ZERO
                }
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
            if self.scanned_at.elapsed() >= RECONCILE
                || self.rescan_at.is_some_and(|t| t.elapsed() >= SETTLE)
            {
                self.rescan_at = None;
                self.scan();
            }
            let work = !self.shared.paused.load(Relaxed) && self.has_work();
            self.shared.busy.store(work, Relaxed);
            if work {
                let t = Instant::now();
                self.step();
                self.last_step = t.elapsed();
            } else {
                self.last_step = Duration::ZERO;
                self.release_idle();
            }
            if !self.pending_work() {
                self.shared.force_indexing.store(false, Relaxed);
            }
            self.emit();
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Configure => {
                let roots = self.shared.folders.lock().unwrap().clone();
                let excluded = self.shared.excluded_folders.lock().unwrap().clone();
                if roots != self.roots || excluded != self.excluded {
                    self.roots = roots;
                    self.excluded = excluded;
                    self.watch();
                    self.scan();
                }
            }
            Msg::Kick => {}
            Msg::Rescan => {
                self.rescan_at = Some(Instant::now());
            }
            Msg::Warning(warning) => self.warn(warning),
            Msg::Fs(p) => {
                self.pending.insert(p, Instant::now());
            }
            Msg::Reindex => {
                self.shared.force_indexing.store(true, Relaxed);
                let _ = self.conn.execute_batch("INSERT OR IGNORE INTO refresh_queue SELECT id FROM shots WHERE hidden = 0; UPDATE shots SET error = NULL WHERE hidden = 0");
                self.scan();
            }
            Msg::RetryFailed => {
                self.shared.force_indexing.store(true, Relaxed);
                let _ = self.conn.execute_batch("INSERT OR IGNORE INTO refresh_queue SELECT id FROM shots WHERE hidden = 0 AND (stage = -1 OR error IS NOT NULL); UPDATE shots SET ocr = 0, error = NULL WHERE hidden = 0 AND (ocr = -1 OR error IS NOT NULL)");
            }
            Msg::RepairModels => {
                self.clip = None;
                self.paddle = None;
                // Block new inference before waiting for the last query to finish, then release sessions.
                *self.shared.model.lock().unwrap() = ModelInfo {
                    state: "downloading",
                    progress: Some(0.0),
                    error: None,
                };
                *self.shared.text_model.lock().unwrap() = ModelInfo {
                    state: "downloading",
                    progress: Some(0.0),
                    error: None,
                };
                *self.shared.query_clip.lock().unwrap() =
                    Clip::new(self.dirs.models.clone(), false);
                if self.shared.semantic.load(Relaxed) {
                    super::fetch_models(
                        self.shared.clone(),
                        self.dirs.models.clone(),
                        self.tx.clone(),
                        |s| &s.model,
                        &super::clip::FILES,
                        true,
                    );
                }
                if self.shared.sharp.load(Relaxed) {
                    super::fetch_models(
                        self.shared.clone(),
                        self.dirs.models.clone(),
                        self.tx.clone(),
                        |s| &s.text_model,
                        &super::ocr::paddle::FILES,
                        true,
                    );
                }
            }
            Msg::Clear(reply) => {
                let result = self.clear();
                let _ = reply.send(result);
            }
            Msg::Remove(ids) => self.remove(&ids),
        }
    }

    fn has_work(&self) -> bool {
        let q = |sql: &str| {
            self.conn
                .query_row(sql, [], |_| Ok(()))
                .optional()
                .ok()
                .flatten()
                .is_some()
        };
        q(
            "SELECT 1 FROM shots WHERE hidden = 0 AND (stage = 0 OR id IN (SELECT id FROM refresh_queue)) LIMIT 1",
        ) || (self.shared.semantic_ready()
            && self.may_embed()
            && q(&format!("SELECT 1 FROM shots WHERE {} LIMIT 1", embed_where())))
            || (self.shared.sharp_ready()
                && self.may_sharpen()
                && q(&format!("SELECT 1 FROM shots WHERE {SHARP_WHERE} LIMIT 1")))
    }

    fn may_embed(&self) -> bool {
        self.may_backlog(&embed_where())
    }

    fn may_sharpen(&self) -> bool {
        self.may_backlog(SHARP_WHERE)
    }

    /// New screenshots go right away; a large backlog waits until the user is away and on mains power.
    fn may_backlog(&self, wh: &str) -> bool {
        if self.shared.force_indexing.load(Relaxed) {
            return true;
        }
        if self.shared.on_battery.load(Relaxed) {
            return false;
        }
        if self.shared.user_idle.load(Relaxed) {
            return true;
        }
        let sql = format!(
            "SELECT count(*) FROM (SELECT 1 FROM shots WHERE {wh} LIMIT {})",
            SMALL_BACKLOG + 1
        );
        self.conn
            .query_row(&sql, [], |r| r.get::<_, i64>(0))
            .unwrap_or(0)
            <= SMALL_BACKLOG
            || self
                .conn
                .query_row(
                    &format!("SELECT 1 FROM shots WHERE {wh} AND mtime >= ? LIMIT 1"),
                    [recent_since()],
                    |_| Ok(()),
                )
                .optional()
                .ok()
                .flatten()
                .is_some()
    }

    fn pending_work(&self) -> bool {
        self.conn.query_row(&format!("SELECT 1 FROM shots WHERE hidden = 0 AND (stage = 0 OR id IN (SELECT id FROM refresh_queue) OR ({} AND ?) OR ({} AND ?)) LIMIT 1", embed_where(), SHARP_WHERE),
            (self.shared.semantic.load(Relaxed), self.shared.sharp.load(Relaxed)), |_| Ok(())).optional().ok().flatten().is_some()
    }

    fn warn(&self, warning: String) {
        let mut warnings = self.shared.warnings.lock().unwrap();
        if !warnings.contains(&warning) {
            if warnings.len() >= 20 {
                warnings.remove(0);
            }
            warnings.push(warning);
        }
    }

    fn root_of(&self, p: &Path) -> Option<PathBuf> {
        // Longest root wins when folders nest.
        if self.excluded.iter().any(|r| p.starts_with(r)) {
            return None;
        }
        self.roots
            .iter()
            .filter(|r| p.starts_with(r))
            .max_by_key(|r| r.as_os_str().len())
            .cloned()
    }

    fn watch(&mut self) {
        self.watchers.clear();
        for root in &self.roots {
            let tx = self.tx.clone();
            let r = root.clone();
            let excluded = self.excluded.clone();
            let w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                match res {
                    Ok(ev) => {
                        let directory_change = matches!(ev.kind,
                            notify::EventKind::Create(notify::event::CreateKind::Folder)
                            | notify::EventKind::Remove(notify::event::RemoveKind::Folder)
                            | notify::EventKind::Modify(notify::event::ModifyKind::Name(_)));
                        if ev.paths.is_empty() { let _ = tx.send(Msg::Rescan); }
                        for p in ev.paths.into_iter().filter(|p| !skipped(&r, p) && !excluded.iter().any(|dir| p.starts_with(dir))) {
                            if is_image(&p) || is_sidecar(&p) {
                                let _ = tx.send(Msg::Fs(p));
                            } else if (directory_change || p.is_dir()) && !p.file_name().is_some_and(|n| skip_dir(&n.to_string_lossy(), p.parent().unwrap_or(&r))) {
                                let _ = tx.send(Msg::Rescan);
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::Warning(format!("Folder watcher: {e}")));
                        let _ = tx.send(Msg::Rescan);
                    }
                }
            });
            match w {
                Ok(mut w) => match w.watch(root, RecursiveMode::Recursive) {
                    Ok(()) => self.watchers.push(w),
                    Err(e) => self.warn(format!(
                        "Cannot watch {}: {e}. Glint will check periodically.",
                        root.display()
                    )),
                },
                Err(e) => self.warn(format!("Cannot create folder watcher: {e}")),
            }
        }
    }

    /// Syncs the database with the folders: adds new files, requeues changed ones, drops missing ones.
    fn scan(&mut self) {
        self.shared.scanning.store(true, Relaxed);
        self.emit_now();
        let mut found: HashMap<PathBuf, (i64, i64, PathBuf)> = HashMap::new();
        let mut complete = vec![];
        for root in self.roots.clone() {
            let result = walk(&root, &self.excluded, &mut |p, mtime, size| {
                if let Some(r) = self.root_of(p) {
                    found.insert(p.to_path_buf(), (mtime, size, r));
                }
            });
            match result {
                Ok(()) => complete.push(root),
                Err(e) => self.warn(format!(
                    "Cannot completely scan {}: {e}. Existing search results are kept.",
                    root.display()
                )),
            }
        }
        let known: Vec<(i64, String, i64, i64)> = self
            .conn
            .prepare("SELECT id, path, mtime, size FROM shots")
            .and_then(|mut st| {
                st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                    .collect()
            })
            .unwrap_or_else(|e| { self.warn(format!("Cannot read the existing index: {e}")); vec![] });
        let mut gone = vec![];
        let reconcile = (|| -> rusqlite::Result<()> {
            let tx = self.conn.transaction()?;
            let mut seen = HashSet::new();
            for (id, path, mtime, size) in known {
                let key = PathBuf::from(&path);
                match found.get(&key) {
                    None => {
                        // Only a complete enumeration proves absence. Removed/excluded roots are intentional.
                        let placeholder = std::fs::metadata(&key).is_ok_and(|m| m.is_file() && !is_local(&m));
                        if self.excluded.iter().any(|r| key.starts_with(r))
                            || !self.roots.iter().any(|r| key.starts_with(r))
                            || (!placeholder && complete.iter().any(|r| key.starts_with(r)))
                        {
                            tx.execute("DELETE FROM shots WHERE id = ?", [id])?;
                            tx.execute("DELETE FROM refresh_queue WHERE id = ?", [id])?;
                            gone.push(id);
                        }
                    }
                    Some((m, s, root)) if *m != mtime || *s != size => {
                        tx.execute(
                            "UPDATE shots SET mtime = ?, size = ?, folder = ?, error = NULL WHERE id = ?",
                            (m, s, root.to_string_lossy(), id),
                        )?;
                        tx.execute("INSERT OR IGNORE INTO refresh_queue VALUES (?)", [id])?;
                    }
                    _ => {}
                }
                seen.insert(key);
            }
            for (p, (m, s, root)) in &found {
                if !seen.contains(p) {
                    let name = p
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    tx.execute(
                        "INSERT OR IGNORE INTO shots(path, folder, name, mtime, size) VALUES (?, ?, ?, ?, ?)",
                        (p.to_string_lossy(), root.to_string_lossy(), name, m, s),
                    )?;
                }
            }
            tx.commit()
        })();
        if let Err(e) = reconcile {
            gone.clear();
            self.warn(format!("Could not update the index: {e}. Existing search results are kept."));
        }
        for id in &gone {
            let _ = std::fs::remove_file(self.thumb(*id));
        }
        for path in found.keys() {
            self.import_sidecar(path);
        }
        if !gone.is_empty() {
            self.shared.invalidate_vectors();
        }
        self.shared.scanning.store(false, Relaxed);
        self.scanned_at = Instant::now();
        self.indexed_dirty = true;
    }

    fn thumb(&self, id: i64) -> PathBuf {
        self.dirs.thumbs.join(format!("{id}.jpg"))
    }

    fn flush_pending(&mut self) {
        let due: Vec<PathBuf> = self
            .pending
            .iter()
            .filter(|(_, t)| t.elapsed() >= SETTLE)
            .map(|(p, _)| p.clone())
            .collect();
        for p in due {
            self.pending.remove(&p);
            self.upsert(&p);
        }
    }

    fn upsert(&mut self, p: &Path) {
        if is_sidecar(p) {
            if let Some(stem) = p
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".magpie.json"))
            {
                for extension in IMAGE_EXT {
                    self.import_sidecar(&p.with_file_name(format!("{stem}.{extension}")));
                }
            }
            return;
        }
        let path = p.to_string_lossy();
        let row: Option<(i64, i64, i64)> = self
            .conn
            .query_row(
                "SELECT id, mtime, size FROM shots WHERE path = ?",
                [&path],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .ok()
            .flatten();
        let meta = match std::fs::metadata(p) {
            Ok(m) if m.is_file() && is_local(&m) => m,
            Ok(_) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // A temporary disconnected root is not proof the image was deleted.
                if self.root_of(p).is_some_and(|root| root.is_dir()) {
                    if let Some((id, _, _)) = row {
                        self.remove(&[id]);
                    }
                }
                return;
            }
            Err(e) => {
                self.warn(format!("Cannot read {}: {e}", p.display()));
                return;
            }
        };
        let root = self.root_of(p);
        let Some(root) = root else {
            return;
        };
        let (mtime, size) = (mtime_ms(&meta), meta.len() as i64);
        match row {
            None => {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if let Err(e) = self.conn.execute(
                    "INSERT OR IGNORE INTO shots(path, folder, name, mtime, size) VALUES (?, ?, ?, ?, ?)",
                    (&path, root.to_string_lossy(), name, mtime, size),
                ) { self.warn(format!("Cannot add {}: {e}", p.display())); }
            }
            Some((id, m, s)) if m != mtime || s != size => {
                let result = (|| -> rusqlite::Result<()> {
                    let tx = self.conn.transaction()?;
                    tx.execute("UPDATE shots SET mtime = ?, size = ?, error = NULL WHERE id = ?", (mtime, size, id))?;
                    tx.execute("INSERT OR IGNORE INTO refresh_queue VALUES (?)", [id])?;
                    tx.commit()
                })();
                if let Err(e) = result { self.warn(format!("Cannot refresh {}: {e}", p.display())); }
            }
            _ => {}
        }
        self.import_sidecar(p);
    }

    fn import_sidecar(&mut self, path: &Path) {
        if self.root_of(path).is_none() {
            return;
        }
        let id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM shots WHERE path = ?",
                [path.to_string_lossy()],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten();
        if let Some(id) = id {
            let has_context = super::capture::sidecar(path).is_file();
            match super::capture::apply_sidecar(&self.conn, id, path) {
                Err(e) => self.warn(format!("Capture context for {}: {e}", path.display())),
                Ok(()) if has_context => self.indexed_dirty = true,
                _ => {}
            }
        }
    }

    fn remove(&mut self, ids: &[i64]) {
        for id in ids {
            let _ = self.conn.execute("DELETE FROM shots WHERE id = ?", [id]);
            let _ = self
                .conn
                .execute("DELETE FROM refresh_queue WHERE id = ?", [id]);
            let _ = std::fs::remove_file(self.thumb(*id));
        }
        self.shared.invalidate_vectors();
        self.indexed_dirty = true;
    }

    fn clear(&mut self) -> anyhow::Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM shots", [])?;
        tx.execute("DELETE FROM refresh_queue", [])?;
        tx.execute("DELETE FROM query_feedback", [])?;
        tx.commit()?;
        self.pending.clear();
        self.shared.warnings.lock().unwrap().clear();
        self.shared.force_indexing.store(false, Relaxed);
        self.shared.invalidate_vectors();
        *self.shared.vocab.lock().unwrap() = None;
        self.indexed_dirty = true;
        for entry in std::fs::read_dir(&self.dirs.thumbs)? {
            let path = entry?.path();
            if managed_thumbnail(&path) {
                std::fs::remove_file(path)?;
            }
        }
        Ok(())
    }

    /// One unit of work: OCR one shot, or embed one batch.
    fn step(&mut self) {
        // Rotate lanes so thousands of OS reads cannot starve visual search or better OCR.
        for _ in 0..3 {
            let lane = self.next_lane;
            self.next_lane = (self.next_lane + 1) % 3;
            match lane {
                0 if self.read_batch() => return,
                1 if self.shared.semantic_ready() && self.may_embed() && self.embed_batch() => {
                    return;
                }
                2 if self.shared.sharp_ready() && self.may_sharpen() && self.sharpen() => return,
                _ => {}
            }
        }
    }

    fn read_batch(&mut self) -> bool {
        // Several images are read at once on background-priority threads; one per thread on battery.
        let workers = if self.shared.on_battery.load(Relaxed) {
            1
        } else {
            read_workers()
        };
        let batch: Vec<(i64, String, String, i64)> = self
            .conn
            .prepare("SELECT id, path, folder, mtime FROM shots WHERE hidden = 0 AND (stage = 0 OR id IN (SELECT id FROM refresh_queue)) ORDER BY mtime DESC LIMIT ?")
            .and_then(|mut st| st.query_map([workers * READS_PER_WORKER], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect())
            .unwrap_or_default();
        if !batch.is_empty() {
            let jobs: Vec<(String, PathBuf)> = batch
                .iter()
                .map(|(id, path, _, _)| (path.clone(), self.thumb(*id)))
                .collect();
            let results = read_all(&jobs, workers);
            for ((id, path, folder, mtime), result) in batch.iter().zip(results) {
                if let Err(e) = result.and_then(|r| self.store(*id, folder, *mtime, r)) {
                    let _ = self.conn.execute("UPDATE shots SET stage = CASE WHEN stage = 0 THEN -1 ELSE stage END, error = ? WHERE id = ?", (e.to_string(), id));
                }
                self.import_sidecar(Path::new(path));
                let _ = self
                    .conn
                    .execute("DELETE FROM refresh_queue WHERE id = ?", [id]);
            }
            self.indexed_dirty = true;
            return true;
        }
        false
    }

    fn store(&mut self, id: i64, folder: &str, mtime: i64, a: Read) -> anyhow::Result<()> {
        let Read {
            lines,
            w,
            h,
            dhash,
            colors,
        } = a;
        let a = (dhash, colors);
        let txt = lines
            .iter()
            .map(|l| l.t.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let mut tags = color_tags(&a.1);
        tags.extend(text::action_tags(&text::detect_actions(&txt)));
        if !txt.is_empty() {
            tags.push("h:text".into());
        }
        let line_texts: Vec<&str> = lines.iter().map(|l| l.t.as_str()).collect();
        let group = self.find_group(id, folder, mtime, &a.0, &line_texts);
        let colors: Vec<&str> = a.1.iter().map(|c| c.hex.as_str()).collect();
        self.conn.execute(
            "UPDATE shots SET stage = 1, ocr = 0, error = NULL, embedding = NULL, embedding_version = NULL, width = ?, height = ?, text = ?, norm = ?, lines = ?,
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
        self.shared.invalidate_vectors();
        Ok(())
    }

    /// Joins a burst: same folder, taken within BURST_MS, showing the same thing. Shots with text compare
    /// their lines (different pages of one app share a sidebar, not the rest); others a perceptual hash.
    fn find_group(
        &self,
        id: i64,
        folder: &str,
        mtime: i64,
        dhash: &str,
        lines: &[&str],
    ) -> Option<i64> {
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
            let _ = self
                .conn
                .execute("UPDATE shots SET group_id = ? WHERE id = ?", (gid, nid));
        }
        Some(gid)
    }

    /// Embeds the next batch. False when there was nothing to embed.
    fn embed_batch(&mut self) -> bool {
        let batch: Vec<(i64, String)> = self
            .conn
            .prepare(&format!("SELECT id, path FROM shots WHERE {} AND id NOT IN (SELECT id FROM refresh_queue) ORDER BY mtime DESC LIMIT ?", embed_where()))
            .and_then(|mut st| st.query_map([EMBED_BATCH], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
            .unwrap_or_default();
        if batch.is_empty() {
            return false;
        }
        let mut ok: Vec<(i64, Vec<f32>)> = Vec::new();
        for (id, path) in &batch {
            let px = std::panic::catch_unwind(|| {
                local_file(Path::new(path)).and_then(|_| image::decode(Path::new(path))).map(|img| image::clip_pixels(&img))
            });
            match px.unwrap_or_else(|_| Err(anyhow::anyhow!("Could not read this file"))) {
                Ok(px) => ok.push((*id, px)),
                Err(e) => {
                    let _ = self.conn.execute(
                        "UPDATE shots SET error = ? WHERE id = ?",
                        (e.to_string(), id),
                    );
                }
            }
        }
        let clip = self
            .clip
            .get_or_insert_with(|| Clip::new(self.dirs.models.clone(), true));
        let pixels: Vec<Vec<f32>> = ok.iter().map(|(_, p)| p.clone()).collect();
        match clip.embed_images(&pixels) {
            Ok(vecs) => {
                for ((id, _), v) in ok.iter().zip(vecs) {
                    let _ = self.conn.execute("UPDATE shots SET stage = 2, embedding = ?, embedding_version = ?, error = NULL WHERE id = ?", (db::to_blob(&v), super::clip::EMBEDDING_VERSION, id));
                }
                self.shared.invalidate_vectors();
                self.indexed_dirty = true;
            }
            Err(e) => {
                *self.shared.model.lock().unwrap() = ModelInfo {
                    state: "error",
                    progress: None,
                    error: Some(e.to_string()),
                };
            }
        }
        true
    }

    /// Re-reads the newest OS-read shot with PaddleOCR. Lines the OS read where PaddleOCR found
    /// nothing (and QR codes) are kept.
    fn sharpen(&mut self) -> bool {
        let sql = format!(
            "SELECT id, path, lines, tags FROM shots WHERE {SHARP_WHERE} ORDER BY mtime DESC LIMIT 1"
        );
        let next: Option<(i64, String, String, String)> = self
            .conn
            .query_row(&sql, [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .optional()
            .ok()
            .flatten();
        let Some((id, path, os_lines, tags)) = next else {
            return false;
        };
        if self.paddle.is_none() {
            let loaded = std::panic::catch_unwind(|| Paddle::load(&self.dirs.models))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("Text model failed. Repair models and retry.")));
            match loaded {
                Ok(p) => self.paddle = Some(p),
                Err(e) => {
                    *self.shared.text_model.lock().unwrap() = ModelInfo {
                        state: "error",
                        progress: None,
                        error: Some(e.to_string()),
                    };
                    return true;
                }
            }
        }
        let paddle = self.paddle.as_mut().unwrap();
        let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            local_file(Path::new(&path)).and_then(|_| image::decode(Path::new(&path)))
                .and_then(|img| ocr::recognize_with(&img, &mut |i| paddle.recognize(i)))
        }));
        if read.is_err() {
            self.paddle = None; // its state after a panic is unknown
        }
        let lines = match read.unwrap_or_else(|_| Err(anyhow::anyhow!("PaddleOCR failed"))) {
            Ok(lines) => ocr::merge(
                lines,
                &serde_json::from_str::<Vec<OcrLine>>(&os_lines).unwrap_or_default(),
            ),
            Err(e) => {
                let _ = self.conn.execute(
                    "UPDATE shots SET ocr = -1, error = ? WHERE id = ?",
                    (format!("Text recognition: {e}"), id),
                );
                return true;
            }
        };
        let txt = lines
            .iter()
            .map(|l| l.t.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let mut tags: Vec<String> = tags
            .split(' ')
            .filter(|t| t.starts_with("c:"))
            .map(String::from)
            .collect();
        tags.extend(text::action_tags(&text::detect_actions(&txt)));
        if !txt.is_empty() {
            tags.push("h:text".into());
        }
        let _ = self.conn.execute(
            "UPDATE shots SET ocr = 1, error = NULL, text = ?, norm = ?, lines = ?, tags = ? WHERE id = ?",
            rusqlite::params![txt, text::fold(&txt), serde_json::to_string(&lines).unwrap_or_default(), tags.join(" "), id],
        );
        self.indexed_dirty = true;
        true
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
            if v.as_ref()
                .is_some_and(|(_, used)| used.elapsed() > IDLE_RELEASE)
            {
                *v = None;
            }
        }
        if let Ok(mut v) = self.shared.vocab.try_lock() {
            if v.as_ref()
                .is_some_and(|(_, used)| used.elapsed() > IDLE_RELEASE)
            {
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
    local_file(Path::new(path))?;
    let a = image::analyze(Path::new(path), thumb)?;
    let (w, h) = (a.img.width(), a.img.height());
    let mut lines = if w.max(h) < ICON {
        vec![]
    } else {
        ocr::recognize(&a.img)?
    };
    lines.extend(ocr::qr_lines(&a.img));
    Ok(Read {
        lines,
        w,
        h,
        dhash: a.dhash,
        colors: a.colors,
    })
}

fn local_file(path: &Path) -> anyhow::Result<()> {
    let metadata = std::fs::metadata(path)?;
    anyhow::ensure!(metadata.is_file() && is_local(&metadata), "File is not available locally. Make it available on this device and retry.");
    Ok(())
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
                            std::panic::catch_unwind(|| read(path, thumb)).unwrap_or_else(|_| {
                                Err(anyhow::anyhow!("Could not read this file"))
                            })
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    })
}

fn mtime_ms(m: &std::fs::Metadata) -> i64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

/// Recursive walk, skipping the folders in [`skip_dir`] and (Windows) hidden or system folders such
/// as AppData. Links are not followed.
fn walk(
    dir: &Path,
    excluded: &[PathBuf],
    f: &mut dyn FnMut(&Path, i64, i64),
) -> std::io::Result<()> {
    if excluded.iter().any(|r| dir.starts_with(r)) {
        return Ok(());
    }
    let entries = std::fs::read_dir(dir)?;
    for e in entries {
        let e = e?;
        let p = e.path();
        let ft = e.file_type()?;
        if ft.is_dir() {
            if !skip_dir(&e.file_name().to_string_lossy(), dir) && !hidden(&e.metadata()?) {
                walk(&p, excluded, f)?;
            }
        } else if ft.is_file() && is_image(&p) {
            let m = e.metadata()?;
            if is_local(&m) {
                if !excluded.iter().any(|r| p.starts_with(r)) {
                    f(&p, mtime_ms(&m), m.len() as i64);
                }
            }
        }
    }
    Ok(())
}

fn recent_since() -> i64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
        - 24 * 60 * 60_000
}

fn is_sidecar(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".magpie.json"))
}

fn managed_thumbnail(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else { return false };
    let parts: Vec<_> = name.split('.').collect();
    if !parts.first().is_some_and(|id| id.parse::<i64>().is_ok()) { return false; }
    matches!(parts.as_slice(), [_, "jpg"] | [_, "part"])
        || matches!(parts.as_slice(), [_, sequence, "part"] if sequence.parse::<u64>().is_ok())
}

/// Folders never worth reading: hidden ones, the OS, installed programs, app data, caches and code
/// dependencies. They hold thousands of icons and cached images nobody searches for.
fn skip_dir(name: &str, parent: &Path) -> bool {
    let n = name.to_lowercase();
    if n.starts_with('.')
        || n.starts_with('$')
        || n.ends_with(".app")
        || n.ends_with(".photoslibrary")
    {
        return true;
    }
    if matches!(
        n.as_str(),
        "appdata"
            | "node_modules"
            | "bower_components"
            | "__pycache__"
            | "__macosx"
            | "site-packages"
            | "steamapps"
            | "system volume information"
    ) {
        return true;
    }
    // Directly under a drive root (C:\Windows), or ~/Library on macOS.
    let top = parent.parent().is_none();
    (top && matches!(
        n.as_str(),
        "windows"
            | "program files"
            | "program files (x86)"
            | "programdata"
            | "recovery"
            | "perflogs"
            | "msocache"
            | "windows.old"
    )) || (cfg!(target_os = "macos")
        && n == "library"
        && std::env::var_os("HOME").is_some_and(|h| Path::new(&h) == parent))
}

/// True if a folder between `root` and the file `p` is one [`walk`] skips.
fn skipped(root: &Path, p: &Path) -> bool {
    let Ok(rel) = p.strip_prefix(root) else {
        return true;
    };
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

    fn fixture(name: &str) -> Indexer {
        use std::sync::{
            Mutex,
            atomic::{AtomicBool, AtomicU64},
        };
        let data =
            std::env::temp_dir().join(format!("glint-indexer-{}-{name}", std::process::id()));
        std::fs::create_dir_all(data.join("thumbs")).unwrap();
        std::fs::create_dir_all(data.join("models")).unwrap();
        let dirs = Dirs {
            thumbs: data.join("thumbs"),
            models: data.join("models"),
            data,
        };
        let shared = Arc::new(Shared {
            folders: Mutex::new(vec![]),
            excluded_folders: Mutex::new(vec![]),
            warnings: Mutex::new(vec![]),
            model_io: Mutex::new(()),
            model_generation: AtomicU64::new(0),
            text_generation: AtomicU64::new(0),
            force_indexing: AtomicBool::new(false),
            semantic: AtomicBool::new(false),
            sharp: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            on_battery: AtomicBool::new(false),
            user_idle: AtomicBool::new(false),
            busy: AtomicBool::new(false),
            scanning: AtomicBool::new(false),
            model: std::sync::Mutex::new(ModelInfo {
                state: "ready",
                progress: None,
                error: None,
            }),
            text_model: Mutex::new(ModelInfo {
                state: "ready",
                progress: None,
                error: None,
            }),
            query_clip: Mutex::new(Clip::new(dirs.models.clone(), false)),
            vectors: Mutex::new(None),
            vocab: Mutex::new(None),
            emit: Box::new(|_| {}),
        });
        let (tx, _) = std::sync::mpsc::channel();
        Indexer {
            conn: db::open(&dirs.data.join("test.db")).unwrap(),
            dirs,
            shared,
            tx,
            watchers: vec![],
            roots: vec![],
            excluded: vec![],
            pending: HashMap::new(),
            clip: None,
            paddle: None,
            last_step: Duration::ZERO,
            status_at: Instant::now(),
            last_status: None,
            status_pending: false,
            indexed_dirty: false,
            indexed_at: Instant::now(),
            scanned_at: Instant::now(),
            rescan_at: None,
            next_lane: 0,
        }
    }

    fn cleanup(i: Indexer) {
        let data = i.dirs.data.clone();
        drop(i);
        std::fs::remove_dir_all(data).unwrap();
    }

    fn shot(i: &Indexer, id: i64, mtime: i64, stage: i64) {
        i.conn.execute("INSERT INTO shots(id,path,folder,name,mtime,size,stage,text,norm,embedding) VALUES (?,'shot-' || ?,'folder','shot',?,1,?,'kept text','kept text',x'01020304')", (id,id,mtime,stage)).unwrap();
    }

    #[test]
    fn recent_items_bypass_large_backlog_and_force_respects_battery() {
        let i = fixture("schedule");
        for id in 1..=50 {
            shot(&i, id, 0, 1);
        }
        assert!(!i.may_embed());
        i.conn
            .execute(
                "UPDATE shots SET mtime = ? WHERE id = 50",
                [recent_since() + 1],
            )
            .unwrap();
        assert!(i.may_embed());
        i.shared.on_battery.store(true, Relaxed);
        assert!(!i.may_embed());
        i.shared.force_indexing.store(true, Relaxed);
        assert!(i.may_embed());
        i.shared.semantic.store(true, Relaxed);
        i.shared.paused.store(true, Relaxed);
        assert_eq!(status(&i.conn, &i.shared).state, "paused");
        cleanup(i);
    }

    #[test]
    fn coverage_and_refresh_follow_the_shared_embedding_version() {
        let i = fixture("embedding-version");
        i.shared.semantic.store(true, Relaxed);
        for id in 1..=3 { shot(&i,id,0,2); }
        i.conn.execute("UPDATE shots SET embedding_version=? WHERE id=1",[super::super::clip::EMBEDDING_VERSION]).unwrap();
        i.conn.execute("UPDATE shots SET embedding_version='prior-embedding-contract' WHERE id=2",[]).unwrap();
        assert_eq!(status(&i.conn,&i.shared).embedded,1);
        assert_eq!(i.conn.query_row(&format!("SELECT count(*) FROM shots WHERE {}",embed_where()),[],|r|r.get::<_,i64>(0)).unwrap(),2);
        assert!(i.pending_work());
        i.conn.execute("UPDATE shots SET embedding_version=?",[super::super::clip::EMBEDDING_VERSION]).unwrap();
        assert_eq!(status(&i.conn,&i.shared).embedded,3);
        assert!(!i.pending_work());
        cleanup(i);
    }

    #[test]
    fn visual_lane_runs_before_initial_ocr_queue_is_drained() {
        let mut i = fixture("interleave");
        i.shared.semantic.store(true, Relaxed);
        i.shared.force_indexing.store(true, Relaxed);
        let count = read_workers() * READS_PER_WORKER + 1;
        for id in 1..=count {
            let path = i.dirs.data.join(format!("{id}.png"));
            ::image::RgbImage::new(2, 2).save(&path).unwrap();
            shot(&i, id as i64, recent_since() + 1, 0);
            i.conn.execute("UPDATE shots SET path=? WHERE id=?", (path.to_string_lossy(), id as i64)).unwrap();
        }
        i.step();
        assert_eq!(i.conn.query_row("SELECT count(*) FROM shots WHERE stage=0", [], |r| r.get::<_,i64>(0)).unwrap(), 1);
        i.step();
        // Missing test models deliberately fail the visual lane. The final OS job must still be queued.
        assert_eq!(i.shared.model.lock().unwrap().state, "error");
        assert_eq!(i.conn.query_row("SELECT count(*) FROM shots WHERE stage=0", [], |r| r.get::<_,i64>(0)).unwrap(), 1);
        cleanup(i);
    }

    #[test]
    fn rebuild_and_failed_retry_preserve_searchable_content() {
        let mut i = fixture("rebuild");
        let root = i.dirs.data.clone();
        i.roots.push(root.join("offline"));
        shot(&i, 1, 0, 2);
        i.conn
            .execute(
                "UPDATE shots SET path = ? WHERE id = 1",
                [root.join("offline").join("shot.png").to_string_lossy()],
            )
            .unwrap();
        i.handle(Msg::Reindex);
        let kept: (String, i64, Vec<u8>) = i
            .conn
            .query_row(
                "SELECT text,stage,embedding FROM shots WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(kept, ("kept text".into(), 2, vec![1, 2, 3, 4]));
        assert_eq!(
            i.conn
                .query_row("SELECT count(*) FROM refresh_queue", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(i.read_batch());
        let after_failure: (String, i64, Vec<u8>) = i.conn.query_row("SELECT text,stage,embedding FROM shots WHERE id=1", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(after_failure, kept);
        assert_eq!(i.conn.query_row("SELECT count(*) FROM refresh_queue", [], |r| r.get::<_,i64>(0)).unwrap(),0);
        i.conn
            .execute("UPDATE shots SET ocr=-1,error='read failed' WHERE id=1", [])
            .unwrap();
        i.handle(Msg::RetryFailed);
        assert_eq!(
            i.conn
                .query_row("SELECT ocr FROM shots WHERE id=1", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            i.conn
                .query_row("SELECT count(*) FROM refresh_queue", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        cleanup(i);
    }

    #[test]
    fn inaccessible_scan_keeps_rows_and_explicit_exclusion_removes_them() {
        let mut i = fixture("offline");
        let root = i.dirs.data.join("unavailable");
        i.roots.push(root.clone());
        shot(&i, 1, 0, 2);
        i.conn
            .execute(
                "UPDATE shots SET path = ? WHERE id=1",
                [root.join("a.png").to_string_lossy()],
            )
            .unwrap();
        i.scan();
        assert_eq!(
            i.conn
                .query_row("SELECT count(*) FROM shots", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(!i.shared.warnings.lock().unwrap().is_empty());
        i.excluded.push(root);
        i.scan();
        assert_eq!(
            i.conn
                .query_row("SELECT count(*) FROM shots", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        cleanup(i);
    }

    #[test]
    fn clear_deletes_only_managed_numeric_thumbnails() {
        let mut i = fixture("clear");
        shot(&i, 1, 0, 2);
        std::fs::write(i.dirs.thumbs.join("1.jpg"), b"thumb").unwrap();
        std::fs::write(i.dirs.thumbs.join("1.0.part"), b"partial thumb").unwrap();
        std::fs::write(i.dirs.thumbs.join("keep.jpg"), b"other").unwrap();
        i.clear().unwrap();
        assert!(!i.dirs.thumbs.join("1.jpg").exists());
        assert!(!i.dirs.thumbs.join("1.0.part").exists());
        assert!(i.dirs.thumbs.join("keep.jpg").exists());
        assert_eq!(
            i.conn
                .query_row("SELECT count(*) FROM shots", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        cleanup(i);
    }

    #[test]
    fn skips_system_and_app_folders() {
        let root = if cfg!(windows) {
            Path::new("C:\\")
        } else {
            Path::new("/")
        };
        assert!(skip_dir("Windows", root));
        assert!(skip_dir("Program Files", root));
        assert!(!skip_dir("Windows", &root.join("wallpapers")));
        assert!(skip_dir("node_modules", &root.join("code")));
        assert!(skip_dir(".git", &root.join("code")));
        assert!(!skip_dir("Screenshots", &root.join("Pictures")));
        let home = root.join("Users").join("me");
        assert!(skipped(
            root,
            &home.join("AppData").join("Local").join("x.png")
        ));
        assert!(!skipped(root, &home.join("Pictures").join("x.png")));
        // A chosen folder inside AppData is a root, so nothing above it is checked.
        let chosen = home.join("AppData").join("Roaming").join("Tool");
        assert!(!skipped(&chosen, &chosen.join("shot.png")));
    }
}
