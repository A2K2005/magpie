//! Magpie: a tray app with a global hotkey. The search window is built on summon and destroyed when
//! hidden, so at rest Magpie is this process alone: a sleeping indexer thread and a database handle.

pub mod engine;
pub mod platform;
mod settings;
pub mod types;

use engine::{Engine, Event};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::http::{Request, Response, StatusCode};
use tauri::ipc::InvokeBody;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, UriSchemeContext, UriSchemeResponder, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent, Wry};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_plugin_opener::OpenerExt;
use types::*;

/// How long a hidden window is kept before it is destroyed. Re-creating it costs ~0.2 s.
// ponytail: fixed; make it a setting if people want instant re-open over memory.
const KEEP_HIDDEN: Duration = Duration::from_secs(30);
const TRASH_DELAY: Duration = Duration::from_secs(8);

struct AppState {
    engine: Arc<Engine>,
    settings: Mutex<Settings>,
    settings_update: Mutex<()>,
    /// Shots hidden now, moved to the OS trash after TRASH_DELAY unless undone.
    pending_trash: Mutex<Option<(u64, Vec<i64>)>>,
    trash_seq: AtomicU64,
    /// A native dialog steals focus; don't treat that as "user left".
    dialog_open: AtomicBool,
    /// Bumped on every show, so a stale destroy timer does nothing.
    shown_seq: AtomicU64,
    status_item: Mutex<Option<MenuItem<Wry>>>,
    pause_item: Mutex<Option<MenuItem<Wry>>>,
    /// Where copied images are saved when `save_clipboard` is on.
    clip_dir: PathBuf,
    /// Clipboard sequence number after Magpie's own last image copy, so it is not saved back.
    own_clip: AtomicU64,
    clip_watching: AtomicBool,
}

type Res<T> = Result<T, String>;
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// ---------- window ----------

fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("main")
}

fn create_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let t0 = std::time::Instant::now();
    let b = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("Magpie")
        .inner_size(980.0, 660.0)
        .min_inner_size(640.0, 440.0)
        // A normal window: the OS's own title bar and buttons (close, minimise, maximise on Windows;
        // traffic lights on macOS), in the taskbar or Dock, and it stays put when you switch apps.
        .visible(false)
        .resizable(true)
        // Let the page receive drops (image search); dragging out is native (`start_drag`).
        .disable_drag_drop_handler()
        .on_page_load(move |w, p| {
            if p.event() == tauri::webview::PageLoadEvent::Finished {
                let _ = w.show();
                let _ = w.set_focus();
                let _ = w.emit("magpie:shown", ());
                eprintln!("[magpie] window built and shown in {} ms", t0.elapsed().as_millis());
            }
        });
    // WebView2 is Chromium: the same switches that cut Electron's idle memory apply (software rendering,
    // GPU and network service in-process). Tauri's own defaults are kept.
    #[cfg(windows)]
    let b = b.additional_browser_args(
        "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --enable-features=NetworkServiceInProcess2 \
         --disable-gpu --disable-gpu-compositing --in-process-gpu",
    );
    let theme = app.state::<AppState>().settings.lock().unwrap().theme.clone();
    let b = b.theme(window_theme(&theme)).background_color(window_color(theme != "light"));
    let w = b.build()?;
    apply_theme(&w, &theme); // "system" on a light OS: the guess above was dark
    let handle = app.clone();
    w.on_window_event(move |e| match e {
        WindowEvent::Focused(false) => {
            let st = handle.state::<AppState>();
            if st.settings.lock().unwrap().hide_on_blur && !st.dialog_open.load(Relaxed) {
                // Taking a screenshot of Magpie moves focus to the capture tool; stay open for that.
                let app = handle.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(150));
                    let refocused = window(&app).is_some_and(|w| w.is_focused().unwrap_or(false));
                    if !refocused && !platform::capture_tool_in_front() {
                        hide(&app);
                    }
                });
            }
        }
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            hide(&handle);
        }
        _ => {}
    });
    Ok(w)
}

