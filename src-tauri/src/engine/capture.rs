//! Optional browser capture sidecars, imported only beside the matching image.
use rusqlite::{Connection, params};
use serde::Deserialize;
use std::io::Read;
use std::path::{Path,PathBuf};

#[derive(Deserialize)]
#[serde(rename_all="camelCase")]
struct Capture { version: u32, image_name: String, source_url: String, title: String }

pub fn sidecar(image: &Path) -> PathBuf { image.with_extension("magpie.json") }

pub fn apply_sidecar(conn: &Connection, id: i64, image: &Path) -> anyhow::Result<()> {
    let path=sidecar(image);
    let file=match std::fs::File::open(&path) { Ok(file)=>file, Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(()), Err(e)=>return Err(e.into()) };
    let mut bytes=Vec::new();
    file.take(64_001).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len()<=64_000,"Capture context is too large");
    let capture:Capture=serde_json::from_slice(&bytes)?;
    anyhow::ensure!(capture.version==1 && image.file_name().and_then(|s|s.to_str())==Some(&capture.image_name),"Capture context belongs to a different image");
    let metadata=super::metadata::validate(crate::types::ShotMetadata {
        note:format!("Captured page: {}",capture.title),source_url:capture.source_url,..Default::default()
    })?;
    let changed=conn.execute("UPDATE shots SET source_url=CASE WHEN source_url='' THEN ? ELSE source_url END, note=CASE WHEN note='' THEN ? ELSE note END WHERE id=? AND path=?",params![metadata.source_url,metadata.note,id,image.to_string_lossy()])?;
    anyhow::ensure!(changed==1,"Capture image is no longer in this library");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sidecar_keeps_parent_and_stem() {
        assert_eq!(sidecar(Path::new("captures/shot.png")),PathBuf::from("captures/shot.magpie.json"));
    }

    fn fixture(name:&str)->(Connection,PathBuf) {
        let dir=std::env::temp_dir().join(format!("glint-capture-{}-{name}",std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let conn=super::super::db::open(&dir.join("test.db")).unwrap();
        let image=dir.join("shot.png");
        conn.execute("INSERT INTO shots(id,path,folder,name,mtime,size) VALUES(1,?,'captures','shot.png',1,1)",[image.to_string_lossy()]).unwrap();
        conn.execute("INSERT INTO shots(id,path,folder,name,mtime,size) VALUES(2,?,'captures','other.png',1,1)",[dir.join("other.png").to_string_lossy()]).unwrap();
        (conn,image)
    }

    fn context(image:&Path,version:u32,image_name:&str,url:&str,title:&str) {
        std::fs::write(sidecar(image),serde_json::to_vec(&serde_json::json!({"version":version,"imageName":image_name,"sourceUrl":url,"title":title})).unwrap()).unwrap();
    }

    #[test]
    fn imports_bound_context_without_overwriting_user_edits() {
        let (conn,image)=fixture("import");
        context(&image,1,"shot.png","https://example.com/original","Original page");
        apply_sidecar(&conn,1,&image).unwrap();
        let row:(String,String)=conn.query_row("SELECT source_url,note FROM shots WHERE id=1",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(row,("https://example.com/original".into(),"Captured page: Original page".into()));
        conn.execute("UPDATE shots SET source_url='https://example.com/manual',note='My note' WHERE id=1",[]).unwrap();
        context(&image,1,"shot.png","https://example.com/replacement","Updated page");
        apply_sidecar(&conn,1,&image).unwrap();
        let edited:(String,String)=conn.query_row("SELECT source_url,note FROM shots WHERE id=1",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(edited,("https://example.com/manual".into(),"My note".into()));
        assert!(apply_sidecar(&conn,2,&image).is_err());
        assert_eq!(conn.query_row("SELECT source_url FROM shots WHERE id=2",[],|r|r.get::<_,String>(0)).unwrap(),"");
        drop(conn);
        std::fs::remove_dir_all(image.parent().unwrap()).unwrap();
    }

    #[test]
    fn rejects_mismatched_unsafe_and_oversized_context() {
        let (conn,image)=fixture("reject");
        for (version,name,url) in [
            (1,"different.png","https://example.com"),
            (2,"shot.png","https://example.com"),
            (1,"shot.png","javascript:alert(1)"),
            (1,"shot.png","https://user:password@example.com"),
        ] {
            context(&image,version,name,url,"Page");
            assert!(apply_sidecar(&conn,1,&image).is_err());
        }
        std::fs::write(sidecar(&image),vec![b' ';64_001]).unwrap();
        assert!(apply_sidecar(&conn,1,&image).unwrap_err().to_string().contains("too large"));
        assert_eq!(conn.query_row("SELECT source_url || note FROM shots WHERE id=1",[],|r|r.get::<_,String>(0)).unwrap(),"");
        drop(conn);
        std::fs::remove_dir_all(image.parent().unwrap()).unwrap();
    }
}
