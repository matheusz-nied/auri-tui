use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::action::{Action, PanelId};
use crate::component::Component;
use crate::git::{FileChange, Section};

use super::{border_style, selection_style};

/// VS Code–style changes list: a "Staged Changes" section followed by a
/// "Changes" section (unstaged + untracked). Section headers are rendered but
/// not selectable.
pub struct Changes {
    rows: Vec<Row>,
    /// Index into `rows`; always on a `Row::File` (or 0 when empty).
    selected: usize,
    scroll: usize,
    view_height: usize,
}

enum Row {
    Header(String),
    File(FileChange),
}

impl Default for Changes {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            selected: 0,
            scroll: 0,
            view_height: 1,
        }
    }
}

impl Changes {
    /// Rebuild the row list from fresh status, keeping the selection stable:
    /// first by exact match (path+section), then by path in any section
    /// (preferring Staged — a staged file moved there), else the nearest file
    /// row. Returns `Some(Action::SelectFile)` when the effective selection
    /// changed so the diff view follows.
    fn set_status(&mut self, files: &[FileChange]) -> Option<Action> {
        let prev = self.selected_file();
        let staged: Vec<_> = files
            .iter()
            .filter(|f| f.section == Section::Staged)
            .collect();
        let rest: Vec<_> = files
            .iter()
            .filter(|f| f.section != Section::Staged)
            .collect();

        let mut rows = Vec::new();
        if !staged.is_empty() {
            rows.push(Row::Header(format!("Staged Changes ({})", staged.len())));
            rows.extend(staged.iter().map(|f| Row::File((*f).clone())));
        }
        rows.push(Row::Header(format!("Changes ({})", rest.len())));
        if rest.is_empty() && staged.is_empty() {
            rows.push(Row::Header("No changes".to_string()));
        }
        rows.extend(rest.iter().map(|f| Row::File((*f).clone())));
        self.rows = rows;

        self.selected = match prev.as_ref() {
            Some(p) => self
                .row_index_of(&p.path, p.section)
                .or_else(|| self.row_index_of_path(&p.path))
                .unwrap_or_else(|| self.first_file_from(self.selected)),
            None => self.first_file_from(0),
        };
        self.ensure_visible();
        let now = self.selected_file();
        if now != prev {
            now.map(Action::SelectFile)
        } else {
            None
        }
    }

    /// Index of the `Row::File` matching `path`+`section`.
    fn row_index_of(&self, path: &str, section: Section) -> Option<usize> {
        self.rows.iter().position(|r| match r {
            Row::File(f) => f.path == path && f.section == section,
            _ => false,
        })
    }

    /// Index of a `Row::File` for `path` in any section, preferring Staged.
    fn row_index_of_path(&self, path: &str) -> Option<usize> {
        self.row_index_of(path, Section::Staged).or_else(|| {
            self.rows.iter().position(|r| match r {
                Row::File(f) => f.path == path,
                _ => false,
            })
        })
    }

    fn selected_file(&self) -> Option<FileChange> {
        match self.rows.get(self.selected) {
            Some(Row::File(f)) => Some(f.clone()),
            _ => None,
        }
    }

    /// First file row index at or after `from`; falls back to the last file
    /// row before `from`, or 0 if there are no files.
    fn first_file_from(&self, from: usize) -> usize {
        if let Some(i) = (from..self.rows.len()).find(|&i| matches!(self.rows[i], Row::File(_))) {
            return i;
        }
        if let Some(i) = (0..from.min(self.rows.len()))
            .rev()
            .find(|&i| matches!(self.rows[i], Row::File(_)))
        {
            return i;
        }
        0
    }

    fn move_selection(&mut self, delta: isize) -> Option<Action> {
        if self.rows.is_empty() {
            return None;
        }
        let mut i = self.selected as isize;
        loop {
            i += delta;
            if i < 0 || i >= self.rows.len() as isize {
                return None;
            }
            if matches!(self.rows[i as usize], Row::File(_)) {
                break;
            }
        }
        self.selected = i as usize;
        self.ensure_visible();
        self.selected_file().map(Action::SelectFile)
    }

