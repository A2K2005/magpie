//! User-authored context and explicit feedback. Originals remain owned by their folders.
use super::Engine;
use anyhow::Context;
use crate::types::ShotMetadata;
use rusqlite::params;
use std::collections::BTreeSet;
use std::path::Path;

pub fn validate(mut value: ShotMetadata) -> anyhow::Result<ShotMetadata> {
    anyhow::ensure!(value.note.len() <= 32_000, "Notes must be at most 32,000 bytes");
    for values in [&mut value.tags, &mut value.collections] {
        anyhow::ensure!(values.len() <= 64, "Use at most 64 tags or collections");
        *values = values.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        anyhow::ensure!(values.iter().all(|s| s.len() <= 128), "Each tag or collection must be at most 128 bytes");
        let mut seen = BTreeSet::new();
        values.retain(|s| seen.insert(s.to_lowercase()));
    }
    value.source_url = value.source_url.trim().to_string();
    anyhow::ensure!(value.source_url.len() <= 8192, "Source URL is too long");
    if !value.source_url.is_empty() {
        let url = tauri::Url::parse(&value.source_url)?;
        anyhow::ensure!(matches!(url.scheme(), "https" | "http") && url.host_str().is_some(), "Source must be an http or https URL");
        anyhow::ensure!(url.username().is_empty() && url.password().is_none(), "Do not store credentials in source URLs");
    }
    Ok(value)
}

impl Engine {
    pub fn update_metadata(&self, id: i64, value: ShotMetadata) -> anyhow::Result<ShotMetadata> {
        let value = validate(value)?;
        let changed = self.conn().execute(
            "UPDATE shots SET note=?,user_tags=?,collections=?,source_url=? WHERE id=? AND hidden=0",
            params![value.note, serde_json::to_string(&value.tags)?, serde_json::to_string(&value.collections)?, value.source_url, id],
        )?;
        anyhow::ensure!(changed == 1, "Image is no longer in this library");
        *self.shared.vocab.lock().unwrap() = None;
        (self.shared.emit)(super::Event::Indexed);
        Ok(value)
    }

    pub fn set_relevant(&self, id: i64, query: &str, relevant: bool) -> anyhow::Result<()> {
        let query = query.trim().to_lowercase();
        anyhow::ensure!(!query.is_empty() && query.len() <= 8192, "A nonempty search is required");
        let conn = self.conn();
        if relevant {
            conn.execute("DELETE FROM query_feedback WHERE query=? AND shot_id=?", params![query,id])?;
        } else {
            anyhow::ensure!(conn.query_row("SELECT EXISTS(SELECT 1 FROM shots WHERE id=? AND hidden=0)", [id], |r| r.get::<_,bool>(0))?, "Image not found");
            conn.execute("INSERT OR IGNORE INTO query_feedback(query,shot_id) VALUES(?,?)", params![query,id])?;
        }
        drop(conn);
        (self.shared.emit)(super::Event::Indexed);
        Ok(())
    }

