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
/// Commit button background, and its color while amending — a warning:
/// amend rewrites the last commit.
const COMMIT_BG: Color = Color::Rgb(0, 95, 160);
const AMEND_BG: Color = Color::Rgb(170, 95, 20);
/// Disabled commit button.
const DISABLED_FG: Color = Color::Rgb(110, 120, 130);
const DISABLED_BG: Color = Color::Rgb(40, 48, 58);
/// Amend toggle while off.
const TOGGLE_OFF_BG: Color = Color::Rgb(55, 62, 72);
/// Amend toggle label, right of the commit button.
const AMEND_LABEL: &str = " amend ";
/// Text columns to keep before the AI button shrinks to just its glyph.
const MIN_TEXT_W: u16 = 16;
/// Message lines the box grows to before it scrolls.
const MAX_LINES: usize = 6;

/// Commit message box with an AI button (`✦ AI`) inside on the right, plus
/// a "✓ Commit" button on the last row — enabled (and clickable) only while
/// the message has non-whitespace text — and an `amend` toggle beside it.
///
/// The message may span lines: Ctrl-J (or Alt/Shift-Enter where the
/// terminal reports them) inserts a newline, and the box grows up to
/// `MAX_LINES` text rows (`preferred_height`), then scrolls. Enter emits
/// `Action::Commit`; Esc returns focus via `FocusNext`.
///
/// Ctrl-A or the toggle switch amend mode (`ToggleAmend`): with an empty
/// box it asks for the last commit's message (`LoadLastCommitMessage`),
/// which is dropped again if amend is switched off unedited.
/// `CommitMessageGenerating` shows progress and turns the AI button into
/// `■ Stop` (cancel).
#[derive(Default)]
pub struct CommitInput {
    /// The message; lines are separated by `'\n'`.
    message: Vec<char>,
    /// Cursor position as a char index into `message`.
    cursor: usize,
    /// First message line shown once the box scrolls.
    scroll_row: usize,
    branch: String,
    /// Provider label while an AI generation is running (`■` cancels).
    generating: Option<String>,
    /// The next commit replaces the last one (`git commit --amend`).
    amend: bool,
    /// The last commit's message, as loaded into the empty box when amend
    /// was switched on.
    prefill: Option<String>,
    hitboxes: Hitboxes,
}

impl CommitInput {
    fn message_text(&self) -> String {
        self.message.iter().collect()
    }

    fn set_message(&mut self, text: &str) {
        self.message = text.chars().collect();
        self.cursor = self.message.len();
    }

    fn can_commit(&self) -> bool {
        self.message.iter().any(|c| !c.is_whitespace())
    }

    fn commit_action(&self) -> Action {
        Action::Commit {
            message: self.message_text(),
            amend: self.amend,
        }
    }

    fn insert(&mut self, c: char) {
        self.message.insert(self.cursor, c);
        self.cursor += 1;
    }

    fn line_count(&self) -> usize {
        self.message.iter().filter(|&&c| c == '\n').count() + 1
    }

    /// Text rows the box shows: one per line, up to `MAX_LINES`.
    fn visible_lines(&self) -> usize {
        self.line_count().min(MAX_LINES)
    }

    /// Start of the line holding char index `i`.
    fn line_start(&self, i: usize) -> usize {
        self.message[..i]
            .iter()
            .rposition(|&c| c == '\n')
            .map_or(0, |n| n + 1)
    }

    /// End (the `'\n'`, or the message end) of the line starting at `start`.
    fn line_end(&self, start: usize) -> usize {
        self.message[start..]
            .iter()
            .position(|&c| c == '\n')
            .map_or(self.message.len(), |n| start + n)
    }

    /// Screen columns taken by `message[range]`.
    fn cols(&self, range: std::ops::Range<usize>) -> usize {
        text::width(&self.message[range].iter().collect::<String>())
    }

    /// Up/Down: the same screen column on the neighbouring line (clamped to
    /// its end). Past the first/last line, the start/end of the message.
    fn move_vertically(&mut self, down: bool) {
        let start = self.line_start(self.cursor);
        let col = self.cols(start..self.cursor);
        let target = if down {
            let end = self.line_end(start);
            if end == self.message.len() {
                self.cursor = end;
                return;
            }
            end + 1
        } else {
            if start == 0 {
                self.cursor = 0;
                return;
            }
            self.line_start(start - 1)
        };
        let end = self.line_end(target);
        let mut i = target;
        while i < end && self.cols(target..i + 1) <= col {
            i += 1;
        }
        self.cursor = i;
    }