    fn ensure_visible(&mut self) {
        if self.selected < self.scroll {
            self.scroll = self.selected;
        }
        if self.selected >= self.scroll + self.view_height {
            self.scroll = self.selected + 1 - self.view_height;
        }
        let max_scroll = self.rows.len().saturating_sub(self.view_height);
        self.scroll = self.scroll.min(max_scroll);
    }
}

fn code_color(code: char) -> Color {
    match code {
        'M' => Color::Yellow,
        'A' | 'U' => Color::Green,
        'D' => Color::Red,
        'R' | 'C' => Color::Blue,
        _ => Color::White,
    }
}

fn file_row_line(f: &FileChange, width: usize, style: Style) -> Line<'static> {
    // VS Code style: `action.rs src` — name, then the dim parent dir.
    let (dir, name) = match f.path.rsplit_once('/') {
        Some((d, n)) => (d.to_string(), n.to_string()),
        None => (String::new(), f.path.clone()),
    };
    let code = f.code.to_string();
    let dir_w = if dir.is_empty() {
        0
    } else {
        1 + dir.chars().count()
    };
    // leading space + name + (" " + dir) + code
    let used = 1 + name.chars().count() + dir_w + code.len();
    let pad = width.saturating_sub(used);
    let mut spans = vec![Span::styled(format!(" {name}"), style)];
    if !dir.is_empty() {
        spans.push(Span::styled(format!(" {dir}"), style.fg(Color::DarkGray)));
    }
    spans.push(Span::styled(" ".repeat(pad), style));
    spans.push(Span::styled(code, style.fg(code_color(f.code))));
    Line::from(spans)
}

