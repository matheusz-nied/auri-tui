use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::action::Action;
use crate::buffer::{display_col, display_line, Buffer};
use crate::component::{Component, EditState};
use crate::fs::{FileDoc, MAX_FILE_BYTES};
use crate::highlight::Highlighter;
use crate::text;
use crate::theme;

use super::{border_style, SCROLL_LINES};

/// Columns moved per horizontal scroll step.
const H_STEP: usize = 4;

/// Key hints in view mode (status bar and help screen).
const VIEW_HINTS: &str = "j/k/g/G scroll · h/l sideways · i edit";
/// Status-bar hints while editing — short, so the bar's message ("Saved",
/// "changed on disk") still fits after them.
const EDIT_STATUS: &str = "esc stop editing · ctrl-z/y undo/redo";
/// Every editing key, for the help screen's "Editor" section.
pub const EDIT_HINTS: &str = "esc stop editing · shift+move select · ctrl/alt+←/→ word · \
     ctrl-z/y undo/redo · ctrl-x/c/v cut/copy/paste · ctrl-a select all · \
     tab/shift-tab indent/dedent";

/// File viewer and editor: line-number gutter + syntax-highlighted text,
/// vertical and horizontal scrolling. Shares the main pane with `DiffView`
/// — `App` renders whichever one owns it.
///
/// Modal: it opens read-only (`j`/`k` scroll); `i`/Enter switches to edit
/// mode, where keys are text, and Esc back. Edits live in the `Buffer`
/// until saved — nothing is written automatically. `Ctrl-S` (a global key,
/// live while `modified`) makes `App` broadcast `SaveFile`, answered here
/// with `WriteFile`; `App` reports back with `FileSaved`.
pub struct FileView {
    /// Open file (repo-relative); `None` before the first open.
    path: Option<String>,
    binary: bool,
    truncated: bool,
    /// Its exact text was loaded (UTF-8, whole file): edit mode allowed.
    editable: bool,
    /// The contents shown — the file's text, edited or not.
    buffer: Buffer,
    /// The text as last loaded or saved: what `DiscardEdits` goes back to.
    saved_text: String,
    /// Edit mode.
    inserting: bool,
    /// Text cut/copied here, for Ctrl-V (the terminal's own paste arrives
    /// as `handle_paste`).
    register: String,
    /// Colors for `buffer`, computed lazily as far as the view has scrolled
    /// and invalidated from the first edited line.
    highlighter: Highlighter,
    scroll_y: usize,
    scroll_x: usize,
    view_height: usize,
    /// Where the text (right of the gutter) was drawn last frame — mouse
    /// positions map through it.
    text_area: Rect,
    /// Widest line in columns, clamps horizontal scrolling.
    max_width: usize,
    /// Scroll the cursor into view on the next render.
    reveal: bool,
    /// A left-button drag is selecting.
    dragging: bool,
}

impl Default for FileView {
    fn default() -> Self {
        Self {
            path: None,
            binary: false,
            truncated: false,
            editable: false,
            buffer: Buffer::new(""),
            saved_text: String::new(),
            inserting: false,
            register: String::new(),
            highlighter: Highlighter::plain(),
            scroll_y: 0,
            scroll_x: 0,
            view_height: 0,
            text_area: Rect::default(),
            max_width: 0,
            reveal: false,
            dragging: false,
        }
    }
}

impl FileView {
    /// `fresh` = a newly opened file (scroll to top, view mode); a reload
    /// of the same file keeps the position (and edit mode), clamped.
    fn set_doc(&mut self, doc: &FileDoc, fresh: bool) {
        let cursor = self.buffer.cursor();
        let cursor_col = display_col(&self.buffer.lines()[cursor.line], cursor.col);
        self.binary = doc.binary;
        self.truncated = doc.truncated;
        self.editable = doc.source.is_some();
        // Not editable: show the display lines (already tab-expanded).
        self.saved_text = doc.source.clone().unwrap_or_else(|| doc.lines.join("\n"));
        self.path = Some(doc.path.clone());
        self.load_text();
        if fresh {
            self.inserting = false;
            self.scroll_y = 0;
            self.scroll_x = 0;
        } else {
            self.buffer.click(cursor.line, cursor_col, false);
        }
        if !self.editable {
            self.inserting = false;
        }
        self.clamp_scroll();
    }

