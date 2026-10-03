pub mod changes;
pub mod commit_input;
pub mod confirm_dialog;
pub mod diff_view;
pub mod file_tree;
pub mod file_view;
pub mod help;
pub mod history;
pub mod hitbox;
pub mod view_tabs;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::action::Action;
use crate::text;
use crate::theme;

use hitbox::button_span;

/// Border style used to show which panel owns keyboard focus.
pub fn border_style(focused: bool) -> Style {
    theme::border(focused)
}

/// Lines scrolled per mouse-wheel event. Trackpads emit one event per small
/// gesture step, so this must stay small — a bigger step amplifies overshoot.
pub const SCROLL_LINES: usize = 1;

/// Selection highlight used by list-like components.
pub fn selection_style(focused: bool) -> Style {
    theme::selection(focused)
}

/// Mark the selected row of a focused list with a gold bar in its first
/// column (the landing page's `inset 2px 0 0 gold`). Every row starts with a
/// spacer column, so nothing is overwritten; a row that doesn't is left as is.
pub fn selection_bar(line: Line<'static>) -> Line<'static> {
    let mut spans = line.spans;
    let Some(first) = spans.first() else {
        return Line::from(spans);
    };
    let Some(rest) = first.content.strip_prefix(' ') else {
        return Line::from(spans);
    };
    let style = first.style;
    let rest = rest.to_string();
    spans[0] = Span::styled(rest, style);
    spans.insert(0, Span::styled("▌", style.fg(theme::GOLD)));
    Line::from(spans)
}

/// One-letter status color for file rows (Changes and History).
pub fn code_color(code: char) -> Color {
    match code {
        'M' => theme::GOLD,
        'A' | 'U' => theme::ADD_FG,
        'D' | '!' => theme::DEL_FG,
        'R' | 'C' => theme::COOL,
        _ => theme::INK,
    }
}

fn button_color(glyph: &str) -> Color {
    match glyph {
        "+" => theme::ADD_FG,
        "−" => theme::COOL,
        "↶" => theme::DEL_FG,
        _ => theme::INK,
    }
}

/// Status letter style: conflicts (`!`) are told apart from deletions by
/// weight, since they share the rose color.
fn code_style(style: Style, code: char) -> Style {
    let style = style.fg(code_color(code));
    if code == '!' {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

/// Build a file-row line: `{indent}name dir[dim] pad buttons code`. Returns
/// the line plus each button's `(x_offset, Action)` hitbox relative to the
/// row's left edge. When `buttons` don't fit (or is empty) they are dropped.
pub fn file_row_line(
    path: &str,
    code: char,
    indent: usize,
    width: usize,
    style: Style,
    buttons: &[(&'static str, Action)],
) -> (Line<'static>, Vec<(u16, Action)>) {
    let code_s = code.to_string();
    // code at the last column, buttons (3 cols each) right before it.
    let btn_w = buttons.len() * 3;
    // Space for the indented "name dir" text, minus code and buttons.
    let avail = width.saturating_sub(code_s.len() + btn_w);
    let (buttons, btn_w, avail) = if btn_w > 0 && avail < indent + 3 {
        (&[][..], 0, width.saturating_sub(code_s.len()))
    } else {
        (buttons, btn_w, avail)
    };

    // VS Code style: `action.rs src` — name, then the dim parent dir.
    let (dir, name) = match path.rsplit_once('/') {
        Some((d, n)) => (d.to_string(), n.to_string()),
        None => (String::new(), path.to_string()),
    };
    let name_span = format!("{:indent$}{name}", "", indent = indent);
    let dir_span = if dir.is_empty() {
        String::new()
    } else {
        format!(" {dir}")
    };
    // The name wins the space; whatever is cut (name or dir) ends in `…`.
    // A dir with room for less than one char plus `…` is dropped.
    let name_txt = text::ellipsize(&name_span, avail);
    let name_w = text::width(&name_txt);
    let dir_room = avail - name_w;
    let dir_txt = if dir_room >= 3 {
        text::ellipsize(&dir_span, dir_room)
    } else if text::width(&dir_span) <= dir_room {
        dir_span
    } else {
        String::new()
    };
    let used = name_w + text::width(&dir_txt);
    let pad = avail.saturating_sub(used);

    let mut spans = vec![
        Span::styled(name_txt, style),
        Span::styled(dir_txt, style.fg(theme::FAINT)),
        Span::styled(" ".repeat(pad), style),
    ];
    let mut hits = Vec::new();
    // Buttons sit right before the code column, at fixed offsets from the
    // right edge.
    let mut x = (width - code_s.len() - btn_w) as u16;
    for (glyph, action) in buttons {
        spans.push(button_span(glyph, style.fg(button_color(glyph))));
        hits.push((x, action.clone()));
        x += 3;
    }
    spans.push(Span::styled(code_s, code_style(style, code)));
    (Line::from(spans), hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(path: &str, width: usize, buttons: &[(&'static str, Action)]) -> String {
        let (line, _) = file_row_line(path, 'M', 1, width, Style::default(), buttons);
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn long_paths_are_ellipsized_and_keep_the_code() {
        // Dir cut first, then the name; the code always holds the last column.
        assert_eq!(
            row("src/components/changes.rs", 30, &[]),
            " changes.rs src/components   M"
        );
        assert_eq!(
            row("src/components/changes.rs", 20, &[]),
            " changes.rs src/co…M"
        );
        assert_eq!(
            row("src/components/a_very_long_name.rs", 12, &[]),
            " a_very_lo…M"
        );
        let buttons = [("+", Action::StageAll)];
        let r = row("src/components/changes.rs", 20, &buttons);
        assert_eq!(r, " changes.rs src… + M");
    }

    #[test]
    fn selection_bar_takes_the_spacer_column_only() {
        let sel = theme::selection(true);
        let (line, _) = file_row_line("a.rs", 'M', 1, 12, sel, &[]);
        let before: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let marked = selection_bar(line);
        let after: String = marked.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(after, before.replacen(' ', "▌", 1));
        assert_eq!(marked.spans[0].style.fg, Some(theme::GOLD));
        assert_eq!(marked.spans[0].style.bg, sel.bg);
        // A row with no spacer column is left untouched.
        let bare = Line::from("x");
        assert_eq!(selection_bar(bare.clone()), bare);
    }

    #[test]
    fn wide_names_are_measured_in_columns() {
        for (path, w) in [("docs/日本語のファイル.md", 16), ("🦀/crab.rs", 12)] {
            let r = row(path, w, &[]);
            assert_eq!(text::width(&r), w, "{r:?}");
            assert!(r.ends_with('M'), "{r:?}");
        }
    }
}
