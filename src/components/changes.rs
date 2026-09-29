use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::action::{Action, PanelId};
use crate::component::Component;
use crate::git::{FileChange, Section};

use super::hitbox::{button_span, Hitboxes};
use super::{border_style, file_row_line, selection_style, SCROLL_LINES};

/// VS Code–style changes list: a "Staged Changes" section followed by a
/// "Changes" section (unstaged + untracked). Section headers are rendered but
/// not selectable. File rows show action buttons (discard/stage/unstage) when
/// hovered or selected; section headers carry a stage-all/unstage-all button.
pub struct Changes {
    rows: Vec<Row>,
    /// Index into `rows`; always on a `Row::File` (or 0 when empty).
    selected: usize,
    /// Row index under the mouse cursor.
    hover: Option<usize>,
    scroll: usize,
    view_height: usize,
    hitboxes: Hitboxes,
    /// Whether this list owns the current diff selection. `false` while the
    /// diff shows a commit file (History owns it): the selection is then
    /// rendered dimmed and clicking/re-navigating re-emits `SelectFile`.
    active: bool,
    /// Whether at least one `StatusLoaded` was seen (first load auto-selects
    /// even when inactive).
    seen_status: bool,
}

enum Row {
    /// Title plus an optional 3-column button (glyph, action) drawn
    /// right-aligned.
    Header(String, Option<(&'static str, Action)>),
    File(FileChange),
}

impl Default for Changes {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            selected: 0,
            hover: None,
            scroll: 0,
            view_height: 1,
            hitboxes: Hitboxes::default(),
            active: true,
            seen_status: false,
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
            rows.push(Row::Header(
                format!("Staged Changes ({})", staged.len()),
                Some(("−", Action::UnstageAll)),
            ));
            rows.extend(staged.iter().map(|f| Row::File((*f).clone())));
        }
        rows.push(Row::Header(
            format!("Changes ({})", rest.len()),
            (!rest.is_empty()).then_some(("+", Action::StageAll)),
        ));
        if rest.is_empty() && staged.is_empty() {
            rows.push(Row::Header("No changes".to_string(), None));
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
        // Don't emit while the user is browsing a commit diff — a tick would
        // otherwise yank the diff back to a working file. The very first
        // load always auto-selects so the diff pane isn't empty on start.
        let emit = now != prev && (self.active || !self.seen_status);
        self.seen_status = true;
        if emit {
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
        // While inactive (a commit diff is open), the first j/k press just
        // re-activates this list's selection instead of moving.
        if !self.active {
            self.active = true;
            return self.selected_file().map(Action::SelectFile);
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

/// `Action::Confirm` wrapping `Discard` with the right prompt for the section.
fn discard_confirm(f: &FileChange) -> Action {
    let prompt = match f.section {
        Section::Untracked => {
            format!("Delete untracked file {}? This cannot be undone.", f.path)
        }
        _ => format!("Discard changes in {}? This cannot be undone.", f.path),
    };
    Action::Confirm {
        prompt,
        confirm_label: "Discard".to_string(),
        then: Box::new(Action::Discard(f.clone())),
    }
}

/// The buttons a file row offers, as `(glyph, action)` in display order.
fn row_buttons(f: &FileChange) -> Vec<(&'static str, Action)> {
    match f.section {
        Section::Staged => vec![("−", Action::ToggleStage(f.clone()))],
        _ => vec![
            ("↶", discard_confirm(f)),
            ("+", Action::ToggleStage(f.clone())),
        ],
    }
}

impl Component for Changes {
    fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Char(' ') | KeyCode::Char('s') => {
                self.selected_file().map(Action::ToggleStage)
            }
            KeyCode::Char('d') => self
                .selected_file()
                .filter(|f| f.section != Section::Staged)
                .map(|f| discard_confirm(&f)),
            KeyCode::Char('a') => Some(Action::StageAll),
            KeyCode::Enter => Some(Action::Focus(PanelId::DiffView)),
            _ => None,
        }
    }

    fn handle_mouse(&mut self, ev: MouseEvent, area: Rect) -> Option<Action> {
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Buttons take precedence over row selection.
                if let Some(action) = self.hitboxes.hit(ev.column, ev.row) {
                    return Some(action);
                }
                // Only clicks strictly inside the borders select a row;
                // `ev.row - area.y - 1` maps a row to `scroll + offset`.
                if ev.row <= area.y || ev.row >= area.y + area.height - 1 {
                    return None;
                }
                let idx = self.scroll + (ev.row - area.y - 1) as usize;
                match self.rows.get(idx) {
                    // Clicking the already-selected row is a no-op only while
                    // this list owns the diff; otherwise it re-activates it.
                    Some(Row::File(_)) if idx == self.selected && self.active => None,
                    Some(Row::File(_)) => {
                        self.selected = idx;
                        self.active = true;
                        self.ensure_visible();
                        self.selected_file().map(Action::SelectFile)
                    }
                    _ => None,
                }
            }
            MouseEventKind::Moved => {
                if ev.row <= area.y || ev.row >= area.y + area.height - 1 {
                    self.hover = None;
                    return None;
                }
                let idx = self.scroll + (ev.row - area.y - 1) as usize;
                self.hover = (idx < self.rows.len()).then_some(idx);
                None
            }
            MouseEventKind::ScrollDown => {
                self.scroll = (self.scroll + SCROLL_LINES)
                    .min(self.rows.len().saturating_sub(self.view_height));
                None
            }
            MouseEventKind::ScrollUp => {
                self.scroll = self.scroll.saturating_sub(SCROLL_LINES);
                None
            }
            _ => None,
        }
    }

    fn mouse_leave(&mut self) {
        self.hover = None;
    }

    fn update(&mut self, action: &Action) -> Option<Action> {
        match action {
            Action::StatusLoaded(files) => self.set_status(files),
            // Follow selections made elsewhere (e.g. App re-selecting a file
            // that moved section after staging).
            Action::SelectFile(file) => {
                self.active = true;
                if let Some(i) = self.row_index_of(&file.path, file.section) {
                    self.selected = i;
                    self.ensure_visible();
                }
                None
            }
            // A commit file was opened — this list no longer owns the diff.
            Action::SelectCommitFile { .. } => {
                self.active = false;
                None
            }
            _ => None,
        }
    }

    fn hints(&self) -> &'static str {
        "space stage/unstage · d discard · a stage all · enter diff · c message"
    }

