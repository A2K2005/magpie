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
    let mut db = Connection::open(file)?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.execute_batch(SCHEMA)?;
    // One transaction serializes schema upgrades across the UI and indexer connections.
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    for (name, definition) in [
        ("note", "TEXT NOT NULL DEFAULT ''"),
        ("user_tags", "TEXT NOT NULL DEFAULT '[]'"),
        ("collections", "TEXT NOT NULL DEFAULT '[]'"),
        ("source_url", "TEXT NOT NULL DEFAULT ''"),
        ("embedding_version", "TEXT"),
        ("ocr", "INTEGER NOT NULL DEFAULT 0"),
    ] {
        let exists: bool = tx.query_row("SELECT count(*) FROM pragma_table_info('shots') WHERE name = ?", [name], |r| r.get(0))?;
        if !exists {
            tx.execute_batch(&format!("ALTER TABLE shots ADD COLUMN {name} {definition}"))?;
        }
    }
    let expanded: bool = tx.query_row("SELECT count(*) FROM pragma_table_info('fts') WHERE name = 'note'", [], |r| r.get(0))?;
    if !expanded {
        tx.execute_batch(r#"
DROP TRIGGER IF EXISTS shots_ai;
DROP TRIGGER IF EXISTS shots_ad;
DROP TRIGGER IF EXISTS shots_au;
DROP TABLE fts;
CREATE VIRTUAL TABLE fts USING fts5(name, text, norm, note, user_tags, content='shots', content_rowid='id', tokenize='trigram');
CREATE TRIGGER shots_ai AFTER INSERT ON shots BEGIN
 INSERT INTO fts(rowid,name,text,norm,note,user_tags) VALUES(new.id,new.name,new.text,new.norm,new.note,new.user_tags);
END;
CREATE TRIGGER shots_ad AFTER DELETE ON shots BEGIN
 INSERT INTO fts(fts,rowid,name,text,norm,note,user_tags) VALUES('delete',old.id,old.name,old.text,old.norm,old.note,old.user_tags);
END;
CREATE TRIGGER shots_au AFTER UPDATE OF name,text,norm,note,user_tags ON shots BEGIN
 INSERT INTO fts(fts,rowid,name,text,norm,note,user_tags) VALUES('delete',old.id,old.name,old.text,old.norm,old.note,old.user_tags);
 INSERT INTO fts(rowid,name,text,norm,note,user_tags) VALUES(new.id,new.name,new.text,new.norm,new.note,new.user_tags);
END;
INSERT INTO fts(fts) VALUES('rebuild');
"#)?;
    }
    tx.execute_batch("CREATE TABLE IF NOT EXISTS refresh_queue(id INTEGER PRIMARY KEY);
        CREATE TABLE IF NOT EXISTS query_feedback(query TEXT NOT NULL, shot_id INTEGER NOT NULL, PRIMARY KEY(query,shot_id));
        CREATE TRIGGER IF NOT EXISTS shots_cleanup AFTER DELETE ON shots BEGIN
          DELETE FROM refresh_queue WHERE id=old.id;
          DELETE FROM query_feedback WHERE shot_id=old.id;
        END;")?;
    tx.commit()?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        let dir=std::env::temp_dir().join(format!("glint-db-{}-{name}",std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("library.db")
    }

    fn matches(conn:&Connection,query:&str)->i64 {
        conn.query_row("SELECT count(*) FROM fts WHERE fts MATCH ?",[query],|r|r.get(0)).unwrap()
    }

    #[test]
    fn migration_preserves_rows_ocr_pins_vectors_and_rebuilds_live_fts() {
        let path=fixture("preserve");
        let old=Connection::open(&path).unwrap();
        old.execute_batch(SCHEMA).unwrap();
        old.execute("INSERT INTO shots(id,path,folder,name,mtime,size,stage,text,norm,ocr,pinned,embedding) VALUES(7,'shot.png','folder','image.png',123,456,2,'oldword dog','oldword dog',1,1,?)",[to_blob(&[0.25,0.75])]).unwrap();
        drop(old);
        let migrated=open(&path).unwrap();
        let row:(i64,i64,i64,String,Vec<u8>,Option<String>)=migrated.query_row("SELECT stage,ocr,pinned,text,embedding,embedding_version FROM shots WHERE id=7",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).unwrap();
        assert_eq!(row,(2,1,1,"oldword dog".into(),to_blob(&[0.25,0.75]),None));
        assert_eq!(matches(&migrated,"oldword"),1);
        migrated.execute("UPDATE shots SET text='newword',norm='newword',note='workflow notes',user_tags='[\"animal\"]' WHERE id=7",[]).unwrap();
        assert_eq!(matches(&migrated,"oldword"),0);
        for query in ["newword","workflow","animal"] { assert_eq!(matches(&migrated,query),1); }
        drop(migrated);
        let reopened=open(&path).unwrap();
        assert_eq!(matches(&reopened,"workflow"),1);
        assert_eq!(reopened.query_row("SELECT ocr+pinned FROM shots WHERE id=7",[],|r|r.get::<_,i64>(0)).unwrap(),2);
        reopened.execute("INSERT INTO refresh_queue VALUES(7)",[]).unwrap();
        reopened.execute("INSERT INTO query_feedback VALUES('dog',7)",[]).unwrap();
        reopened.execute("DELETE FROM shots WHERE id=7",[]).unwrap();
        assert_eq!(matches(&reopened,"workflow"),0);
        for table in ["refresh_queue","query_feedback"] { assert_eq!(reopened.query_row(&format!("SELECT count(*) FROM {table}"),[],|r|r.get::<_,i64>(0)).unwrap(),0); }
        drop(reopened);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn concurrent_migration_of_pre_ocr_database_is_idempotent() {
        let path=fixture("pre-ocr");
        let old=Connection::open(&path).unwrap();
        let schema=SCHEMA.replace("  embedding BLOB,\n  ocr INTEGER NOT NULL DEFAULT 0  -- text read by: 0 the OS, 1 PaddleOCR, -1 PaddleOCR failed","  embedding BLOB");
        old.execute_batch(&schema).unwrap();
        old.execute("INSERT INTO shots(path,folder,name,mtime,size,text,norm,pinned) VALUES('old.png','folder','legacy.png',1,1,'legacy text','legacy text',1)",[]).unwrap();
        drop(old);
        std::thread::scope(|scope| {
            let handles:Vec<_>=(0..3).map(|_|scope.spawn(|| {
                let conn=open(&path).unwrap();
                assert_eq!(conn.query_row("SELECT ocr FROM shots",[],|r|r.get::<_,i64>(0)).unwrap(),0);
                assert_eq!(matches(&conn,"legacy"),1);
            })).collect();
            for handle in handles { handle.join().unwrap(); }
        });
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