    /// A new directory avoids overwriting any existing export or original.
    pub fn export_shots(&self, ids: &[i64], destination: &Path) -> anyhow::Result<String> {
        let ids: BTreeSet<_> = ids.iter().copied().collect();
        anyhow::ensure!(!ids.is_empty() && ids.len() <= 5000, "Select between 1 and 5,000 images");
        let dir = destination.join(format!("Magpie export {}", chrono::Local::now().format("%Y-%m-%d %H.%M.%S%.3f")));
        std::fs::create_dir(&dir).with_context(||format!("Cannot create export folder {}",dir.display()))?;
        let result=(|| -> anyhow::Result<()> {
        let mut entries = Vec::new();
        for id in ids {
            let detail = self.get_shot(id)?.ok_or_else(|| anyhow::anyhow!("Image {id} is no longer available; export is incomplete"))?;
            let basename = Path::new(&detail.shot.path).file_name().ok_or_else(|| anyhow::anyhow!("Invalid image path"))?.to_string_lossy();
            let name = format!("{id}-{basename}");
            let mut src = std::fs::File::open(&detail.shot.path).with_context(||format!("Cannot read image {id}: {}",detail.shot.path))?;
            let mut dst = std::fs::OpenOptions::new().write(true).create_new(true).open(dir.join(&name))?;
            std::io::copy(&mut src, &mut dst).with_context(||format!("Cannot copy image {id}"))?;
            entries.push(serde_json::json!({"file":name,"metadata":detail.metadata,"pinned":detail.shot.pinned}));
        }
        std::fs::write(dir.join("manifest.json"), serde_json::to_vec_pretty(&serde_json::json!({"version":1,"images":entries}))?)?;
        Ok(())
        })();
        if let Err(error)=result {
            anyhow::bail!("Export incomplete in {}. Files already copied were kept. {error:#}",dir.display());
        }
        Ok(dir.to_string_lossy().into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name:&str)->Engine {
        use crate::engine::{Dirs,Shared,clip::Clip};
        use std::sync::{Arc,Mutex,atomic::{AtomicBool,AtomicU64},mpsc::channel};
        let data=std::env::temp_dir().join(format!("glint-export-{}-{name}",std::process::id()));
        std::fs::create_dir_all(data.join("exports")).unwrap();
        let dirs=Dirs{thumbs:data.join("thumbs"),models:data.join("models"),data};
        let shared=Arc::new(Shared {
            folders:Mutex::new(vec![]),excluded_folders:Mutex::new(vec![]),warnings:Mutex::new(vec![]),force_indexing:AtomicBool::new(false),
            model_io:Mutex::new(()),model_generation:AtomicU64::new(0),text_generation:AtomicU64::new(0),
            semantic:AtomicBool::new(false),sharp:AtomicBool::new(false),paused:AtomicBool::new(true),on_battery:AtomicBool::new(false),
            user_idle:AtomicBool::new(false),busy:AtomicBool::new(false),scanning:AtomicBool::new(false),
            model:Mutex::new(Default::default()),text_model:Mutex::new(Default::default()),query_clip:Mutex::new(Clip::new(dirs.models.clone(),false)),
            vectors:Mutex::new(None),vocab:Mutex::new(None),emit:Box::new(|_|{}),
        });
        let (tx,_)=channel();
        Engine { read:Mutex::new(crate::engine::db::open(Path::new(":memory:")).unwrap()),dirs,shared,tx }
    }

    fn add_shot(engine:&Engine,id:i64,name:&str) {
        engine.conn().execute("INSERT INTO shots(id,path,folder,name,mtime,size,stage) VALUES(?,?,?, ?,1,25,1)",
            params![id,engine.dirs.data.join(name).to_string_lossy(),engine.dirs.data.to_string_lossy(),name]).unwrap();
    }

    #[test]
    fn export_deduplicates_ids_preserves_originals_and_writes_metadata_manifest() {
        let engine=fixture("complete");
        let original=engine.dirs.data.join("first.png");
        let bytes=b"original screenshot bytes";
        std::fs::write(&original,bytes).unwrap();
        add_shot(&engine,1,"first.png");
        let metadata=ShotMetadata{note:"Reviewed".into(),tags:vec!["design".into()],collections:vec!["Research".into()],source_url:"https://example.com/source".into()};
        engine.update_metadata(1,metadata.clone()).unwrap();
        engine.pin(1,true);
        let exported=engine.export_shots(&[1,1],&engine.dirs.data.join("exports")).unwrap();
        let directory=Path::new(&exported);
        assert_eq!(std::fs::read(&original).unwrap(),bytes);
        assert_eq!(std::fs::read(directory.join("1-first.png")).unwrap(),bytes);
        let manifest:serde_json::Value=serde_json::from_slice(&std::fs::read(directory.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["version"],1);
        let images=manifest["images"].as_array().unwrap();
        assert_eq!(images.len(),1);
        assert_eq!(images[0]["file"],"1-first.png");
        assert_eq!(images[0]["pinned"],true);
        assert_eq!(images[0]["metadata"],serde_json::to_value(metadata).unwrap());
        assert_eq!(std::fs::read_dir(directory).unwrap().count(),2);
        let root=engine.dirs.data.clone();
        drop(engine);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_export_reports_and_keeps_partial_directory() {
        let engine=fixture("partial");
        let original=engine.dirs.data.join("first.png");
        let bytes=b"original screenshot bytes";
        std::fs::write(&original,bytes).unwrap();
        add_shot(&engine,1,"first.png");
        add_shot(&engine,2,"missing.png");
        let destination=engine.dirs.data.join("exports");
        let error=engine.export_shots(&[1,2],&destination).unwrap_err().to_string();
        let directory=std::fs::read_dir(&destination).unwrap().next().unwrap().unwrap().path();
        assert!(error.contains(directory.to_str().unwrap()),"{error}");
        assert!(error.contains("Files already copied were kept"),"{error}");
        assert!(error.contains("image 2") && error.contains("missing.png"),"{error}");
        assert_eq!(std::fs::read(directory.join("1-first.png")).unwrap(),bytes);
        assert!(!directory.join("manifest.json").exists());
        assert_eq!(std::fs::read(original).unwrap(),bytes);
        let root=engine.dirs.data.clone();
        drop(engine);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn metadata_is_bounded_and_urls_cannot_execute() {
        let v=validate(ShotMetadata{tags:vec![" PM ".into(),"pm".into(),"".into()], ..Default::default()}).unwrap();
        assert_eq!(v.tags, vec!["PM"]);
        for url in ["javascript:alert(1)","file:///secret","https://user:password@example.com"] {
            assert!(validate(ShotMetadata{source_url:url.into(),..Default::default()}).is_err());
        }
        assert!(validate(ShotMetadata{source_url:"https://example.com/page".into(),..Default::default()}).is_ok());
    }
}
