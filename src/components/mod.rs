pub mod changes;
pub mod commit_input;
pub mod confirm_dialog;
pub mod diff_view;
pub mod history;
pub mod hitbox;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::action::Action;

use hitbox::button_span;

/// Border style used to show which panel owns keyboard focus.
pub fn border_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

/// Lines scrolled per mouse-wheel event. Trackpads emit one event per small
/// gesture step, so this must stay small — a bigger step amplifies overshoot.
pub const SCROLL_LINES: usize = 1;

/// Selection highlight used by list-like components.
pub fn selection_style(focused: bool) -> Style {
    if focused {
        Style::default().bg(Color::Rgb(40, 45, 65))
    } else {
        Style::default().bg(Color::Rgb(30, 30, 40))
    }
}

/// One-letter status color for file rows (Changes and History).
pub fn code_color(code: char) -> Color {
    match code {
        'M' => Color::Yellow,
        'A' | 'U' => Color::Green,
        'D' => Color::Red,
        'R' | 'C' => Color::Blue,
        _ => Color::White,
    }
}

fn button_color(glyph: &str) -> Color {
    match glyph {
        "+" => Color::Green,
        "−" => Color::Blue,
        "↶" => Color::Red,
        _ => Color::White,
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
    let name_w = name_span.chars().count().min(avail);
    let name_txt: String = name_span.chars().take(name_w).collect();
    let dir_w = dir_span.chars().count().min(avail.saturating_sub(name_w));
    let dir_txt: String = dir_span.chars().take(dir_w).collect();
    let used = name_w + dir_w;
    let pad = avail.saturating_sub(used);

    let mut spans = vec![
        Span::styled(name_txt, style),
        Span::styled(dir_txt, style.fg(Color::DarkGray)),
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
    spans.push(Span::styled(code_s, style.fg(code_color(code))));
    (Line::from(spans), hits)
}
