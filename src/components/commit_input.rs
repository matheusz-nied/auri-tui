use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::action::Action;
use crate::component::Component;

use super::border_style;
use super::hitbox::Hitboxes;

/// Single-line commit message box plus a centered "✓ Commit" button on the
/// last row. When focused, printable characters edit the message; Enter emits
/// `Action::Commit`, Esc returns focus via `FocusNext`.
#[derive(Default)]
pub struct CommitInput {
    message: Vec<char>,
    /// Cursor position as a char index into `message`.
    cursor: usize,
    branch: String,
    hitboxes: Hitboxes,
}

impl CommitInput {
    fn message_text(&self) -> String {
        self.message.iter().collect()
    }
}

impl Component for CommitInput {
    fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Char(c) => {
                self.message.insert(self.cursor, c);
                self.cursor += 1;
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.message.remove(self.cursor);
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.message.len() {
                    self.message.remove(self.cursor);
                }
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.message.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.message.len(),
            KeyCode::Enter => {
                let msg = self.message_text();
                if !msg.trim().is_empty() {
                    return Some(Action::Commit(msg));
                }
            }
            _ => {}
        }
        None
    }

    fn handle_mouse(&mut self, ev: MouseEvent, _area: Rect) -> Option<Action> {
        if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
            if let Some(Action::Commit(msg)) = self.hitboxes.hit(ev.column, ev.row) {
                if msg.trim().is_empty() {
                    return Some(Action::Error("Type a commit message".to_string()));
                }
                return Some(Action::Commit(msg));
            }
        }
        None
    }

    fn update(&mut self, action: &Action) -> Option<Action> {
        match action {
            Action::BranchLoaded(branch) => self.branch = branch.clone(),
            Action::CommitDone => {
                self.message.clear();
                self.cursor = 0;
            }
            _ => {}
        }
        None
    }

    fn hints(&self) -> &'static str {
        "enter commit · esc back"
    }

    fn render(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        self.hitboxes.clear();
        // Last row is the button bar; the rest is the bordered input box.
        let input = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
        let button_row = Rect::new(area.x, area.y + input.height, area.width, 1);

        let block = Block::bordered()
            .title("Message (Enter to commit)")
            .border_style(border_style(focused));
        let inner = block.inner(input);
        f.render_widget(block, input);

        let text = self.message_text();
        let line = if text.is_empty() {
            let placeholder = if self.branch.is_empty() {
                "Commit message".to_string()
            } else {
                format!("Commit on '{}'", self.branch)
            };
            Line::from(Span::styled(
                placeholder,
                Style::default().fg(Color::DarkGray),
            ))
        } else {
            Line::from(text)
        };
        f.render_widget(Paragraph::new(line), inner);

        // "✓ Commit" full-width button bar; dimmer while the message is empty.
        let label = "✓ Commit";
        let label_w = label.chars().count() as u16;
        let left = button_row.width.saturating_sub(label_w) / 2;
        let right = button_row.width.saturating_sub(left + label_w);
        let bg = if self.message.is_empty() {
            Color::Rgb(40, 60, 80)
        } else {
            Color::Rgb(0, 95, 160)
        };
        let bar = format!(
            "{}{}{}",
            " ".repeat(left as usize),
            label,
            " ".repeat(right as usize)
        );
        f.render_widget(
            Span::styled(bar, Style::default().fg(Color::White).bg(bg)),
            button_row,
        );
        self.hitboxes
            .push(button_row, Action::Commit(self.message_text()));

        if focused {
            let x = inner.x + self.cursor as u16;
            if x < inner.x + inner.width {
                f.set_cursor_position((x, inner.y));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn area() -> Rect {
        Rect::new(0, 0, 60, 4)
    }

    fn click(col: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row: 3,
            modifiers: ratatui::crossterm::event::KeyModifiers::empty(),
        }
    }

    #[test]
    fn commit_button_is_a_full_width_bar() {
        let mut c = CommitInput::default();
        let mut term = Terminal::new(TestBackend::new(60, 4)).unwrap();
        term.draw(|f| c.render(f, area(), true)).unwrap();
        let buf = term.backend().buffer();
        // Empty message -> dimmed bar across the whole row.
        for x in 0..60 {
            assert_eq!(buf[(x, 3)].bg, Color::Rgb(40, 60, 80), "col {x}");
        }
        // Clicking the far edges of the row still hits the button.
        assert!(matches!(
            c.handle_mouse(click(0), area()),
            Some(Action::Error(_))
        ));
        assert!(matches!(
            c.handle_mouse(click(59), area()),
            Some(Action::Error(_))
        ));
    }

    #[test]
    fn commit_button_click_with_message_commits() {
        let mut c = CommitInput::default();
        c.handle_key(KeyEvent::new(
            KeyCode::Char('x'),
            ratatui::crossterm::event::KeyModifiers::empty(),
        ));
        let mut term = Terminal::new(TestBackend::new(60, 4)).unwrap();
        term.draw(|f| c.render(f, area(), true)).unwrap();
        let buf = term.backend().buffer();
        assert_eq!(buf[(0, 3)].bg, Color::Rgb(0, 95, 160));
        assert!(matches!(
            c.handle_mouse(click(59), area()),
            Some(Action::Commit(m)) if m == "x"
        ));
    }
}
