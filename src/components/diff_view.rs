use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::action::Action;
use crate::component::Component;
use crate::git::{CellKind, DiffCell, DiffDoc, FileChange, RowKind};

use super::border_style;

const REMOVED_BG: Color = Color::Rgb(60, 20, 20);
const ADDED_BG: Color = Color::Rgb(20, 50, 20);
const FILLER_BG: Color = Color::Rgb(30, 30, 30);

/// Side-by-side diff viewer. Each half shows a right-aligned line-number
/// gutter plus text; removed cells are red, added cells green, and empty
/// filler cells are hatched.
pub struct DiffView {
    doc: Option<DiffDoc>,
    /// The file the current `doc` belongs to; cleared when it disappears
    /// from the status.
    file: Option<FileChange>,
    scroll_y: usize,
    scroll_x: usize,
    view_height: usize,
    /// Widest cell text, used to clamp horizontal scrolling.
    max_width: usize,
}

impl Default for DiffView {
    fn default() -> Self {
        Self {
            doc: None,
            file: None,
            scroll_y: 0,
            scroll_x: 0,
            view_height: 1,
            max_width: 0,
        }
    }
}

impl DiffView {
    /// `fresh` = a newly selected file: scroll resets to the first changed
    /// block. A reload of the same file keeps the scroll position (clamped).
    fn set_doc(&mut self, doc: DiffDoc, fresh: bool) {
        self.max_width = doc
            .rows
            .iter()
            .flat_map(|r| [r.left.as_ref(), r.right.as_ref()])
            .flatten()
            .map(|c| c.text.chars().count())
            .max()
            .unwrap_or(0);
        if fresh {
            self.scroll_y = doc
                .rows
                .iter()
                .position(|r| r.kind == RowKind::Changed)
                .unwrap_or(0);
            self.scroll_x = 0;
        }
        self.doc = Some(doc);
        self.clamp_scroll();
    }

    fn clear(&mut self) {
        self.doc = None;
        self.file = None;
        self.scroll_y = 0;
        self.scroll_x = 0;
    }

    fn clamp_scroll(&mut self) {
        let max_y = self
            .doc
            .as_ref()
            .map(|d| d.rows.len().saturating_sub(self.view_height))
            .unwrap_or(0);
        self.scroll_y = self.scroll_y.min(max_y);
        self.scroll_x = self.scroll_x.min(self.max_width);
    }

    /// First row index of each contiguous run of `Changed` rows.
    fn change_blocks(&self) -> Vec<usize> {
        let Some(doc) = &self.doc else {
            return Vec::new();
        };
        let mut starts = Vec::new();
        for (i, row) in doc.rows.iter().enumerate() {
            if row.kind == RowKind::Changed && (i == 0 || doc.rows[i - 1].kind != RowKind::Changed)
            {
                starts.push(i);
            }
        }
        starts
    }

    fn jump_to_change(&mut self, forward: bool) {
        let blocks = self.change_blocks();
        let target = if forward {
            blocks.iter().find(|&&i| i > self.scroll_y).copied()
        } else {
            blocks.iter().rev().find(|&&i| i < self.scroll_y).copied()
        };
        if let Some(i) = target {
            self.scroll_y = i;
            self.clamp_scroll();
        }
    }
}

/// Build one pane line from a cell: right-aligned gutter + cropped/padded
/// text. `None` cells become hatched fillers.
fn cell_line(
    cell: Option<&DiffCell>,
    gutter_w: usize,
    text_w: usize,
    scroll_x: usize,
) -> Line<'static> {
    let (gutter, raw_text, style) = match cell {
        Some(c) => {
            let style = match c.kind {
                CellKind::Removed => Style::default().bg(REMOVED_BG),
                CellKind::Added => Style::default().bg(ADDED_BG),
                CellKind::Context => Style::default(),
            };
            (format!("{:>gutter_w$} ", c.line_no), c.text.clone(), style)
        }
        None => (
            " ".repeat(gutter_w + 1),
            "░".repeat(text_w + scroll_x),
            Style::default().fg(Color::DarkGray).bg(FILLER_BG),
        ),
    };
    let text: String = raw_text.chars().skip(scroll_x).take(text_w).collect();
    let pad = text_w.saturating_sub(text.chars().count());
    Line::from(vec![
        Span::styled(gutter, Style::default().fg(Color::DarkGray)),
        Span::styled(format!("{text}{:pad$}", ""), style),
    ])
}

