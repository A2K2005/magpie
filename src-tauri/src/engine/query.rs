//! Query parsing: `invoice in:discord date:week -draft "net 30" color:red has:url is:pinned`, plus
//! voidtools Everything's `ext:png;jpg size:>1mb path:2024 width:>1920 height:<800 dm:today sort:largest`.

use crate::types::ActiveFilter;
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone};
use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Default, PartialEq)]
pub struct ParsedQuery {
    /// Lowercased words and phrases that must all appear.
    pub terms: Vec<String>,
    /// Lowercased words and phrases that must not appear.
    pub excludes: Vec<String>,
    /// Prefixes of a folder name somewhere in the path (`in:`).
    pub folders: Vec<String>,
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub colors: Vec<String>,
    pub has: Vec<String>,
    pub pinned: bool,
    pub tags: Vec<String>,
    pub collections: Vec<String>,
    /// Lowercased extensions without the dot.
    pub exts: Vec<String>,
    /// `[min, max)` in bytes.
    pub size: Option<(i64, i64)>,
    /// Substrings of the full path, lowercased, with `/` as the separator.
    pub paths: Vec<String>,
    /// `[min, max)` in pixels.
    pub width: Option<(i64, i64)>,
    pub height: Option<(i64, i64)>,
    /// landscape | portrait
    pub orientation: Option<&'static str>,
    /// newest | oldest | largest | smallest | name | relevance
    pub sort: Option<&'static str>,
    pub filters: Vec<ActiveFilter>,
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
pub const COLOR_NAMES: [&str; 12] = [
    "red", "orange", "yellow", "green", "teal", "blue", "purple", "pink", "brown", "black",
    "white", "gray",
];
const HAS: [&str; 6] = ["url", "email", "phone", "code", "color", "text"];
const KB: i64 = 1024;
const MB: i64 = 1024 * KB;

/// A number with an optional unit (`1.5mb`, `200kb`, `512`), units in powers of 1024 as Everything.
fn num(v: &str, units: bool) -> Option<i64> {
    let v = v.trim();
    let (n, u) = v.split_at(v.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(v.len()));
    let n: f64 = n.trim().parse().ok()?;
    let mult = match (units, u.to_ascii_lowercase().as_str()) {
        (_, "") => 1,
        (true, "b") => 1,
        (true, "k" | "kb") => KB,
        (true, "m" | "mb") => MB,
        (true, "g" | "gb") => 1024 * MB,
        _ => return None,
    };
    (n >= 0.0).then(|| (n * mult as f64).round() as i64)
}

/// `>x`, `>=x`, `<x`, `<=x`, `a..b`, `a-b` or `x` as `[min, max)`. With units, also Everything's
/// size words: empty, tiny, small, medium, large, huge, gigantic.
pub fn range(v: &str, units: bool) -> Option<(i64, i64)> {
    if units {
        let named = match v {
            "empty" => Some((0, 1)),
            "tiny" => Some((0, 10 * KB)),
            "small" => Some((10 * KB, 100 * KB)),
            "medium" => Some((100 * KB, MB)),
            "large" => Some((MB, 16 * MB)),
            "huge" => Some((16 * MB, 128 * MB)),
            "gigantic" => Some((128 * MB, i64::MAX)),
            _ => None,
        };
        if named.is_some() {
            return named;
        }
    }
    if let Some(r) = v.strip_prefix(">=") {
        return Some((num(r, units)?, i64::MAX));
    }
    if let Some(r) = v.strip_prefix("<=") {
        return Some((0, num(r, units)? + 1));
    }
    if let Some(r) = v.strip_prefix('>') {
        return Some((num(r, units)? + 1, i64::MAX));
    }
    if let Some(r) = v.strip_prefix('<') {
        return Some((0, num(r, units)?));
    }
    if let Some((a, b)) = v.split_once("..").or_else(|| v.split_once('-')) {
        return Some((num(a, units)?, num(b, units)? + 1));
    }
    let n = num(v, units)?;
    Some((n, n + 1))
}

pub fn sort_key(v: &str) -> Option<&'static str> {
    Some(match v {
        "newest" | "date" | "recent" => "newest",
        "oldest" => "oldest",
        "largest" | "size" | "biggest" => "largest",
        "smallest" => "smallest",
        "name" => "name",
        "relevance" | "best" => "relevance",
        _ => return None,
    })
}

fn ms(d: NaiveDate) -> i64 {
    Local
        .from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .map(|t| t.timestamp_millis())
        .unwrap_or(0)
}

