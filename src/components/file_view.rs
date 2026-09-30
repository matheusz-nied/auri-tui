use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::action::Action;
use crate::component::Component;
use crate::fs::{FileDoc, MAX_FILE_BYTES};
use crate::highlight::Highlighter;
use crate::text;

use super::{border_style, SCROLL_LINES};

/// Columns moved per horizontal scroll step.
const H_STEP: usize = 4;

/// Read-only file viewer: line-number gutter + syntax-highlighted text,
/// vertical and horizontal scrolling. Shares the main pane with `DiffView`
/// — `App` renders whichever one owns it.
pub struct FileView {
    doc: Option<FileDoc>,
    /// Colors for `doc`, computed lazily as far as the view has scrolled.
    highlighter: Highlighter,
    scroll_y: usize,
    scroll_x: usize,
    view_height: usize,
    /// Longest line in chars, clamps horizontal scrolling.
    max_width: usize,
}

impl Default for FileView {
    fn default() -> Self {
        Self {
            doc: None,
            highlighter: Highlighter::plain(),
            scroll_y: 0,
            scroll_x: 0,
            view_height: 0,
            max_width: 0,
        }
    }
}

impl FileView {
    /// `fresh` = a newly opened file (scroll to top); a reload of the same
    /// file keeps the position, clamped.
    fn set_doc(&mut self, doc: FileDoc, fresh: bool) {
        self.max_width = doc.lines.iter().map(|l| text::width(l)).max().unwrap_or(0);
        if fresh {
            self.scroll_y = 0;
            self.scroll_x = 0;
        }
        // A reload means new contents: highlighting restarts from the top.
        self.highlighter = Highlighter::for_file(&doc.path, doc.lines.first().map(String::as_str));
        self.doc = Some(doc);
        self.clamp_scroll();
    }

    fn line_count(&self) -> usize {
        self.doc.as_ref().map_or(0, |d| d.lines.len())
    }

    fn clamp_scroll(&mut self) {
        self.scroll_y = self
            .scroll_y
            .min(self.line_count().saturating_sub(self.view_height));
        self.scroll_x = self.scroll_x.min(self.max_width);
    }
}

impl Component for FileView {
    fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
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
            _ => {}
        }
        self.clamp_scroll();
        None
    }

    fn update(&mut self, action: &Action) -> Option<Action> {
        match action {
            Action::FileLoaded(doc) => self.set_doc(doc.clone(), true),
            Action::FileReloaded(doc) if self.doc.as_ref().is_some_and(|d| d.path == doc.path) => {
                self.set_doc(doc.clone(), false)
            }
            _ => {}
        }
        None
    }

    fn hints(&self) -> &'static str {
        "j/k/g/G scroll · h/l sideways"
    }

    fn render(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        let title = match &self.doc {
            Some(d) if d.truncated => format!(
                " {} · first {} MiB ",
                d.path.replace('/', " › "),
                MAX_FILE_BYTES / (1024 * 1024)
            ),
            Some(d) => format!(" {} ", d.path.replace('/', " › ")),
            None => " File ".to_string(),
        };
        let block = Block::bordered()
            .title(title)
            .border_style(border_style(focused));
        let inner = block.inner(area);
        self.view_height = inner.height.max(1) as usize;
        self.clamp_scroll();
        f.render_widget(block, area);

        let hint = match &self.doc {
            None => Some("Select a file in the explorer"),
            Some(d) if d.binary => Some("Binary file — not shown"),
            Some(d) if d.lines.is_empty() => Some("Empty file"),
            Some(_) => None,
        };
        if let Some(hint) = hint {
            let y = inner.y + inner.height / 2;
            f.render_widget(
                Paragraph::new(hint).style(Style::default().fg(Color::DarkGray)),
                Rect::new(inner.x, y, inner.width, 1.min(inner.height)),
            );
            return;
        }
        let Some(doc) = &self.doc else {
            return;
        };
        let gutter_w = doc.lines.len().to_string().len();
        let text_w = (inner.width as usize).saturating_sub(gutter_w + 1);
        let gutter_style = Style::default().fg(Color::DarkGray);
        let highlighter = &mut self.highlighter;
        highlighter.advance(
            doc.lines.iter().map(String::as_str),
            self.scroll_y + self.view_height,
        );
        // Only the visible slice — files can have many thousands of lines.
        let lines: Vec<Line> = doc
            .lines
            .iter()
            .enumerate()
            .skip(self.scroll_y)
            .take(self.view_height)
            .map(|(i, text)| {
                let mut spans = vec![Span::styled(format!("{:>gutter_w$} ", i + 1), gutter_style)];
                spans.extend(highlighter.spans(i, text, self.scroll_x, text_w));
                Line::from(spans)
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn doc(path: &str, n: usize) -> FileDoc {
        FileDoc {
            path: path.to_string(),
            lines: (1..=n)
                .map(|i| format!("line {i} with some text"))
                .collect(),
            binary: false,
            truncated: false,
        }
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
        v.handle_key(KeyEvent::from(KeyCode::Char('G')));
        assert_eq!(v.scroll_y, 15);
        v.handle_key(KeyEvent::from(KeyCode::Char('l')));
        assert_eq!(v.scroll_x, H_STEP);
        // Same file changed on disk: position kept (clamped to 10 lines).
        v.update(&Action::FileReloaded(doc("a", 10)));
        assert_eq!(v.scroll_y, 5);
        assert_eq!(v.scroll_x, H_STEP);
        // A reload for another path is ignored.
        v.update(&Action::FileReloaded(doc("b", 1)));
        assert_eq!(v.doc.as_ref().unwrap().path, "a");
        // A fresh open resets.
        v.update(&Action::FileLoaded(doc("b", 30)));
        assert_eq!((v.scroll_y, v.scroll_x), (0, 0));
    }

    #[test]
    fn source_code_is_highlighted() {
        let mut v = FileView::default();
        v.update(&Action::FileLoaded(FileDoc {
            path: "main.rs".to_string(),
            lines: vec!["fn main() {}".to_string()],
            binary: false,
            truncated: false,
        }));
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
    }
}
