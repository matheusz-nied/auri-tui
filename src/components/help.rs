use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;

use crate::action::Action;
use crate::component::Component;
use crate::keymap::HelpSection;
use crate::text;
use crate::theme;

use super::SCROLL_LINES;

/// Widest the help box gets.
const MAX_WIDTH: u16 = 76;

/// Modal key reference (`?`). Opens on `Action::ShowHelp` — `App` builds
/// the sections from `keymap::GLOBAL` and every panel's `hints()`, so it
/// can't drift from the real keys. While open it captures all input:
/// j/k/PgUp/PgDn/wheel scroll; Esc, `q`, `?`, Enter or a click outside
/// close it.
#[derive(Default)]
pub struct Help {
    open: Option<Vec<HelpSection>>,
    scroll: usize,
    /// Content rows that fit, from the last render.
    view_height: usize,
    /// The box from the last render (clicks outside close it).
    area: Rect,
}

impl Help {
    /// Section titles, rows and blank separators, as lines; keys padded to
    /// one column.
    fn lines(sections: &[HelpSection]) -> Vec<Line<'static>> {
        let key_w = sections
            .iter()
            .flat_map(|s| &s.rows)
            .map(|(k, _)| text::width(k))
            .max()
            .unwrap_or(0);
        let mut lines = Vec::new();
        for (i, section) in sections.iter().enumerate() {
            if i > 0 {
                lines.push(Line::default());
            }
            lines.push(Line::from(Span::styled(
                section.title.clone(),
                Style::default()
                    .fg(theme::GOLD)
                    .add_modifier(Modifier::BOLD),
            )));
            for (key, what) in &section.rows {
                let pad = key_w.saturating_sub(text::width(key));
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {key}{:pad$}  ", ""),
                        Style::default().fg(theme::INK).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(what.clone(), theme::muted()),
                ]));
            }
        }
        lines
    }

    fn line_count(&self) -> usize {
        self.open.as_deref().map_or(0, |s| Self::lines(s).len())
    }

    fn scroll_by(&mut self, delta: isize) {
        let max = self.line_count().saturating_sub(self.view_height);
        self.scroll = self.scroll.saturating_add_signed(delta).min(max);
    }

    fn close(&mut self) -> Option<Action> {
        self.open = None;
        None
    }
}

impl Component for Help {
    fn captures_input(&self) -> bool {
        self.open.is_some()
    }

    fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        self.open.as_ref()?;
        let page = self.view_height.max(1) as isize;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') | KeyCode::Enter => {
                return self.close()
            }
            KeyCode::Char('j') | KeyCode::Down => self.scroll_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_by(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll_by(page),
            KeyCode::PageUp => self.scroll_by(-page),
            KeyCode::Char('g') | KeyCode::Home => self.scroll = 0,
            KeyCode::Char('G') | KeyCode::End => self.scroll_by(isize::MAX / 2),
            _ => {}
        }
        None
    }

    fn handle_mouse(&mut self, ev: MouseEvent, _area: Rect) -> Option<Action> {
        self.open.as_ref()?;
        match ev.kind {
            MouseEventKind::ScrollDown => self.scroll_by(SCROLL_LINES as isize),
            MouseEventKind::ScrollUp => self.scroll_by(-(SCROLL_LINES as isize)),
            MouseEventKind::Down(MouseButton::Left)
                if !self.area.contains((ev.column, ev.row).into()) =>
            {
                return self.close();
            }
            _ => {}
        }
        None
    }

    fn update(&mut self, action: &Action) -> Option<Action> {
        if let Action::ShowHelp(sections) = action {
            self.open = Some(sections.clone());
            self.scroll = 0;
        }
        None
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        let Some(sections) = &self.open else {
            return;
        };
        let lines = Self::lines(sections);
        let width = MAX_WIDTH
            .min(area.width.saturating_sub(4))
            .max(20)
            .min(area.width);
        let height = (lines.len() as u16 + 2)
            .min(area.height.saturating_sub(2))
            .max(3)
            .min(area.height);
        let rect = Rect::new(
            area.x + area.width.saturating_sub(width) / 2,
            area.y + area.height.saturating_sub(height) / 2,
            width,
            height,
        );
        self.area = rect;
        self.view_height = height.saturating_sub(2) as usize;
        self.scroll = self
            .scroll
            .min(lines.len().saturating_sub(self.view_height));
        let more = self.scroll + self.view_height < lines.len();
        let mut block = Block::bordered()
            .border_style(theme::border(true))
            .style(theme::base())
            .title(" Keys ")
            .title_bottom(Line::from(" esc close ").right_aligned());
        if more {
            block = block.title_bottom(Line::from(" j/k more ↓ ").left_aligned());
        }
        f.render_widget(Clear, rect);
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let visible: Vec<Line> = lines
            .into_iter()
            .skip(self.scroll)
            .take(self.view_height)
            .collect();
        f.render_widget(Paragraph::new(visible), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;
    use ratatui::Terminal;

    fn sections(rows: usize) -> Vec<HelpSection> {
        vec![
            HelpSection {
                title: "Global".to_string(),
                rows: vec![("q".to_string(), "quit".to_string())],
            },
            HelpSection {
                title: "Changes".to_string(),
                rows: (0..rows)
                    .map(|i| (format!("k{i}"), format!("does {i}")))
                    .collect(),
            },
        ]
    }

    fn screen(h: &mut Help, w: u16, ht: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, ht)).unwrap();
        term.draw(|f| h.render(f, f.area(), true)).unwrap();
        let buf = term.backend().buffer();
        let mut out = String::new();
        for y in 0..ht {
            for x in 0..w {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    #[test]
    fn opens_on_show_help_with_aligned_keys() {
        let mut h = Help::default();
        assert!(!h.captures_input());
        assert_eq!(screen(&mut h, 60, 20).trim(), "");
        h.update(&Action::ShowHelp(sections(2)));
        assert!(h.captures_input());
        let s = screen(&mut h, 60, 20);
        assert!(
            s.contains("Keys") && s.contains("Global") && s.contains("Changes"),
            "{s}"
        );
        // Keys share one column: "q " is padded to the width of "k0".
        assert!(
            s.contains("  q   quit") && s.contains("  k0  does 0"),
            "{s}"
        );
    }

    #[test]
    fn scrolls_when_taller_than_the_screen_and_closes() {
        let mut h = Help::default();
        h.update(&Action::ShowHelp(sections(30)));
        let s = screen(&mut h, 60, 12);
        assert!(s.contains("more") && !s.contains("does 29"), "{s}");
        h.handle_key(key(KeyCode::Char('G')));
        let s = screen(&mut h, 60, 12);
        assert!(s.contains("does 29") && !s.contains("more"), "{s}");
        h.handle_key(key(KeyCode::Char('g')));
        assert_eq!(h.scroll, 0);
        // Every other key is swallowed while open.
        assert!(h.handle_key(key(KeyCode::Char('x'))).is_none());
        assert!(h.captures_input());
        h.handle_key(key(KeyCode::Char('?')));
        assert!(!h.captures_input());
    }

    #[test]
    fn click_outside_closes() {
        let mut h = Help::default();
        h.update(&Action::ShowHelp(sections(2)));
        screen(&mut h, 60, 20);
        let click = |col, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        };
        h.handle_mouse(click(30, 10), Rect::default());
        assert!(h.captures_input(), "a click inside keeps it open");
        h.handle_mouse(click(0, 0), Rect::default());
        assert!(!h.captures_input());
    }
}
