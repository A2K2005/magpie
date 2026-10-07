//! Search: exact text and file names (FTS5 trigram), then, when few match, spelling corrections and
//! OCR misreads from a vocabulary of every word read ("did you mean"), visual (CLIP), similar images,
//! and the empty-query recent view. Bursts fold into one tile. Images not read yet are found by name,
//! path and Everything-style filters.

use super::colors::color_name;
use super::query::{ParsedQuery, parse_query};
use super::text::{detect_actions, distance, fold, words};
use crate::types::*;
use chrono::{Datelike, Local};
use rusqlite::types::Value;
use rusqlite::{Connection, Row, params_from_iter};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

// ponytail: CLIP cut-offs calibrated by eye on screenshots; tune if visual hits feel noisy or sparse.
const VISUAL_MIN: f32 = 0.25;
const VISUAL_SPREAD: f32 = 0.05;
const SIMILAR_MIN: f32 = 0.72;
/// Image + text blended query: scores land lower, since the text half matches images weakly.
const HYBRID_MIN: f32 = 0.55;
const TEXT_LIMIT: usize = 400;
/// Fuzzy matching kicks in when exact matches number fewer than this (about two screens of tiles).
const NEAR_BELOW: usize = 30;
const COLS: &str =
    "s.id, s.path, s.folder, s.name, s.mtime, s.size, s.width, s.height, s.group_id, s.pinned, s.colors, s.stage";

/// Asset URL served by the `magpie` protocol. WebView2 exposes custom schemes as http://<scheme>.localhost.
pub fn asset(kind: &str, id: i64, v: i64) -> String {
    if cfg!(windows) {
        format!("http://magpie.localhost/{kind}/{id}?v={v}")
    } else {
        format!("magpie://localhost/{kind}/{id}?v={v}")
    }
}

pub fn row_to_shot(r: &Row, group_size: i64) -> rusqlite::Result<Shot> {
    let id: i64 = r.get(0)?;
    let mtime: i64 = r.get(4)?;
    let colors: String = r.get(10)?;
    let path: String = r.get(1)?;
    let folder = std::path::Path::new(&path).parent().map_or_else(|| r.get(2), |p| Ok(p.to_string_lossy().into_owned()))?;
    Ok(Shot {
        id,
        folder,
        path,
        name: r.get(3)?,
        mtime,
        size: r.get(5)?,
        width: r.get(6)?,
        height: r.get(7)?,
        thumb: asset("thumb", id, mtime),
        src: asset("file", id, mtime),
        group_id: r.get(8)?,
        group_size,
        pinned: r.get::<_, i64>(9)? != 0,
        colors: serde_json::from_str(&colors).unwrap_or_default(),
        indexed: r.get::<_, i64>(11)? >= 1,
    })
}

/// SQL order for an explicit sort; None for relevance.
fn order_sql(sort: Option<&str>) -> Option<&'static str> {
    Some(match sort? {
        "newest" => "s.mtime DESC",
        "oldest" => "s.mtime ASC",
        "largest" => "s.size DESC",
        "smallest" => "s.size ASC",
        "name" => "s.name COLLATE NOCASE ASC",
        _ => return None,
    })
}

fn sort_hits(hits: &mut [SearchHit], sort: &str) {
    match sort {
        "newest" => hits.sort_by_key(|h| std::cmp::Reverse(h.shot.mtime)),
        "oldest" => hits.sort_by_key(|h| h.shot.mtime),
        "largest" => hits.sort_by_key(|h| std::cmp::Reverse(h.shot.size)),
        "smallest" => hits.sort_by_key(|h| h.shot.size),
        "name" => hits.sort_by_cached_key(|h| h.shot.name.to_lowercase()),
        _ => {}
    }
}

