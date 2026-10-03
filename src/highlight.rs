//! Syntax highlighting for the file and diff viewers.
//!
//! Pure (no I/O): grammars and the theme are embedded — `two-face` ships
//! bat's syntax set (TypeScript, TOML, Dockerfile, … on top of Sublime's
//! defaults) — and parsed once on first use. Highlighting is stateful
//! (a line's colors depend on every line before it), so a `Highlighter`
//! works through a document incrementally and caches the result: only as
//! far as the viewer has scrolled, which keeps opening big files cheap.

use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use syntect::easy::HighlightLines;
use syntect::highlighting::{
    FontStyle, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings,
};
use syntect::parsing::{SyntaxReference, SyntaxSet};

use crate::text;
use crate::theme as palette;

/// Lines past this are left plain — highlighting is linear in the distance
/// scrolled and a jump to the end of a huge file would freeze the UI.
const MAX_LINES: usize = 20_000;
/// A line longer than this (minified code) stops highlighting for the rest
/// of the document: the regex engine can take seconds on such lines.
const MAX_LINE_BYTES: usize = 4096;

/// One colored run of a highlighted line.
pub type Chunk = (Style, String);

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

fn theme() -> &'static Theme {
    static THEME_CELL: OnceLock<Theme> = OnceLock::new();
    THEME_CELL.get_or_init(astro_theme)
}

fn syn_color(c: Color) -> syntect::highlighting::Color {
    let Color::Rgb(r, g, b) = c else {
        unreachable!("palette colors are Rgb")
    };
    syntect::highlighting::Color { r, g, b, a: 0xff }
}

/// The landing page's token colors (see `theme.rs`), built in code rather
/// than loaded from a `.tmTheme`. Later items win only when their selector
/// is more specific, so `keyword.operator` below can override `keyword`.
fn astro_theme() -> Theme {
    let item = |selectors: &str, fg: Color, font: Option<FontStyle>| ThemeItem {
        scope: selectors
            .parse::<ScopeSelectors>()
            .expect("valid scope selector"),
        style: StyleModifier {
            foreground: Some(syn_color(fg)),
            background: None,
            font_style: font,
        },
    };
    let ink = palette::INK;
    Theme {
        name: Some("astro".into()),
        author: None,
        settings: ThemeSettings {
            foreground: Some(syn_color(ink)),
            background: Some(syn_color(palette::BG)),
            ..ThemeSettings::default()
        },
        scopes: vec![
            item(
                "comment, meta.attribute, meta.annotation, punctuation.definition.annotation",
                palette::SYN_COMMENT,
                Some(FontStyle::ITALIC),
            ),
            item(
                "string, markup.raw, markup.inline.raw",
                palette::SYN_STRING,
                None,
            ),
            item(
                "keyword, storage, constant.language, variable.language, markup.list",
                palette::SYN_KEYWORD,
                None,
            ),
            item("keyword.operator", ink, None),
            item(
                "entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, \
                 entity.name.trait, entity.name.namespace, support.type, support.class, \
                 storage.type.numeric, storage.type.primitive, constant.numeric, \
                 entity.name.section",
                palette::SYN_TYPE,
                None,
            ),
            item(
                "entity.name.function, support.function, variable.function, meta.function-call",
                ink,
                None,
            ),
            item("markup.heading", ink, Some(FontStyle::BOLD)),
        ],
    }
}

/// Grammar for a repo-relative path: whole file name first (`Makefile`,
/// `Dockerfile`), then the extension, then the first line (shebangs,
/// `<?xml`, modelines). `None` = plain text.
fn find_syntax(path: &str, first_line: Option<&str>) -> Option<&'static SyntaxReference> {
    let ss = syntaxes();
    let name = path.rsplit('/').next().unwrap_or(path);
    ss.find_syntax_by_extension(name)
        .or_else(|| {
            name.rsplit_once('.')
                .and_then(|(_, ext)| ss.find_syntax_by_extension(ext))
        })
        .or_else(|| first_line.and_then(|l| ss.find_syntax_by_first_line(l)))
        .filter(|s| s.name != "Plain Text")
}

/// Incremental highlighter for one document (or one side of a diff).
pub struct Highlighter {
    /// `None` = plain text, or highlighting was given up on.
    state: Option<HighlightLines<'static>>,
    /// Highlighted lines `0..lines.len()`.
    lines: Vec<Vec<Chunk>>,
}

impl Highlighter {
    pub fn plain() -> Self {
        Self {
            state: None,
            lines: Vec::new(),
        }
    }

    pub fn for_file(path: &str, first_line: Option<&str>) -> Self {
        Self {
            state: find_syntax(path, first_line).map(|s| HighlightLines::new(s, theme())),
            lines: Vec::new(),
        }
    }

    /// A grammar was found and highlighting has not been given up on.
    #[cfg(test)]
    fn active(&self) -> bool {
        self.state.is_some()
    }

    /// Highlight up to line `upto` (exclusive). `all` must yield the
    /// document's lines from the very first one; lines already done are
    /// skipped.
    pub fn advance<'a>(&mut self, all: impl IntoIterator<Item = &'a str>, upto: usize) {
        let upto = upto.min(MAX_LINES);
        let done = self.lines.len();
        if upto <= done {
            return;
        }
        for text in all.into_iter().skip(done).take(upto - done) {
            let Some(state) = self.state.as_mut() else {
                return;
            };
            if text.len() > MAX_LINE_BYTES {
                self.state = None;
                return;
            }
            // The grammars expect the line terminator (`extra_newlines`).
            let line = format!("{text}\n");
            let Ok(ranges) = state.highlight_line(&line, syntaxes()) else {
                self.state = None;
                return;
            };
            let chunks = ranges
                .into_iter()
                .map(|(style, s)| (convert(style), s.trim_end_matches('\n').to_string()))
                .filter(|(_, s)| !s.is_empty())
                .collect();
            self.lines.push(chunks);
        }
    }

    /// Spans for line `i` cropped to the columns `[skip, skip + take)`.
    /// Lines not highlighted (plain file, past the limits, not yet
    /// `advance`d to) come back as `raw` in the default style.
    pub fn spans(&self, i: usize, raw: &str, skip: usize, take: usize) -> Vec<Span<'static>> {
        match self.lines.get(i) {
            Some(chunks) => crop(chunks, skip, take),
            None => vec![Span::raw(text::slice(raw, skip, take))],
        }
    }
}

