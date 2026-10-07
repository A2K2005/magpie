//! Search: text and file names (FTS5 trigram), spelling corrections and
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

// Conservative starting thresholds, not validated accuracy guarantees. See docs/search-quality.md.
const VISUAL_MIN: f32 = 0.25;
const SIMILAR_MIN: f32 = 0.72;
/// Image + text blended query: scores land lower, since the text half matches images weakly.
const HYBRID_MIN: f32 = 0.55;
/// Fuzzy matching kicks in when exact matches number fewer than this (about two screens of tiles).
const NEAR_BELOW: usize = 30;
const COLS: &str = "s.id, s.path, s.folder, s.name, s.mtime, s.size, s.width, s.height, s.group_id, s.pinned, s.colors, s.stage";

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
    let folder = std::path::Path::new(&path)
        .parent()
        .map_or_else(|| r.get(2), |p| Ok(p.to_string_lossy().into_owned()))?;
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
        "newest" => "s.mtime DESC, s.id DESC",
        "oldest" => "s.mtime ASC, s.id ASC",
        "largest" => "s.size DESC, s.id ASC",
        "smallest" => "s.size ASC, s.id ASC",
        "name" => "s.name COLLATE NOCASE ASC, s.id ASC",
        _ => return None,
    })
}

fn sort_hits(hits: &mut [SearchHit], sort: &str) {
    hits.sort_by_key(|h| h.shot.id);
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
    for (column, values) in [("user_tags", &p.tags), ("collections", &p.collections)] {
        for value in values {
            w.push(format!(
                "EXISTS (SELECT 1 FROM json_each(s.{column}) WHERE lower(value) = ?)"
            ));
            params.push(Value::Text(value.clone()));
        }
    }
    let query = req.q.trim().to_lowercase();
    if !query.is_empty() {
        w.push(
            "NOT EXISTS (SELECT 1 FROM query_feedback qf WHERE qf.shot_id = s.id AND qf.query = ?)"
                .into(),
        );
        params.push(Value::Text(query));
    }
    if p.pinned {
        w.push("s.pinned = 1".into());
    }
    if !p.exts.is_empty() {
        w.push(format!(
            "({})",
            vec!["s.name LIKE ? ESCAPE '\\'"; p.exts.len()].join(" OR ")
        ));
        // "%.png": the name ends with the extension.
        params.extend(p.exts.iter().map(|e| {
            let l = like(&format!(".{e}"));
            Value::Text(l[..l.len() - 1].to_string())
        }));
    }
    for (col, r) in [
        ("s.size", p.size),
        ("s.width", p.width),
        ("s.height", p.height),
    ] {
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
        w.push("NOT (s.text LIKE ? ESCAPE '\\' OR s.name LIKE ? ESCAPE '\\' OR s.note LIKE ? ESCAPE '\\' OR s.user_tags LIKE ? ESCAPE '\\')".into());
        params.extend((0..4).map(|_| Value::Text(like(x))));
    }
    if let Some(g) = req.expand_group {
        w.push("(s.group_id = ? OR s.id = ?)".into());
        params.push(Value::Integer(g));
        params.push(Value::Integer(g));
    }
    Filter {
        sql: w.join(" AND "),
        params,
    }
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
        let mut st =
            conn.prepare("SELECT text, name, note, user_tags FROM shots WHERE hidden = 0")?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let (text, name, note, tags): (String, String, String, String) =
                (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
            let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
            for w in words(&text)
                .chain(words(stem))
                .chain(words(&note))
                .chain(words(&tags))
            {
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
        self.by_fold.get(&fold(term)).map_or(vec![], |v| {
            v.iter().filter(|w| *w != term).cloned().collect()
        })
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
        regex::Regex::new(&format!(r"(?i)\b{}\b", regex::escape(from)))
            .map_or(q.clone(), |re| re.replace(&q, to.as_str()).into_owned())
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
        let mut st = conn.prepare(
            "SELECT id, embedding FROM shots WHERE embedding IS NOT NULL AND hidden = 0 AND embedding_version = ?",
        )?;
        let (mut ids, mut data, mut scales) = (Vec::new(), Vec::new(), Vec::new());
        let mut rows = st.query([super::clip::EMBEDDING_VERSION])?;
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
    pub fn nearest(
        &self,
        q: &[f32],
        min: f32,
        k: usize,
        eligible: &HashSet<i64>,
    ) -> Vec<(i64, f32)> {
        if q.len() != super::clip::DIM || q.iter().any(|x| !x.is_finite()) {
            return vec![];
        }
        let mut out: Vec<(i64, f32)> = self
            .data
            .chunks_exact(super::clip::DIM)
            .zip(self.ids.iter().zip(&self.scales))
            .filter(|(_, (id, _))| eligible.contains(id))
            .map(|(v, (&id, &scale))| {
                (
                    id,
                    scale * v.iter().zip(q).map(|(&a, b)| f32::from(a) * b).sum::<f32>(),
                )
            })
            .filter(|(_, s)| *s >= min)
            .collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
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
                    for evidence in h.evidence {
                        if !out[i].evidence.contains(&evidence) {
                            out[i].evidence.push(evidence);
                        }
                    }
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
    let mut rows: HashMap<i64, Shot> = HashMap::new();
    // Stay below SQLite's parameter limit even for large libraries.
    for chunk in scored.chunks(400) {
        let marks = vec!["?"; chunk.len()].join(",");
        let sql = format!(
            "SELECT {COLS} FROM shots s WHERE {} AND s.id IN ({marks})",
            f.sql
        );
        let mut params = f.params.clone();
        params.extend(chunk.iter().map(|(id, _)| Value::Integer(*id)));
        let mut st = conn.prepare(&sql)?;
        for row in st.query_map(params_from_iter(params), |r| row_to_shot(r, 1))? {
            let shot = row?;
            rows.insert(shot.id, shot);
        }
    }
    Ok(scored
        .iter()
        .filter_map(|(id, score)| {
            rows.get(id).map(|s| SearchHit {
                shot: s.clone(),
                kind,
                evidence: vec![kind],
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
        let groups: Vec<String> = long
            .iter()
            .map(|g| {
                format!(
                    "({})",
                    g.iter()
                        .map(|t| fts_phrase(t))
                        .collect::<Vec<_>>()
                        .join(" OR ")
                )
            })
            .collect();
        params.push(Value::Text(format!(
            "{{name text note user_tags}} : ({})",
            groups.join(" AND ")
        )));
        rank = "bm25(fts, 4.0, 1.0, 0.0, 1.0, 2.0)";
    }
    params.extend(f.params.iter().cloned());
    for t in short {
        wheres.push("(s.text LIKE ? ESCAPE '\\' OR s.name LIKE ? ESCAPE '\\' OR s.note LIKE ? ESCAPE '\\' OR s.user_tags LIKE ? ESCAPE '\\')".into());
        params.extend((0..4).map(|_| Value::Text(like(t))));
    }
    let sql = format!(
        "SELECT {COLS}, s.text || char(10) || s.note || char(10) || s.user_tags, {rank} AS r FROM {from} WHERE {} ORDER BY {}",
        wheres.join(" AND "),
        order.unwrap_or("r, s.id ASC")
    );
    // Every spelling, for highlighting; a word counts as whole if any of its spellings is.
    let wanted = long.len() + short.len();
    let mut st = conn.prepare(&sql)?;
    // (shot, typos, whole-word matches, bm25)
    let rows: Vec<(Shot, usize, usize, f64)> = st
        .query_map(params_from_iter(params), |r| {
            Ok((
                row_to_shot(r, 1)?,
                r.get::<_, String>(12)?,
                r.get::<_, f64>(13)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter(|(s, _, _)| !skip.contains(&s.id))
        .map(|(s, text, r)| {
            let hay = format!("{text}\n{}", s.name).to_lowercase();
            let whole = |t: &String| super::text::has_word(&hay, t);
            let words = long.iter().filter(|g| g.iter().any(whole)).count()
                + short.iter().filter(|t| whole(t)).count();
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
    let bucket = |r: f64| {
        if worst < 0.0 {
            (r / worst * 4.0).floor() as i64
        } else {
            0
        }
    };
    let mut ranked: Vec<(
        Shot,
        f64,
        (
            usize,
            std::cmp::Reverse<usize>,
            std::cmp::Reverse<i64>,
            std::cmp::Reverse<i64>,
        ),
    )> = rows
        .into_iter()
        .map(|(s, typos, words, r)| {
            let key = (
                typos,
                std::cmp::Reverse(words),
                std::cmp::Reverse(bucket(r)),
                std::cmp::Reverse(s.mtime),
            );
            let score = if worst < 0.0 { r / worst } else { 1.0 };
            (s, score, key)
        })
        .collect();
    // An explicit sort keeps the SQL order.
    if order.is_none() {
        ranked.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.0.id.cmp(&b.0.id)));
    }
    // Exact sorts ahead of partial ("cred" as a word before "cred" inside "Credila") even under an
    // explicit sort, since the two show as separate sections.
    ranked.sort_by_key(|r| usize::from((r.2).1.0 < wanted));

    Ok(ranked
        .into_iter()
        .map(|(shot, score, key)| {
            let kind = if near {
                "near"
            } else if key.1.0 >= wanted {
                "text"
            } else {
                "partial"
            };
            SearchHit {
                shot,
                kind,
                evidence: vec![kind],
                score,
                highlights: vec![],
                snippet: None,
            }
        })
        .collect())
}

fn eligible_ids(conn: &Connection, f: &Filter) -> rusqlite::Result<HashSet<i64>> {
    let mut st = conn.prepare(&format!("SELECT s.id FROM shots s WHERE {}", f.sql))?;
    st.query_map(params_from_iter(f.params.clone()), |r| r.get(0))?
        .collect()
}

/// Rank positions share a scale; raw BM25 and cosine do not.
fn fuse(text: Vec<SearchHit>, visual: Vec<SearchHit>) -> Vec<SearchHit> {
    let mut by_id: HashMap<i64, SearchHit> = HashMap::new();
    for list in [text, visual] {
        for (rank, mut hit) in list.into_iter().enumerate() {
            let score = 1.0 / (60.0 + rank as f64 + 1.0);
            if let Some(existing) = by_id.get_mut(&hit.shot.id) {
                existing.score += score;
                for evidence in hit.evidence {
                    if !existing.evidence.contains(&evidence) {
                        existing.evidence.push(evidence);
                    }
                }
            } else {
                hit.score = score;
                by_id.insert(hit.shot.id, hit);
            }
        }
    }
    let mut hits: Vec<_> = by_id.into_values().collect();
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.shot.id.cmp(&b.shot.id))
    });
    hits
}

pub fn run(
    conn: &Connection,
    req: &SearchRequest,
    image: Option<Img>,
    sem: &impl Semantic,
) -> rusqlite::Result<SearchResponse> {
    let t0 = Instant::now();
    let p = parse_query(&req.q, Local::now());
    let f = filter_sql(&p, req);
    let limit = req.limit.unwrap_or(200).clamp(1, 500);
    let offset = req.offset.unwrap_or(0);
    let mode = match req.mode.as_deref() {
        Some("text") => "text",
        Some("visual") => "visual",
        _ => "all",
    };
    let fold_on = req.expand_group.is_none();
    let sort = p
        .sort
        .or_else(|| req.sort.as_deref().and_then(super::query::sort_key));
    let order = order_sql(sort);
    let state = sem.state();
    let done = |hits: Vec<SearchHit>,
                text_count: usize,
                filters: Vec<ActiveFilter>|
     -> rusqlite::Result<SearchResponse> {
        let has_more = hits.len().saturating_sub(offset) > limit;
        let mut hits: Vec<_> = hits.into_iter().skip(offset).take(limit).collect();
        let mut highlight_terms = p.terms.clone();
        if hits.iter().any(|hit| hit.evidence.contains(&"near")) {
            sem.with_vocab(conn, |v| {
                for term in &p.terms {
                    highlight_terms.extend(v.misreads(term));
                    highlight_terms.extend(v.correct(term));
                }
            });
        }
        // Only parse OCR geometry for the returned page.
        let mut lines_of = conn.prepare("SELECT lines FROM shots WHERE id = ?")?;
        for hit in &mut hits {
            if hit
                .evidence
                .iter()
                .any(|e| matches!(*e, "text" | "partial" | "near"))
            {
                let lines: String = lines_of.query_row([hit.shot.id], |r| r.get(0))?;
                (hit.highlights, hit.snippet) = highlight(&lines, &highlight_terms);
            }
        }
        Ok(SearchResponse {
            hits,
            text_count,
            has_more,
            next_offset: has_more.then(|| offset.saturating_add(limit)),
            query: req.q.clone(),
            mode: mode.into(),
            took_ms: t0.elapsed().as_millis() as u64,
            filters,
            semantic: sem.state(),
            did_you_mean: None,
            corrected: false,
        })
    };

    let image = match (req.similar_to, req.image_path.as_deref(), image) {
        (Some(id), _, _) => Some(Err(id)),
        (None, Some(path), _) => Some(Ok(Img::Path(path))),
        (None, None, Some(img)) => Some(Ok(img)),
        _ => None,
    };
    if mode == "text" && image.is_some() {
        return Err(rusqlite::Error::InvalidParameterName(
            "Image similarity requires All or Visual mode".into(),
        ));
    }
    if let Some(image) = image {
        if state != "ready" {
            return done(vec![], 0, p.filters);
        }
        let q = match image {
            Err(id) => conn
                .query_row(
                    "SELECT embedding FROM shots WHERE id = ? AND embedding_version = ?",
                    rusqlite::params![id, super::clip::EMBEDDING_VERSION],
                    |r| r.get::<_, Option<Vec<u8>>>(0),
                )
                .ok()
                .flatten()
                .map(|b| super::db::from_blob(&b)),
            Ok(img) => sem.embed_image(img),
        };
        let Some(mut q) = q else {
            return done(vec![], 0, p.filters);
        };
        let mut min = SIMILAR_MIN;
        if !p.terms.is_empty() {
            if let Some(t) = sem.embed_text(&p.terms.join(" ")) {
                q.iter_mut().zip(&t).for_each(|(a, b)| *a += b);
                let n = q.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
                q.iter_mut().for_each(|x| *x /= n);
                min = HYBRID_MIN;
            }
        }
        let mut eligible = eligible_ids(conn, &f)?;
        if let Some(id) = req.similar_to {
            eligible.remove(&id);
        }
        let near = sem
            .with_vectors(conn, |v| v.nearest(&q, min, usize::MAX, &eligible))
            .unwrap_or_default();
        let mut hits = hits_from(conn, &near, &f, "similar")?;
        if let Some(sort) = sort {
            sort_hits(&mut hits, sort);
        }
        return done(fold_groups(hits, fold_on), 0, p.filters);
    }

    // Enumerate all eligible rows before folding so large bursts cannot starve later pages.
    // ponytail: O(matches) ranking per request; add stored cursors if large-library profiling warrants it.
    if p.terms.is_empty() {
        // A stable shuffle survives pagination. Request a new search to switch sort modes.
        let order = if req.shuffle == Some(true) {
            "((s.id * 1103515245 + 12345) & 2147483647), s.id"
        } else {
            order.unwrap_or("s.mtime DESC, s.id DESC")
        };
        let sql = format!(
            "SELECT {COLS} FROM shots s WHERE {} ORDER BY {order}",
            f.sql
        );
        let mut st = conn.prepare(&sql)?;
        let hits = st
            .query_map(params_from_iter(f.params.clone()), |r| {
                Ok(SearchHit {
                    shot: row_to_shot(r, 1)?,
                    kind: "recent",
                    evidence: vec!["recent"],
                    score: 0.0,
                    highlights: vec![],
                    snippet: None,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        return done(fold_groups(hits, fold_on), 0, p.filters);
    }

    let long: Vec<String> = p
        .terms
        .iter()
        .filter(|t| t.chars().count() >= 3)
        .cloned()
        .collect();
    let short: Vec<String> = p
        .terms
        .iter()
        .filter(|t| t.chars().count() < 3)
        .cloned()
        .collect();
    let typed: Vec<Vec<String>> = long.iter().map(|t| vec![t.clone()]).collect();
    let text_hits = if mode == "visual" {
        vec![]
    } else {
        text_query(conn, &typed, &short, false, &f, &HashSet::new(), order)?
    };
    let seen: HashSet<i64> = text_hits.iter().map(|h| h.shot.id).collect();
    let mut near_hits = vec![];
    let mut fixes: Vec<(String, String)> = vec![];
    if mode != "visual"
        && text_hits.len() < NEAR_BELOW
        && long.iter().any(|t| t.chars().count() >= 4)
    {
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
            near_hits = text_query(conn, &spellings, &short, true, &f, &seen, order)?;
        }
    }
    let did_you_mean =
        (!fixes.is_empty() && !near_hits.is_empty()).then(|| corrected_query(&req.q, &fixes));
    let corrected = did_you_mean.is_some() && text_hits.is_empty();
    let mut visual = vec![];
    if mode != "text" && state == "ready" {
        let eligible = eligible_ids(conn, &f)?;
        let raw = p.terms.join(" ");
        // CLIP's reference zero-shot labels use a photo template. Keep raw-query recall too.
        let photo = (p.terms.len() == 1 && raw.chars().all(char::is_alphabetic))
            .then(|| format!("a photo of a {raw}."));
        let mut scores: HashMap<i64, f32> = HashMap::new();
        for prompt in std::iter::once(raw).chain(photo) {
            if let Some(q) = sem.embed_text(&prompt) {
                let near = sem
                    .with_vectors(conn, |v| v.nearest(&q, VISUAL_MIN, usize::MAX, &eligible))
                    .unwrap_or_default();
                for (id, score) in near {
                    scores
                        .entry(id)
                        .and_modify(|s| *s = s.max(score))
                        .or_insert(score);
                }
            }
        }
        let mut scored: Vec<_> = scores.into_iter().collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        visual = hits_from(conn, &scored, &f, "visual")?;
    }
    let text_count = text_hits.len() + near_hits.len();
    let found: Vec<SearchHit> = text_hits.into_iter().chain(near_hits).collect();
    let mut hits = if mode == "all" {
        fuse(found, visual)
    } else if mode == "visual" {
        visual
    } else {
        found
    };
    if let Some(sort) = sort.filter(|_| order.is_some()) {
        sort_hits(&mut hits, sort);
        if mode == "text" {
            hits.sort_by_key(|h| match h.kind {
                "text" => 0,
                "partial" => 1,
                _ => 2,
            });
        }
    }
    let mut response = done(fold_groups(hits, fold_on), text_count, p.filters)?;
    response.did_you_mean = did_you_mean;
    response.corrected = corrected;
    Ok(response)
}

pub fn get_shot(conn: &Connection, id: i64) -> rusqlite::Result<Option<ShotDetail>> {
    let row = conn.query_row(
        &format!("SELECT {COLS}, s.text, s.lines, s.note, s.user_tags, s.collections, s.source_url FROM shots s WHERE s.id = ?"),
        [id],
        |r| Ok((row_to_shot(r, 1)?, r.get::<_, String>(12)?, r.get::<_, String>(13)?, ShotMetadata {
            note: r.get(14)?, tags: serde_json::from_str(&r.get::<_, String>(15)?).unwrap_or_default(),
            collections: serde_json::from_str(&r.get::<_, String>(16)?).unwrap_or_default(), source_url: r.get(17)?,
        })),
    );
    let (mut shot, text, lines, metadata) = match row {
        Ok(v) => v,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(e) => return Err(e),
    };
    if let Some(g) = shot.group_id {
        shot.group_size = conn.query_row(
            "SELECT count(*) FROM shots WHERE group_id = ? AND hidden = 0",
            [g],
            |r| r.get(0),
        )?;
    }
    let actions = detect_actions(&text);
    Ok(Some(ShotDetail {
        shot,
        metadata,
        lines: serde_json::from_str(&lines).unwrap_or_default(),
        text,
        actions,
    }))
}

pub fn stats(conn: &Connection) -> rusqlite::Result<Stats> {
    let (total, bytes, with_text): (i64, i64, i64) = conn.query_row(
        "SELECT count(*), coalesce(sum(size), 0), coalesce(sum(text != ''), 0) FROM shots WHERE hidden = 0",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let folders = conn
        .prepare(
            "SELECT folder, count(*) FROM shots WHERE hidden = 0 GROUP BY folder ORDER BY 2 DESC",
        )?
        .query_map([], |r| {
            Ok(FolderCount {
                path: r.get(0)?,
                count: r.get(1)?,
            })
        })?
        .filter_map(Result::ok)
        .collect();
    let now = Local::now();
    let mut months = Vec::new();
    for i in (0..12).rev() {
        let total_months = now.year() * 12 + now.month0() as i32 - i;
        let (y, m) = (total_months.div_euclid(12), total_months.rem_euclid(12) + 1);
        months.push(MonthCount {
            month: format!("{y}-{m:02}"),
            count: 0,
        });
    }
    let first = &months[0].month;
    let mut st = conn.prepare(
        "SELECT strftime('%Y-%m', mtime / 1000, 'unixepoch', 'localtime') AS mo, count(*) FROM shots
         WHERE hidden = 0 GROUP BY mo HAVING mo >= ?",
    )?;
    let by: HashMap<String, i64> = st
        .query_map([first], |r| Ok((r.get(0)?, r.get(1)?)))?
        .filter_map(Result::ok)
        .collect();
    for m in &mut months {
        m.count = *by.get(&m.month).unwrap_or(&0);
    }
    // Most common dominant color per shot, by filter name.
    let mut counts: HashMap<&'static str, ColorCount> = HashMap::new();
    let mut st = conn.prepare("SELECT colors FROM shots WHERE hidden = 0 AND colors != '[]'")?;
    for c in st
        .query_map([], |r| r.get::<_, String>(0))?
        .filter_map(Result::ok)
    {
        let Some(first) = serde_json::from_str::<Vec<String>>(&c)
            .ok()
            .and_then(|v| v.into_iter().next())
        else {
            continue;
        };
        let e = counts.entry(color_name(&first)).or_insert(ColorCount {
            hex: first,
            count: 0,
        });
        e.count += 1;
    }
    let mut colors: Vec<ColorCount> = counts.into_values().collect();
    colors.sort_by(|a, b| b.count.cmp(&a.count));
    colors.truncate(8);
    Ok(Stats {
        total,
        bytes,
        folders,
        months,
        colors,
        with_text,
    })
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
            let lines: Vec<OcrLine> = text
                .lines()
                .map(|t| OcrLine {
                    t: t.into(),
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 0.1,
                })
                .collect();
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
        let req = SearchRequest {
            q: q.into(),
            ..Default::default()
        };
        run(conn, &req, None, &NoClip)
            .unwrap()
            .hits
            .into_iter()
            .map(|h| (h.shot.name, h.kind))
            .collect()
    }

    #[test]
    fn exact_typo_and_misread() {
        let conn = db(&[
            "New session started",
            "Your inv0ice #42 is ready",
            "We discussed it internally",
            "c++ NOT \"quotes\"",
        ]);
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
        let conn = db(&[
            "SaveSage optimises redemption",
            "Average strain",
            "Daily average calories",
        ]);
        // A word that exists is never "corrected", so nothing merely similar ("average") shows up.
        assert_eq!(find(&conn, "savesage"), [("s0.png".into(), "text")]);
        // A misspelling matches nothing, so the results are for the correction.
        let req = SearchRequest {
            q: "saceage in:shots".into(),
            ..Default::default()
        };
        let res = run(&conn, &req, None, &NoClip).unwrap();
        assert_eq!(res.did_you_mean.as_deref(), Some("savesage in:shots"));
        assert!(res.corrected);
        assert_eq!(
            res.hits
                .iter()
                .map(|h| h.shot.name.as_str())
                .collect::<Vec<_>>(),
            ["s0.png"]
        );
        // A wrong first letter is not a typo people make; no wild guesses.
        assert!(find(&conn, "xaverage").is_empty());
    }

    #[test]
    fn whole_words_before_partial() {
        // The partial match is newer, so recency alone would put it first.
        let conn = db(&["Switch CRED to AXIS", "Credila loans"]);
        assert_eq!(
            find(&conn, "cred"),
            [("s0.png".into(), "text"), ("s1.png".into(), "partial")]
        );
        let sorted = SearchRequest {
            q: "cred".into(),
            mode: Some("text".into()),
            sort: Some("newest".into()),
            ..Default::default()
        };
        let kinds: Vec<&str> = run(&conn, &sorted, None, &NoClip)
            .unwrap()
            .hits
            .iter()
            .map(|h| h.kind)
            .collect();
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
        assert_eq!(
            find(&conn, "size:>1mb"),
            [("Beach Trip.JPG".into(), "recent")]
        );
        assert_eq!(find(&conn, "path:photos/2024").len(), 1);
        assert_eq!(find(&conn, "path:2024 trip").len(), 1);
        assert!(find(&conn, "ext:gif").is_empty());
        // Sorting: by name, and by size with a search word.
        let names: Vec<String> = find(&conn, "sort:name").into_iter().map(|h| h.0).collect();
        assert_eq!(names, ["Beach Trip.JPG", "s0.png", "s1.png"]);
        assert_eq!(find(&conn, "a sort:largest")[0].0, "Beach Trip.JPG");
        let req = SearchRequest {
            q: String::new(),
            sort: Some("oldest".into()),
            ..Default::default()
        };
        assert_eq!(
            run(&conn, &req, None, &NoClip).unwrap().hits[0].shot.name,
            "Beach Trip.JPG"
        );
        let unread = run(
            &conn,
            &SearchRequest {
                q: "beach".into(),
                ..Default::default()
            },
            None,
            &NoClip,
        )
        .unwrap();
        assert!(!unread.hits[0].shot.indexed);
        assert_eq!(unread.hits[0].shot.folder, "/photos/2024");
    }
    struct FakeClip;
    impl Semantic for FakeClip {
        fn state(&self) -> &'static str {
            "ready"
        }
        fn embed_text(&self, _: &str) -> Option<Vec<f32>> {
            let mut q = vec![0.0; super::super::clip::DIM];
            q[0] = 1.0;
            Some(q)
        }
        fn embed_image(&self, _: Img) -> Option<Vec<f32>> {
            self.embed_text("")
        }
        fn with_vectors<R>(&self, conn: &Connection, f: impl FnOnce(&Vectors) -> R) -> Option<R> {
            Some(f(&Vectors::load(conn).unwrap()))
        }
        fn with_vocab<R>(&self, conn: &Connection, f: impl FnOnce(&Vocab) -> R) -> Option<R> {
            Some(f(&Vocab::load(conn).unwrap()))
        }
    }
    fn vector(conn: &Connection, id: i64, score: f32) {
        let mut v = vec![0.0; super::super::clip::DIM];
        v[0] = score;
        v[1] = (1.0 - score * score).sqrt();
        conn.execute(
            "UPDATE shots SET embedding = ?, embedding_version = ? WHERE id = ?",
            rusqlite::params![
                super::super::db::to_blob(&v),
                super::super::clip::EMBEDDING_VERSION,
                id
            ],
        )
        .unwrap();
    }

    #[test]
    fn visual_filters_before_top_k_and_no_relative_cutoff() {
        let conn = db(&vec!["unrelated"; 360]);
        for id in 1..=359 {
            vector(&conn, id, 0.9);
        }
        vector(&conn, 360, 0.30);
        conn.execute(r#"UPDATE shots SET path = '/pets/dog.png', name = 'dog.png', user_tags = '["pets"]', collections = '["animals"]' WHERE id = 360"#, []).unwrap();
        let req = SearchRequest {
            q: "dog in:pets tag:pets collection:animals".into(),
            mode: Some("visual".into()),
            ..Default::default()
        };
        let hits = run(&conn, &req, None, &FakeClip).unwrap().hits;
        assert_eq!(hits.iter().map(|h| h.shot.id).collect::<Vec<_>>(), [360]);
        let all = run(
            &conn,
            &SearchRequest {
                q: "dog".into(),
                mode: Some("visual".into()),
                offset: Some(359),
                ..Default::default()
            },
            None,
            &FakeClip,
        )
        .unwrap();
        assert_eq!(all.hits[0].shot.id, 360); // 0.30 remains eligible despite a 0.90 global best.
        let eligibility = HashSet::from([360]);
        assert_eq!(
            Vectors::load(&conn).unwrap().nearest(
                &FakeClip.embed_text("").unwrap(),
                VISUAL_MIN,
                1,
                &eligibility
            )[0]
            .0,
            360
        );
    }

    #[test]
    fn modes_dual_evidence_and_visual_not_buried() {
        let conn = db(&vec!["dog"; 350]);
        conn.execute("UPDATE shots SET text = 'a photograph' WHERE id = 350", [])
            .unwrap();
        vector(&conn, 1, 0.80);
        vector(&conn, 350, 0.90);
        let request = |mode: &str| SearchRequest {
            q: "dog".into(),
            mode: Some(mode.into()),
            limit: Some(10),
            ..Default::default()
        };
        let text = run(&conn, &request("text"), None, &FakeClip).unwrap();
        assert!(
            text.hits
                .iter()
                .all(|h| h.kind == "text" && h.evidence == ["text"])
        );
        let visual = run(&conn, &request("visual"), None, &FakeClip).unwrap();
        assert_eq!(
            visual.hits.iter().map(|h| h.shot.id).collect::<Vec<_>>(),
            [350, 1]
        );
        assert!(visual.hits.iter().all(|h| h.evidence == ["visual"]));
        let all = run(&conn, &request("all"), None, &FakeClip).unwrap();
        assert!(all.hits.iter().take(3).any(|h| h.shot.id == 350));
        assert_eq!(
            all.hits.iter().find(|h| h.shot.id == 1).unwrap().evidence,
            ["text", "visual"]
        );
        assert_eq!(all.text_count, 349);
    }

    #[test]
    fn pagination_folds_complete_groups_and_supports_beyond_old_caps() {
        let conn = db(&vec!["invoice"; 1005]);
        conn.execute("UPDATE shots SET group_id = 7 WHERE id <= 801", [])
            .unwrap();
        let mut request = SearchRequest {
            q: "invoice".into(),
            mode: Some("text".into()),
            sort: Some("oldest".into()),
            limit: Some(2),
            ..Default::default()
        };
        let first = run(&conn, &request, None, &NoClip).unwrap();
        assert_eq!(first.hits[0].shot.group_size, 801);
        assert_eq!(first.hits[1].shot.id, 802);
        assert!(first.has_more);
        assert_eq!(first.next_offset, Some(2));
        request.offset = first.next_offset;
        assert_eq!(
            run(&conn, &request, None, &NoClip).unwrap().hits[0].shot.id,
            803
        );
        request.offset = Some(204);
        let last = run(&conn, &request, None, &NoClip).unwrap();
        assert_eq!(last.hits.len(), 1);
        assert_eq!(last.hits[0].shot.id, 1005);
        assert!(!last.has_more);
        assert_eq!(last.next_offset, None);
        request.q.clear();
        request.offset = Some(0);
        assert_eq!(
            run(&conn, &request, None, &NoClip).unwrap().hits[0]
                .shot
                .group_size,
            801
        );
        request.offset = Some(10000);
        assert!(run(&conn, &request, None, &NoClip).unwrap().hits.is_empty());
    }

    #[test]
    fn metadata_feedback_no_matches_and_ties() {
        let conn = db(&["unrelated", "unrelated"]);
        conn.execute(r#"UPDATE shots SET note = 'A dog at the beach', user_tags = '["pets"]', collections = '["Summer trips"]' WHERE id = 1"#, []).unwrap();
        let req = SearchRequest {
            q: r#"dog tag:Pets collection:"Summer trips""#.into(),
            mode: Some("text".into()),
            ..Default::default()
        };
        assert_eq!(
            run(&conn, &req, None, &FakeClip).unwrap().hits[0].shot.id,
            1
        );
        let detail = get_shot(&conn, 1).unwrap().unwrap();
        assert_eq!(detail.metadata.tags, ["pets"]);
        conn.execute(
            "INSERT INTO query_feedback(query, shot_id) VALUES (?, 1)",
            [req.q.trim().to_lowercase()],
        )
        .unwrap();
        assert!(run(&conn, &req, None, &FakeClip).unwrap().hits.is_empty());
        let visual = SearchRequest {
            q: "not a match".into(),
            mode: Some("visual".into()),
            ..Default::default()
        };
        assert!(
            run(&conn, &visual, None, &FakeClip)
                .unwrap()
                .hits
                .is_empty()
        );
        vector(&conn, 1, 0.50);
        vector(&conn, 2, 0.50);
        assert_eq!(
            run(&conn, &visual, None, &FakeClip)
                .unwrap()
                .hits
                .iter()
                .map(|h| h.shot.id)
                .collect::<Vec<_>>(),
            [1, 2]
        );
    }
    struct InferenceForbidden;
    impl Semantic for InferenceForbidden {
        fn state(&self) -> &'static str {
            "ready"
        }
        fn embed_text(&self, _: &str) -> Option<Vec<f32>> {
            panic!("Text mode invoked model inference")
        }
        fn embed_image(&self, _: Img) -> Option<Vec<f32>> {
            panic!("Text mode invoked model inference")
        }
        fn with_vectors<R>(&self, _: &Connection, _: impl FnOnce(&Vectors) -> R) -> Option<R> {
            panic!("Text mode queried visual vectors")
        }
        fn with_vocab<R>(&self, conn: &Connection, f: impl FnOnce(&Vocab) -> R) -> Option<R> {
            Some(f(&Vocab::load(conn).unwrap()))
        }
    }
    #[test]
    fn text_and_browse_modes_never_infer_and_reject_image_intent() {
        let conn = db(&["invoice"]);
        for q in ["invoice", "invioce", "", "in:shots"] {
            let req = SearchRequest {
                q: q.into(),
                mode: Some("text".into()),
                ..Default::default()
            };
            assert_eq!(
                run(&conn, &req, None, &InferenceForbidden)
                    .unwrap()
                    .hits
                    .len(),
                1
            );
        }
        let req = SearchRequest {
            similar_to: Some(1),
            mode: Some("text".into()),
            ..Default::default()
        };
        assert!(
            run(&conn, &req, None, &InferenceForbidden)
                .unwrap_err()
                .to_string()
                .contains("requires All or Visual")
        );
        for mode in ["all", "visual"] {
            let req = SearchRequest {
                q: "in:shots".into(),
                mode: Some(mode.into()),
                ..Default::default()
            };
            assert_eq!(
                run(&conn, &req, None, &InferenceForbidden).unwrap().hits[0].kind,
                "recent"
            );
        }
    }
    #[test]
    fn incompatible_embeddings_and_similar_source_are_excluded() {
        let conn = db(&["one", "two", "three"]);
        for id in 1..=3 {
            vector(&conn, id, 0.90);
        }
        conn.execute("UPDATE shots SET embedding_version = NULL WHERE id = 2", [])
            .unwrap();
        conn.execute(
            "UPDATE shots SET embedding_version = 'other-model' WHERE id = 3",
            [],
        )
        .unwrap();
        assert_eq!(Vectors::load(&conn).unwrap().ids, [1]);
        let req = SearchRequest {
            similar_to: Some(3),
            mode: Some("visual".into()),
            ..Default::default()
        };
        assert!(run(&conn, &req, None, &FakeClip).unwrap().hits.is_empty());
    }
    struct PromptClip {
        calls: std::cell::RefCell<Vec<String>>,
    }
    impl Semantic for PromptClip {
        fn state(&self) -> &'static str {
            "ready"
        }
        fn embed_text(&self, q: &str) -> Option<Vec<f32>> {
            self.calls.borrow_mut().push(q.into());
            let mut v = vec![0.0; super::super::clip::DIM];
            v[usize::from(q.starts_with("a photo of a "))] = 1.0;
            Some(v)
        }
        fn embed_image(&self, _: Img) -> Option<Vec<f32>> {
            None
        }
        fn with_vectors<R>(&self, conn: &Connection, f: impl FnOnce(&Vectors) -> R) -> Option<R> {
            Some(f(&Vectors::load(conn).unwrap()))
        }
        fn with_vocab<R>(&self, conn: &Connection, f: impl FnOnce(&Vocab) -> R) -> Option<R> {
            Some(f(&Vocab::load(conn).unwrap()))
        }
    }
    #[test]
    fn single_label_adds_photo_prompt_without_losing_raw_recall() {
        let conn = db(&["unrelated", "unrelated"]);
        vector(&conn, 1, 1.0);
        vector(&conn, 2, 0.0);
        let model = PromptClip {
            calls: Default::default(),
        };
        let mut request = SearchRequest {
            q: "dog".into(),
            mode: Some("visual".into()),
            ..Default::default()
        };
        let hits = run(&conn, &request, None, &model).unwrap().hits;
        assert_eq!(hits.iter().map(|h| h.shot.id).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(*model.calls.borrow(), ["dog", "a photo of a dog."]);
        model.calls.borrow_mut().clear();
        request.q = "a running dog".into();
        assert_eq!(
            run(&conn, &request, None, &model)
                .unwrap()
                .hits
                .iter()
                .map(|h| h.shot.id)
                .collect::<Vec<_>>(),
            [1]
        );
        assert_eq!(*model.calls.borrow(), ["a running dog"]);
        model.calls.borrow_mut().clear();
        request.q = "dog".into();
        request.mode = Some("text".into());
        assert!(run(&conn, &request, None, &model).unwrap().hits.is_empty());
        assert!(model.calls.borrow().is_empty());
    }
}
