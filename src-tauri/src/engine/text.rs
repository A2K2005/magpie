//! Text helpers: OCR look-alike folding, approximate matching, smart actions.

use crate::types::SmartAction;
use regex::Regex;
use std::sync::LazyLock;

/// Folds characters OCR confuses, so "inv0ice" finds "invoice": two words that fold the same are
/// treated as the same word read two ways.
pub fn fold(s: &str) -> String {
    let s = s
        .to_lowercase()
        .replace("rn", "m")
        .replace("vv", "w")
        .replace("cl", "d");
    s.chars()
        .map(strip_accent)
        .map(|c| match c {
            '0' => 'o',
            '1' | 'i' | 'l' | '|' | '!' => 'l',
            '5' | '$' => 's',
            '8' => 'b',
            c => c,
        })
        .collect()
}


/// Typos between two whole words: insertions, deletions, substitutions and swapped neighbours
/// ("teh" is one typo from "the"). Pass both lowercased.
pub fn distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for j in 0..=b.len() {
        d[0][j] = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j - 1] + cost).min(d[i - 1][j] + 1).min(d[i][j - 1] + 1);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

/// Lowercased words of at least 3 letters or digits.
pub fn words(s: &str) -> impl Iterator<Item = String> + '_ {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 3).map(str::to_lowercase)
}

/// Latin letters without accents, so "cafe" finds "café" in near matches.
fn strip_accent(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' => 'a',
        'ç' | 'ć' | 'č' => 'c',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' => 'e',
        'ì' | 'í' | 'î' | 'ï' | 'ī' => 'i',
        'ñ' | 'ń' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' => 'o',
        'ù' | 'ú' | 'û' | 'ü' | 'ū' => 'u',
        'ý' | 'ÿ' => 'y',
        'š' | 'ś' => 's',
        'ž' | 'ź' | 'ż' => 'z',
        'ł' => 'l',
        'đ' => 'd',
        c => c,
    }
}

/// True if `term` occurs in `text` as a whole word (not inside a longer word). Both lowercased.
pub fn has_word(text: &str, term: &str) -> bool {
    text.match_indices(term).any(|(i, m)| {
        let before = text[..i].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        let after = text[i + m.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        before && after
    })
}

static URL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\bhttps?://[^\s<>"'`]+|\b(?:www\.)?[a-z0-9][a-z0-9-]*(?:\.[a-z0-9-]+)*\.(?:com|net|org|io|dev|app|ai|co|in|me|gg|xyz|edu|gov|info|so|sh|tv|ly|uk|us|de)(?:/[^\s<>"'`]*)?"#).unwrap()
});
static EMAIL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[\w.+-]+@[\w-]+(?:\.[\w-]+)+\b").unwrap());
// Separators never include a newline: numbers on different lines are not one phone number.
static PHONE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\+\d{1,3}[ .-]?)?(?:\(\d{2,4}\)[ .-]?)?\d{2,5}(?:[ .-]\d{2,5}){1,3}|\+\d{10,13}\b").unwrap()
});
static DATE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{1,4}[.-]\d{1,2}[.-]\d{1,4}$").unwrap());
static CODE_HINT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(code|otp|one[- ]time|verification|passcode|pin|2fa|security)\b").unwrap());
static CODE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(?:[A-Z]-)?\d{4,8}\b").unwrap());
static COLOR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)#(?:[0-9a-f]{6}|[0-9a-f]{3})\b").unwrap());

/// Things a user can act on: links, emails, phone numbers, one-time codes, hex colors.
pub fn detect_actions(text: &str) -> Vec<SmartAction> {
    let mut out: Vec<SmartAction> = Vec::new();
    let mut add = |kind: &'static str, value: String| {
        let dup = out.iter().any(|a| a.kind == kind && a.value.eq_ignore_ascii_case(&value));
        if !dup && out.len() < 16 {
            out.push(SmartAction { kind, value });
        }
    };
    let emails: Vec<String> = EMAIL_RE.find_iter(text).map(|m| m.as_str().to_string()).collect();
    for e in &emails {
        add("email", e.clone());
    }
    for m in URL_RE.find_iter(text) {
        let url = m.as_str().trim_end_matches(|c| ").,;:!?]".contains(c));
        // The domain part of an email is not a link.
        if emails.iter().any(|e| e.to_lowercase().ends_with(&url.to_lowercase())) {
            continue;
        }
        add("url", url.to_string());
    }
    for line in text.lines() {
        if CODE_HINT_RE.is_match(line) {
            for c in CODE_RE.find_iter(line) {
                add("code", c.as_str().to_string());
            }
        }
    }
    for m in PHONE_RE.find_iter(text) {
        let raw = m.as_str().trim();
        let digits = raw.chars().filter(char::is_ascii_digit).count();
        if (10..=13).contains(&digits) && !DATE_RE.is_match(raw) {
            add("phone", raw.to_string());
        }
    }
    for m in COLOR_RE.find_iter(text) {
        add("color", m.as_str().to_lowercase());
    }
    out
}

/// `h:<kind>` tags stored with a shot so `has:url` and friends are a cheap LIKE.
pub fn action_tags(actions: &[SmartAction]) -> Vec<String> {
    let mut tags: Vec<String> = actions.iter().map(|a| format!("h:{}", a.kind)).collect();
    tags.dedup();
    tags.sort();
    tags.dedup();
    tags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding_and_near_matching() {
        assert_eq!(fold("Inv0ice"), fold("invoice"));
        assert_eq!(fold("rnodern"), "modem");
        assert_eq!(fold("lnvoice"), fold("invoice"));
        // Swapped letters are one typo; whole words, so a word inside another is far away.
        assert_eq!(distance("receipt", "reciept"), 1);
        assert_eq!(distance("saceage", "savesage"), 2);
        assert_eq!(distance("savesage", "average"), 2);
        assert_eq!(distance("email", "internally"), 7);
        assert_eq!(words("SaveSage, 2x!").collect::<Vec<_>>(), ["savesage"]);
        assert_eq!(fold("Café"), "cafe");
        assert!(has_word("cat on the mat", "cat"));
        assert!(!has_word("concatenate", "cat"));
    }

    #[test]
    fn actions() {
        let a = detect_actions(
            "Your OTP code is 482913\nmail me: a.b@corp.io or visit https://magpie.app/docs.\n52\n21-08-2026\nCall +91 98765 43210 #ffcc00",
        );
        let kinds = |k: &str| a.iter().filter(|x| x.kind == k).map(|x| x.value.as_str()).collect::<Vec<_>>();
        assert_eq!(kinds("code"), ["482913"]);
        assert_eq!(kinds("email"), ["a.b@corp.io"]);
        assert_eq!(kinds("url"), ["https://magpie.app/docs"]);
        assert_eq!(kinds("phone"), ["+91 98765 43210"]);
        assert_eq!(kinds("color"), ["#ffcc00"]);
    }
}