fn next_month(y: i32, m: u32) -> NaiveDate {
    if m == 12 {
        NaiveDate::from_ymd_opt(y + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(y, m + 1, 1)
    }
    .unwrap()
}

/// Resolves a date word to `[start, end)` in epoch ms (local midnights), plus a label.
pub fn date_range(v: &str, now: DateTime<Local>) -> Option<(i64, i64, String)> {
    let today = now.date_naive();
    let day = |offset: i64| today + Duration::days(offset);
    match v {
        "today" => return Some((ms(today), ms(day(1)), "Today".into())),
        "yesterday" => return Some((ms(day(-1)), ms(today), "Yesterday".into())),
        "week" => return Some((ms(day(-6)), ms(day(1)), "Last 7 days".into())),
        "month" => return Some((ms(day(-29)), ms(day(1)), "Last 30 days".into())),
        "year" => return Some((ms(day(-364)), ms(day(1)), "Last 12 months".into())),
        _ => {}
    }
    if v.len() >= 3 && v.chars().all(|c| c.is_ascii_alphabetic()) {
        if let Some(i) = MONTHS.iter().position(|m| m.to_lowercase().starts_with(v)) {
            let m = i as u32 + 1;
            // Most recent occurrence of that month that is not in the future.
            let y = if m <= today.month() {
                today.year()
            } else {
                today.year() - 1
            };
            let start = NaiveDate::from_ymd_opt(y, m, 1)?;
            return Some((
                ms(start),
                ms(next_month(y, m)),
                format!("{} {y}", MONTHS[i]),
            ));
        }
    }
    static ISO: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(\d{4})(?:-(\d{1,2})(?:-(\d{1,2}))?)?$").unwrap());
    let c = ISO.captures(v)?;
    let y: i32 = c[1].parse().ok()?;
    let Some(mo) = c.get(2) else {
        let start = NaiveDate::from_ymd_opt(y, 1, 1)?;
        return Some((
            ms(start),
            ms(NaiveDate::from_ymd_opt(y + 1, 1, 1)?),
            y.to_string(),
        ));
    };
    let m: u32 = mo.as_str().parse().ok()?;
    let Some(d) = c.get(3) else {
        let start = NaiveDate::from_ymd_opt(y, m, 1)?;
        return Some((
            ms(start),
            ms(next_month(y, m)),
            format!("{} {y}", MONTHS[(m - 1) as usize]),
        ));
    };
    let start = NaiveDate::from_ymd_opt(y, m, d.as_str().parse().ok()?)?;
    Some((
        ms(start),
        ms(start + Duration::days(1)),
        start.format("%Y-%m-%d").to_string(),
    ))
}

static TOKEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(-?)(?:([A-Za-z]+):)?(?:"([^"]*)"?|(\S+))"#).unwrap());