    /// Start over from `saved_text`: new buffer, highlighting from the top.
    fn load_text(&mut self) {
        self.buffer = Buffer::new(&self.saved_text);
        self.max_width = self
            .buffer
            .lines()
            .iter()
            .map(|l| text::width(&display_line(l)))
            .max()
            .unwrap_or(0);
        let first = self.buffer.lines().first().map(String::as_str);
        self.highlighter = match &self.path {
            Some(p) => Highlighter::for_file(p, first),
            None => Highlighter::plain(),
        };
    }

    fn line_count(&self) -> usize {
        if self.path.is_none() || self.binary {
            0
        } else {
            self.buffer.line_count()
        }
    }

    fn clamp_scroll(&mut self) {
        self.scroll_y = self
            .scroll_y
            .min(self.line_count().saturating_sub(self.view_height));
        self.scroll_x = self.scroll_x.min(self.max_width);
    }

    fn modified(&self) -> bool {
        self.editable && self.buffer.modified()
    }

    /// `i`/Enter: edit mode, cursor on the top line if it scrolled away.
    fn start_editing(&mut self) -> Option<Action> {
        if self.path.is_none() || !self.editable {
            return self.edit_refusal();
        }
        self.inserting = true;
        let line = self.buffer.cursor().line;
        if line < self.scroll_y || line >= self.scroll_y + self.view_height.max(1) {
            self.buffer.click(self.scroll_y, self.scroll_x, false);
        }
        self.reveal = true;
        None
    }

    /// Why this file can't be edited, as an error — `None` if it can.
    fn edit_refusal(&self) -> Option<Action> {
        let path = self.path.as_ref()?;
        if !self.editable {
            let why = if self.binary {
                "binary file"
            } else if self.truncated {
                "file too large"
            } else {
                "not UTF-8 text"
            };
            return Some(Action::Error(format!("can't edit {path}: {why}")));
        }
        None
    }

    /// After an edit or cursor move: recolor from the first changed line,
    /// widen the scroll range, keep the cursor in view.
    fn after_edit(&mut self) {
        if let Some(line) = self.buffer.take_changed_from() {
            self.highlighter.invalidate_from(line);
        }
        let cursor = self.buffer.cursor();
        let width = text::width(&display_line(&self.buffer.lines()[cursor.line]));
        self.max_width = self.max_width.max(width);
        self.reveal = true;
    }

    fn edit_key(&mut self, key: KeyEvent) -> Option<Action> {
        let m = key.modifiers;
        let shift = m.contains(KeyModifiers::SHIFT);
        // AltGr arrives as Ctrl+Alt on some terminals: that's text.
        let ctrl = m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT);
        let word = m.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        let b = &mut self.buffer;
        let mut out = None;
        match key.code {
            KeyCode::Esc if b.selection().is_some() => b.clear_selection(),
            KeyCode::Esc => {
                self.inserting = false;
                return None;
            }
            KeyCode::Char(c) if ctrl => match c {
                'z' => {
                    b.undo();
                }
                'y' => {
                    b.redo();
                }
                'a' => b.select_all(),
                'c' => {
                    if let Some(text) = b.selected_text() {
                        self.register = text.clone();
                        out = Some(Action::CopyToClipboard(text));
                    }
                }
                'x' => {
                    if let Some(text) = b.cut() {
                        self.register = text.clone();
                        out = Some(Action::CopyToClipboard(text));
                    }
                }
                'v' if !self.register.is_empty() => b.insert(&self.register),
                _ => return None,
            },
            KeyCode::Char(c) => b.type_char(c),
            KeyCode::Enter => b.newline(),
            KeyCode::Backspace => b.backspace(),
            KeyCode::Delete => b.delete(),
            KeyCode::Tab => b.tab(),
            KeyCode::BackTab => b.dedent(),
            KeyCode::Left if word => b.word_left(shift),
            KeyCode::Right if word => b.word_right(shift),
            KeyCode::Left => b.left(shift),
            KeyCode::Right => b.right(shift),
            KeyCode::Up => b.vertical(-1, shift),
            KeyCode::Down => b.vertical(1, shift),
            KeyCode::Home if ctrl => b.doc_start(shift),
            KeyCode::End if ctrl => b.doc_end(shift),
            KeyCode::Home => b.home(shift),
            KeyCode::End => b.end(shift),
            KeyCode::PageUp | KeyCode::PageDown => {
                let page = self.view_height.max(1);
                let up = key.code == KeyCode::PageUp;
                b.vertical(if up { -(page as isize) } else { page as isize }, shift);
                self.scroll_y = if up {
                    self.scroll_y.saturating_sub(page)
                } else {
                    self.scroll_y + page
                };
            }
            _ => return None,
        }
        self.after_edit();
        out
    }

    /// The document line and display column under a screen position,
    /// clamped into the text area (a drag past an edge scrolls by one).
    fn text_pos(&self, col: u16, row: u16) -> (usize, usize) {
        let a = self.text_area;
        let line = if row < a.y {
            self.scroll_y.saturating_sub(1)
        } else if row >= a.bottom() {
            self.scroll_y + a.height as usize
        } else {
            self.scroll_y + (row - a.y) as usize
        };
        let col = self.scroll_x + col.saturating_sub(a.x) as usize;
        (line, col)
    }
}

