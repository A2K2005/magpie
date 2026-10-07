//! SQLite schema and connection setup. One FTS5 trigram row per screenshot.

use rusqlite::Connection;
use std::path::Path;

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
CREATE TABLE IF NOT EXISTS shots (
  id INTEGER PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,
  folder TEXT NOT NULL,
  name TEXT NOT NULL,
  mtime INTEGER NOT NULL,
  size INTEGER NOT NULL,
  width INTEGER NOT NULL DEFAULT 0,
  height INTEGER NOT NULL DEFAULT 0,
  stage INTEGER NOT NULL DEFAULT 0,  -- 0 queued, 1 text indexed, 2 embedded, -1 failed
  error TEXT,
  text TEXT NOT NULL DEFAULT '',
  norm TEXT NOT NULL DEFAULT '',
  lines TEXT NOT NULL DEFAULT '[]',
  dhash TEXT,
  colors TEXT NOT NULL DEFAULT '[]',
  tags TEXT NOT NULL DEFAULT '',
  group_id INTEGER,
  pinned INTEGER NOT NULL DEFAULT 0,
  hidden INTEGER NOT NULL DEFAULT 0,
  embedding BLOB,
  ocr INTEGER NOT NULL DEFAULT 0  -- text read by: 0 the OS, 1 PaddleOCR, -1 PaddleOCR failed
);
CREATE INDEX IF NOT EXISTS shots_mtime ON shots(mtime DESC);
CREATE INDEX IF NOT EXISTS shots_stage ON shots(stage, mtime DESC);
CREATE INDEX IF NOT EXISTS shots_group ON shots(group_id);
CREATE INDEX IF NOT EXISTS shots_folder_mtime ON shots(folder, mtime);
CREATE VIRTUAL TABLE IF NOT EXISTS fts USING fts5(
  name, text, norm, content='shots', content_rowid='id', tokenize='trigram'
);
CREATE TRIGGER IF NOT EXISTS shots_ai AFTER INSERT ON shots BEGIN
  INSERT INTO fts(rowid, name, text, norm) VALUES (new.id, new.name, new.text, new.norm);
END;
CREATE TRIGGER IF NOT EXISTS shots_ad AFTER DELETE ON shots BEGIN
  INSERT INTO fts(fts, rowid, name, text, norm) VALUES ('delete', old.id, old.name, old.text, old.norm);
END;
CREATE TRIGGER IF NOT EXISTS shots_au AFTER UPDATE OF name, text, norm ON shots BEGIN
  INSERT INTO fts(fts, rowid, name, text, norm) VALUES ('delete', old.id, old.name, old.text, old.norm);
  INSERT INTO fts(rowid, name, text, norm) VALUES (new.id, new.name, new.text, new.norm);
END;
"#;

/// Opens (and migrates) the database. Each thread gets its own connection; WAL lets readers run
/// while the indexer writes.
pub fn open(file: &Path) -> rusqlite::Result<Connection> {
    let db = Connection::open(file)?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.execute_batch(SCHEMA)?;
    // Databases from before PaddleOCR.
    let has_ocr: bool = db.query_row("SELECT count(*) FROM pragma_table_info('shots') WHERE name = 'ocr'", [], |r| r.get(0))?;
    if !has_ocr {
        db.execute_batch("ALTER TABLE shots ADD COLUMN ocr INTEGER NOT NULL DEFAULT 0")?;
    }
    db.execute_batch("CREATE INDEX IF NOT EXISTS shots_ocr ON shots(ocr, mtime DESC);")?;
    // Small page cache: search touches few pages, and memory is the budget we care about.
    db.execute_batch("PRAGMA cache_size = -2000;")?;
    Ok(db)
}

pub fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

pub fn from_blob(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}