/// Centre on the monitor under the pointer, a little above the middle like Spotlight.
fn place(app: &AppHandle, w: &WebviewWindow) {
    let (Ok(cursor), Ok(monitors), Ok(size)) = (app.cursor_position(), app.available_monitors(), w.outer_size()) else {
        return;
    };
    let m = monitors.iter().find(|m| {
        let (p, s) = (m.position(), m.size());
        cursor.x >= f64::from(p.x)
            && cursor.y >= f64::from(p.y)
            && cursor.x < f64::from(p.x) + f64::from(s.width)
            && cursor.y < f64::from(p.y) + f64::from(s.height)
    });
    if let Some(m) = m {
        let (p, s) = (m.position(), m.size());
        let x = p.x + (s.width as i32 - size.width as i32) / 2;
        let y = p.y + ((s.height as i32 - size.height as i32) as f32 * 0.3) as i32;
        let _ = w.set_position(tauri::PhysicalPosition::new(x, y));
    }
}

fn show(app: &AppHandle) {
    let st = app.state::<AppState>();
    st.shown_seq.fetch_add(1, Relaxed);
    st.engine.warm();
    match window(app) {
        Some(w) => {
            if !w.is_visible().unwrap_or(false) {
                place(app, &w);
            }
            let _ = w.unminimize();
            let _ = w.show();
            let _ = w.set_focus();
            let _ = w.emit("magpie:shown", ());
        }
        None => {
            if let Ok(w) = create_window(app) {
                place(app, &w);
            }
        }
    }
}

fn hide(app: &AppHandle) {
    let Some(w) = window(app) else { return };
    if !w.is_visible().unwrap_or(false) {
        return;
    }
    let _ = w.hide();
    #[cfg(target_os = "macos")]
    let _ = app.hide(); // give focus back to the app the user came from
    // Destroy after a short grace period: a summon right after a dismiss stays instant, and an idle Magpie
    // costs no web view at all.
    let st = app.state::<AppState>();
    let seq = st.shown_seq.load(Relaxed);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio_sleep(KEEP_HIDDEN).await;
        let st = app.state::<AppState>();
        if st.shown_seq.load(Relaxed) == seq {
            if let Some(w) = window(&app) {
                if !w.is_visible().unwrap_or(true) {
                    let _ = w.destroy();
                }
            }
        }
    });
}

async fn tokio_sleep(d: Duration) {
    let (tx, rx) = tauri::async_runtime::channel::<()>(1);
    std::thread::spawn(move || {
        std::thread::sleep(d);
        let _ = tx.blocking_send(());
    });
    let mut rx = rx;
    let _ = rx.recv().await;
}

/// Copies the newest screenshot's image to the clipboard (Raycast-style "paste latest screenshot").
fn copy_latest_shot(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let st = app.state::<AppState>();
        let Some(path) = st.engine.latest_path() else { return };
        let _ = put_image(&app, &path);
    });
}

/// Copies an image file to the clipboard.
fn put_image(app: &AppHandle, path: &str) -> Res<()> {
    let img = engine::image::decode(Path::new(path)).map_err(err)?.to_rgba8();
    let (w, h) = img.dimensions();
    app.clipboard().write_image(&tauri::image::Image::new_owned(img.into_raw(), w, h)).map_err(err)?;
    app.state::<AppState>().own_clip.store(platform::clipboard_seq(), Relaxed);
    Ok(())
}

/// While `save_clipboard` is on, saves each image copied anywhere as a PNG in `clip_dir`, which is
/// indexed like any folder. Checks a sequence number once a second; reads the clipboard only when it
/// changed. The thread ends when the setting is turned off.
fn watch_clipboard(app: &AppHandle) {
    let st = app.state::<AppState>();
    if !st.settings.lock().unwrap().save_clipboard || st.clip_watching.swap(true, Relaxed) {
        return;
    }
    let app = app.clone();
    let _ = std::thread::Builder::new().name("magpie-clipboard".into()).spawn(move || {
        let st = app.state::<AppState>();
        let mut last = platform::clipboard_seq();
        while st.settings.lock().unwrap().save_clipboard {
            std::thread::sleep(Duration::from_secs(1));
            let seq = platform::clipboard_seq();
            if seq == last || seq == st.own_clip.load(Relaxed) {
                last = seq;
                continue;
            }
            last = seq;
            let Ok(img) = app.clipboard().read_image() else { continue };
            let Some(buf) = image::RgbaImage::from_raw(img.width(), img.height(), img.rgba().to_vec()) else { continue };
            let name = chrono::Local::now().format("Clipboard %Y-%m-%d %H.%M.%S.png").to_string();
            let _ = std::fs::create_dir_all(&st.clip_dir);
            let _ = buf.save(st.clip_dir.join(name));
        }
        st.clip_watching.store(false, Relaxed);
    });
}

