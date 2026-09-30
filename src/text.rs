//! Display-width helpers: sizes in terminal columns, not `char`s. CJK and
//! most emoji take two columns and combining marks none, so counting
//! `chars()` misaligns padding, cursors and truncation. Pure; strings are
//! measured grapheme by grapheme the way ratatui's buffer draws them, so a
//! width computed here is what ends up on screen.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Columns one grapheme takes, matching `ratatui`'s `Buffer::set_stringn`:
/// control characters are dropped (0), and halfwidth (han)dakuten, which
/// `unicode-width` calls zero-width, take a cell of their own.
fn grapheme_width(g: &str) -> usize {
    if g.contains(char::is_control) {
        0
    } else if g.len() == 1 {
        1
    } else {
        g.width()
            + g.chars()
                .filter(|c| matches!(c, '\u{FF9E}' | '\u{FF9F}'))
                .count()
    }
}

/// Columns `s` takes on screen.
pub fn width(s: &str) -> usize {
    s.graphemes(true).map(grapheme_width).sum()
}

/// The columns `[skip, skip + take)` of `s`. A wide grapheme cut by either
/// edge becomes spaces for its visible half, so the result is exactly as
/// wide as the part of `s` inside the range and later columns stay aligned.
pub fn slice(s: &str, skip: usize, take: usize) -> String {
    let end = skip.saturating_add(take);
    let mut out = String::new();
    let mut col = 0;
    for g in s.graphemes(true) {
        if col >= end {
            break;
        }
        let w = grapheme_width(g);
        let next = col + w;
        if w > 0 && col >= skip && next <= end {
            out.push_str(g);
        } else if w > 0 && next > skip {
            // Straddles an edge: pad the visible columns.
            let visible = next.min(end) - col.max(skip);
            out.extend(std::iter::repeat_n(' ', visible));
        }
        col = next;
    }
    out
}

/// The first `max` columns of `s` (a wide grapheme cut in half becomes a
/// space).
pub fn truncate(s: &str, max: usize) -> String {
    slice(s, 0, max)
}

/// `s` if it fits in `max` columns, else cut to `max - 1` columns plus `…`.
pub fn ellipsize(s: &str, max: usize) -> String {
    if width(s) <= max {
        s.to_string()
    } else if max == 0 {
        String::new()
    } else {
        let mut out = truncate(s, max - 1);
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_counts_columns_not_chars() {
        assert_eq!(width("abc"), 3);
        assert_eq!(width("日本"), 4);
        assert_eq!(width("🦀"), 2);
        // e + combining acute: one grapheme, one column.
        assert_eq!(width("e\u{301}"), 1);
        assert_eq!(width("a\tb"), 2);
    }

    #[test]
    fn slice_pads_wide_graphemes_cut_by_the_edges() {
        assert_eq!(slice("abcdef", 2, 3), "cde");
        assert_eq!(slice("日本語", 0, 4), "日本");
        // Cut in the middle of 本 on both sides.
        assert_eq!(slice("日本語", 1, 4), " 本 ");
        assert_eq!(slice("日本語", 3, 10), " 語");
        assert_eq!(slice("ab", 5, 3), "");
        for (s, skip, take) in [("a日b本c", 1, 4), ("🦀x🦀", 1, 3), ("日本語", 1, 4)] {
            assert_eq!(width(&slice(s, skip, take)), take.min(width(s) - skip));
        }
    }

    #[test]
    fn ellipsize_marks_cut_text() {
        assert_eq!(ellipsize("short", 10), "short");
        assert_eq!(ellipsize("abcdef", 4), "abc…");
        assert_eq!(ellipsize("日本語", 4), "日 …");
        assert_eq!(width(&ellipsize("日本語", 4)), 4);
        assert_eq!(ellipsize("abc", 0), "");
    }
}
