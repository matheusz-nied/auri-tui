use std::sync::Arc;

use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::action::Action;
use crate::component::Component;
use crate::git::{CellKind, DiffCell, DiffDoc, DiffRow, DiffSource, RowKind};
use crate::highlight::Highlighter;
use crate::text;

use super::hitbox::{button_span, Hitboxes};
use super::{border_style, SCROLL_LINES};

const REMOVED_BG: Color = Color::Rgb(60, 20, 20);
const ADDED_BG: Color = Color::Rgb(20, 50, 20);
const FILLER_BG: Color = Color::Rgb(30, 30, 30);

/// Side-by-side diff viewer. Each half shows a right-aligned line-number
/// gutter plus syntax-highlighted text; removed cells get a red background,
/// added cells green, and empty filler cells are hatched.
pub struct DiffView {
    /// Shared with `App` (it compares reloads against it), never copied.
    doc: Option<Arc<DiffDoc>>,
    /// One highlighter per side: the old side's cells in order are the old
    /// file (diffs have full context), the new side's the new file.
    highlighters: [Highlighter; 2],
    /// Per row, the index of each side's cell within that side's sequence.
    side_index: Vec<[Option<usize>; 2]>,
    /// What the current `doc` belongs to; a working-tree source is cleared
    /// when it disappears from the status, a commit source never is.
    source: Option<DiffSource>,
    scroll_y: usize,
    scroll_x: usize,
    view_height: usize,
    /// Widest cell text, used to clamp horizontal scrolling.
    max_width: usize,
    hitboxes: Hitboxes,
}

impl Default for DiffView {
    fn default() -> Self {
        Self {
            doc: None,
            highlighters: [Highlighter::plain(), Highlighter::plain()],
            side_index: Vec::new(),
            source: None,
            scroll_y: 0,
            scroll_x: 0,
            view_height: 1,
            max_width: 0,
            hitboxes: Hitboxes::default(),
        }
    }
}

impl DiffView {
    /// `o`: edit the shown file. A working-tree diff opens at the first
    /// changed line in view (else the top line), on the new side — for a
    /// removed-only row, the next new-side line. A commit diff's line
    /// numbers are the commit's, not the worktree's, so it opens at the top.
    fn edit_action(&self) -> Option<Action> {
        let (path, line) = match self.source.as_ref()? {
            DiffSource::Working(file) => {
                let line = self.doc.as_ref().and_then(|doc| {
                    let rows = &doc.rows;
                    let end = (self.scroll_y + self.view_height).min(rows.len());
                    let start = (self.scroll_y..end)
                        .find(|&i| rows[i].kind == RowKind::Changed)
                        .unwrap_or(self.scroll_y);
                    rows.iter()
                        .skip(start)
                        .find_map(|r| side_cell(r, 1).map(|c| c.line_no))
                });
                (file.path.clone(), line)
            }
            DiffSource::Commit { file, .. } => (file.path.clone(), None),
        };
        Some(Action::OpenInEditor { path, line })
    }