fn toggle(app: &AppHandle) {
    match window(app) {
        Some(w) if w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false) => hide(app),
        _ => show(app),
    }
}

// ---------- commands ----------

#[tauri::command]
async fn search(state: State<'_, AppState>, req: SearchRequest) -> Res<SearchResponse> {
    let engine = state.engine.clone();
    tauri::async_runtime::spawn_blocking(move || engine.search(&req, None).map_err(err)).await.map_err(err)?
}

/// Pasted image bytes arrive as the raw request body.
#[tauri::command]
async fn search_image(state: State<'_, AppState>, request: tauri::ipc::Request<'_>) -> Res<SearchResponse> {
    let InvokeBody::Raw(bytes) = request.body() else { return Err("expected image bytes".into()) };
    // The rest of the request (query words, filters) comes as URL-encoded JSON in a header.
    let req: SearchRequest = request
        .headers()
        .get("x-magpie-req")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| serde_json::from_str(&percent_decode(v)).ok())
        .unwrap_or_default();
    let bytes = bytes.clone();
    let engine = state.engine.clone();
    tauri::async_runtime::spawn_blocking(move || engine.search(&req, Some(&bytes)).map_err(err)).await.map_err(err)?
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        // encodeURIComponent output is ASCII, so byte slicing is safe.
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[tauri::command]
async fn get_shot(state: State<'_, AppState>, id: i64) -> Res<Option<ShotDetail>> {
    state.engine.get_shot(id).map_err(err)
}

#[tauri::command]
async fn status(state: State<'_, AppState>) -> Res<IndexStatus> {
    Ok(state.engine.status())
}