    /// Switch amend mode. Switching on with an empty box asks `App` for the
    /// last commit's message; switching off drops that message unless it
    /// was edited.
    fn toggle_amend(&mut self) -> Option<Action> {
        self.amend = !self.amend;
        if self.amend {
            return (!self.can_commit()).then_some(Action::LoadLastCommitMessage);
        }
        if self
            .prefill
            .take()
            .is_some_and(|p| p == self.message_text())
        {
            self.set_message("");
        }
        None
    }
}

impl Component for CommitInput {
    fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let newline = (ctrl && key.code == KeyCode::Char('j'))
            || (key.code == KeyCode::Enter
                && key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT));
        if newline {
            self.insert('\n');
            return None;
        }
        match key.code {
            KeyCode::Char('a') if ctrl => return Some(Action::ToggleAmend),
            // Other Ctrl-chars are commands (^g/^t AI, ^c quit) — never text.
            KeyCode::Char(c) if !ctrl => self.insert(c),
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
            KeyCode::Up => self.move_vertically(false),
            KeyCode::Down => self.move_vertically(true),
            KeyCode::Home => self.cursor = self.line_start(self.cursor),
            KeyCode::End => self.cursor = self.line_end(self.line_start(self.cursor)),
            KeyCode::Enter if self.can_commit() => return Some(self.commit_action()),
            _ => {}
        }
        None
    }

    /// Pasted text is inserted as-is, newlines included (CRLF/CR become
    /// LF, tabs 4 spaces); other control characters are dropped. It never
    /// commits.
    fn handle_paste(&mut self, text: &str) -> Option<Action> {
        let text = text
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\t', "    ");
        for c in text.chars().filter(|&c| c == '\n' || !c.is_control()) {
            self.insert(c);
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
                self.set_message("");
                self.amend = false;
                self.prefill = None;
            }
            Action::ToggleAmend => return self.toggle_amend(),
            // Only into a box still empty and still amending.
            Action::LastCommitMessageLoaded(text) if self.amend && !self.can_commit() => {
                self.set_message(text);
                self.prefill = Some(text.clone());
            }
            Action::CommitMessageGenerating { provider } => {
                self.generating = Some(provider.clone());
            }
            Action::CommitMessageGenerated(text) => {
                self.generating = None;
                self.set_message(text);
            }
            Action::CommitMessageFailed => self.generating = None,
            _ => {}
        }
        None
    }

    fn hints(&self) -> &'static str {
        "enter commit · ^j newline · ^a amend · ^g AI message · ^t AI provider · esc back"
    }

    fn preferred_height(&self) -> Option<u16> {
        // Border + text rows + border + commit button row.
        Some(self.visible_lines() as u16 + 3)
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

        // The AI button sits inside the box on the first row, right, as a
        // filled pill (`✦ AI`, `■ Stop` while generating; just the glyph
        // when narrow); the text gets the rest minus a one-column gap, on
        // every row, scrolled so the cursor stays visible.
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
        let rows = inner.height as usize;

        // Scroll rows so the cursor's line is shown, and columns (wide
        // chars take two) so the cursor cell stays inside the text area.
        let start = self.line_start(self.cursor);
        let cursor_row = self.message[..start].iter().filter(|&&c| c == '\n').count();
        if cursor_row < self.scroll_row {
            self.scroll_row = cursor_row;
        } else if rows > 0 && cursor_row >= self.scroll_row + rows {
            self.scroll_row = cursor_row + 1 - rows;
        }
        self.scroll_row = self
            .scroll_row
            .min(self.line_count().saturating_sub(rows.max(1)));
        let cursor_col = self.cols(start..self.cursor);
        let scroll = (cursor_col + 1).saturating_sub(text_w as usize);
        if self.message.is_empty() {
            let placeholder = match (&self.generating, self.amend, self.branch.is_empty()) {
                (Some(provider), ..) => format!("Generating with {provider}… (Esc to cancel)"),
                (None, true, _) => "Message (Enter to amend the last commit)".to_string(),
                (None, false, true) => "Message (Enter to commit)".to_string(),
                (None, false, false) => {
                    format!("Message (Enter to commit on \"{}\")", self.branch)
                }
            };
            let line = Line::from(Span::styled(
                placeholder,
                Style::default().fg(Color::DarkGray),
            ));
            let rect = Rect::new(inner.x, inner.y, text_w, inner.height.min(1));
            f.render_widget(Paragraph::new(line), rect);
        } else {
            let message = self.message_text();
            for (i, line) in message
                .split('\n')
                .skip(self.scroll_row)
                .take(rows)
                .enumerate()
            {
                let rect = Rect::new(inner.x, inner.y + i as u16, text_w, 1);
                let line = Line::from(text::slice(line, scroll, text_w as usize));
                f.render_widget(Paragraph::new(line), rect);
            }
        }

        let style = Style::default()
            .fg(Color::White)
            .bg(bg)
            .add_modifier(Modifier::BOLD);
        f.render_widget(Span::styled(label, style), ai_rect);
        self.hitboxes.push(ai_rect, action);

        // "✓ Commit" (or "✓ Amend") button, disabled (dim, not clickable)
        // until there is a message, then the amend toggle — dropped when
        // the row is too narrow (Ctrl-A still works).
        let toggle_w = AMEND_LABEL.len() as u16;
        let toggle_w = if button_row.width >= toggle_w + 12 {
            toggle_w
        } else {
            0
        };
        let commit_row = Rect::new(button_row.x, button_row.y, button_row.width - toggle_w, 1);
        let enabled = self.can_commit();
        let label = if self.amend {
            "✓ Amend"
        } else {
            "✓ Commit"
        };
        let label_w = label.chars().count() as u16;
        let left = commit_row.width.saturating_sub(label_w) / 2;
        let right = commit_row.width.saturating_sub(left + label_w);
        let style = match (enabled, self.amend) {
            (true, false) => Style::default().fg(Color::White).bg(COMMIT_BG),
            (true, true) => Style::default().fg(Color::White).bg(AMEND_BG),
            (false, _) => Style::default().fg(DISABLED_FG).bg(DISABLED_BG),
        };
        let bar = format!(
            "{}{}{}",
            " ".repeat(left as usize),
            label,
            " ".repeat(right as usize)
        );
        f.render_widget(Span::styled(bar, style), commit_row);
        if enabled {
            self.hitboxes.push(commit_row, self.commit_action());
        }
        if toggle_w > 0 {
            let rect = Rect::new(commit_row.x + commit_row.width, button_row.y, toggle_w, 1);
            let style = if self.amend {
                Style::default()
                    .fg(Color::White)
                    .bg(AMEND_BG)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(DISABLED_FG).bg(TOGGLE_OFF_BG)
            };
            f.render_widget(Span::styled(AMEND_LABEL, style), rect);
            self.hitboxes.push(rect, Action::ToggleAmend);
        }

        if focused && text_w > 0 && rows > 0 {
            let y = inner.y + (cursor_row - self.scroll_row) as u16;
            f.set_cursor_position((inner.x + (cursor_col - scroll) as u16, y));
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
        // Commit button cols 0..53, then the 7-col amend toggle.
        for x in 0..53 {
            assert_eq!(buf[(x, 3)].bg, DISABLED_BG, "col {x}");
        }
        assert!(c.handle_mouse(click(0, 3), area()).is_none());
        assert!(c.handle_mouse(click(52, 3), area()).is_none());
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
        for x in 0..53 {
            assert_eq!(buf[(x, 3)].bg, COMMIT_BG, "col {x}");
        }
        for col in [0, 52] {
            assert!(matches!(
                c.handle_mouse(click(col, 3), area()),
                Some(Action::Commit { message, amend: false }) if message == "x"
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

    fn press(c: &mut CommitInput, code: KeyCode, modifiers: KeyModifiers) -> Option<Action> {
        c.handle_key(KeyEvent::new(code, modifiers))
    }

    fn screen(c: &mut CommitInput, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(30, h)).unwrap();
        term.draw(|f| c.render(f, f.area(), true)).unwrap();
        let buf = term.backend().buffer();
        let mut out = String::new();
        for y in 0..h {
            for x in 0..30 {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn ctrl_j_and_alt_enter_insert_newlines_and_the_box_grows() {
        let mut c = CommitInput::default();
        assert_eq!(c.preferred_height(), Some(4));
        type_str(&mut c, "feat: x");
        assert!(press(&mut c, KeyCode::Char('j'), KeyModifiers::CONTROL).is_none());
        assert!(press(&mut c, KeyCode::Enter, KeyModifiers::ALT).is_none());
        type_str(&mut c, "body");
        assert_eq!(c.message_text(), "feat: x\n\nbody");
        assert_eq!(c.preferred_height(), Some(6));
        let s = screen(&mut c, 6);
        let lines: Vec<&str> = s.lines().collect();
        assert!(lines[1].contains("feat: x"), "{s}");
        assert!(lines[3].contains("body"), "{s}");
        assert!(matches!(
            press(&mut c, KeyCode::Enter, KeyModifiers::NONE),
            Some(Action::Commit { message, amend: false }) if message == "feat: x\n\nbody"
        ));
    }

    #[test]
    fn up_down_home_end_move_by_line() {
        let mut c = CommitInput::default();
        type_str(&mut c, "abcdef");
        press(&mut c, KeyCode::Char('j'), KeyModifiers::CONTROL);
        type_str(&mut c, "xy");
        // Up from col 2 keeps col 2; Down clamps to the shorter line.
        press(&mut c, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(c.cursor, 2);
        press(&mut c, KeyCode::End, KeyModifiers::NONE);
        assert_eq!(c.cursor, 6);
        press(&mut c, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(c.cursor, 9);
        press(&mut c, KeyCode::Home, KeyModifiers::NONE);
        assert_eq!(c.cursor, 7);
        // Past the first/last line: the message start/end.
        press(&mut c, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(c.cursor, 9);
        press(&mut c, KeyCode::Up, KeyModifiers::NONE);
        press(&mut c, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(c.cursor, 0);
    }

    #[test]
    fn up_down_keep_the_screen_column_across_wide_chars() {
        let mut c = CommitInput::default();
        type_str(&mut c, "日本語");
        press(&mut c, KeyCode::Char('j'), KeyModifiers::CONTROL);
        type_str(&mut c, "abcd");
        // Col 4 on "abcd" is after "日本" (2 chars, 4 columns).
        press(&mut c, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(c.cursor, 2);
    }

    #[test]
    fn long_messages_stop_growing_and_scroll_to_the_cursor() {
        let mut c = CommitInput::default();
        for i in 0..10 {
            if i > 0 {
                press(&mut c, KeyCode::Char('j'), KeyModifiers::CONTROL);
            }
            type_str(&mut c, &format!("line{i}"));
        }
        assert_eq!(c.preferred_height(), Some(MAX_LINES as u16 + 3));
        let s = screen(&mut c, 9);
        assert!(s.contains("line9") && s.contains("line4"), "{s}");
        assert!(!s.contains("line3"), "{s}");
        for _ in 0..9 {
            press(&mut c, KeyCode::Up, KeyModifiers::NONE);
        }
        let s = screen(&mut c, 9);
        assert!(s.contains("line0") && s.contains("line5"), "{s}");
        assert!(!s.contains("line6"), "{s}");
    }

    #[test]
    fn amend_prefills_an_empty_box_and_drops_it_unedited() {
        let mut c = CommitInput::default();
        assert!(matches!(
            press(&mut c, KeyCode::Char('a'), KeyModifiers::CONTROL),
            Some(Action::ToggleAmend)
        ));
        assert!(matches!(
            c.update(&Action::ToggleAmend),
            Some(Action::LoadLastCommitMessage)
        ));
        c.update(&Action::LastCommitMessageLoaded(
            "fix: y\n\nwhy".to_string(),
        ));
        assert_eq!(c.message_text(), "fix: y\n\nwhy");
        let s = screen(&mut c, 6);
        assert!(s.contains("✓ Amend") && s.contains("amend"), "{s}");
        assert!(matches!(
            press(&mut c, KeyCode::Enter, KeyModifiers::NONE),
            Some(Action::Commit { amend: true, .. })
        ));
        // Off again, untouched: the loaded message goes away.
        c.update(&Action::ToggleAmend);
        assert_eq!(c.message_text(), "");
        assert!(screen(&mut c, 4).contains("✓ Commit"));
    }

    #[test]
    fn amend_keeps_a_typed_or_edited_message() {
        let mut c = CommitInput::default();
        type_str(&mut c, "mine");
        // Something typed already: nothing to load, and it stays.
        assert!(c.update(&Action::ToggleAmend).is_none());
        c.update(&Action::LastCommitMessageLoaded("theirs".to_string()));
        assert_eq!(c.message_text(), "mine");
        c.update(&Action::ToggleAmend);
        assert_eq!(c.message_text(), "mine");
        // Loaded then edited: kept on toggle-off too.
        let mut c = CommitInput::default();
        c.update(&Action::ToggleAmend);
        c.update(&Action::LastCommitMessageLoaded("fix".to_string()));
        type_str(&mut c, "ed");
        c.update(&Action::ToggleAmend);
        assert_eq!(c.message_text(), "fixed");
    }

    #[test]
    fn toggle_click_and_commit_done_reset_amend() {
        let mut c = CommitInput::default();
        draw(&mut c);
        // The toggle is the last 7 columns of the button row.
        assert!(matches!(
            c.handle_mouse(click(55, 3), area()),
            Some(Action::ToggleAmend)
        ));
        c.update(&Action::ToggleAmend);
        assert!(c.amend);
        c.update(&Action::CommitDone);
        assert!(!c.amend && c.message.is_empty());
    }

    #[test]
    fn paste_inserts_lines_at_the_cursor_without_committing() {
        let mut c = CommitInput::default();
        type_str(&mut c, "ab");
        press(&mut c, KeyCode::Left, KeyModifiers::NONE);
        assert!(c.handle_paste("x\r\ny\rz\t\u{1b}").is_none());
        assert_eq!(c.message_text(), "ax\ny\nz    b");
        assert_eq!(c.cursor, 10);
    }
}