    fn render(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        self.hitboxes.clear();
        let block = Block::bordered()
            .title("Changes")
            .border_style(border_style(focused));
        let inner = block.inner(area);
        self.view_height = inner.height.max(1) as usize;
        // Re-clamp now that the real height is known (e.g. after a resize or
        // a selection made before the first render).
        self.ensure_visible();
        f.render_widget(block, area);

        let width = inner.width as usize;
        // Only build the visible slice — the list can be long.
        let mut hits: Vec<(u16, u16, Action)> = Vec::new();
        let lines: Vec<Line> = self
            .rows
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(self.view_height)
            .map(|(i, row)| match row {
                Row::Header(h, button) => {
                    let style = Style::default().fg(Color::DarkGray);
                    let spans = if let Some((glyph, action)) = button {
                        // 3-column button, right-aligned; truncate the title
                        // so the button always fits.
                        let bx = width.saturating_sub(3);
                        let title: String = format!(" {h}").chars().take(bx).collect();
                        let pad = bx - title.chars().count();
                        let y = inner.y + (i - self.scroll) as u16;
                        hits.push((inner.x + bx as u16, y, action.clone()));
                        vec![
                            Span::styled(title, style),
                            Span::styled(" ".repeat(pad), style),
                            button_span(glyph, style),
                        ]
                    } else {
                        vec![Span::styled(format!(" {h}"), style)]
                    };
                    Line::from(spans)
                }
                Row::File(fc) => {
                    let style = if i == self.selected {
                        // Dimmed selection while a commit diff owns the pane.
                        selection_style(focused && self.active)
                    } else {
                        Style::default()
                    };
                    let show_buttons = i == self.selected || self.hover == Some(i);
                    let buttons = if show_buttons {
                        row_buttons(fc)
                    } else {
                        Vec::new()
                    };
                    let (line, row_hits) =
                        file_row_line(&fc.path, fc.code, 1, width, style, &buttons);
                    let y = inner.y + (i - self.scroll) as u16;
                    for (x, action) in row_hits {
                        hits.push((inner.x + x, y, action));
                    }
                    line
                }
            })
            .collect();
        for (x, y, action) in hits {
            self.hitboxes.push(Rect::new(x, y, 3, 1), action);
        }
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::Section;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

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

    fn draw(c: &mut Changes, w: u16, h: u16) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| c.render(f, f.area(), true)).unwrap();
        term
    }

    fn click(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, ratatui::crossterm::event::KeyModifiers::NONE)
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

    // Button layout for a 20-wide panel: inner 18 cols, code at offset 17,
    // "+" at offsets 14..17, "↶" at offsets 11..14 (unstaged rows).
    #[test]
    fn click_stage_button_emits_toggle_stage() {
        let mut c = Changes::default();
        c.update(&Action::StatusLoaded(vec![fc("f.rs", Section::Unstaged)]));
        let area = Rect::new(0, 0, 20, 8);
        draw(&mut c, 20, 8); // populates hitboxes
                             // "+" glyph is the middle column of its 3-col hitbox: inner.x + 14 + 1.
        let act = c.handle_mouse(click(16, 2), area);
        assert!(
            matches!(&act, Some(Action::ToggleStage(f)) if f.path == "f.rs"),
            "expected ToggleStage, got {act:?}"
        );
    }

    #[test]
    fn click_discard_button_emits_confirm() {
        let mut c = Changes::default();
        c.update(&Action::StatusLoaded(vec![fc("f.rs", Section::Unstaged)]));
        let area = Rect::new(0, 0, 20, 8);
        draw(&mut c, 20, 8);
        // "↶" button hitbox: inner.x + 11 .. +14 → click column 13.
        let act = c.handle_mouse(click(13, 2), area);
        match act {
            Some(Action::Confirm { then, .. }) => {
                assert!(matches!(*then, Action::Discard(ref f) if f.path == "f.rs"));
            }
            other => panic!("expected Confirm, got {other:?}"),
        }
    }

    #[test]
    fn staged_header_button_emits_unstage_all() {
        let mut c = Changes::default();
        c.update(&Action::StatusLoaded(vec![fc("f.rs", Section::Staged)]));
        let area = Rect::new(0, 0, 20, 8);
        draw(&mut c, 20, 8);
        // "Staged Changes" header is row 0 (screen y=1); "−" at inner.x+15..+18.
        let act = c.handle_mouse(click(16, 1), area);
        assert!(matches!(act, Some(Action::UnstageAll)));
    }

    #[test]
    fn click_selected_row_emits_nothing() {
        let mut c = Changes::default();
        c.update(&Action::StatusLoaded(vec![fc("f.rs", Section::Unstaged)]));
        let area = Rect::new(0, 0, 20, 8);
        draw(&mut c, 20, 8);
        // Click the file name area of the already-selected row.
        assert!(c.handle_mouse(click(5, 2), area).is_none());
        assert_eq!(selected(&c), Some(("f.rs".into(), Section::Unstaged)));
    }

    #[test]
    fn hover_renders_buttons_on_unselected_row() {
        let mut c = Changes::default();
        c.update(&Action::StatusLoaded(vec![
            fc("a.rs", Section::Unstaged),
            fc("b.rs", Section::Unstaged),
        ]));
        let area = Rect::new(0, 0, 20, 8);
        draw(&mut c, 20, 8);
        // Hover the second file row (index 2 → screen y = 1 + 2 = 3).
        let moved = MouseEvent {
            kind: MouseEventKind::Moved,
            column: 5,
            row: 3,
            modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
        };
        assert!(c.handle_mouse(moved, area).is_none());
        let term = draw(&mut c, 20, 8);
        // "+" glyph sits at the middle of its 3-col button: inner.x + 14 + 1.
        assert_eq!(term.backend().buffer()[(16, 3)].symbol(), "+");
        assert_eq!(term.backend().buffer()[(13, 3)].symbol(), "↶");
        // Selected row (a.rs at y=2) shows its buttons too.
        assert_eq!(term.backend().buffer()[(16, 2)].symbol(), "+");
    }

    fn commit_select() -> Action {
        use crate::git::{Commit, CommitFile};
        Action::SelectCommitFile {
            commit: Commit {
                hash: "abc".into(),
                short: "abc".into(),
                author: "a".into(),
                time: 0,
                subject: "s".into(),
            },
            file: CommitFile {
                path: "x.rs".into(),
                orig_path: None,
                code: 'M',
            },
        }
    }

    #[test]
    fn inactive_changes_reemit_selectfile_and_ignore_status_moves() {
        let mut c = Changes::default();
        c.update(&Action::StatusLoaded(vec![fc("f.rs", Section::Unstaged)]));
        assert!(c.active);
        c.update(&commit_select());
        assert!(!c.active);

        // Status refresh while inactive emits nothing even though the
        // selection silently moved — the user is browsing a commit diff.
        let act = c.update(&Action::StatusLoaded(vec![fc("g.rs", Section::Unstaged)]));
        assert!(act.is_none(), "inactive status move emitted {act:?}");

        // j/k re-emits SelectFile for the current selection and reactivates.
        let out = c.handle_key(key(KeyCode::Char('j')));
        assert!(
            matches!(&out, Some(Action::SelectFile(f)) if f.path == "g.rs"),
            "expected SelectFile(g.rs), got {out:?}"
        );
        assert!(c.active);

        // Deactivate again; clicking the already-selected row re-emits.
        c.update(&commit_select());
        let area = Rect::new(0, 0, 20, 8);
        draw(&mut c, 20, 8);
        let out = c.handle_mouse(click(5, 2), area);
        assert!(
            matches!(&out, Some(Action::SelectFile(f)) if f.path == "g.rs"),
            "expected SelectFile(g.rs), got {out:?}"
        );
    }
}