impl Component for Changes {
    fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Char(' ') | KeyCode::Char('s') => {
                self.selected_file().map(Action::ToggleStage)
            }
            KeyCode::Char('a') => Some(Action::StageAll),
            KeyCode::Enter => Some(Action::Focus(PanelId::DiffView)),
            _ => None,
        }
    }

    fn handle_mouse(&mut self, ev: MouseEvent, area: Rect) -> Option<Action> {
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Only clicks strictly inside the borders select a row;
                // `ev.row - area.y - 1` maps a row to `scroll + offset`.
                if ev.row <= area.y || ev.row >= area.y + area.height - 1 {
                    return None;
                }
                let idx = self.scroll + (ev.row - area.y - 1) as usize;
                if let Some(Row::File(_)) = self.rows.get(idx) {
                    self.selected = idx;
                    self.ensure_visible();
                    return self.selected_file().map(Action::SelectFile);
                }
                None
            }
            MouseEventKind::ScrollDown => {
                self.scroll =
                    (self.scroll + 3).min(self.rows.len().saturating_sub(self.view_height));
                None
            }
            MouseEventKind::ScrollUp => {
                self.scroll = self.scroll.saturating_sub(3);
                None
            }
            _ => None,
        }
    }

    fn update(&mut self, action: &Action) -> Option<Action> {
        match action {
            Action::StatusLoaded(files) => self.set_status(files),
            // Follow selections made elsewhere (e.g. App re-selecting a file
            // that moved section after staging).
            Action::SelectFile(file) => {
                if let Some(i) = self.row_index_of(&file.path, file.section) {
                    self.selected = i;
                    self.ensure_visible();
                }
                None
            }
            _ => None,
        }
    }

    fn hints(&self) -> &'static str {
        "space stage/unstage · a stage all · enter diff · c message"
    }

    fn render(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        let block = Block::bordered()
            .title("Changes")
            .border_style(border_style(focused));
        let inner = block.inner(area);
        self.view_height = inner.height.max(1) as usize;
        f.render_widget(block, area);

        let width = inner.width as usize;
        // Only build the visible slice — the list can be long.
        let lines: Vec<Line> = self
            .rows
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(self.view_height)
            .map(|(i, row)| match row {
                Row::Header(h) => Line::from(Span::styled(
                    format!(" {h}"),
                    Style::default().fg(Color::DarkGray),
                )),
                Row::File(fc) => {
                    let style = if i == self.selected {
                        selection_style(focused)
                    } else {
                        Style::default()
                    };
                    file_row_line(fc, width, style)
                }
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::Section;

    fn fc(path: &str, section: Section) -> FileChange {
        FileChange {
            path: path.to_string(),
            orig_path: None,
            section,
            code: 'M',
        }
    }

    fn selected(c: &Changes) -> Option<(String, Section)> {
        match c.rows.get(c.selected) {
            Some(Row::File(f)) => Some((f.path.clone(), f.section)),
            _ => None,
        }
    }

    #[test]
    fn select_file_action_moves_selection_to_matching_row() {
        let mut c = Changes::default();
        let files = vec![fc("m.rs", Section::Staged), fc("a.rs", Section::Unstaged)];
        // First load auto-selects the first file row (staged m.rs).
        let act = c.update(&Action::StatusLoaded(files));
        assert!(
            matches!(&act, Some(Action::SelectFile(f)) if f.path == "m.rs" && f.section == Section::Staged),
            "expected SelectFile for staged m.rs, got {act:?}"
        );

        // A broadcast SelectFile for another entry moves the selection.
        c.update(&Action::SelectFile(fc("a.rs", Section::Unstaged)));
        assert_eq!(selected(&c), Some(("a.rs".into(), Section::Unstaged)));

        c.update(&Action::SelectFile(fc("m.rs", Section::Staged)));
        assert_eq!(selected(&c), Some(("m.rs".into(), Section::Staged)));
    }

    #[test]
    fn disappeared_selection_emits_select_file_for_fallback() {
        let mut c = Changes::default();
        c.update(&Action::StatusLoaded(vec![
            fc("a.rs", Section::Unstaged),
            fc("b.rs", Section::Unstaged),
        ]));
        assert_eq!(selected(&c), Some(("a.rs".into(), Section::Unstaged)));

        // a.rs is committed/discarded: only b.rs remains.
        let act = c.update(&Action::StatusLoaded(vec![fc("b.rs", Section::Unstaged)]));
        assert!(
            matches!(&act, Some(Action::SelectFile(f)) if f.path == "b.rs"),
            "expected SelectFile(b.rs), got {act:?}"
        );
        assert_eq!(selected(&c), Some(("b.rs".into(), Section::Unstaged)));
    }

    #[test]
    fn staged_fallback_keeps_selection_on_moved_file() {
        let mut c = Changes::default();
        c.update(&Action::StatusLoaded(vec![fc("m.rs", Section::Unstaged)]));
        // Staging moved m.rs to the Staged section; selection follows the path
        // and a new SelectFile is emitted so the staged diff loads.
        let act = c.update(&Action::StatusLoaded(vec![fc("m.rs", Section::Staged)]));
        assert!(
            matches!(&act, Some(Action::SelectFile(f)) if f.section == Section::Staged),
            "expected SelectFile(staged m.rs), got {act:?}"
        );
        assert_eq!(selected(&c), Some(("m.rs".into(), Section::Staged)));
    }

    #[test]
    fn first_load_selects_first_file() {
        let mut c = Changes::default();
        let act = c.update(&Action::StatusLoaded(vec![fc("x.rs", Section::Unstaged)]));
        assert!(matches!(&act, Some(Action::SelectFile(f)) if f.path == "x.rs"));
    }

    #[test]
    fn unchanged_selection_emits_nothing() {
        let mut c = Changes::default();
        c.update(&Action::StatusLoaded(vec![fc("x.rs", Section::Unstaged)]));
        let act = c.update(&Action::StatusLoaded(vec![fc("x.rs", Section::Unstaged)]));
        assert!(act.is_none());
        assert_eq!(selected(&c), Some(("x.rs".into(), Section::Unstaged)));
    }

    #[test]
    fn empty_status_emits_nothing() {
        let mut c = Changes::default();
        let act = c.update(&Action::StatusLoaded(vec![]));
        assert!(act.is_none());
    }
}