fn hunk_header_line(text: &str, gutter_w: usize, text_w: usize) -> Line<'static> {
    let style = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::ITALIC);
    let text: String = text.chars().take(text_w).collect();
    let pad = text_w.saturating_sub(text.chars().count());
    Line::from(vec![
        Span::styled(" ".repeat(gutter_w + 1), style),
        Span::styled(format!("{text}{:pad$}", ""), style),
    ])
}

impl Component for DiffView {
    fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.scroll_y += 1,
            KeyCode::Char('k') | KeyCode::Up => self.scroll_y = self.scroll_y.saturating_sub(1),
            KeyCode::PageDown => self.scroll_y += self.view_height,
            KeyCode::PageUp => self.scroll_y = self.scroll_y.saturating_sub(self.view_height),
            KeyCode::Char('g') | KeyCode::Home => self.scroll_y = 0,
            KeyCode::Char('G') | KeyCode::End => {
                if let Some(doc) = &self.doc {
                    self.scroll_y = doc.rows.len();
                }
            }
            KeyCode::Char('n') => self.jump_to_change(true),
            KeyCode::Char('N') => self.jump_to_change(false),
            KeyCode::Char('h') | KeyCode::Left => self.scroll_x = self.scroll_x.saturating_sub(4),
            KeyCode::Char('l') | KeyCode::Right => self.scroll_x += 4,
            _ => {}
        }
        self.clamp_scroll();
        None
    }

    fn handle_mouse(&mut self, ev: MouseEvent, _area: Rect) -> Option<Action> {
        match ev.kind {
            MouseEventKind::ScrollDown => self.scroll_y += 3,
            MouseEventKind::ScrollUp => self.scroll_y = self.scroll_y.saturating_sub(3),
            _ => {}
        }
        self.clamp_scroll();
        None
    }

    fn update(&mut self, action: &Action) -> Option<Action> {
        match action {
            Action::SelectFile(file) => {
                self.file = Some(file.clone());
                self.doc = None;
            }
            Action::DiffLoaded(doc) => self.set_doc(doc.clone(), true),
            Action::DiffReloaded(doc) => self.set_doc(doc.clone(), false),
            Action::StatusLoaded(files) => {
                if let Some(file) = &self.file {
                    let still_there = files
                        .iter()
                        .any(|f| f.path == file.path && f.section == file.section);
                    if !still_there {
                        self.clear();
                    }
                }
            }
            _ => {}
        }
        None
    }

    fn render(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        let breadcrumb = self
            .doc
            .as_ref()
            .map(|d| d.path.replace('/', " › "))
            .unwrap_or_else(|| "Diff".to_string());
        let block = Block::bordered()
            .title(format!(" {breadcrumb} "))
            .border_style(border_style(focused));
        let inner = block.inner(area);
        self.view_height = inner.height.max(1) as usize;
        f.render_widget(block, area);

        let Some(doc) = &self.doc else {
            let hint = Paragraph::new("Select a file to view its diff")
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(hint, centered_hint(inner));
            return;
        };
        if doc.binary {
            let msg = Paragraph::new("Binary file").style(Style::default().fg(Color::DarkGray));
            f.render_widget(msg, centered_hint(inner));
            return;
        }

        let max_line_no = doc
            .rows
            .iter()
            .flat_map(|r| [r.left.as_ref(), r.right.as_ref()])
            .flatten()
            .map(|c| c.line_no)
            .max()
            .unwrap_or(1);
        let gutter_w = max_line_no.to_string().len().max(1);
        let halves = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(inner);
        let text_w = |half: Rect| (half.width as usize).saturating_sub(gutter_w + 1);

        // Only build lines for the visible slice — docs can be thousands of
        // rows and Paragraph::scroll would overflow at u16::MAX anyway.
        let start = self.scroll_y.min(doc.rows.len());
        let end = (start + self.view_height).min(doc.rows.len());
        let mut left_lines = Vec::with_capacity(end - start);
        let mut right_lines = Vec::with_capacity(end - start);
        for row in &doc.rows[start..end] {
            match row.kind {
                RowKind::HunkHeader => {
                    let text = row.left.as_ref().map(|c| c.text.as_str()).unwrap_or("");
                    left_lines.push(hunk_header_line(text, gutter_w, text_w(halves[0])));
                    right_lines.push(hunk_header_line(text, gutter_w, text_w(halves[1])));
                }
                _ => {
                    left_lines.push(cell_line(
                        row.left.as_ref(),
                        gutter_w,
                        text_w(halves[0]),
                        self.scroll_x,
                    ));
                    right_lines.push(cell_line(
                        row.right.as_ref(),
                        gutter_w,
                        text_w(halves[1]),
                        self.scroll_x,
                    ));
                }
            }
        }

        f.render_widget(Paragraph::new(left_lines), halves[0]);
        f.render_widget(Paragraph::new(right_lines), halves[1]);
    }
}

