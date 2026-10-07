//! Color names for the `color:` filter.

#[derive(Clone, Debug)]
pub struct Color {
    pub hex: String,
    pub share: f32,
}

/// Maps a `#rrggbb` color to one of the `color:` filter names.
pub fn color_name(hex: &str) -> &'static str {
    let ch = |i: usize| u8::from_str_radix(hex.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0) as f32 / 255.0;
    let (r, g, b) = (ch(1), ch(3), ch(5));
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    let s = if d == 0.0 { 0.0 } else { d / (1.0 - (2.0 * l - 1.0).abs()) };
    if l < 0.12 {
        return "black";
    }
    if l > 0.92 && s < 0.5 {
        return "white";
    }
    if s < 0.15 {
        return "gray";
    }
    let mut h = if max == r {
        ((g - b) / d) % 6.0
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    h = (h * 60.0 + 360.0) % 360.0;
    match h {
        h if !(15.0..345.0).contains(&h) => "red",
        h if h < 45.0 => {
            if l < 0.4 {
                "brown"
            } else {
                "orange"
            }
        }
        h if h < 70.0 => "yellow",
        h if h < 165.0 => "green",
        h if h < 195.0 => "teal",
        h if h < 255.0 => "blue",
        h if h < 290.0 => "purple",
        _ => "pink",
    }
}

/// `c:<name>` tags for colors that take a noticeable part of the image.
pub fn color_tags(colors: &[Color]) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for c in colors {
        let name = color_name(&c.hex);
        let neutral = matches!(name, "black" | "white" | "gray");
        // ponytail: fixed share thresholds; tune if color: filters feel too strict or too loose.
        let tag = format!("c:{name}");
        if c.share >= if neutral { 0.3 } else { 0.06 } && !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    tags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(color_name("#ff0000"), "red");
        assert_eq!(color_name("#0a0a0a"), "black");
        assert_eq!(color_name("#ffffff"), "white");
        assert_eq!(color_name("#808080"), "gray");
        assert_eq!(color_name("#1e6fd9"), "blue");
        assert_eq!(color_name("#2fa84f"), "green");
    }
}