fn like(s: &str) -> String {
    let mut out = String::from("%");
    for c in s.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

fn fts_phrase(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

struct Filter {
    sql: String,
    params: Vec<Value>,
}

/// SQL conditions for everything except the search terms, on alias `s`.
fn filter_sql(p: &ParsedQuery, req: &SearchRequest) -> Filter {
    let mut w = vec!["s.hidden = 0".to_string()];
    let mut params: Vec<Value> = Vec::new();
    // `in:disc` matches a folder whose name starts with "disc", anywhere in the path, never the file name.
    for f in &p.folders {
        w.push("instr(lower(replace(substr(s.path, 1, length(s.path) - length(s.name)), '\\', '/')), ?) > 0".into());
        params.push(Value::Text(format!("/{f}")));
    }
    if let Some(from) = p.from {
        w.push("s.mtime >= ?".into());
        params.push(Value::Integer(from));
    }
    if let Some(to) = p.to {
        w.push("s.mtime < ?".into());
        params.push(Value::Integer(to));
    }
    for c in &p.colors {
        w.push("(' ' || s.tags || ' ') LIKE ?".into());
        params.push(Value::Text(format!("% c:{c} %")));
    }
    for h in &p.has {
        w.push("(' ' || s.tags || ' ') LIKE ?".into());
        params.push(Value::Text(format!("% h:{h} %")));
    }
    if p.pinned {
        w.push("s.pinned = 1".into());
    }
    if !p.exts.is_empty() {
        w.push(format!("({})", vec!["s.name LIKE ? ESCAPE '\\'"; p.exts.len()].join(" OR ")));
        // "%.png": the name ends with the extension.
        params.extend(p.exts.iter().map(|e| {
            let l = like(&format!(".{e}"));
            Value::Text(l[..l.len() - 1].to_string())
        }));
    }
    for (col, r) in [("s.size", p.size), ("s.width", p.width), ("s.height", p.height)] {
        if let Some((lo, hi)) = r {
            w.push(format!("{col} >= ? AND {col} < ?"));
            params.push(Value::Integer(lo));
            params.push(Value::Integer(hi));
        }
    }
    for x in &p.paths {
        w.push("replace(s.path, '\\', '/') LIKE ? ESCAPE '\\'".into());
        params.push(Value::Text(like(x)));
    }
    match p.orientation {
        Some("landscape") => w.push("s.width > s.height".into()),
        Some(_) => w.push("s.height > s.width".into()),
        None => {}
    }
    for x in &p.excludes {
        w.push("NOT (s.text LIKE ? ESCAPE '\\' OR s.name LIKE ? ESCAPE '\\')".into());
        params.push(Value::Text(like(x)));
        params.push(Value::Text(like(x)));
    }
    if let Some(g) = req.expand_group {
        w.push("(s.group_id = ? OR s.id = ?)".into());
        params.push(Value::Integer(g));
        params.push(Value::Integer(g));
    }
    Filter { sql: w.join(" AND "), params }
}

/// Boxes to light up for each term, sized by character offsets within the OCR line.
fn highlight(lines_json: &str, terms: &[String]) -> (Vec<OcrLine>, Option<String>) {
    let lines: Vec<OcrLine> = serde_json::from_str(lines_json).unwrap_or_default();
    let mut boxes = Vec::new();
    let mut snippet = None;
    for l in &lines {
        let hay: Vec<char> = l.t.to_lowercase().chars().collect();
        let len = hay.len().max(1) as f32;
        for n in terms {
            if boxes.len() >= 64 {
                break;
            }
            let nlen = n.chars().count();
            let mut add = |start: usize| {
                boxes.push(OcrLine {
                    t: l.t.chars().skip(start).take(nlen).collect(),
                    x: l.x + l.w * start as f32 / len,
                    y: l.y,
                    w: l.w * nlen as f32 / len,
                    h: l.h,
                });
                snippet.get_or_insert_with(|| l.t.clone());
            };
            let needle: Vec<char> = n.chars().collect();
            let mut i = 0;
            while i + needle.len() <= hay.len() && !needle.is_empty() {
                if hay[i..i + needle.len()] == needle[..] {
                    add(i);
                    i += needle.len();
                } else {
                    i += 1;
                }
            }
        }
    }
    (boxes, snippet)
}

/// Every word Magpie has read (text and file names), with counts: the dictionary for spelling
/// corrections. Built on demand, dropped after changes and when idle.
pub struct Vocab {
    counts: HashMap<String, u32>,
    /// Folded form ("lnvolce") → the words that fold to it ("invoice", "inv0ice").
    by_fold: HashMap<String, Vec<String>>,
}

impl Vocab {
    pub fn load(conn: &Connection) -> rusqlite::Result<Self> {
        let mut counts: HashMap<String, u32> = HashMap::new();
        let mut st = conn.prepare("SELECT text, name FROM shots WHERE hidden = 0")?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let (text, name): (String, String) = (r.get(0)?, r.get(1)?);
            let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
            for w in words(&text).chain(words(stem)) {
                *counts.entry(w).or_default() += 1;
            }
        }
        let mut by_fold: HashMap<String, Vec<String>> = HashMap::new();
        for w in counts.keys() {
            by_fold.entry(fold(w)).or_default().push(w.clone());
        }
        Ok(Self { counts, by_fold })
    }

    /// Other spellings of `term` that are the same word misread by OCR ("inv0ice" for "invoice").
    pub fn misreads(&self, term: &str) -> Vec<String> {
        self.by_fold.get(&fold(term)).map_or(vec![], |v| v.iter().filter(|w| *w != term).cloned().collect())
    }

    /// The word the user most likely meant, as Elasticsearch's term suggester does it: only for a
    /// word found nowhere (not even inside a longer word), within 1 typo at 4 letters or 2 from 5,
    /// with the first letter right (people rarely mistype it; Meilisearch counts it double), closest
    /// first and then the most common.
    pub fn correct(&self, term: &str) -> Option<String> {
        let n = term.chars().count();
        let k = match n {
            0..=3 => return None,
            4 => 1,
            _ => 2,
        };
        if self.counts.keys().any(|w| w.contains(term)) {
            return None;
        }
        let first = term.chars().next();
        self.counts
            .iter()
            .filter(|(w, _)| w.chars().next() == first && w.chars().count().abs_diff(n) <= k)
            .filter_map(|(w, &c)| {
                let d = distance(term, w);
                (d <= k).then_some((d, std::cmp::Reverse(c), w))
            })
            .min()
            .map(|(_, _, w)| w.clone())
    }
}

/// `q` with each misspelled word replaced by its correction, filters and the rest kept as typed.
fn corrected_query(q: &str, fixes: &[(String, String)]) -> String {
    fixes.iter().fold(q.to_string(), |q, (from, to)| {
        regex::Regex::new(&format!(r"(?i)\b{}\b", regex::escape(from))).map_or(q.clone(), |re| re.replace(&q, to.as_str()).into_owned())
    })
}

/// In-memory CLIP vectors, rebuilt lazily after changes and dropped when idle. Stored as int8 with
/// one scale per vector: 512 bytes per image instead of 2 KB (100k images: 51 MB, not 205 MB);
/// cosine scores move by about 0.002.
pub struct Vectors {
    pub ids: Vec<i64>,
    data: Vec<i8>,
    scales: Vec<f32>,
}

impl Vectors {
    pub fn load(conn: &Connection) -> rusqlite::Result<Self> {
        let mut st = conn.prepare("SELECT id, embedding FROM shots WHERE embedding IS NOT NULL AND hidden = 0")?;
        let (mut ids, mut data, mut scales) = (Vec::new(), Vec::new(), Vec::new());
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let blob: Vec<u8> = r.get(1)?;
            if blob.len() == super::clip::DIM * 4 {
                let v = super::db::from_blob(&blob);
                let scale = v.iter().fold(0f32, |m, x| m.max(x.abs())).max(1e-12) / 127.0;
                ids.push(r.get(0)?);
                data.extend(v.iter().map(|x| (x / scale).round() as i8));
                scales.push(scale);
            }
        }
        Ok(Self { ids, data, scales })
    }

    /// (id, cosine) of shots scoring at least `min`, best first.
    pub fn nearest(&self, q: &[f32], min: f32, k: usize) -> Vec<(i64, f32)> {
        let mut out: Vec<(i64, f32)> = self
            .data
            .chunks_exact(super::clip::DIM)
            .zip(self.ids.iter().zip(&self.scales))
            .map(|(v, (&id, &scale))| (id, scale * v.iter().zip(q).map(|(&a, b)| f32::from(a) * b).sum::<f32>()))
            .filter(|(_, s)| *s >= min)
            .collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        out.truncate(k);
        out
    }
}

