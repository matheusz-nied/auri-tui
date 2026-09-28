pub mod changes;
pub mod commit_input;
pub mod diff_view;

use ratatui::style::{Color, Style};

/// Border style used to show which panel owns keyboard focus.
pub fn border_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

/// Selection highlight used by list-like components.
pub fn selection_style(focused: bool) -> Style {
    if focused {
        Style::default().bg(Color::Rgb(40, 45, 65))
    } else {
        Style::default().bg(Color::Rgb(30, 30, 40))
    }
}
