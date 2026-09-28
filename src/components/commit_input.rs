use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::action::Action;
use crate::component::Component;

use super::border_style;

/// Single-line commit message box. When focused, printable characters edit the
/// message; Enter emits `Action::Commit`, Esc returns focus via `FocusNext`.
#[derive(Default)]
pub struct CommitInput {
    message: Vec<char>,
    /// Cursor position as a char index into `message`.
    cursor: usize,
    branch: String,
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
                let msg: String = self.message.iter().collect();
                if !msg.trim().is_empty() {
                    return Some(Action::Commit(msg));
                }
            }
            _ => {}
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
        let block = Block::bordered()
            .title("Message (Enter to commit)")
            .border_style(border_style(focused));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let text: String = self.message.iter().collect();
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

        if focused {
            let x = inner.x + self.cursor as u16;
            if x < inner.x + inner.width {
                f.set_cursor_position((x, inner.y));
            }
        }
    }
}