/// What search needs from the rest of the engine.
pub trait Semantic {
    /// off | downloading | loading | ready | error
    fn state(&self) -> &'static str;
    /// Text embedding, or None if the model is not ready in time.
    fn embed_text(&self, q: &str) -> Option<Vec<f32>>;
    fn embed_image(&self, img: Img) -> Option<Vec<f32>>;
    fn with_vectors<R>(&self, conn: &Connection, f: impl FnOnce(&Vectors) -> R) -> Option<R>;
    fn with_vocab<R>(&self, conn: &Connection, f: impl FnOnce(&Vocab) -> R) -> Option<R>;
}

pub enum Img<'a> {
    Path(&'a str),
    Bytes(&'a [u8]),
}

fn fold_groups(hits: Vec<SearchHit>, enabled: bool) -> Vec<SearchHit> {
    if !enabled {
        return hits;
    }
    let mut lead: HashMap<i64, usize> = HashMap::new();
    let mut out: Vec<SearchHit> = Vec::with_capacity(hits.len());
    for h in hits {
        match h.shot.group_id {
            Some(g) => {
                if let Some(&i) = lead.get(&g) {
                    out[i].shot.group_size += 1;
                } else {
                    lead.insert(g, out.len());
                    out.push(h);
                }
            }
            None => out.push(h),
        }
    }
    out
}

fn hits_from(
    conn: &Connection,
    scored: &[(i64, f32)],
    f: &Filter,
    kind: &'static str,
) -> rusqlite::Result<Vec<SearchHit>> {
    if scored.is_empty() {
        return Ok(vec![]);
    }
    let marks = vec!["?"; scored.len()].join(",");
    let sql = format!("SELECT {COLS} FROM shots s WHERE {} AND s.id IN ({marks})", f.sql);
    let mut params = f.params.clone();
    params.extend(scored.iter().map(|(id, _)| Value::Integer(*id)));
    let mut st = conn.prepare(&sql)?;
    let rows: HashMap<i64, Shot> =
        st.query_map(params_from_iter(params), |r| row_to_shot(r, 1))?.filter_map(Result::ok).map(|s| (s.id, s)).collect();
    Ok(scored
        .iter()
        .filter_map(|(id, score)| {
            rows.get(id).map(|s| SearchHit {
                shot: s.clone(),
                kind,
                score: f64::from(*score),
                highlights: vec![],
                snippet: None,
            })
        })
        .collect())
}

/// Shots containing every word, each word in any of its spellings (`long[i]` lists the spellings of
/// word i: as typed, OCR misreads, a correction). `near` marks the hits as not what was typed.
#[allow(clippy::too_many_arguments)]
fn text_query(
    conn: &Connection,
    long: &[Vec<String>],
    short: &[String],
    near: bool,
    f: &Filter,
    limit: usize,
    skip: &HashSet<i64>,
    order: Option<&str>,
) -> rusqlite::Result<Vec<SearchHit>> {
    if long.is_empty() && short.is_empty() {
        return Ok(vec![]);
    }
    let mut params: Vec<Value> = Vec::new();
    let mut wheres = vec![f.sql.clone()];
    let mut from = "shots s".to_string();
    let mut rank = "0";
    if !long.is_empty() {
        from = "fts JOIN shots s ON s.id = fts.rowid".into();
        wheres.insert(0, "fts MATCH ?".into());
        let groups: Vec<String> =
            long.iter().map(|g| format!("({})", g.iter().map(|t| fts_phrase(t)).collect::<Vec<_>>().join(" OR "))).collect();
        params.push(Value::Text(format!("{{name text}} : ({})", groups.join(" AND "))));
        rank = "bm25(fts, 4.0, 1.0, 0.0)";
    }
    params.extend(f.params.iter().cloned());
    for t in short {
        wheres.push("(s.text LIKE ? ESCAPE '\\' OR s.name LIKE ? ESCAPE '\\')".into());
        params.push(Value::Text(like(t)));
        params.push(Value::Text(like(t)));
    }
    let sql = format!(
        "SELECT {COLS}, s.text, {rank} AS r FROM {from} WHERE {} ORDER BY {} LIMIT {TEXT_LIMIT}",
        wheres.join(" AND "),
        order.unwrap_or("r")
    );
    // Every spelling, for highlighting; a word counts as whole if any of its spellings is.
    let terms: Vec<String> = long.iter().flatten().chain(short).cloned().collect();
    let wanted = long.len() + short.len();
    let mut st = conn.prepare(&sql)?;
    // (shot, typos, whole-word matches, bm25)
    let rows: Vec<(Shot, usize, usize, f64)> = st
        .query_map(params_from_iter(params), |r| Ok((row_to_shot(r, 1)?, r.get::<_, String>(12)?, r.get::<_, f64>(13)?)))?
        .filter_map(Result::ok)
        .filter(|(s, _, _)| !skip.contains(&s.id))
        .map(|(s, text, r)| {
            let hay = format!("{text}\n{}", s.name).to_lowercase();
            let whole = |t: &String| super::text::has_word(&hay, t);
            let words = long.iter().filter(|g| g.iter().any(whole)).count() + short.iter().filter(|t| whole(t)).count();
            (s, usize::from(near), words, r)
        })
        .collect();
    if rows.is_empty() {
        return Ok(vec![]);
    }
    // Ranking rules in order, each only breaking ties of the one before (Meilisearch/Typesense style):
    // fewer typos → more terms as whole words → BM25 in coarse buckets → newer.
    // A clearly better match wins even if older; among similar matches the newest comes first.
    let worst = rows.iter().map(|r| r.3).fold(0.0, f64::min);
    let bucket = |r: f64| if worst < 0.0 { (r / worst * 4.0).floor() as i64 } else { 0 };
    let mut ranked: Vec<(Shot, f64, (usize, std::cmp::Reverse<usize>, std::cmp::Reverse<i64>, std::cmp::Reverse<i64>))> = rows
        .into_iter()
        .map(|(s, typos, words, r)| {
            let key = (typos, std::cmp::Reverse(words), std::cmp::Reverse(bucket(r)), std::cmp::Reverse(s.mtime));
            let score = if worst < 0.0 { r / worst } else { 1.0 };
            (s, score, key)
        })
        .collect();
    // An explicit sort keeps the SQL order.
    if order.is_none() {
        ranked.sort_by(|a, b| a.2.cmp(&b.2));
    }
    ranked.truncate(limit);
    // Exact sorts ahead of partial ("cred" as a word before "cred" inside "Credila") even under an
    // explicit sort, since the two show as separate sections.
    ranked.sort_by_key(|r| usize::from((r.2).1.0 < wanted));

    // Highlights only for hits that are returned: parsing every match's lines costs more than the search.
    let mut lines_of = conn.prepare("SELECT lines FROM shots WHERE id = ?")?;
    ranked
        .into_iter()
        .map(|(shot, score, key)| {
            let lines: String = lines_of.query_row([shot.id], |r| r.get(0))?;
            let (highlights, snippet) = highlight(&lines, &terms);
            let kind = if near {
                "near"
            } else if key.1.0 >= wanted {
                "text"
            } else {
                "partial"
            };
            Ok(SearchHit { shot, kind, score, highlights, snippet })
        })
        .collect()
}

pub fn run(conn: &Connection, req: &SearchRequest, image: Option<Img>, sem: &impl Semantic) -> rusqlite::Result<SearchResponse> {
    let t0 = Instant::now();
    let p = parse_query(&req.q, Local::now());
    let f = filter_sql(&p, req);
    let limit = req.limit.unwrap_or(200).min(500);
    let fold_on = req.expand_group.is_none();
    let sort = p.sort.or_else(|| req.sort.as_deref().and_then(super::query::sort_key));
    let order = order_sql(sort);
    let state = sem.state();
    let done = |hits: Vec<SearchHit>, text_count: usize, filters: Vec<ActiveFilter>| SearchResponse {
        hits: hits.into_iter().take(limit).collect(),
        text_count,
        took_ms: t0.elapsed().as_millis() as u64,
        filters,
        semantic: state,
        did_you_mean: None,
        corrected: false,
    };

    // Image → similar images.
    let image = match (req.similar_to, req.image_path.as_deref(), image) {
        (Some(id), _, _) => Some(Err(id)),
        (None, Some(path), _) => Some(Ok(Img::Path(path))),
        (None, None, Some(img)) => Some(Ok(img)),
        _ => None,
    };
    if let Some(image) = image {
        if state != "ready" {
            return Ok(done(vec![], 0, p.filters));
        }
        let q = match image {
            Err(id) => conn
                .query_row("SELECT embedding FROM shots WHERE id = ?", [id], |r| r.get::<_, Option<Vec<u8>>>(0))
                .ok()
                .flatten()
                .map(|b| super::db::from_blob(&b)),
            Ok(img) => sem.embed_image(img),
        };
        let Some(mut q) = q else { return Ok(done(vec![], 0, p.filters)) };
        // Hybrid (from neurasnip): "at the beach" + a photo finds that photo's look, narrowed by the words.
        let mut min = SIMILAR_MIN;
        if !p.terms.is_empty() {
            if let Some(t) = sem.embed_text(&p.terms.join(" ")) {
                q.iter_mut().zip(&t).for_each(|(a, b)| *a += b);
                let n = q.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
                q.iter_mut().for_each(|x| *x /= n);
                min = HYBRID_MIN;
            }
        }
        let near = sem.with_vectors(conn, |v| v.nearest(&q, min, 300)).unwrap_or_default();
        let near: Vec<(i64, f32)> = near.into_iter().filter(|(id, _)| Some(*id) != req.similar_to).collect();
        let hits = fold_groups(hits_from(conn, &near, &f, "similar")?, fold_on);
        return Ok(done(hits, 0, p.filters));
    }

    // Empty query → recent (or shuffled) shots, one tile per burst.
    if p.terms.is_empty() {
        let order = if req.shuffle == Some(true) { "random()" } else { order.unwrap_or("s.mtime DESC") };
        // Only the first rows in order are read (an index walk, not a sort of the whole library), then
        // bursts fold among them. ponytail: a burst's count covers only members within those rows.
        let fetch = if fold_on { limit * 4 } else { limit };
        let sql = format!("SELECT {COLS} FROM shots s WHERE {} ORDER BY {order} LIMIT {fetch}", f.sql);
        let mut st = conn.prepare(&sql)?;
        let hits = st
            .query_map(params_from_iter(f.params.clone()), |r| {
                Ok(SearchHit { shot: row_to_shot(r, 1)?, kind: "recent", score: 0.0, highlights: vec![], snippet: None })
            })?
            .filter_map(Result::ok)
            .collect();
        return Ok(done(fold_groups(hits, fold_on), 0, p.filters));
    }

    // Text: every term must appear in the OCR text or the file name.
    let long: Vec<String> = p.terms.iter().filter(|t| t.chars().count() >= 3).cloned().collect();
    let short: Vec<String> = p.terms.iter().filter(|t| t.chars().count() < 3).cloned().collect();
    let typed: Vec<Vec<String>> = long.iter().map(|t| vec![t.clone()]).collect();
    let text_hits = text_query(conn, &typed, &short, false, &f, limit, &HashSet::new(), order)?;
    let mut seen: HashSet<i64> = text_hits.iter().map(|h| h.shot.id).collect();

    // Near, only when exact matches are few (so a normal keystroke never pays for it): each word also
    // in its OCR misreads ("inv0ice") and, if it appears nowhere, in its likeliest correction.
    let mut near_hits = vec![];
    let mut fixes: Vec<(String, String)> = vec![];
    if text_hits.len() < NEAR_BELOW && long.iter().any(|t| t.chars().count() >= 4) {
        let spellings: Vec<Vec<String>> = sem
            .with_vocab(conn, |v| {
                long.iter()
                    .map(|t| {
                        let mut g = vec![t.clone()];
                        if t.chars().count() >= 4 {
                            g.extend(v.misreads(t));
                            if let Some(fix) = v.correct(t) {
                                fixes.push((t.clone(), fix.clone()));
                                g.push(fix);
                            }
                        }
                        g
                    })
                    .collect()
            })
            .unwrap_or_default();
        if spellings.iter().any(|g| g.len() > 1) {
            near_hits = text_query(conn, &spellings, &short, true, &f, limit, &seen, order)?;
            seen.extend(near_hits.iter().map(|h| h.shot.id));
        }
    }
    let did_you_mean = (!fixes.is_empty() && !near_hits.is_empty()).then(|| corrected_query(&req.q, &fixes));
    let corrected = did_you_mean.is_some() && text_hits.is_empty();

    // Visual: CLIP thinks the image shows the query.
    let mut visual = vec![];
    if state == "ready" {
        if let Some(q) = sem.embed_text(&p.terms.join(" ")) {
            let near = sem.with_vectors(conn, |v| v.nearest(&q, VISUAL_MIN, 300)).unwrap_or_default();
            let best = near.first().map_or(0.0, |n| n.1);
            let kept: Vec<(i64, f32)> =
                near.into_iter().filter(|(id, s)| *s >= best - VISUAL_SPREAD && !seen.contains(id)).take(80).collect();
            visual = hits_from(conn, &kept, &f, "visual")?;
        }
    }

    let text_count = text_hits.len() + near_hits.len();
    let mut found: Vec<SearchHit> = text_hits.into_iter().chain(near_hits).collect();
    if let Some(sort) = sort.filter(|_| order.is_some()) {
        sort_hits(&mut found, sort);
        // Stable: whole words, then partial words, then corrections stay in that order.
        found.sort_by_key(|h| match h.kind {
            "text" => 0,
            "partial" => 1,
            _ => 2,
        });
    }
    let mut hits = fold_groups(found, fold_on);
    hits.extend(fold_groups(visual, fold_on));
    Ok(SearchResponse { did_you_mean, corrected, ..done(hits, text_count, p.filters) })
}

pub fn get_shot(conn: &Connection, id: i64) -> rusqlite::Result<Option<ShotDetail>> {
    let row = conn.query_row(
        &format!("SELECT {COLS}, s.text, s.lines FROM shots s WHERE s.id = ?"),
        [id],
        |r| Ok((row_to_shot(r, 1)?, r.get::<_, String>(12)?, r.get::<_, String>(13)?)),
    );
    let (mut shot, text, lines) = match row {
        Ok(v) => v,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(e) => return Err(e),
    };
    if let Some(g) = shot.group_id {
        shot.group_size = conn.query_row("SELECT count(*) FROM shots WHERE group_id = ? AND hidden = 0", [g], |r| r.get(0))?;
    }
    let actions = detect_actions(&text);
    Ok(Some(ShotDetail { shot, lines: serde_json::from_str(&lines).unwrap_or_default(), text, actions }))
}

pub fn stats(conn: &Connection) -> rusqlite::Result<Stats> {
    let (total, bytes, with_text): (i64, i64, i64) = conn.query_row(
        "SELECT count(*), coalesce(sum(size), 0), coalesce(sum(text != ''), 0) FROM shots WHERE hidden = 0",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let folders = conn
        .prepare("SELECT folder, count(*) FROM shots WHERE hidden = 0 GROUP BY folder ORDER BY 2 DESC")?
        .query_map([], |r| Ok(FolderCount { path: r.get(0)?, count: r.get(1)? }))?
        .filter_map(Result::ok)
        .collect();
    let now = Local::now();
    let mut months = Vec::new();
    for i in (0..12).rev() {
        let total_months = now.year() * 12 + now.month0() as i32 - i;
        let (y, m) = (total_months.div_euclid(12), total_months.rem_euclid(12) + 1);
        months.push(MonthCount { month: format!("{y}-{m:02}"), count: 0 });
    }
    let first = &months[0].month;
    let mut st = conn.prepare(
        "SELECT strftime('%Y-%m', mtime / 1000, 'unixepoch', 'localtime') AS mo, count(*) FROM shots
         WHERE hidden = 0 GROUP BY mo HAVING mo >= ?",
    )?;
    let by: HashMap<String, i64> =
        st.query_map([first], |r| Ok((r.get(0)?, r.get(1)?)))?.filter_map(Result::ok).collect();
    for m in &mut months {
        m.count = *by.get(&m.month).unwrap_or(&0);
    }
    // Most common dominant color per shot, by filter name.
    let mut counts: HashMap<&'static str, ColorCount> = HashMap::new();
    let mut st = conn.prepare("SELECT colors FROM shots WHERE hidden = 0 AND colors != '[]'")?;
    for c in st.query_map([], |r| r.get::<_, String>(0))?.filter_map(Result::ok) {
        let Some(first) = serde_json::from_str::<Vec<String>>(&c).ok().and_then(|v| v.into_iter().next()) else {
            continue;
        };
        let e = counts.entry(color_name(&first)).or_insert(ColorCount { hex: first, count: 0 });
        e.count += 1;
    }
    let mut colors: Vec<ColorCount> = counts.into_values().collect();
    colors.sort_by(|a, b| b.count.cmp(&a.count));
    colors.truncate(8);
    Ok(Stats { total, bytes, folders, months, colors, with_text })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoClip;
    impl Semantic for NoClip {
        fn state(&self) -> &'static str {
            "off"
        }
        fn embed_text(&self, _: &str) -> Option<Vec<f32>> {
            None
        }
        fn embed_image(&self, _: Img) -> Option<Vec<f32>> {
            None
        }
        fn with_vectors<R>(&self, _: &Connection, _: impl FnOnce(&Vectors) -> R) -> Option<R> {
            None
        }
        fn with_vocab<R>(&self, conn: &Connection, f: impl FnOnce(&Vocab) -> R) -> Option<R> {
            Some(f(&Vocab::load(conn).ok()?))
        }
    }

    fn db(docs: &[&str]) -> Connection {
        let conn = super::super::db::open(std::path::Path::new(":memory:")).unwrap();
        for (i, text) in docs.iter().enumerate() {
            let lines: Vec<OcrLine> =
                text.lines().map(|t| OcrLine { t: t.into(), x: 0.0, y: 0.0, w: 1.0, h: 0.1 }).collect();
            conn.execute(
                "INSERT INTO shots(path, folder, name, mtime, size, stage, text, norm, lines)
                 VALUES (?, '/shots', ?, ?, 1, 1, ?, ?, ?)",
                rusqlite::params![
                    format!("/shots/discord/s{i}.png"),
                    format!("s{i}.png"),
                    1_700_000_000_000i64 + i as i64,
                    text,
                    fold(text),
                    serde_json::to_string(&lines).unwrap()
                ],
            )
            .unwrap();
        }
        conn
    }

    fn find(conn: &Connection, q: &str) -> Vec<(String, &'static str)> {
        let req = SearchRequest { q: q.into(), ..Default::default() };
        run(conn, &req, None, &NoClip).unwrap().hits.into_iter().map(|h| (h.shot.name, h.kind)).collect()
    }

    #[test]
    fn exact_typo_and_misread() {
        let conn = db(&["New session started", "Your inv0ice #42 is ready", "We discussed it internally", "c++ NOT \"quotes\""]);
        assert_eq!(find(&conn, "session"), [("s0.png".into(), "text")]);
        // Typo: a missing letter.
        assert_eq!(find(&conn, "sesion"), [("s0.png".into(), "near")]);
        // OCR misread in the screenshot (0 for o).
        assert_eq!(find(&conn, "invoice"), [("s1.png".into(), "near")]);
        // Folding alone would find "email" inside "internally"; verification rejects it.
        assert!(find(&conn, "email").is_empty());
        // Operators and quotes are literal text, never FTS syntax errors.
        assert_eq!(find(&conn, "c++").len(), 1);
        assert_eq!(find(&conn, "NOT").len(), 1);
        assert_eq!(find(&conn, "\"quotes").len(), 1);
        // Filters: folder prefix and exclusion.
        assert_eq!(find(&conn, "session in:disc").len(), 1);
        assert!(find(&conn, "session in:scord").is_empty());
        assert!(find(&conn, "session -started").is_empty());
    }

    #[test]
    fn did_you_mean() {
        let conn = db(&["SaveSage optimises redemption", "Average strain", "Daily average calories"]);
        // A word that exists is never "corrected", so nothing merely similar ("average") shows up.
        assert_eq!(find(&conn, "savesage"), [("s0.png".into(), "text")]);
        // A misspelling matches nothing, so the results are for the correction.
        let req = SearchRequest { q: "saceage in:shots".into(), ..Default::default() };
        let res = run(&conn, &req, None, &NoClip).unwrap();
        assert_eq!(res.did_you_mean.as_deref(), Some("savesage in:shots"));
        assert!(res.corrected);
        assert_eq!(res.hits.iter().map(|h| h.shot.name.as_str()).collect::<Vec<_>>(), ["s0.png"]);
        // A wrong first letter is not a typo people make; no wild guesses.
        assert!(find(&conn, "xaverage").is_empty());
    }

    #[test]
    fn whole_words_before_partial() {
        // The partial match is newer, so recency alone would put it first.
        let conn = db(&["Switch CRED to AXIS", "Credila loans"]);
        assert_eq!(find(&conn, "cred"), [("s0.png".into(), "text"), ("s1.png".into(), "partial")]);
        let sorted = SearchRequest { q: "cred".into(), sort: Some("newest".into()), ..Default::default() };
        let kinds: Vec<&str> = run(&conn, &sorted, None, &NoClip).unwrap().hits.iter().map(|h| h.kind).collect();
        assert_eq!(kinds, ["text", "partial"]);
    }

    #[test]
    fn unread_images_and_everything_filters() {
        let conn = db(&["New session started", "Invoice total"]);
        // An image not read yet: found by name, path and size, never by text.
        conn.execute(
            "INSERT INTO shots(path, folder, name, mtime, size) VALUES ('/photos/2024/Beach Trip.JPG', '/photos', 'Beach Trip.JPG', 1, 5000000)",
            [],
        )
        .unwrap();
        assert_eq!(find(&conn, "beach"), [("Beach Trip.JPG".into(), "text")]);
        assert_eq!(find(&conn, "ext:jpg").len(), 1);
        assert_eq!(find(&conn, "ext:png;jpg").len(), 3);
        assert_eq!(find(&conn, "size:>1mb"), [("Beach Trip.JPG".into(), "recent")]);
        assert_eq!(find(&conn, "path:photos/2024").len(), 1);
        assert_eq!(find(&conn, "path:2024 trip").len(), 1);
        assert!(find(&conn, "ext:gif").is_empty());
        // Sorting: by name, and by size with a search word.
        let names: Vec<String> = find(&conn, "sort:name").into_iter().map(|h| h.0).collect();
        assert_eq!(names, ["Beach Trip.JPG", "s0.png", "s1.png"]);
        assert_eq!(find(&conn, "a sort:largest")[0].0, "Beach Trip.JPG");
        let req = SearchRequest { q: String::new(), sort: Some("oldest".into()), ..Default::default() };
        assert_eq!(run(&conn, &req, None, &NoClip).unwrap().hits[0].shot.name, "Beach Trip.JPG");
        let unread = run(&conn, &SearchRequest { q: "beach".into(), ..Default::default() }, None, &NoClip).unwrap();
        assert!(!unread.hits[0].shot.indexed);
        assert_eq!(unread.hits[0].shot.folder, "/photos/2024");
    }
}
