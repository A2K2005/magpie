//! Settings file and the first-run folder suggestions.

use crate::engine::is_image;
use crate::types::{FolderSuggestion, Settings};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

fn file(app: &AppHandle) -> PathBuf {
    app.path().app_data_dir().unwrap_or_default().join("settings.json")
}

pub fn load(app: &AppHandle) -> Settings {
    std::fs::read_to_string(file(app)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save(app: &AppHandle, s: &Settings) {
    if let Ok(json) = serde_json::to_string_pretty(s) {
        let _ = std::fs::write(file(app), json);
    }
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