#[tauri::command]
async fn stats(state: State<'_, AppState>) -> Res<Stats> {
    state.engine.stats().map_err(err)
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

/// The settings theme as a window theme; None follows the OS.
fn window_theme(theme: &str) -> Option<tauri::Theme> {
    match theme {
        "dark" => Some(tauri::Theme::Dark),
        "light" => Some(tauri::Theme::Light),
        _ => None,
    }
}

/// The page background (`--bg` in styles.css), shown at the edges while the window resizes.
fn window_color(dark: bool) -> tauri::window::Color {
    if dark { tauri::window::Color(18, 18, 18, 255) } else { tauri::window::Color(247, 246, 243, 255) }
}

/// Title bar and window color follow the app's theme, not only the OS's.
fn apply_theme(w: &WebviewWindow, theme: &str) {
    let _ = w.set_theme(window_theme(theme));
    let dark = w.theme().map_or(true, |t| t == tauri::Theme::Dark);
    let _ = w.set_background_color(Some(window_color(dark)));
}

#[tauri::command]
async fn set_settings(app: AppHandle, state: State<'_, AppState>, patch: serde_json::Value) -> Res<Settings> {
    let _update = state.settings_update.lock().unwrap();
    let prev = state.settings.lock().unwrap().clone();
    let mut merged = serde_json::to_value(&prev).map_err(err)?;
    if let (Some(m), Some(p)) = (merged.as_object_mut(), patch.as_object()) {
        for (k, v) in p {
            m.insert(k.clone(), v.clone());
        }
    }
    let mut next: Settings = serde_json::from_value(merged).map_err(err)?;
    if !["folders", "everywhere"].contains(&next.scope.as_str()) || !["system", "light", "dark"].contains(&next.theme.as_str()) {
        return Err("Invalid settings value".into());
    }
    if next.saved_searches.len() > 100 || next.saved_searches.iter().any(|s| s.name.len() > 128 || s.query.len() > 8192 || !["all","text","visual"].contains(&s.mode.as_str())) {
        return Err("Saved searches exceed supported limits".into());
    }
    if next.launch_at_login != prev.launch_at_login {
        let al = app.autolaunch();
        if next.launch_at_login { al.enable() } else { al.disable() }.map_err(err)?;
    }
    if next.hotkey != prev.hotkey && !register_hotkey(&app, &next.hotkey, Some(&prev.hotkey)) {
        next.hotkey = prev.hotkey.clone();
    }
    if next.copy_latest_hotkey != prev.copy_latest_hotkey {
        let old = (!prev.copy_latest_hotkey.is_empty()).then_some(prev.copy_latest_hotkey.as_str());
        if next.copy_latest_hotkey.is_empty() {
            if let Some(o) = old {
                let _ = app.global_shortcut().unregister(o);
            }
        } else if !register_hotkey(&app, &next.copy_latest_hotkey, old) {
            next.copy_latest_hotkey = prev.copy_latest_hotkey.clone();
        }
    }
    if let Err(e) = settings::save(&app, &next) {
        if next.hotkey != prev.hotkey { register_hotkey(&app, &prev.hotkey, Some(&next.hotkey)); }
        if next.copy_latest_hotkey != prev.copy_latest_hotkey {
            let gs = app.global_shortcut();
            if !next.copy_latest_hotkey.is_empty() { let _ = gs.unregister(next.copy_latest_hotkey.as_str()); }
            if !prev.copy_latest_hotkey.is_empty() { let _ = gs.register(prev.copy_latest_hotkey.as_str()); }
        }
        if next.launch_at_login != prev.launch_at_login {
            let al = app.autolaunch();
            let restored = if prev.launch_at_login { al.enable() } else { al.disable() };
            if let Err(rollback) = restored { return Err(format!("Settings could not be saved: {e}. Login setting could not be restored: {rollback}")); }
        }
        return Err(format!("Settings could not be saved: {e}"));
    }
    if next.theme != prev.theme {
        if let Some(w) = window(&app) { apply_theme(&w, &next.theme); }
    }
    if next.folders != prev.folders || next.excluded_folders != prev.excluded_folders
        || next.semantic != prev.semantic || next.sharp_text != prev.sharp_text
        || next.scope != prev.scope || next.save_clipboard != prev.save_clipboard {
        configure(&state, &next);
    }
    *state.settings.lock().unwrap() = next.clone();
    watch_clipboard(&app);
    Ok(next)
}

#[tauri::command]
async fn suggest_folders(app: AppHandle) -> Res<Vec<FolderSuggestion>> {
    Ok(settings::suggest(&app))
}

#[tauri::command]
async fn pick_folder(app: AppHandle, state: State<'_, AppState>) -> Res<Option<String>> {
    state.dialog_open.store(true, Relaxed);
    let picked = app.dialog().file().set_title("Choose a folder to search").blocking_pick_folder();
    state.dialog_open.store(false, Relaxed);
    if let Some(w) = window(&app) {
        let _ = w.set_focus();
    }
    Ok(picked.and_then(|p| p.into_path().ok()).map(|p| p.to_string_lossy().into_owned()))
}

#[tauri::command]
async fn copy_text(app: AppHandle, text: String) -> Res<()> {
    app.clipboard().write_text(text).map_err(err)
}

fn path_of(state: &AppState, id: i64) -> Res<String> {
    state.engine.paths(&[id]).pop().flatten().ok_or_else(|| "not found".to_string())
}

#[tauri::command]
async fn copy_image(app: AppHandle, state: State<'_, AppState>, id: i64) -> Res<()> {
    put_image(&app, &path_of(&state, id)?)
}

#[tauri::command]
async fn open_shot(app: AppHandle, state: State<'_, AppState>, id: i64) -> Res<()> {
    let p = path_of(&state, id)?;
    hide(&app);
    app.opener().open_path(p, None::<&str>).map_err(err)
}

#[tauri::command]
async fn reveal(app: AppHandle, state: State<'_, AppState>, id: i64) -> Res<()> {
    app.opener().reveal_item_in_dir(path_of(&state, id)?).map_err(err)
}

#[tauri::command]
async fn open_external(app: AppHandle, url: String) -> Res<()> {
    // Only web and mail links found in screenshots; never file: or custom schemes.
    let url = if url.contains(':') { url } else { format!("https://{url}") };
    let lower = url.to_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:")) {
        return Err("unsupported link".into());
    }
    hide(&app);
    app.opener().open_url(url, None::<&str>).map_err(err)
}

