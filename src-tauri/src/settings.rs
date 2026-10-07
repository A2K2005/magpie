//! Settings file and the first-run folder suggestions.

use crate::engine::is_image;
use crate::types::{FolderSuggestion, Settings};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

fn file(app: &AppHandle) -> PathBuf {
    std::env::var_os("MAGPIE_USER_DATA").map(PathBuf::from)
        .unwrap_or_else(|| app.path().app_data_dir().expect("application data directory"))
        .join("settings.json")
}

pub fn load(app: &AppHandle) -> anyhow::Result<Settings> {
    match std::fs::read_to_string(file(app)) {
        Ok(json) => Ok(serde_json::from_str(&json)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(e.into()),
    }
}

pub fn save(app: &AppHandle, s: &Settings) -> anyhow::Result<()> {
    save_to(&file(app), s)
}

fn save_to(path: &Path, s: &Settings) -> anyhow::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(path.parent().ok_or_else(|| anyhow::anyhow!("Invalid settings path"))?)?;
    let pending = path.with_extension("json.pending");
    let mut f = std::fs::File::create(&pending)?;
    f.write_all(&serde_json::to_vec_pretty(s)?)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&pending, path)?;
    Ok(())
}

/// Where common screenshot tools save on this OS. Existing folders only.
fn candidates(app: &AppHandle) -> Vec<(PathBuf, String)> {
    let p = app.path();
    let home = p.home_dir().unwrap_or_default();
    let pictures = p.picture_dir().unwrap_or_else(|_| home.join("Pictures"));
    let desktop = p.desktop_dir().unwrap_or_else(|_| home.join("Desktop"));
    let mut out: Vec<(PathBuf, String)> = Vec::new();
    if cfg!(target_os = "macos") {
        if let Ok(o) = std::process::Command::new("defaults").args(["read", "com.apple.screencapture", "location"]).output() {
            let loc = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !loc.is_empty() {
                let loc = loc.strip_prefix('~').map_or_else(|| PathBuf::from(&loc), |r| home.join(r.trim_start_matches('/')));
                out.push((loc, "macOS screenshots".into()));
            }
        }
        out.push((desktop.clone(), "macOS screenshots (default)".into()));
        out.push((pictures.join("Screenshots"), "Screenshots".into()));
        out.push((pictures.join("CleanShot"), "CleanShot X".into()));
    } else {
        out.push((pictures.join("Screenshots"), "Windows screenshots (Win+PrtScn, Snipping Tool)".into()));
        // OneDrive can redirect Pictures, sometimes into an organisation-named folder.
        if let Ok(rd) = std::fs::read_dir(&home) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with("OneDrive") {
                    out.push((e.path().join("Pictures").join("Screenshots"), format!("{name} screenshots")));
                }
            }
        }
        if let Ok(v) = p.video_dir() {
            out.push((v.join("Captures"), "Xbox Game Bar captures".into()));
        }
        if let Ok(d) = p.document_dir() {
            out.push((d.join("ShareX").join("Screenshots"), "ShareX".into()));
        }
        out.push((desktop, "Desktop".into()));
    }
    out.push((pictures, "Pictures".into()));
    if let Ok(d) = p.download_dir() {
        out.push((d, "Downloads".into()));
    }
    let mut seen = std::collections::HashSet::new();
    out.into_iter()
        .filter(|(path, _)| path.is_dir() && seen.insert(path.to_string_lossy().to_lowercase()))
        .collect()
}

/// Counts images up to a cap, so a huge Pictures folder does not stall onboarding.
fn count_images(dir: &Path, n: &mut usize) {
    const CAP: usize = 100_000;
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if *n >= CAP {
            return;
        }
        match e.file_type() {
            Ok(t) if t.is_dir() && !e.file_name().to_string_lossy().starts_with('.') => count_images(&e.path(), n),
            Ok(t) if t.is_file() && is_image(&e.path()) => *n += 1,
            _ => {}
        }
    }
}

pub fn suggest(app: &AppHandle) -> Vec<FolderSuggestion> {
    candidates(app)
        .into_iter()
        .filter_map(|(path, label)| {
            let mut count = 0;
            count_images(&path, &mut count);
            (count > 0).then(|| FolderSuggestion { path: path.to_string_lossy().into_owned(), count, label })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name:&str)->PathBuf {
        let dir=std::env::temp_dir().join(format!("glint-settings-{}-{name}",std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("settings.json")
    }

    #[test]
    fn sequential_save_replaces_existing_file_and_retains_new_fields() {
        let path=fixture("replace");
        let mut settings=Settings { theme:"light".into(),..Default::default() };
        save_to(&path,&settings).unwrap();
        settings.theme="dark".into();
        settings.excluded_folders=vec!["C:\\Pictures\\Private".into()];
        settings.saved_searches=vec![crate::types::SavedSearch{id:"dogs".into(),name:"Dogs".into(),query:"dog folder:Pictures".into(),mode:"visual".into()}];
        settings.save_clipboard=true;
        save_to(&path,&settings).unwrap();
        let loaded:Settings=serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(serde_json::to_value(&loaded).unwrap(),serde_json::to_value(&settings).unwrap());
        assert!(!path.with_extension("json.pending").exists());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn failed_pending_write_keeps_previous_settings_intact() {
        let path=fixture("failed-write");
        let previous=Settings{theme:"light".into(),..Default::default()};
        save_to(&path,&previous).unwrap();
        let bytes=std::fs::read(&path).unwrap();
        std::fs::create_dir(path.with_extension("json.pending")).unwrap();
        assert!(save_to(&path,&Settings{theme:"dark".into(),..Default::default()}).is_err());
        assert_eq!(std::fs::read(&path).unwrap(),bytes);
        let loaded:Settings=serde_json::from_slice(&bytes).unwrap();
        assert_eq!(loaded.theme,"light");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