impl Component for FileView {
    fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        if self.inserting {
            return self.edit_key(key);
        }
        match key.code {
            KeyCode::Char('i') | KeyCode::Enter => return self.start_editing(),
            KeyCode::Char('j') | KeyCode::Down => self.scroll_y += 1,
            KeyCode::Char('k') | KeyCode::Up => self.scroll_y = self.scroll_y.saturating_sub(1),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll_y += self.view_height,
            KeyCode::PageUp => self.scroll_y = self.scroll_y.saturating_sub(self.view_height),
            KeyCode::Char('g') | KeyCode::Home => self.scroll_y = 0,
            KeyCode::Char('G') | KeyCode::End => self.scroll_y = self.line_count(),
            KeyCode::Char('h') | KeyCode::Left => {
                self.scroll_x = self.scroll_x.saturating_sub(H_STEP)
            }
            KeyCode::Char('l') | KeyCode::Right => self.scroll_x += H_STEP,
            _ => {}
        }
        self.clamp_scroll();
        None
    }

    fn handle_paste(&mut self, text: &str) -> Option<Action> {
        if self.inserting {
            self.buffer.insert(text);
            self.after_edit();
        }
        None
    }

    fn handle_mouse(&mut self, ev: MouseEvent, _area: Rect) -> Option<Action> {
        // Shift+wheel scrolls sideways on terminals without horizontal
        // wheel events.
        let shift = ev.modifiers.contains(KeyModifiers::SHIFT);
        match ev.kind {
            MouseEventKind::ScrollDown if shift => self.scroll_x += H_STEP,
            MouseEventKind::ScrollUp if shift => {
                self.scroll_x = self.scroll_x.saturating_sub(H_STEP)
            }
            MouseEventKind::ScrollDown => self.scroll_y += SCROLL_LINES,
            MouseEventKind::ScrollUp => self.scroll_y = self.scroll_y.saturating_sub(SCROLL_LINES),
            MouseEventKind::ScrollRight => self.scroll_x += H_STEP,
            MouseEventKind::ScrollLeft => self.scroll_x = self.scroll_x.saturating_sub(H_STEP),
            // Edit mode: click places the cursor (shift extends), drag
            // selects.
            MouseEventKind::Down(MouseButton::Left) if self.inserting => {
                let (line, col) = self.text_pos(ev.column, ev.row);
                self.buffer.click(line, col, shift);
                self.dragging = true;
                self.after_edit();
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                let (line, col) = self.text_pos(ev.column, ev.row);
                self.buffer.click(line, col, true);
                self.after_edit();
            }
            MouseEventKind::Up(MouseButton::Left) => self.dragging = false,
            _ => {}
        }
        self.clamp_scroll();
        None
    }

    fn update(&mut self, action: &Action) -> Option<Action> {
        let ours = |p: &str| self.path.as_deref() == Some(p);
        match action {
            Action::FileLoaded(doc) => self.set_doc(doc, true),
            Action::FileReloaded(doc) if ours(&doc.path) => self.set_doc(doc, false),
            // From Changes/the diff: the view may not have been drawn yet,
            // so the cursor goes exactly where asked.
            Action::StartEditing { path, line } if ours(path) => {
                if let Some(refusal) = self.edit_refusal() {
                    return Some(refusal);
                }
                if let Some(line) = line {
                    // On the line's first non-blank character.
                    self.buffer.click(line.saturating_sub(1), 0, false);
                    self.buffer.home(false);
                    // A few lines of context above it.
                    self.scroll_y = line.saturating_sub(4);
                    self.scroll_x = 0;
                }
                self.inserting = true;
                self.reveal = true;
            }
            Action::SaveFile { then } => {
                if self.modified() {
                    return Some(Action::WriteFile {
                        path: self.path.clone()?,
                        contents: self.buffer.text(),
                        version: self.buffer.version(),
                        force: false,
                        then: then.clone(),
                    });
                }
                return then.as_ref().map(|t| (**t).clone());
            }
            Action::FileSaved { path, version } if ours(path) => {
                if *version == self.buffer.version() {
                    self.saved_text = self.buffer.text();
                }
                self.buffer.mark_saved(*version);
            }
            Action::DiscardEdits { .. } if self.modified() => {
                let cursor = self.buffer.cursor();
                let col = display_col(&self.buffer.lines()[cursor.line], cursor.col);
                self.load_text();
                self.buffer.click(cursor.line, col, false);
                self.inserting = false;
                self.clamp_scroll();
            }
            _ => {}
        }
        None
    }

    fn edit_state(&self) -> Option<EditState> {
        self.path.as_ref()?;
        Some(EditState {
            inserting: self.inserting,
            modified: self.modified(),
            selection: self.buffer.selection().is_some(),
        })
    }

    fn hints(&self) -> &'static str {
        if self.inserting {
            EDIT_STATUS
        } else {
            VIEW_HINTS
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        let mut title = vec![Span::raw(match &self.path {
            Some(p) if self.truncated => format!(
                " {} · first {} MiB ",
                p.replace('/', " › "),
                MAX_FILE_BYTES / (1024 * 1024)
            ),
            Some(p) => format!(" {} ", p.replace('/', " › ")),
            None => " File ".to_string(),
        })];
        if self.modified() {
            title.push(Span::styled("● ", theme::accent()));
        }
        if self.inserting {
            title.push(Span::styled("EDITING ", theme::accent()));
        }
        let block = Block::bordered()
            .title(Line::from(title))
            .border_style(border_style(focused));
        let inner = block.inner(area);
        self.view_height = inner.height.max(1) as usize;
        f.render_widget(block, area);

        let empty = !self.inserting && self.buffer.text().is_empty();
        let hint = match &self.path {
            None => Some("Select a file in the explorer"),
            Some(_) if self.binary => Some("Binary file — not shown"),
            Some(_) if empty && self.editable => Some("Empty file — i to edit"),
            Some(_) if empty => Some("Empty file"),
            Some(_) => None,
        };
        if let Some(hint) = hint {
            self.text_area = Rect::default();
            let y = inner.y + inner.height / 2;
            f.render_widget(
                Paragraph::new(hint).style(theme::muted()),
                Rect::new(inner.x, y, inner.width, 1.min(inner.height)),
            );
            return;
        }

        let gutter_w = self.buffer.line_count().to_string().len();
        let text_w = (inner.width as usize).saturating_sub(gutter_w + 1);
        self.text_area = Rect::new(
            inner.x + (gutter_w + 1).min(inner.width as usize) as u16,
            inner.y,
            text_w as u16,
            inner.height,
        );
        let cursor = self.buffer.cursor();
        let cursor_col = display_col(&self.buffer.lines()[cursor.line], cursor.col);
        if std::mem::take(&mut self.reveal) {
            let h = self.view_height;
            if cursor.line < self.scroll_y {
                self.scroll_y = cursor.line;
            } else if cursor.line >= self.scroll_y + h {
                self.scroll_y = cursor.line + 1 - h;
            }
            if cursor_col < self.scroll_x {
                self.scroll_x = cursor_col;
            } else if text_w > 0 && cursor_col >= self.scroll_x + text_w {
                self.scroll_x = cursor_col + 1 - text_w;
            }
        }
        self.clamp_scroll();

        let lines = self.buffer.lines();
        self.highlighter.advance(
            lines.iter().map(|l| display_line(l)),
            self.scroll_y + self.view_height,
        );
        let gutter = theme::faint();
        // Only the visible slice — files can have many thousands of lines.
        let rows: Vec<Line> = lines
            .iter()
            .enumerate()
            .skip(self.scroll_y)
            .take(self.view_height)
            .map(|(i, raw)| {
                let style = if self.inserting && i == cursor.line {
                    theme::muted()
                } else {
                    gutter
                };
                let mut spans = vec![Span::styled(format!("{:>gutter_w$} ", i + 1), style)];
                let shown = display_line(raw);
                spans.extend(self.highlighter.spans(i, &shown, self.scroll_x, text_w));
                Line::from(spans)
            })
            .collect();
        f.render_widget(Paragraph::new(rows), inner);

        // Selection: tint its cells (a selected line break shows as one
        // extra cell past the line's end).
        if let Some((s, e)) = self.buffer.selection() {
            let first = s.line.max(self.scroll_y);
            let last = e.line.min(self.scroll_y + self.view_height - 1);
            for (i, line) in lines.iter().enumerate().take(last + 1).skip(first) {
                let from = if i == s.line {
                    display_col(line, s.col)
                } else {
                    0
                };
                let to = if i == e.line {
                    display_col(line, e.col)
                } else {
                    display_col(line, line.len()) + 1
                };
                let y = self.text_area.y + (i - self.scroll_y) as u16;
                let (from, to) = (from.max(self.scroll_x), to.min(self.scroll_x + text_w));
                for col in from..to {
                    let x = self.text_area.x + (col - self.scroll_x) as u16;
                    if let Some(cell) = f.buffer_mut().cell_mut(Position::new(x, y)) {
                        cell.set_style(Style::default().bg(theme::TEXT_SEL_BG));
                    }
                }
            }
        }

        let visible = cursor.line >= self.scroll_y
            && cursor.line < self.scroll_y + self.view_height
            && cursor_col >= self.scroll_x
            && cursor_col < self.scroll_x + text_w;
        if focused && self.inserting && visible {
            f.set_cursor_position(Position::new(
                self.text_area.x + (cursor_col - self.scroll_x) as u16,
                self.text_area.y + (cursor.line - self.scroll_y) as u16,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use ratatui::Terminal;

    fn doc(path: &str, n: usize) -> FileDoc {
        FileDoc {
            path: path.to_string(),
            lines: (1..=n)
                .map(|i| format!("line {i} with some text"))
                .collect(),
            binary: false,
            truncated: false,
            source: None,
        }
    }

    /// An editable doc with exactly `text`.
    fn text_doc(path: &str, text: &str) -> FileDoc {
        crate::fs::decode_file(path, text.as_bytes(), false)
    }

    fn screen(v: &mut FileView, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| v.render(f, f.area(), true)).unwrap();
        let buf = term.backend().buffer();
        let mut out = String::new();
        for y in 0..h {
            for x in 0..w {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn typed(v: &mut FileView, s: &str) {
        for c in s.chars() {
            v.handle_key(key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn renders_gutter_and_breadcrumb() {
        let mut v = FileView::default();
        v.update(&Action::FileLoaded(doc("src/a.rs", 12)));
        let s = screen(&mut v, 40, 5);
        assert!(s.contains("src › a.rs"), "{s}");
        assert!(s.contains(" 1 line 1 with"), "{s}");
        assert!(s.contains(" 3 line 3 with"), "{s}");
    }

    #[test]
    fn scroll_is_clamped_and_reload_keeps_it() {
        let mut v = FileView::default();
        v.update(&Action::FileLoaded(doc("a", 20)));
        screen(&mut v, 40, 7); // view_height = 5
        v.handle_key(key(KeyCode::Char('G')));
        assert_eq!(v.scroll_y, 15);
        v.handle_key(key(KeyCode::Char('l')));
        assert_eq!(v.scroll_x, H_STEP);
        // Same file changed on disk: position kept (clamped to 10 lines).
        v.update(&Action::FileReloaded(doc("a", 10)));
        assert_eq!(v.scroll_y, 5);
        assert_eq!(v.scroll_x, H_STEP);
        // A reload for another path is ignored.
        v.update(&Action::FileReloaded(doc("b", 1)));
        assert_eq!(v.path.as_deref(), Some("a"));
        // A fresh open resets.
        v.update(&Action::FileLoaded(doc("b", 30)));
        assert_eq!((v.scroll_y, v.scroll_x), (0, 0));
    }

    #[test]
    fn source_code_is_highlighted() {
        let mut v = FileView::default();
        v.update(&Action::FileLoaded(text_doc("main.rs", "fn main() {}\n")));
        let mut term = Terminal::new(TestBackend::new(30, 3)).unwrap();
        term.draw(|f| v.render(f, f.area(), true)).unwrap();
        let buf = term.backend().buffer();
        // Row 1: border, "1 ", then `fn` (keyword) and `main` (function).
        let (kw, func) = (buf[(3, 1)].fg, buf[(6, 1)].fg);
        assert_eq!(buf[(3, 1)].symbol(), "f");
        assert!(matches!(kw, Color::Rgb(..)), "{kw:?}");
        assert_ne!(kw, func);
    }

    #[test]
    fn binary_and_empty_show_hints() {
        let mut v = FileView::default();
        assert!(screen(&mut v, 40, 5).contains("Select a file"));
        v.update(&Action::FileLoaded(FileDoc {
            binary: true,
            ..doc("x.png", 0)
        }));
        assert!(screen(&mut v, 40, 5).contains("Binary file"));
        v.update(&Action::FileLoaded(doc("e", 0)));
        assert!(screen(&mut v, 40, 5).contains("Empty file"));
        v.update(&Action::FileLoaded(text_doc("e", "")));
        assert!(screen(&mut v, 40, 5).contains("Empty file — i to edit"));
    }

    #[test]
    fn edit_mode_types_marks_modified_and_shows_the_cursor() {
        let mut v = FileView::default();
        v.update(&Action::FileLoaded(text_doc("a.txt", "hello\n")));
        assert_eq!(v.edit_state(), Some(EditState::default()));
        // View mode: letters are commands, not text.
        v.handle_key(key(KeyCode::Char('x')));
        assert!(!v.modified());

        assert!(v.handle_key(key(KeyCode::Char('i'))).is_none());
        v.handle_key(key(KeyCode::End));
        typed(&mut v, "!");
        let state = v.edit_state().unwrap();
        assert!(state.inserting && state.modified && !state.selection);

        let mut term = Terminal::new(TestBackend::new(30, 4)).unwrap();
        term.draw(|f| v.render(f, f.area(), true)).unwrap();
        let s = format!("{:?}", term.backend().buffer());
        assert!(s.contains("a.txt ● EDITING"), "{s}");
        assert!(s.contains("1 hello!"), "{s}");
        // Border + "1 " + "hello!" -> column 9.
        term.backend_mut().assert_cursor_position((9, 1));

        // Esc leaves edit mode; the change stays.
        v.handle_key(key(KeyCode::Esc));
        assert!(!v.edit_state().unwrap().inserting);
        assert!(v.modified());
    }

    #[test]
    fn save_round_trip_and_discard() {
        let mut v = FileView::default();
        v.update(&Action::FileLoaded(text_doc("a.txt", "one\r\n")));
        // Nothing to save: `then` passes straight through.
        let then = Some(Box::new(Action::Quit));
        assert!(matches!(
            v.update(&Action::SaveFile { then: then.clone() }),
            Some(Action::Quit)
        ));

        v.handle_key(key(KeyCode::Char('i')));
        typed(&mut v, "1");
        let Some(Action::WriteFile {
            path,
            contents,
            version,
            force,
            then: back,
        }) = v.update(&Action::SaveFile { then })
        else {
            panic!("expected WriteFile")
        };
        assert_eq!(
            (path.as_str(), contents.as_str(), force),
            ("a.txt", "1one\r\n", false)
        );
        assert!(matches!(back.as_deref(), Some(Action::Quit)));
        v.update(&Action::FileSaved {
            path: "a.txt".to_string(),
            version,
        });
        assert!(!v.modified());

        // Discard goes back to the saved text, not the loaded one.
        typed(&mut v, "2");
        assert!(v.modified());
        v.update(&Action::DiscardEdits { then: None });
        assert!(!v.modified());
        assert_eq!(v.buffer.text(), "1one\r\n");
        assert!(!v.inserting);
    }

    #[test]
    fn files_that_cannot_be_saved_safely_are_read_only() {
        let mut v = FileView::default();
        v.update(&Action::FileLoaded(FileDoc {
            truncated: true,
            ..doc("big.log", 3)
        }));
        assert!(matches!(
            v.handle_key(key(KeyCode::Char('i'))),
            Some(Action::Error(e)) if e.contains("too large")
        ));
        assert!(!v.inserting);
    }

    #[test]
    fn copy_cut_paste_and_undo() {
        let mut v = FileView::default();
        v.update(&Action::FileLoaded(text_doc("a.txt", "abc\n")));
        v.handle_key(key(KeyCode::Enter));
        v.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        v.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        assert!(v.edit_state().unwrap().selection);
        assert!(matches!(
            v.handle_key(ctrl('c')),
            Some(Action::CopyToClipboard(t)) if t == "ab"
        ));
        v.handle_key(key(KeyCode::End));
        v.handle_key(ctrl('v'));
        assert_eq!(v.buffer.text(), "abcab\n");
        // Ctrl-X without a selection cuts the line.
        assert!(matches!(
            v.handle_key(ctrl('x')),
            Some(Action::CopyToClipboard(t)) if t == "abcab\n"
        ));
        v.handle_key(ctrl('z'));
        v.handle_key(ctrl('z'));
        assert_eq!(v.buffer.text(), "abc\n");
        assert!(!v.modified());
        // A terminal paste goes in as text, newlines and all.
        v.handle_paste("x\ny");
        // Undo put the cursor back where the paste was made.
        assert_eq!(v.buffer.text(), "abcx\ny\n");
    }

    #[test]
    fn start_editing_puts_the_cursor_on_the_line() {
        let mut v = FileView::default();
        v.update(&Action::FileLoaded(doc("a.rs", 30)));
        screen(&mut v, 40, 7);
        let start = |path: &str, line| Action::StartEditing {
            path: path.to_string(),
            line,
        };
        // Not editable (no source): an error, still viewing.
        assert!(matches!(
            v.update(&start("a.rs", Some(1))),
            Some(Action::Error(_))
        ));
        v.update(&Action::FileLoaded(text_doc("a.rs", &"    x\n".repeat(30))));
        // Another file: ignored.
        assert!(v.update(&start("b.rs", Some(20))).is_none());
        assert!(!v.inserting);
        assert!(v.update(&start("a.rs", Some(20))).is_none());
        assert!(v.inserting);
        assert_eq!(v.buffer.cursor(), crate::buffer::Pos::new(19, 4));
        assert_eq!(v.scroll_y, 16);
    }

    #[test]
    fn reload_keeps_edit_mode_and_cursor() {
        let mut v = FileView::default();
        v.update(&Action::FileLoaded(text_doc("a.txt", "one\ntwo\n")));
        v.handle_key(key(KeyCode::Char('i')));
        v.handle_key(key(KeyCode::Down));
        v.handle_key(key(KeyCode::End));
        v.update(&Action::FileReloaded(text_doc("a.txt", "one\ntwo three\n")));
        assert!(v.inserting);
        assert_eq!(v.buffer.cursor(), crate::buffer::Pos::new(1, 3));
    }

    #[test]
    fn click_and_drag_select_in_edit_mode() {
        let mut v = FileView::default();
        v.update(&Action::FileLoaded(text_doc("a.txt", "hello world\nbye\n")));
        screen(&mut v, 30, 5);
        let ev = |kind, column, row| MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        // View mode: a click does nothing.
        v.handle_mouse(
            ev(MouseEventKind::Down(MouseButton::Left), 5, 1),
            Rect::default(),
        );
        assert_eq!(v.buffer.cursor(), crate::buffer::Pos::new(0, 0));

        v.handle_key(key(KeyCode::Char('i')));
        screen(&mut v, 30, 5);
        // Text starts at column 3 (border + "1 ").
        v.handle_mouse(
            ev(MouseEventKind::Down(MouseButton::Left), 9, 1),
            Rect::default(),
        );
        v.handle_mouse(
            ev(MouseEventKind::Drag(MouseButton::Left), 4, 2),
            Rect::default(),
        );
        v.handle_mouse(
            ev(MouseEventKind::Up(MouseButton::Left), 4, 2),
            Rect::default(),
        );
        assert_eq!(v.buffer.selected_text().as_deref(), Some("world\nb"));
    }
}
