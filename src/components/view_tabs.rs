//! The Explorer / Source Control tab bar on the sidebar's top row. Not a
//! `Component` (never focused): `App` draws it each frame and resolves
//! clicks through the returned hitboxes.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::Frame;

use crate::action::Action;
use crate::layout::SidebarView;

use super::hitbox::Hitboxes;

const TABS: [(SidebarView, &str, &str); 2] = [
    (SidebarView::Explorer, "Explorer", "Files"),
    (SidebarView::SourceControl, "Source Control", "Git"),
];

/// Draw two equal-width tabs into `area` (the active one highlighted) and
/// register each as a `SetSidebarView` button. Labels shorten when a half
/// is too narrow for the full names.
pub fn render(f: &mut Frame, area: Rect, active: SidebarView, hits: &mut Hitboxes) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let left_w = area.width / 2;
    let halves = [
        Rect::new(area.x, area.y, left_w, 1),
        Rect::new(area.x + left_w, area.y, area.width - left_w, 1),
    ];
    // Both tabs switch to short labels together so they stay consistent.
    let long_fits = TABS
        .iter()
        .all(|(_, long, _)| long.chars().count() + 2 <= left_w as usize);
    for ((view, long, short), rect) in TABS.into_iter().zip(halves) {
        let w = rect.width as usize;
        let label = if long_fits { long } else { short };
        let label: String = label.chars().take(w).collect();
        let left = (w - label.chars().count()) / 2;
        let text = format!(
            "{:left$}{label}{:right$}",
            "",
            "",
            right = w - left - label.chars().count()
        );
        let style = if view == active {
            Style::default()
                .fg(Color::White)
                .bg(Color::Rgb(40, 45, 65))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::DarkGray)
                .bg(Color::Rgb(25, 25, 32))
        };
        f.render_widget(Span::styled(text, style), rect);
        hits.push(rect, Action::SetSidebarView(view));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn draw(width: u16, active: SidebarView) -> (String, Hitboxes) {
        let mut hits = Hitboxes::default();
        let mut term = Terminal::new(TestBackend::new(width, 1)).unwrap();
        term.draw(|f| render(f, f.area(), active, &mut hits))
            .unwrap();
        let buf = term.backend().buffer();
        let text = (0..width).map(|x| buf[(x, 0)].symbol()).collect();
        (text, hits)
    }

    #[test]
    fn tabs_split_the_row_and_switch_views() {
        let (text, hits) = draw(40, SidebarView::Explorer);
        assert_eq!(text, "      Explorer         Source Control   ");
        assert!(matches!(
            hits.hit(0, 0),
            Some(Action::SetSidebarView(SidebarView::Explorer))
        ));
        assert!(matches!(
            hits.hit(39, 0),
            Some(Action::SetSidebarView(SidebarView::SourceControl))
        ));
    }

    #[test]
    fn narrow_tabs_use_short_labels() {
        let (text, _) = draw(20, SidebarView::SourceControl);
        assert_eq!(text, "  Files      Git    ");
    }
}