fn commit_trash(state: &AppState) {
    let Some((_, ids)) = state.pending_trash.lock().unwrap().take() else { return };
    let paths = state.engine.paths(&ids);
    let (mut done, mut failed) = (vec![], vec![]);
    for (id, p) in ids.into_iter().zip(paths) {
        match p.map(|p| trash::delete(p)) {
            Some(Ok(())) | None => done.push(id),
            Some(Err(_)) => failed.push(id),
        }
    }
    state.engine.remove(done);
    if !failed.is_empty() {
        state.engine.set_hidden(&failed, false);
    }
}

#[tauri::command]
async fn trash_shots(app: AppHandle, state: State<'_, AppState>, ids: Vec<i64>) -> Res<()> {
    commit_trash(&state);
    state.engine.set_hidden(&ids, true);
    let seq = state.trash_seq.fetch_add(1, Relaxed) + 1;
    *state.pending_trash.lock().unwrap() = Some((seq, ids));
    tauri::async_runtime::spawn(async move {
        tokio_sleep(TRASH_DELAY).await;
        let st = app.state::<AppState>();
        let mine = st.pending_trash.lock().unwrap().as_ref().is_some_and(|(s, _)| *s == seq);
        if mine {
            commit_trash(&st);
        }
    });
    Ok(())
}

#[tauri::command]
async fn undo_trash(state: State<'_, AppState>) -> Res<usize> {
    let Some((_, ids)) = state.pending_trash.lock().unwrap().take() else { return Ok(0) };
    state.engine.set_hidden(&ids, false);
    Ok(ids.len())
}

#[tauri::command]
async fn pin(state: State<'_, AppState>, id: i64, pinned: bool) -> Res<()> {
    state.engine.pin(id, pinned);
    Ok(())
}

#[tauri::command]
async fn reindex(state: State<'_, AppState>) -> Res<()> {
    state.engine.reindex();
    Ok(())
}

#[tauri::command]
async fn pause(app: AppHandle, state: State<'_, AppState>, paused: bool) -> Res<()> {
    state.engine.pause(paused);
    if let Some(item) = state.pause_item.lock().unwrap().as_ref() {
        let _ = item.set_text(if paused { "Resume indexing" } else { "Pause indexing" });
    }
    let _ = app;
    Ok(())
}

#[tauri::command]
fn hide_window(app: AppHandle) {
    hide(&app);
}

#[tauri::command]
async fn start_drag(app: AppHandle, state: State<'_, AppState>, id: i64, path: String) -> Res<()> {
    // Only files Magpie has indexed can be dragged out.
    if path_of(&state, id)? != path {
        return Err("not indexed".into());
    }
    let w = window(&app).ok_or("no window")?;
    let thumb = state.engine.dirs.thumbs.join(format!("{id}.jpg"));
    let w2 = w.clone();
    w.run_on_main_thread(move || {
        let _ = drag::start_drag(
            &w2,
            drag::DragItem::Files(vec![PathBuf::from(path)]),
            drag::Image::File(thumb),
            |_, _| {},
            drag::Options::default(),
        );
    })
    .map_err(err)
}

fn configure(st: &AppState, s: &Settings) {
    let mut folders = s.folders.clone();
    if s.save_clipboard {
        let _ = std::fs::create_dir_all(&st.clip_dir);
        folders.push(st.clip_dir.to_string_lossy().into_owned());
    }
    st.engine.configure(&folders, s.semantic, s.sharp_text, s.scope == "everywhere", &s.excluded_folders);
}

// ---------- assets ----------