/// Foreground + font style only: the background stays the terminal's (or
/// the diff's added/removed tint).
fn convert(style: syntect::highlighting::Style) -> Style {
    let c = style.foreground;
    let mut out = Style::default().fg(Color::Rgb(c.r, c.g, c.b));
    if style.font_style.contains(FontStyle::BOLD) {
        out = out.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        out = out.add_modifier(Modifier::ITALIC);
    }
    out
}

/// Cut the columns `[skip, skip + take)` out of a run of chunks (a wide
/// character cut by an edge becomes spaces, see `text::slice`).
fn crop(chunks: &[Chunk], skip: usize, take: usize) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let end = skip + take;
    let mut col = 0;
    for (style, text) in chunks {
        if col >= end {
            break;
        }
        let w = text::width(text);
        if col + w > skip {
            let from = skip.saturating_sub(col);
            let piece = text::slice(text, from, end - col.max(skip));
            if !piece.is_empty() {
                out.push(Span::styled(piece, *style));
            }
        }
        col += w;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn detects_by_name_extension_and_first_line() {
        let name = |p: &str, first: Option<&str>| find_syntax(p, first).map(|s| s.name.as_str());
        assert_eq!(name("src/main.rs", None), Some("Rust"));
        assert_eq!(name("web/App.TSX", None), Some("TypeScriptReact"));
        assert_eq!(name("Cargo.toml", None), Some("TOML"));
        assert_eq!(name("Makefile", None), Some("Makefile"));
        assert_eq!(
            name("bin/run", Some("#!/usr/bin/env python3")),
            Some("Python")
        );
        assert_eq!(name("notes.txt", None), None);
        assert_eq!(name("LICENSE", None), None);
    }

    #[test]
    fn highlights_code_with_several_colors() {
        let src = ["fn main() {", "    let s = \"hi\"; // note", "}"];
        let mut h = Highlighter::for_file("a.rs", None);
        assert!(h.active());
        h.advance(src, 3);
        let line = h.spans(1, src[1], 0, 100);
        assert_eq!(text(&line), src[1]);
        let colors: std::collections::HashSet<_> = line.iter().map(|s| s.style.fg).collect();
        assert!(colors.len() >= 3, "{line:?}");
    }

    #[test]
    fn state_carries_across_lines() {
        // Line 2 is inside a block comment opened on line 1.
        let src = ["/* start", "let x = 1;", "*/ let y = 2;"];
        let mut h = Highlighter::for_file("a.rs", None);
        h.advance(src, 3);
        let inside = h.spans(1, src[1], 0, 100);
        assert_eq!(inside.len(), 1, "{inside:?}");
        let comment = h.spans(0, src[0], 0, 100)[0].style;
        assert_eq!(inside[0].style, comment);
    }

    #[test]
    fn advance_is_lazy_and_resumes() {
        let src: Vec<String> = (0..10).map(|i| format!("let x{i} = {i};")).collect();
        let mut h = Highlighter::for_file("a.rs", None);
        h.advance(src.iter().map(String::as_str), 3);
        assert_eq!(h.lines.len(), 3);
        // Not reached yet: plain.
        assert_eq!(h.spans(5, &src[5], 0, 100).len(), 1);
        h.advance(src.iter().map(String::as_str), 8);
        assert_eq!(h.lines.len(), 8);
        assert!(h.spans(5, &src[5], 0, 100).len() > 1);
    }

    #[test]
    fn plain_and_overlong_lines_fall_back_to_raw() {
        let mut h = Highlighter::for_file("notes.txt", None);
        h.advance(["hello world"], 1);
        assert_eq!(h.spans(0, "hello world", 6, 3)[0].content, "wor");

        let long = "x".repeat(MAX_LINE_BYTES + 1);
        let mut h = Highlighter::for_file("a.js", None);
        h.advance(["let a = 1;", &long, "let b = 2;"], 3);
        assert_eq!(h.lines.len(), 1);
        assert!(!h.active());
    }

    #[test]
    fn crop_spans_chunk_boundaries() {
        let s = Style::default();
        let chunks = vec![
            (s, "abc".to_string()),
            (s, "déf".to_string()),
            (s, "ghi".to_string()),
        ];
        assert_eq!(text(&crop(&chunks, 2, 5)), "cdéfg");
        assert_eq!(text(&crop(&chunks, 3, 3)), "déf");
        assert_eq!(text(&crop(&chunks, 8, 10)), "i");
        assert!(crop(&chunks, 20, 5).is_empty());
    }

    #[test]
    fn crop_counts_wide_chars_as_two_columns() {
        let s = Style::default();
        let chunks = vec![(s, "a日".to_string()), (s, "本b".to_string())];
        assert_eq!(text(&crop(&chunks, 0, 6)), "a日本b");
        assert_eq!(text(&crop(&chunks, 1, 4)), "日本");
        // Half of 日 and half of 本 fall outside: padded with spaces.
        assert_eq!(text(&crop(&chunks, 2, 4)), " 本b");
        assert_eq!(text(&crop(&chunks, 0, 4)), "a日 ");
    }
}