    /// `fresh` = a newly selected file: scroll resets to the first changed
    /// block. A reload of the same file keeps the scroll position (clamped).
    fn set_doc(&mut self, doc: Arc<DiffDoc>, fresh: bool) {
        self.max_width = doc
            .rows
            .iter()
            .flat_map(|r| [r.left.as_ref(), r.right.as_ref()])
            .flatten()
            .map(|c| text::width(&c.text))
            .max()
            .unwrap_or(0);
        let mut counts = [0; 2];
        self.side_index = doc
            .rows
            .iter()
            .map(|row| {
                let mut idx = [None; 2];
                for (side, n) in counts.iter_mut().enumerate() {
                    if side_cell(row, side).is_some() {
                        idx[side] = Some(*n);
                        *n += 1;
                    }
                }
                idx
            })
            .collect();
        // New contents: highlighting restarts from the top of each side.
        self.highlighters = [0, 1].map(|side| {
            let first = doc.rows.iter().find_map(|r| side_cell(r, side));
            Highlighter::for_file(&doc.path, first.map(|c| c.text.as_str()))
        });
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
        self.side_index.clear();
        self.source = None;
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

/// A row's cell on one side (0 = old/left, 1 = new/right). Hunk headers
/// carry their text in `left` but are not file lines.
fn side_cell(row: &DiffRow, side: usize) -> Option<&DiffCell> {
    if row.kind == RowKind::HunkHeader {
        return None;
    }
    if side == 0 {
        row.left.as_ref()
    } else {
        row.right.as_ref()
    }
}

/// Build one pane line from a cell: right-aligned gutter + highlighted,
/// cropped and padded text on the cell's added/removed background. `None`
/// cells become hatched fillers.
fn cell_line(
    cell: Option<(&DiffCell, usize)>,
    highlighter: &Highlighter,
    gutter_w: usize,
    text_w: usize,
    scroll_x: usize,
) -> Line<'static> {
    let gutter_style = Style::default().fg(Color::DarkGray);
    let Some((c, idx)) = cell else {
        let filler: String = "░".repeat(text_w);
        return Line::from(vec![
            Span::styled(" ".repeat(gutter_w + 1), gutter_style),
            Span::styled(filler, Style::default().fg(Color::DarkGray).bg(FILLER_BG)),
        ]);
    };
    let bg = match c.kind {
        CellKind::Removed => Style::default().bg(REMOVED_BG),
        CellKind::Added => Style::default().bg(ADDED_BG),
        CellKind::Context => Style::default(),
    };
    let mut spans = vec![Span::styled(
        format!("{:>gutter_w$} ", c.line_no),
        gutter_style,
    )];
    let mut used = 0;
    for span in highlighter.spans(idx, &c.text, scroll_x, text_w) {
        used += text::width(&span.content);
        spans.push(span.patch_style(bg));
    }
    spans.push(Span::styled(" ".repeat(text_w.saturating_sub(used)), bg));
    Line::from(spans)
}

fn hunk_header_line(text: &str, gutter_w: usize, text_w: usize) -> Line<'static> {
    let style = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::ITALIC);
    let text = text::truncate(text, text_w);
    let pad = text_w.saturating_sub(text::width(&text));
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
            KeyCode::Char('o') => return self.edit_action(),
            _ => {}
        }
        self.clamp_scroll();
        None
    }

    fn handle_mouse(&mut self, ev: MouseEvent, _area: Rect) -> Option<Action> {
        if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
            if let Some(action) = self.hitboxes.hit(ev.column, ev.row) {
                return Some(action);
            }
        }
        match ev.kind {
            MouseEventKind::ScrollDown => self.scroll_y += SCROLL_LINES,
            MouseEventKind::ScrollUp => self.scroll_y = self.scroll_y.saturating_sub(SCROLL_LINES),
            _ => {}
        }
        self.clamp_scroll();
        None
    }

    fn update(&mut self, action: &Action) -> Option<Action> {
        match action {
            Action::SelectFile(file) => {
                self.source = Some(DiffSource::Working(file.clone()));
                self.doc = None;
            }
            Action::SelectCommitFile { commit, file } => {
                self.source = Some(DiffSource::Commit {
                    hash: commit.hash.clone(),
                    file: file.clone(),
                });
                self.doc = None;
            }
            Action::DiffLoaded(doc) => self.set_doc(doc.clone(), true),
            Action::DiffReloaded(doc) => self.set_doc(doc.clone(), false),
            Action::DiffNextChange => self.jump_to_change(true),
            Action::DiffPrevChange => self.jump_to_change(false),
            Action::StatusLoaded(files) => {
                // Working-tree diffs follow the status; commit diffs are
                // immutable and stay.
                if let Some(DiffSource::Working(file)) = &self.source {
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

    fn hints(&self) -> &'static str {
        "n/N next/prev change · h/l scroll · o edit"
    }

    fn render(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        self.hitboxes.clear();
        // Commit diffs get the short hash as a breadcrumb prefix.
        let breadcrumb = match &self.source {
            Some(DiffSource::Commit { hash, file }) => {
                let short = &hash[..hash.len().min(7)];
                format!("{short} · {}", file.path.replace('/', " › "))
            }
            _ => self
                .doc
                .as_ref()
                .map(|d| d.path.replace('/', " › "))
                .unwrap_or_else(|| "Diff".to_string()),
        };
        let block = Block::bordered()
            .title(format!(" {breadcrumb} "))
            .border_style(border_style(focused));
        let inner = block.inner(area);
        self.view_height = inner.height.max(1) as usize;
        self.clamp_scroll();
        f.render_widget(block, area);

        // Toolbar drawn over the top border, right-aligned with a 1-col gap
        // from the corner: ↑/↓ jump between changes (only with a doc), ↻
        // refreshes.
        let has_doc = self.doc.is_some();
        let toolbar: &[(&str, Action)] = if has_doc {
            &[
                ("↑", Action::DiffPrevChange),
                ("↓", Action::DiffNextChange),
                ("↻", Action::Refresh),
            ]
        } else {
            &[("↻", Action::Refresh)]
        };
        let mut bx = area.x + area.width.saturating_sub(2);
        for (glyph, action) in toolbar.iter().rev() {
            if bx < area.x + 3 {
                break;
            }
            bx -= 3;
            let rect = Rect::new(bx, area.y, 3, 1);
            f.render_widget(
                button_span(glyph, Style::default().fg(Color::DarkGray)),
                rect,
            );
            self.hitboxes.push(rect, action.clone());
        }

        let Some(doc) = &self.doc else {
            // A selected file whose diff the git worker is still loading.
            let text = if self.source.is_some() {
                "Loading…"
            } else {
                "Select a file to view its diff"
            };
            let hint = Paragraph::new(text).style(Style::default().fg(Color::DarkGray));
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
        let visible_index = &self.side_index[start..end];
        for (side, highlighter) in self.highlighters.iter_mut().enumerate() {
            let upto = visible_index.iter().filter_map(|idx| idx[side]).max();
            if let Some(upto) = upto {
                let texts = doc.rows.iter().filter_map(|r| side_cell(r, side));
                highlighter.advance(texts.map(|c| c.text.as_str()), upto + 1);
            }
        }
        let mut left_lines = Vec::with_capacity(end - start);
        let mut right_lines = Vec::with_capacity(end - start);
        for (row, idx) in doc.rows[start..end].iter().zip(visible_index) {
            match row.kind {
                RowKind::HunkHeader => {
                    let text = row.left.as_ref().map(|c| c.text.as_str()).unwrap_or("");
                    left_lines.push(hunk_header_line(text, gutter_w, text_w(halves[0])));
                    right_lines.push(hunk_header_line(text, gutter_w, text_w(halves[1])));
                }
                _ => {
                    left_lines.push(cell_line(
                        row.left.as_ref().zip(idx[0]),
                        &self.highlighters[0],
                        gutter_w,
                        text_w(halves[0]),
                        self.scroll_x,
                    ));
                    right_lines.push(cell_line(
                        row.right.as_ref().zip(idx[1]),
                        &self.highlighters[1],
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
    use crate::git::{Commit, CommitFile, DiffRow, FileChange, Section};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

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
        v.update(&Action::DiffLoaded(doc(20).into()));
        // Fresh load lands on the first changed block.
        assert_eq!(v.scroll_y, 0);

        // Simulate the user scrolling away.
        v.scroll_y = 7;
        v.scroll_x = 8;

        // A reload of the same file keeps the position (clamped to a
        // shorter doc here: 12 rows - 5 visible = max scroll 7).
        v.update(&Action::DiffReloaded(doc(12).into()));
        assert_eq!(v.scroll_y, 7);
        assert_eq!(v.scroll_x, 8);

        // Shorter still: clamps to the bottom.
        v.update(&Action::DiffReloaded(doc(8).into()));
        assert_eq!(v.scroll_y, 3);

        // A new file resets to the first change again.
        v.update(&Action::SelectFile(file()));
        v.update(&Action::DiffLoaded(doc(20).into()));
        assert_eq!(v.scroll_y, 0);
        assert_eq!(v.scroll_x, 0);
    }

    #[test]
    fn toolbar_down_click_jumps_to_next_change() {
        let mut v = DiffView {
            view_height: 5,
            ..Default::default()
        };
        // Two changed blocks: rows 0..2 and 10..12.
        let mut d = doc(20);
        for row in d.rows.iter_mut().skip(10).take(2) {
            row.kind = RowKind::Changed;
            row.left = cell(1, CellKind::Removed);
            row.right = cell(1, CellKind::Added);
        }
        v.update(&Action::SelectFile(file()));
        v.update(&Action::DiffLoaded(d.into()));

        // Render once so toolbar hitboxes exist (40-wide terminal).
        let mut term = Terminal::new(TestBackend::new(40, 10)).unwrap();
        term.draw(|f| v.render(f, f.area(), true)).unwrap();
        // Toolbar: "↑" at cols 29..32, "↓" at 32..35, "↻" at 35..38.
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 33,
            row: 0,
            modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
        };
        let act = v.handle_mouse(click, Rect::new(0, 0, 40, 10));
        assert!(matches!(act, Some(Action::DiffNextChange)));

        v.update(&Action::DiffNextChange);
        assert_eq!(v.scroll_y, 10);
    }

    #[test]
    fn cells_are_highlighted_on_their_diff_background() {
        let mut v = DiffView::default();
        let cell = |text: &str, kind| {
            Some(DiffCell {
                line_no: 1,
                text: text.to_string(),
                kind,
            })
        };
        v.update(&Action::DiffLoaded(
            DiffDoc {
                path: "f.rs".to_string(),
                rows: vec![DiffRow {
                    left: cell("let a = 1;", CellKind::Removed),
                    right: cell("fn b() {}", CellKind::Added),
                    kind: RowKind::Changed,
                }],
                binary: false,
            }
            .into(),
        ));
        let mut term = Terminal::new(TestBackend::new(40, 3)).unwrap();
        term.draw(|f| v.render(f, f.area(), true)).unwrap();
        let buf = term.backend().buffer();
        // Left half: border + "1 " gutter, text from col 3; right from 22.
        let (left, right) = (&buf[(3, 1)], &buf[(22, 1)]);
        assert_eq!((left.symbol(), right.symbol()), ("l", "f"));
        assert_eq!((left.bg, right.bg), (REMOVED_BG, ADDED_BG));
        assert!(matches!(left.fg, Color::Rgb(..)), "{:?}", left.fg);
        // Padding past the text keeps the background.
        assert_eq!(buf[(18, 1)].bg, REMOVED_BG);
    }

    fn commit_file() -> (Commit, CommitFile) {
        (
            Commit {
                hash: "abcdef1234567890".to_string(),
                short: "abcdef1".to_string(),
                author: "a".to_string(),
                time: 0,
                subject: "s".to_string(),
            },
            CommitFile {
                path: "f.rs".to_string(),
                orig_path: None,
                code: 'M',
            },
        )
    }

    #[test]
    fn status_load_clears_vanished_working_file_but_not_commit() {
        let mut v = DiffView::default();
        // Working-file diff vanishes from status -> cleared.
        v.update(&Action::SelectFile(file()));
        v.update(&Action::DiffLoaded(doc(5).into()));
        v.update(&Action::StatusLoaded(vec![]));
        assert!(v.doc.is_none());
        assert!(v.source.is_none());

        // Commit-sourced diff survives an empty status.
        let (c, cf) = commit_file();
        v.update(&Action::SelectCommitFile {
            commit: c,
            file: cf,
        });
        v.update(&Action::DiffLoaded(doc(5).into()));
        v.update(&Action::StatusLoaded(vec![]));
        assert!(v.doc.is_some());
        assert!(matches!(v.source, Some(DiffSource::Commit { .. })));
    }

    #[test]
    fn o_edits_at_the_change_or_top_line_in_view() {
        let key = |c| KeyEvent::from(KeyCode::Char(c));
        let mut v = DiffView {
            view_height: 5,
            ..Default::default()
        };
        assert!(v.handle_key(key('o')).is_none(), "nothing shown");
        // Rows 0-1 and 7 changed; 12 is removed-only (no new side).
        let mut d = doc(20);
        d.rows[7].kind = RowKind::Changed;
        d.rows[12].right = None;
        v.update(&Action::SelectFile(file()));
        v.update(&Action::DiffLoaded(d.into()));
        let edit_line = |v: &mut DiffView, scroll| {
            v.scroll_y = scroll;
            match v.handle_key(key('o')) {
                Some(Action::OpenInEditor { path, line }) if path == "f.rs" => line,
                other => panic!("{other:?}"),
            }
        };
        // The first change in view wins over the top line.
        assert_eq!(edit_line(&mut v, 4), Some(8));
        // No change in view: the top line, or the next new-side line.
        assert_eq!(edit_line(&mut v, 13), Some(14));
        assert_eq!(edit_line(&mut v, 12), Some(14));
        // A commit diff's line numbers aren't the worktree's.
        let (c, cf) = commit_file();
        v.update(&Action::SelectCommitFile {
            commit: c,
            file: cf,
        });
        v.update(&Action::DiffLoaded(doc(20).into()));
        assert!(matches!(
            v.handle_key(key('o')),
            Some(Action::OpenInEditor { path, line: None }) if path == "f.rs"
        ));
    }
}
