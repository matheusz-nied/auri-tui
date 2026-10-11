//! The Explorer / Source Control tab bar on the sidebar's top rows. Not a
//! `Component` (never focused): `App` draws it each frame and resolves
//! clicks through the returned hitboxes.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::action::Action;
use crate::layout::SidebarView;
use crate::text;
use crate::theme;

use super::hitbox::Hitboxes;

/// Draw two equal-width 1-row tabs on `area`'s first row (the active one
/// highlighted) and register each as a `SetSidebarView` button; the rows
/// below are left blank as spacing before the panels. Labels shorten when a
/// half is too narrow for the full names. (Taller tabs can't center their
/// label at even heights, and half-block `▄`/`▀` edges leave a visible seam
/// in Terminal.app.)
pub fn render(f: &mut Frame, area: Rect, active: SidebarView, has_git: bool, hits: &mut Hitboxes) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let left_w = area.width / 2;
    let halves = [
        Rect::new(area.x, area.y, left_w, 1),
        Rect::new(area.x + left_w, area.y, area.width - left_w, 1),
    ];
    let (git_label, git_short) = if has_git {
        ("Source Control", "Git")
    } else {
        ("Initialize Git", "Init Git")
    };
    let tabs = [
        (SidebarView::Explorer, "Explorer", "Files"),
        (SidebarView::SourceControl, git_label, git_short),
    ];
    // Both tabs switch to short labels together so they stay consistent.
    let long_fits = tabs
        .iter()
        .all(|(_, long, _)| text::width(long) + 2 <= left_w as usize);
    for ((view, long, short), rect) in tabs.into_iter().zip(halves) {
        let w = rect.width as usize;
        let label = if long_fits { long } else { short };
        let label = text::truncate(label, w);
        let left = (w - text::width(&label)) / 2;
        let right = w - left - text::width(&label);
        // The active tab is lifted off the bar (lighter fill) with a bold,
        // underlined gold label; the other recedes into the bar, muted.
        let (bg, label_style) = if view == active {
            (
                theme::SEL_FOCUS_BG,
                Style::default()
                    .fg(theme::GOLD)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )
        } else {
            (theme::BG_BAR, Style::default().fg(theme::MUTED))
        };
        let line = Line::from(vec![
            Span::raw(" ".repeat(left)),
            Span::styled(label, label_style),
            Span::raw(" ".repeat(right)),
        ]);
        f.render_widget(Paragraph::new(line).style(Style::default().bg(bg)), rect);
        hits.push(rect, Action::SetSidebarView(view));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use ratatui::Terminal;

    fn draw(width: u16, active: SidebarView, has_git: bool) -> (String, Hitboxes) {
        let mut hits = Hitboxes::default();
        let mut term = Terminal::new(TestBackend::new(width, 2)).unwrap();
        term.draw(|f| render(f, f.area(), active, has_git, &mut hits))
            .unwrap();
        let buf = term.backend().buffer();
        // Tabs on row 0; row 1 is blank spacing (no tab background).
        let text = (0..width).map(|x| buf[(x, 0)].symbol()).collect();
        for x in [0, width - 1] {
            assert_ne!(buf[(x, 0)].bg, Color::Reset);
            assert_eq!(buf[(x, 1)].symbol(), " ");
            assert_eq!(buf[(x, 1)].bg, Color::Reset);
            assert!(hits.hit(x, 1).is_none(), "spacing isn't clickable");
        }
        (text, hits)
    }

    #[test]
    fn tabs_split_the_row_and_switch_views() {
        let (text, hits) = draw(40, SidebarView::Explorer, true);
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
        let (text, _) = draw(20, SidebarView::SourceControl, true);
        assert_eq!(text, "  Files      Git    ");
    }

    #[test]
    fn plain_folder_offers_initialization_at_both_widths() {
        for (width, label) in [(40, "Initialize Git"), (20, "Init Git")] {
            let (text, hits) = draw(width, SidebarView::Explorer, false);
            assert!(text.contains(label), "{text}");
            assert!(matches!(
                hits.hit(width - 1, 0),
                Some(Action::SetSidebarView(SidebarView::SourceControl))
            ));
        }
    }
}
