use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use ratatui::Frame;

use crate::action::Action;
use crate::component::Component;
use crate::text;

use super::border_style;
use super::hitbox::Hitboxes;

/// AI button background (idle / generating).
const AI_BG: Color = Color::Rgb(110, 60, 170);
const AI_STOP_BG: Color = Color::Rgb(160, 50, 70);
/// Text columns to keep before the AI button shrinks to just its glyph.
const MIN_TEXT_W: u16 = 16;

/// Single-line commit message box with an AI button (`✦ AI`) inside on the right,
/// plus a full-width "✓ Commit" button on the last row that is only enabled
/// (and clickable) while the message has non-whitespace text. When focused,
/// printable characters edit the message; Enter emits `Action::Commit`, Esc
/// returns focus via `FocusNext`. `CommitMessageGenerating` shows progress
/// and turns the AI button into `■ Stop` (cancel).
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

    fn can_commit(&self) -> bool {
        self.message.iter().any(|c| !c.is_whitespace())
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
            KeyCode::Enter if self.can_commit() => {
                return Some(Action::Commit(self.message_text()))
            }
            _ => {}
        }
        None
    }

    fn handle_mouse(&mut self, ev: MouseEvent, _area: Rect) -> Option<Action> {
        if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
            return self.hitboxes.hit(ev.column, ev.row);
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
        // Last row is the commit button; the rest is the rounded input box.
        let input = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
        let button_row = Rect::new(area.x, area.y + input.height, area.width, 1);

        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(border_style(focused));
        let inner = block.inner(input);
        f.render_widget(block, input);

        // The AI button sits inside the box on the right as a filled pill
        // (`✦ AI`, `■ Stop` while generating; just the glyph when narrow);
        // the text gets the rest minus a one-column gap, scrolled so the
        // cursor stays visible.
        let (glyph, word, action, bg) = if self.generating.is_some() {
            ("■", "Stop", Action::CancelCommitMessage, AI_STOP_BG)
        } else {
            ("✦", "AI", Action::GenerateCommitMessage, AI_BG)
        };
        let full = format!(" {glyph} {word} ");
        let full_w = full.chars().count() as u16;
        let label = if inner.width > full_w + MIN_TEXT_W {
            full
        } else {
            format!(" {glyph} ")
        };
        let ai_w = (label.chars().count() as u16).min(inner.width);
        let ai_rect = Rect::new(
            inner.x + inner.width - ai_w,
            inner.y,
            ai_w,
            inner.height.min(1),
        );
        let text_w = inner.width.saturating_sub(ai_w + 1);
        let text_rect = Rect::new(inner.x, inner.y, text_w, inner.height.min(1));

        // Scroll in columns (wide chars take two) so the cursor cell stays
        // inside the text area.
        let message = self.message_text();
        let cursor_col = text::width(&self.message[..self.cursor].iter().collect::<String>());
        let scroll = (cursor_col + 1).saturating_sub(text_w as usize);
        let line = if self.message.is_empty() {
            let placeholder = match (&self.generating, self.branch.is_empty()) {
                (Some(provider), _) => format!("Generating with {provider}… (Esc to cancel)"),
                (None, true) => "Message (Enter to commit)".to_string(),
                (None, false) => format!("Message (Enter to commit on \"{}\")", self.branch),
            };
            Line::from(Span::styled(
                placeholder,
                Style::default().fg(Color::DarkGray),
            ))
        } else {
            Line::from(text::slice(&message, scroll, text_w as usize))
        };
        f.render_widget(Paragraph::new(line), text_rect);

        let style = Style::default()
            .fg(Color::White)
            .bg(bg)
            .add_modifier(Modifier::BOLD);
        f.render_widget(Span::styled(label, style), ai_rect);
        self.hitboxes.push(ai_rect, action);

        // Full-width "✓ Commit" button; disabled (dim, not clickable) until
        // there is a message.
        let enabled = self.can_commit();
        let label = "✓ Commit";
        let label_w = label.chars().count() as u16;
        let left = button_row.width.saturating_sub(label_w) / 2;
        let right = button_row.width.saturating_sub(left + label_w);
        let style = if enabled {
            Style::default().fg(Color::White).bg(Color::Rgb(0, 95, 160))
        } else {
            Style::default()
                .fg(Color::Rgb(110, 120, 130))
                .bg(Color::Rgb(40, 48, 58))
        };
        let bar = format!(
            "{}{}{}",
            " ".repeat(left as usize),
            label,
            " ".repeat(right as usize)
        );
        f.render_widget(Span::styled(bar, style), button_row);
        if enabled {
            self.hitboxes
                .push(button_row, Action::Commit(self.message_text()));
        }

        if focused && text_w > 0 {
            f.set_cursor_position((text_rect.x + (cursor_col - scroll) as u16, text_rect.y));
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

    fn click(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::empty(),
        }
    }

    fn draw(c: &mut CommitInput) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(60, 4)).unwrap();
        term.draw(|f| c.render(f, area(), true)).unwrap();
        term
    }

    fn type_str(c: &mut CommitInput, s: &str) {
        for ch in s.chars() {
            c.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::empty()));
        }
    }

    #[test]
    fn ai_button_is_a_filled_pill_inside_the_input_box() {
        let mut c = CommitInput::default();
        let term = draw(&mut c);
        let buf = term.backend().buffer();
        // Inner row is y=1, cols 1..=58; " ✦ AI " fills its last 6 cols.
        assert_eq!(buf[(54, 1)].symbol(), "✦");
        assert_eq!(buf[(56, 1)].symbol(), "A");
        for col in 53..=58 {
            assert_eq!(buf[(col, 1)].bg, AI_BG, "col {col}");
            assert!(matches!(
                c.handle_mouse(click(col, 1), area()),
                Some(Action::GenerateCommitMessage)
            ));
        }
        assert!(c.handle_mouse(click(52, 1), area()).is_none());
    }

    #[test]
    fn narrow_box_shrinks_ai_button_to_its_glyph() {
        let mut c = CommitInput::default();
        let narrow = Rect::new(0, 0, 20, 4);
        let mut term = Terminal::new(TestBackend::new(20, 4)).unwrap();
        term.draw(|f| c.render(f, narrow, true)).unwrap();
        let buf = term.backend().buffer();
        // Inner cols 1..=18; " ✦ " takes 16..=18, still filled.
        assert_eq!(buf[(17, 1)].symbol(), "✦");
        assert_eq!(buf[(16, 1)].bg, AI_BG);
        assert!(c.handle_mouse(click(15, 1), narrow).is_none());
        assert!(matches!(
            c.handle_mouse(click(16, 1), narrow),
            Some(Action::GenerateCommitMessage)
        ));
    }

    #[test]
    fn commit_button_is_disabled_without_a_message() {
        let mut c = CommitInput::default();
        type_str(&mut c, "  ");
        let term = draw(&mut c);
        let buf = term.backend().buffer();
        for x in 0..60 {
            assert_eq!(buf[(x, 3)].bg, Color::Rgb(40, 48, 58), "col {x}");
        }
        assert!(c.handle_mouse(click(0, 3), area()).is_none());
        assert!(c.handle_mouse(click(59, 3), area()).is_none());
        assert!(c
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()))
            .is_none());
    }

    #[test]
    fn commit_button_spans_the_row_and_commits_with_a_message() {
        let mut c = CommitInput::default();
        type_str(&mut c, "x");
        let term = draw(&mut c);
        let buf = term.backend().buffer();
        for x in 0..60 {
            assert_eq!(buf[(x, 3)].bg, Color::Rgb(0, 95, 160), "col {x}");
        }
        for col in [0, 59] {
            assert!(matches!(
                c.handle_mouse(click(col, 3), area()),
                Some(Action::Commit(m)) if m == "x"
            ));
        }
    }

    #[test]
    fn long_message_scrolls_and_never_overlaps_the_ai_button() {
        let mut c = CommitInput::default();
        type_str(&mut c, &"a".repeat(80));
        let term = draw(&mut c);
        let buf = term.backend().buffer();
        // Text width = 58 - 6 - 1 gap = 51 (cols 1..=51); the end cursor
        // takes col 51, the gap (52) and button (53..) stay clear.
        assert_eq!(buf[(1, 1)].symbol(), "a");
        assert_eq!(buf[(50, 1)].symbol(), "a");
        assert_eq!(buf[(51, 1)].symbol(), " ");
        assert_eq!(buf[(52, 1)].symbol(), " ");
        assert_eq!(buf[(54, 1)].symbol(), "✦");
    }

    #[test]
    fn generating_shows_progress_and_ai_button_cancels() {
        let mut c = CommitInput::default();
        c.update(&Action::CommitMessageGenerating {
            provider: "codex (gpt-6-luna)".to_string(),
        });
        let term = draw(&mut c);
        // " ■ Stop " takes the last 8 inner cols (51..=58).
        let buf = term.backend().buffer();
        assert_eq!(buf[(52, 1)].symbol(), "■");
        assert_eq!(buf[(51, 1)].bg, AI_STOP_BG);
        assert!(matches!(
            c.handle_mouse(click(51, 1), area()),
            Some(Action::CancelCommitMessage)
        ));
        // Success fills the message; failure clears the busy state.
        c.update(&Action::CommitMessageGenerated("feat: x".to_string()));
        draw(&mut c);
        assert_eq!(c.message_text(), "feat: x");
        c.update(&Action::CommitMessageFailed);
        assert!(c.generating.is_none());
    }

    #[test]
    fn wide_chars_keep_the_cursor_on_screen() {
        let mut c = CommitInput::default();
        // 30 CJK chars = 60 columns, wider than the 51-column text area.
        type_str(&mut c, &"日".repeat(30));
        let mut term = draw(&mut c);
        // Cursor sits right after the last char, inside the text area.
        term.backend_mut().assert_cursor_position((51, 1));
        let buf = term.backend().buffer();
        assert_eq!(buf[(52, 1)].symbol(), " ", "gap before the AI button");
        assert_eq!(buf[(54, 1)].symbol(), "✦");
        // Home: no scroll, cursor at the first column.
        c.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::empty()));
        c.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::empty()));
        let mut term = draw(&mut c);
        term.backend_mut().assert_cursor_position((3, 1));
        assert_eq!(term.backend().buffer()[(1, 1)].symbol(), "日");
    }

    #[test]
    fn ctrl_char_is_not_inserted_as_text() {
        let mut c = CommitInput::default();
        c.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
        assert_eq!(c.message_text(), "");
    }
}
