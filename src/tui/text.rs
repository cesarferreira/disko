//! Width-aware text helpers. Column alignment has to count display cells, not
//! chars, or a single CJK filename knocks the whole table crooked.

use unicode_width::UnicodeWidthStr;

pub fn width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// Trim to `max` display cells, marking the cut with an ellipsis.
pub fn truncate(text: &str, max: usize) -> String {
    if width(text) <= max {
        return text.to_string();
    }
    if max <= 1 {
        return "…".to_string();
    }

    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let cell = UnicodeWidthStr::width(ch.to_string().as_str());
        if used + cell > max - 1 {
            break;
        }
        out.push(ch);
        used += cell;
    }
    out.push('…');
    out
}

/// Pad on the right to `width` display cells.
pub fn pad(text: &str, target: usize) -> String {
    let current = width(text);
    if current >= target {
        return text.to_string();
    }
    format!("{text}{}", " ".repeat(target - current))
}

/// Fit exactly `width` cells: truncate if long, pad if short.
pub fn fit(text: &str, target: usize) -> String {
    pad(&truncate(text, target), target)
}

/// Long paths lose their middle rather than their tail — the last components
/// are the ones that identify the thing.
pub fn shorten_path(path: &str, max: usize) -> String {
    if width(path) <= max || max < 6 {
        return truncate(path, max);
    }
    let chars: Vec<char> = path.chars().collect();
    let keep_end = max * 2 / 3;
    let keep_start = max.saturating_sub(keep_end + 1);
    let start: String = chars.iter().take(keep_start).collect();
    let end: String = chars.iter().skip(chars.len() - keep_end).collect();
    format!("{start}…{end}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_characters_count_double() {
        assert_eq!(width("abc"), 3);
        assert_eq!(width("日本"), 4);
    }

    #[test]
    fn truncation_marks_the_cut() {
        assert_eq!(truncate("Downloads", 20), "Downloads");
        assert_eq!(truncate("Downloads", 5), "Down…");
        assert_eq!(truncate("Downloads", 1), "…");
    }

    #[test]
    fn fitting_produces_exactly_the_requested_width() {
        for target in 1..12 {
            assert_eq!(width(&fit("Applications", target)), target);
            assert_eq!(width(&fit("ab", target)), target);
        }
    }

    #[test]
    fn wide_text_never_overflows_its_column() {
        assert!(width(&fit("日本語のフォルダ", 7)) <= 7);
    }

    #[test]
    fn shortened_paths_keep_their_tail() {
        let shortened = shorten_path("/home/cesar/code/disko/crates/core/src", 20);
        assert!(width(&shortened) <= 20);
        assert!(shortened.ends_with("src"));
        assert!(shortened.contains('…'));
    }
}