fn centered_hint(inner: Rect) -> Rect {
    let y = inner.y + inner.height / 2;
    Rect::new(inner.x, y, inner.width, 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{DiffRow, Section};

    fn cell(line_no: usize, kind: CellKind) -> Option<DiffCell> {
        Some(DiffCell {
            line_no,
            // Long enough that scroll_x=8 stays below max_width in the test.
            text: format!("content of line {line_no} — some more text"),
            kind,
        })
    }

    /// `n` rows: first two are Changed, the rest Context.
    fn doc(n: usize) -> DiffDoc {
        let rows = (0..n)
            .map(|i| {
                if i < 2 {
                    DiffRow {
                        left: cell(i + 1, CellKind::Removed),
                        right: cell(i + 1, CellKind::Added),
                        kind: RowKind::Changed,
                    }
                } else {
                    DiffRow {
                        left: cell(i + 1, CellKind::Context),
                        right: cell(i + 1, CellKind::Context),
                        kind: RowKind::Context,
                    }
                }
            })
            .collect();
        DiffDoc {
            path: "f.rs".to_string(),
            rows,
            binary: false,
        }
    }

    fn file() -> FileChange {
        FileChange {
            path: "f.rs".to_string(),
            orig_path: None,
            section: Section::Unstaged,
            code: 'M',
        }
    }

    #[test]
    fn fresh_diff_resets_scroll_reload_preserves_it() {
        let mut v = DiffView {
            view_height: 5,
            ..Default::default()
        };

        v.update(&Action::SelectFile(file()));
        v.update(&Action::DiffLoaded(doc(20)));
        // Fresh load lands on the first changed block.
        assert_eq!(v.scroll_y, 0);

        // Simulate the user scrolling away.
        v.scroll_y = 7;
        v.scroll_x = 8;

        // A reload of the same file keeps the position (clamped to a
        // shorter doc here: 12 rows - 5 visible = max scroll 7).
        v.update(&Action::DiffReloaded(doc(12)));
        assert_eq!(v.scroll_y, 7);
        assert_eq!(v.scroll_x, 8);

        // Shorter still: clamps to the bottom.
        v.update(&Action::DiffReloaded(doc(8)));
        assert_eq!(v.scroll_y, 3);

        // A new file resets to the first change again.
        v.update(&Action::SelectFile(file()));
        v.update(&Action::DiffLoaded(doc(20)));
        assert_eq!(v.scroll_y, 0);
        assert_eq!(v.scroll_x, 0);
    }
}