#[tauri::command]
fn finish_indexing(state: State<'_, AppState>, enabled: bool) { state.engine.finish_indexing(enabled); }
#[tauri::command]
fn retry_failed(state: State<'_, AppState>) { state.engine.retry_failed(); }
#[tauri::command]
fn repair_models(state: State<'_, AppState>) { state.engine.repair_models(); }
#[tauri::command]
fn failures(state: State<'_, AppState>) -> Res<Vec<IndexFailure>> { state.engine.failures().map_err(err) }
#[tauri::command]
fn update_metadata(state: State<'_, AppState>, id: i64, metadata: ShotMetadata) -> Res<ShotMetadata> {
    state.engine.update_metadata(id, metadata).map_err(err)
}
#[tauri::command]
fn set_relevant(state: State<'_, AppState>, id: i64, query: String, relevant: bool) -> Res<()> {
    state.engine.set_relevant(id, &query, relevant).map_err(err)
}

#[tauri::command]
async fn export_shots(app: AppHandle, state: State<'_, AppState>, ids: Vec<i64>) -> Res<Option<String>> {
    state.dialog_open.store(true, Relaxed);
    let folder = app.dialog().file().set_title("Export originals and metadata to a new folder").blocking_pick_folder();
    state.dialog_open.store(false, Relaxed);
    let Some(folder) = folder.and_then(|f| f.into_path().ok()) else { return Ok(None) };
    let engine = state.engine.clone();
    tauri::async_runtime::spawn_blocking(move || engine.export_shots(&ids, &folder).map(Some).map_err(err)).await.map_err(err)?
}

#[tauri::command]
fn export_diagnostics(app: AppHandle, state: State<'_, AppState>) -> Res<Option<String>> {
    state.dialog_open.store(true, Relaxed);
    let file = app.dialog().file().set_title("Save diagnostics (no images, paths or search text)")
        .set_file_name("magpie-diagnostics.json").blocking_save_file();
    state.dialog_open.store(false, Relaxed);
    let Some(file) = file.and_then(|f| f.into_path().ok()) else { return Ok(None) };
    let s = state.engine.status();
    let report = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"), "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
        "generatedAt": chrono::Utc::now().to_rfc3339(), "state":s.state, "images":s.total,
        "textProcessed":s.ocr_done,"embedded":s.embedded,"failed":s.errors,
        "visualModel":s.model.state,"textModel":s.text_model.state,
        "warningCount":s.warnings.len(),"embeddingVersion":engine::clip::EMBEDDING_VERSION
    });
    std::fs::write(&file, serde_json::to_vec_pretty(&report).map_err(err)?).map_err(err)?;
    Ok(Some(file.to_string_lossy().into_owned()))
}

#[tauri::command]
async fn clear_index(app: AppHandle, state: State<'_, AppState>) -> Res<()> {
    {
        let _update = state.settings_update.lock().unwrap();
        let mut s = state.settings.lock().unwrap().clone();
        s.folders.clear();
        s.scope = "folders".into();
        s.save_clipboard = false;
        settings::save(&app, &s).map_err(err)?;
        configure(&state, &s);
        *state.settings.lock().unwrap() = s;
    }
    let engine = state.engine.clone();
    tauri::async_runtime::spawn_blocking(move || engine.clear_index().map_err(err)).await.map_err(err)?
}

/// At most two thumbnails are made at once: a grid of unread photos must not decode 200 of them
/// in parallel (a 12-megapixel photo decodes to ~36 MB).
static THUMB_SLOTS: (Mutex<usize>, std::sync::Condvar) = (Mutex::new(2), std::sync::Condvar::new());

/// The thumbnail of `id`, made now if the indexer has not got to it yet.
fn thumb_of(st: &AppState, id: i64) -> Option<PathBuf> {
    let thumb = st.engine.dirs.thumbs.join(format!("{id}.jpg"));
    if thumb.exists() {
        return Some(thumb);
    }
    let src = st.engine.paths(&[id]).pop().flatten()?;
    let (lock, cv) = &THUMB_SLOTS;
    let mut free = cv.wait_while(lock.lock().unwrap(), |n| *n == 0).unwrap();
    *free -= 1;
    drop(free);
    let made = thumb.exists() || engine::image::decode(Path::new(&src)).and_then(|img| engine::image::write_thumb(&img, &thumb)).is_ok();
    *lock.lock().unwrap() += 1;
    cv.notify_one();
    made.then_some(thumb)
}

/// `magpie://localhost/thumb/<id>` and `/file/<id>` (http://magpie.localhost/... on Windows).
/// Only thumbnails and indexed files can be read.
fn serve(app: &AppHandle, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let st = app.state::<AppState>();
    let mut parts = req.uri().path().trim_start_matches('/').split('/');
    let (kind, id) = (parts.next(), parts.next().and_then(|s| s.parse::<i64>().ok()));
    let file = match (kind, id) {
        (Some("thumb"), Some(id)) => thumb_of(&st, id),
        (Some("file"), Some(id)) => st.engine.paths(&[id]).pop().flatten().map(PathBuf::from),
        _ => None,
    };
    let Some(bytes) = file.as_ref().and_then(|f| std::fs::read(f).ok()) else {
        return Response::builder().status(StatusCode::NOT_FOUND).body(vec![]).unwrap();
    };
    let ext = file.and_then(|f| f.extension().map(|e| e.to_string_lossy().to_lowercase())).unwrap_or_default();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        _ => "application/octet-stream",
    };
    Response::builder()
        .header("Content-Type", mime)
        .header("Cache-Control", "max-age=31536000, immutable")
        .header("Access-Control-Allow-Origin", "*")
        .body(bytes)
        .unwrap()
}

// ---------- setup ----------

fn register_hotkey(app: &AppHandle, next: &str, prev: Option<&str>) -> bool {
    let gs = app.global_shortcut();
    if let Some(p) = prev {
        let _ = gs.unregister(p);
    }
    if gs.register(next).is_ok() {
        return true;
    }
    if let Some(p) = prev {
        let _ = gs.register(p);
    }
    false
}

fn status_line(s: &IndexStatus) -> String {
    if s.state == "paused" {
        "Indexing paused".into()
    } else if s.ocr_done < s.total {
        format!("Reading {} of {}", s.ocr_done, s.total)
    } else {
        format!("{} images searchable", s.total)
    }
}

fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Magpie", true, None::<&str>)?;
    let line = MenuItem::with_id(app, "status", "Starting…", false, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", "Pause indexing", true, None::<&str>)?;
    let prefs = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Magpie", true, None::<&str>)?;
    let sep = || PredefinedMenuItem::separator(app);
    let menu = Menu::with_items(app, &[&open, &line, &sep()?, &pause, &prefs, &sep()?, &quit])?;
    let st = app.state::<AppState>();
    *st.status_item.lock().unwrap() = Some(line);
    *st.pause_item.lock().unwrap() = Some(pause);
    let icon = if cfg!(target_os = "macos") {
        tauri::image::Image::from_bytes(include_bytes!("../icons/trayTemplate.png"))?
    } else {
        tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?
    };
    TrayIconBuilder::with_id("magpie")
        .icon(icon)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("Magpie")
        .menu(&menu)
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        .on_menu_event(|app, e| match e.id().as_ref() {
            "open" => show(app),
            "settings" => {
                show(app);
                let _ = app.emit("magpie:openSettings", ());
            }
            "pause" => {
                let st = app.state::<AppState>();
                let paused = !st.engine.shared.paused.load(Relaxed);
                st.engine.pause(paused);
                if let Some(item) = st.pause_item.lock().unwrap().as_ref() {
                    let _ = item.set_text(if paused { "Resume indexing" } else { "Pause indexing" });
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                if !cfg!(target_os = "macos") {
                    toggle(tray.app_handle());
                }
            }
        })
        .build(app)?;
    Ok(())
}

pub fn run() {
    let hidden_start = std::env::args().any(|a| a == "--hidden");
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| show(app)))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, e| {
                    if e.state != ShortcutState::Pressed {
                        return;
                    }
                    let copy_latest = app.state::<AppState>().settings.lock().unwrap().copy_latest_hotkey.clone();
                    if copy_latest.parse::<tauri_plugin_global_shortcut::Shortcut>().is_ok_and(|s| &s == shortcut) {
                        copy_latest_shot(app);
                    } else {
                        toggle(app);
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--hidden"])))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .register_asynchronous_uri_scheme_protocol("magpie", |ctx: UriSchemeContext<'_, Wry>, req, responder: UriSchemeResponder| {
            let app = ctx.app_handle().clone();
            std::thread::spawn(move || responder.respond(serve(&app, &req)));
        })
        .invoke_handler(tauri::generate_handler![
            search,
            search_image,
            get_shot,
            status,
            stats,
            get_settings,
            set_settings,
            suggest_folders,
            pick_folder,
            copy_text,
            copy_image,
            open_shot,
            reveal,
            open_external,
            trash_shots,
            undo_trash,
            pin,
            reindex,
            pause,
            hide_window,
            start_drag,
            finish_indexing, retry_failed, repair_models, failures, update_metadata, set_relevant,
            export_shots, export_diagnostics, clear_index
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let app_data = app.path().app_data_dir()?;
            // One-time move of the pre-rename "Glint" data folder and database.
            if !app_data.exists() {
                let _ = std::fs::rename(app_data.with_file_name("app.glint.desktop"), &app_data);
            }
            // Tests point this at a throwaway profile.
            let data = std::env::var_os("MAGPIE_USER_DATA").map(PathBuf::from).unwrap_or(app_data);
            std::fs::create_dir_all(&data)?;
            if !data.join("magpie.db").exists() {
                for ext in ["", "-wal", "-shm"] {
                    let _ = std::fs::rename(data.join(format!("glint.db{ext}")), data.join(format!("magpie.db{ext}")));
                }
            }
            let emitter = handle.clone();
            let engine = Arc::new(Engine::start(
                &data,
                Box::new(move |e| match e {
                    Event::Status(s) => {
                        let st = emitter.state::<AppState>();
                        if let Some(item) = st.status_item.lock().unwrap().as_ref() {
                            let _ = item.set_text(status_line(&s));
                        }
                        let _ = emitter.emit("magpie:status", s);
                    }
                    Event::Indexed => {
                        let _ = emitter.emit("magpie:indexed", ());
                    }
                }),
            )?);
            let s = settings::load(&handle)?;
            let hotkey = s.hotkey.clone();
            let copy_latest = s.copy_latest_hotkey.clone();
            app.manage(AppState {
                engine: engine.clone(),
                settings: Mutex::new(s),
                settings_update: Mutex::new(()),
                pending_trash: Mutex::new(None),
                trash_seq: AtomicU64::new(0),
                dialog_open: AtomicBool::new(false),
                shown_seq: AtomicU64::new(0),
                status_item: Mutex::new(None),
                pause_item: Mutex::new(None),
                clip_dir: app.path().picture_dir().unwrap_or_else(|_| data.clone()).join("Magpie Clipboard"),
                own_clip: AtomicU64::new(0),
                clip_watching: AtomicBool::new(false),
            });
            {
                let st = handle.state::<AppState>();
                configure(&st, &st.settings.lock().unwrap());
            }
            watch_clipboard(&handle);
            setup_tray(&handle)?;
            if !register_hotkey(&handle, &hotkey, None) {
                eprintln!("[magpie] hotkey {hotkey} is taken by another app");
            }
            if !copy_latest.is_empty() && !register_hotkey(&handle, &copy_latest, None) {
                eprintln!("[magpie] hotkey {copy_latest} is taken by another app");
            }
            // Large CLIP backlogs wait for the user to step away and for mains power.
            std::thread::Builder::new().name("magpie-power".into()).spawn(move || {
                let mut last = None;
                loop {
                    let now = (platform::on_battery(), platform::idle_seconds() >= 60);
                    if Some(now) != last {
                        engine.power(now.0, now.1);
                        last = Some(now);
                    }
                    std::thread::sleep(Duration::from_secs(30));
                }
            })?;
            if !hidden_start {
                show(&handle);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Magpie")
        .run(|app, e| match e {
            // Closing the window keeps Magpie in the tray.
            RunEvent::ExitRequested { api, code: None, .. } => api.prevent_exit(),
            RunEvent::Exit => commit_trash(&app.state::<AppState>()),
            _ => {}
        });
}
