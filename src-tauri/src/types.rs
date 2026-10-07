//! IPC contract. Mirrors `src/shared/types.ts`; field names are camelCase on the wire.

use serde::{Deserialize, Serialize};

/// One OCR line. Box coordinates are normalized (0–1), origin top-left.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct OcrLine {
    pub t: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Shot {
    pub id: i64,
    pub path: String,
    pub name: String,
    pub folder: String,
    pub mtime: i64,
    pub size: i64,
    pub width: i64,
    pub height: i64,
    pub thumb: String,
    pub src: String,
    pub group_id: Option<i64>,
    pub group_size: i64,
    pub pinned: bool,
    pub colors: Vec<String>,
    /// False until the image has been read; until then it is found by name and path only.
    pub indexed: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct SmartAction {
    pub kind: &'static str,
    pub value: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct ShotDetail {
    #[serde(flatten)]
    pub shot: Shot,
    pub text: String,
    pub lines: Vec<OcrLine>,
    pub actions: Vec<SmartAction>,
}

#[derive(Serialize, Clone, Debug)]
pub struct SearchHit {
    pub shot: Shot,
    /// text | partial | near | visual | similar | recent
    #[serde(rename = "match")]
    pub kind: &'static str,
    pub score: f64,
    pub highlights: Vec<OcrLine>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchRequest {
    pub q: String,
    pub similar_to: Option<i64>,
    pub image_path: Option<String>,
    pub expand_group: Option<i64>,
    pub shuffle: Option<bool>,
    /// relevance | newest | oldest | largest | smallest | name
    pub sort: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ActiveFilter {
    pub key: &'static str,
    pub value: String,
    pub label: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub hits: Vec<SearchHit>,
    pub text_count: usize,
    pub took_ms: u64,
    pub filters: Vec<ActiveFilter>,
    pub semantic: &'static str,
    /// The query with misspelled words corrected, when a correction found images.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub did_you_mean: Option<String>,
    /// The typed words matched nothing, so the hits are for `did_you_mean`.
    pub corrected: bool,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct ModelInfo {
    /// off | downloading | loading | ready | error
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IndexStatus {
    /// idle | scanning | indexing | paused
    pub state: &'static str,
    pub total: i64,
    pub ocr_done: i64,
    pub embedded: i64,
    pub errors: i64,
    pub model: ModelInfo,
    /// Shots re-read by PaddleOCR.
    pub sharp: i64,
    pub text_model: ModelInfo,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub folders: Vec<String>,
    pub hotkey: String,
    /// Copies the newest screenshot's image to the clipboard from anywhere. Empty = off.
    pub copy_latest_hotkey: String,
    pub launch_at_login: bool,
    pub semantic: bool,
    pub theme: String,
    pub hide_on_blur: bool,
    pub onboarded: bool,
    /// folders | everywhere
    pub scope: String,
    /// Windows: re-read text with PaddleOCR in the background.
    pub sharp_text: bool,
    /// Save copied images to Pictures/Magpie Clipboard.
    pub save_clipboard: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            folders: vec![],
            hotkey: "Alt+Shift+S".into(),
            copy_latest_hotkey: "Alt+Shift+V".into(),
            launch_at_login: false,
            semantic: false,
            theme: "system".into(),
            hide_on_blur: false,
            onboarded: false,
            scope: "folders".into(),
            sharp_text: true,
            save_clipboard: false,
        }
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct FolderSuggestion {
    pub path: String,
    pub count: usize,
    pub label: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct FolderCount {
    pub path: String,
    pub count: i64,
}

#[derive(Serialize, Clone, Debug)]
pub struct MonthCount {
    pub month: String,
    pub count: i64,
}

#[derive(Serialize, Clone, Debug)]
pub struct ColorCount {
    pub hex: String,
    pub count: i64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub total: i64,
    pub bytes: i64,
    pub folders: Vec<FolderCount>,
    pub months: Vec<MonthCount>,
    pub colors: Vec<ColorCount>,
    pub with_text: i64,
}
