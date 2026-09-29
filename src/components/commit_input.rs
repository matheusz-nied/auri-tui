use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::action::Action;
use crate::component::Component;

use super::border_style;
use super::hitbox::{button_span, Hitboxes};

/// Single-line commit message box plus a centered "✓ Commit" button on the
/// last row. When focused, printable characters edit the message; Enter emits
/// `Action::Commit`, Esc returns focus via `FocusNext`. The row's rightmost
/// button runs AI generation (`CommitMessageGenerating` shows progress).
#[derive(Default)]
pub struct CommitInput {
    message: Vec<char>,
    /// Cursor position as a char index into `message`.
    cursor: usize,
    branch: String,
    /// Provider label while an AI generation is running (`■` cancels).
    generating: Option<String>,
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
            // Ctrl-chars are commands (^g/^t AI, ^c quit) — never text.
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.message.insert(self.cursor, c);
                self.cursor += 1;
            }
            KeyCode::Char(_) => {}
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
            match self.hitboxes.hit(ev.column, ev.row) {
                Some(Action::Commit(msg)) => {
                    if msg.trim().is_empty() {
                        return Some(Action::Error("Type a commit message".to_string()));
                    }
                    return Some(Action::Commit(msg));
                }
                Some(action) => return Some(action),
                None => {}
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
            Action::CommitMessageGenerating { provider } => {
                self.generating = Some(provider.clone());
            }
            Action::CommitMessageGenerated(text) => {
                self.generating = None;
                self.message = text.chars().collect();
                self.cursor = self.message.len();
            }
            Action::CommitMessageFailed => self.generating = None,
            _ => {}
        }
        None
    }

    fn hints(&self) -> &'static str {
        "enter commit · ^g AI message · ^t AI provider · esc back"
    }

    fn render(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        self.hitboxes.clear();
        // Last row is the button bar; the rest is the bordered input box.
        let input = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
        let button_row = Rect::new(area.x, area.y + input.height, area.width, 1);

        let title = match &self.generating {
            Some(provider) => format!("Message (generating with {provider}…)"),
            None => "Message (Enter to commit)".to_string(),
        };
        let block = Block::bordered()
            .title(title)
            .border_style(border_style(focused));
        let inner = block.inner(input);
        f.render_widget(block, input);

        let text = self.message_text();
        let line = if text.is_empty() {
            let placeholder = if self.generating.is_some() {
                "Generating… (Esc to cancel)".to_string()
            } else if self.branch.is_empty() {
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

        // "✓ Commit" bar with the AI button on its rightmost 3 columns;
        // dimmer while the message is empty.
        let ai_w = 3.min(button_row.width);
        let commit_row = Rect::new(button_row.x, button_row.y, button_row.width - ai_w, 1);
        let ai_row = Rect::new(button_row.x + commit_row.width, button_row.y, ai_w, 1);
        let label = "✓ Commit";
        let label_w = label.chars().count() as u16;
        let left = commit_row.width.saturating_sub(label_w) / 2;
        let right = commit_row.width.saturating_sub(left + label_w);
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
            commit_row,
        );
        self.hitboxes
            .push(commit_row, Action::Commit(self.message_text()));

        // AI button: ✦ starts a generation, ■ cancels the running one.
        let (glyph, action, style) = if self.generating.is_some() {
            (
                "■",
                Action::CancelCommitMessage,
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Rgb(140, 40, 60)),
            )
        } else {
            (
                "✦",
                Action::GenerateCommitMessage,
                Style::default().fg(Color::Magenta).bg(bg),
            )
        };
        f.render_widget(button_span(glyph, style), ai_row);
        self.hitboxes.push(ai_row, action);

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
    fn commit_button_is_a_bar_and_ai_button_takes_last_3_cols() {
        let mut c = CommitInput::default();
        let mut term = Terminal::new(TestBackend::new(60, 4)).unwrap();
        term.draw(|f| c.render(f, area(), true)).unwrap();
        let buf = term.backend().buffer();
        // Empty message -> dimmed bar across all but the AI button's 3 cols.
        for x in 0..57 {
            assert_eq!(buf[(x, 3)].bg, Color::Rgb(40, 60, 80), "col {x}");
        }
        // Clicking the far left/right of the bar still hits the commit button.
        assert!(matches!(
            c.handle_mouse(click(0), area()),
            Some(Action::Error(_))
        ));
        assert!(matches!(
            c.handle_mouse(click(56), area()),
            Some(Action::Error(_))
        ));
        // The rightmost 3 columns are the AI generate button.
        assert!(matches!(
            c.handle_mouse(click(59), area()),
            Some(Action::GenerateCommitMessage)
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
            c.handle_mouse(click(56), area()),
            Some(Action::Commit(m)) if m == "x"
        ));
    }

    #[test]
    fn generating_shows_progress_and_ai_button_cancels() {
        let mut c = CommitInput::default();
        c.update(&Action::CommitMessageGenerating {
            provider: "codex (gpt-6-luna)".to_string(),
        });
        let mut term = Terminal::new(TestBackend::new(60, 4)).unwrap();
        term.draw(|f| c.render(f, area(), true)).unwrap();
        assert!(matches!(
            c.handle_mouse(click(59), area()),
            Some(Action::CancelCommitMessage)
        ));
        // Failure clears the busy state; success fills the message.
        c.update(&Action::CommitMessageGenerated("feat: x".to_string()));
        let mut term = Terminal::new(TestBackend::new(60, 4)).unwrap();
        term.draw(|f| c.render(f, area(), true)).unwrap();
        assert_eq!(c.message_text(), "feat: x");
        c.update(&Action::CommitMessageFailed);
        assert!(c.generating.is_none());
    }

    #[test]
    fn ctrl_char_is_not_inserted_as_text() {
        let mut c = CommitInput::default();
        c.handle_key(KeyEvent::new(
            KeyCode::Char('g'),
            ratatui::crossterm::event::KeyModifiers::CONTROL,
        ));
        assert_eq!(c.message_text(), "");
    }
}