/// Parses a query. Unknown `key:value` tokens (such as URLs) stay as search terms.
pub fn parse_query(q: &str, now: DateTime<Local>) -> ParsedQuery {
    let mut out = ParsedQuery::default();
    for c in TOKEN_RE.captures_iter(q) {
        let neg = &c[1] == "-";
        let key = c.get(2).map(|k| k.as_str().to_lowercase());
        let value = c
            .get(3)
            .or(c.get(4))
            .map(|v| v.as_str().trim())
            .unwrap_or("");
        let lower = value.to_lowercase();
        let Some(key) = key else {
            if !lower.is_empty() {
                if neg {
                    out.excludes.push(lower)
                } else {
                    out.terms.push(lower)
                }
            }
            continue;
        };
        let mut push = |k: &'static str, v: String, label: String| {
            out.filters.push(ActiveFilter {
                key: k,
                value: v,
                label,
            })
        };
        let handled = !neg
            && match key.as_str() {
                k @ ("tag" | "collection") if !lower.is_empty() => {
                    if k == "tag" {
                        out.tags.push(lower.clone());
                    } else {
                        out.collections.push(lower.clone());
                    }
                    push(
                        if k == "tag" { "tag" } else { "collection" },
                        lower.clone(),
                        format!("{}: {value}", if k == "tag" { "Tag" } else { "Collection" }),
                    );
                    true
                }
                "in" if !lower.is_empty() => {
                    out.folders.push(lower.clone());
                    push("in", lower.clone(), format!("In \u{201c}{value}\u{201d}"));
                    true
                }
                k @ ("date" | "dm" | "before" | "after") => match date_range(&lower, now) {
                    Some((start, end, label)) => {
                        match k {
                            "date" | "dm" => {
                                out.from = Some(start);
                                out.to = Some(end);
                                push("date", lower.clone(), label);
                            }
                            "before" => {
                                out.to = Some(start);
                                push("before", lower.clone(), format!("Before {label}"));
                            }
                            _ => {
                                out.from = Some(start);
                                push("after", lower.clone(), format!("Since {label}"));
                            }
                        }
                        true
                    }
                    None => false,
                },
                "color" => {
                    let c = if lower == "grey" {
                        "gray".to_string()
                    } else {
                        lower.clone()
                    };
                    if COLOR_NAMES.contains(&c.as_str()) {
                        out.colors.push(c.clone());
                        push("color", c.clone(), format!("Color: {c}"));
                        true
                    } else {
                        false
                    }
                }
                "has" if HAS.contains(&lower.as_str()) => {
                    out.has.push(lower.clone());
                    push("has", lower.clone(), format!("Has {lower}"));
                    true
                }
                "is" if lower == "pinned" => {
                    out.pinned = true;
                    push("is", "pinned".into(), "Pinned".into());
                    true
                }
                "is" if lower == "landscape" || lower == "portrait" => {
                    out.orientation = Some(if lower == "landscape" {
                        "landscape"
                    } else {
                        "portrait"
                    });
                    push(
                        "is",
                        lower.clone(),
                        if lower == "landscape" {
                            "Landscape".into()
                        } else {
                            "Portrait".into()
                        },
                    );
                    true
                }
                "ext" => {
                    let exts: Vec<String> = lower
                        .split([';', ',', '|'])
                        .map(|e| e.trim().trim_start_matches('.').to_string())
                        .filter(|e| !e.is_empty())
                        .collect();
                    let ok = !exts.is_empty();
                    if ok {
                        push("ext", lower.clone(), format!("Type: {}", exts.join(", ")));
                        out.exts.extend(exts);
                    }
                    ok
                }
                "size" => match range(&lower, true) {
                    Some(r) => {
                        out.size = Some(r);
                        push("size", lower.clone(), format!("Size {lower}"));
                        true
                    }
                    None => false,
                },
                k @ ("width" | "height") => match range(&lower, false) {
                    Some(r) => {
                        if k == "width" {
                            out.width = Some(r)
                        } else {
                            out.height = Some(r)
                        }
                        push(
                            if k == "width" { "width" } else { "height" },
                            lower.clone(),
                            format!(
                                "{} {lower} px",
                                if k == "width" { "Width" } else { "Height" }
                            ),
                        );
                        true
                    }
                    None => false,
                },
                "path" if !lower.is_empty() => {
                    out.paths.push(lower.replace('\\', "/"));
                    push(
                        "path",
                        lower.clone(),
                        format!("Path has \u{201c}{value}\u{201d}"),
                    );
                    true
                }
                "sort" => match sort_key(&lower) {
                    Some(k) => {
                        out.sort = Some(k);
                        push("sort", k.into(), format!("Sorted by {k}"));
                        true
                    }
                    None => false,
                },
                _ => false,
            };
        if !handled {
            let raw = format!("{key}:{value}").to_lowercase();
            if neg {
                out.excludes.push(raw)
            } else {
                out.terms.push(raw)
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 10, 6, 15, 0, 0).unwrap()
    }

    #[test]
    fn parses_filters() {
        let p = parse_query(
            r#"Invoice in:Discord date:week -draft "net 30" color:grey has:url is:pinned http://x.io"#,
            now(),
        );
        assert_eq!(p.terms, ["invoice", "net 30", "http://x.io"]);
        assert_eq!(p.excludes, ["draft"]);
        assert_eq!(p.folders, ["discord"]);
        assert_eq!(p.colors, ["gray"]);
        assert_eq!(p.has, ["url"]);
        assert!(p.pinned);
        let metadata = parse_query(r#"tag:Pets collection:"Summer trips""#, now());
        assert_eq!(metadata.tags, ["pets"]);
        assert_eq!(metadata.collections, ["summer trips"]);
        assert_eq!(
            p.from,
            Some(ms(NaiveDate::from_ymd_opt(2026, 9, 30).unwrap()))
        );
        assert_eq!(parse_query("-in:foo", now()).excludes, ["in:foo"]);
        assert_eq!(
            parse_query("before:aug", now()).to,
            Some(ms(NaiveDate::from_ymd_opt(2026, 8, 1).unwrap()))
        );
    }

    #[test]
    fn parses_everything_filters() {
        let p = parse_query(
            r"cat ext:PNG;.jpg size:>1mb path:Pictures\2024 width:1920 height:<=800 is:portrait sort:size dm:today",
            now(),
        );
        assert_eq!(p.terms, ["cat"]);
        assert_eq!(p.exts, ["png", "jpg"]);
        assert_eq!(p.size, Some((MB + 1, i64::MAX)));
        assert_eq!(p.paths, ["pictures/2024"]);
        assert_eq!(p.width, Some((1920, 1921)));
        assert_eq!(p.height, Some((0, 801)));
        assert_eq!(p.orientation, Some("portrait"));
        assert_eq!(p.sort, Some("largest"));
        assert!(p.from.is_some());
        assert_eq!(range("small", true), Some((10 * KB, 100 * KB)));
        assert_eq!(range("1.5kb..2kb", true), Some((1536, 2049)));
        assert_eq!(range("10q", true), None);
        // Unknown values stay search words.
        assert_eq!(parse_query("size:big", now()).terms, ["size:big"]);
    }

    #[test]
    fn dates() {
        assert_eq!(date_range("aug", now()).unwrap().2, "August 2026");
        assert_eq!(date_range("dec", now()).unwrap().2, "December 2025");
        assert_eq!(
            date_range("2026-08-01", now()).unwrap().0,
            ms(NaiveDate::from_ymd_opt(2026, 8, 1).unwrap())
        );
        assert!(date_range("nope", now()).is_none());
    }
}
