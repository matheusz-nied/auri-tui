use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::action::{Action, PanelId};
use crate::component::Component;
use crate::git::{Commit, CommitFile};

use super::{border_style, file_row_line, selection_style, SCROLL_LINES};

/// Commits fetched per page.
pub const HISTORY_PAGE: usize = 200;
/// When the viewport's last visible row comes this close to the end of the
/// row list, the next page of commits is requested.
const PAGE_NEAR_END: usize = 20;

/// VS Code–style linear commit list at the bottom of the sidebar. Clicking a
/// commit expands its changed files inline; clicking a file opens that
/// file's diff (vs the commit's first parent) in the diff pane.
pub struct History {
    commits: Vec<Commit>,
    /// Commit hash -> files, populated by `CommitFilesLoaded`.
    files: HashMap<String, Vec<CommitFile>>,
    /// Hashes of expanded commits.
    expanded: HashSet<String>,
    /// Flattened view of commits + expanded files.
    rows: Vec<Row>,
    /// Index into `rows`.
    selected: usize,
    hover: Option<usize>,
    scroll: usize,
    view_height: usize,
    /// Last page was shorter than `HISTORY_PAGE` — nothing more to fetch.
    exhausted: bool,
    /// A `LoadHistory` request is in flight.
    loading_more: bool,
    /// Whether this list owns the current diff selection (see `Changes`).
    active: bool,
}

#[derive(Clone, Copy)]
enum Row {
    /// Index into `commits`.
    Commit(usize),
    /// (commit index, index into `files[hash]`).
    File(usize, usize),
    /// Placeholder under an expanded commit whose files aren't loaded yet.
    Loading(usize),
}

impl Default for History {
    fn default() -> Self {
        Self {
            commits: Vec::new(),
            files: HashMap::new(),
            expanded: HashSet::new(),
            rows: Vec::new(),
            selected: 0,
            hover: None,
            scroll: 0,
            view_height: 1,
            exhausted: false,
            loading_more: false,
            active: false,
        }
    }
}

impl History {
    /// Selection identity across rebuilds: (commit hash, file path if a file
    /// row).
    fn selected_key(&self) -> Option<(String, Option<String>)> {
        match self.rows.get(self.selected)? {
            Row::Commit(ci) | Row::Loading(ci) => Some((self.commits[*ci].hash.clone(), None)),
            Row::File(ci, fi) => {
                let hash = self.commits[*ci].hash.clone();
                let path = self
                    .files
                    .get(&hash)
                    .and_then(|fs| fs.get(*fi))
                    .map(|f| f.path.clone());
                Some((hash, path))
            }
        }
    }

    /// Row index matching a `(hash, path)` selection key.
    fn find_key(&self, key: &(String, Option<String>)) -> Option<usize> {
        self.rows.iter().position(|r| match r {
            Row::Commit(ci) | Row::Loading(ci) => {
                self.commits[*ci].hash == key.0 && key.1.is_none()
            }
            Row::File(ci, fi) => {
                self.commits[*ci].hash == key.0
                    && key.1.as_deref()
                        == self
                            .files
                            .get(&key.0)
                            .and_then(|fs| fs.get(*fi))
                            .map(|f| f.path.as_str())
            }
        })
    }

    /// Rebuild `rows` from commits/expanded/files, keeping the selection on
    /// the same (hash, path) when it still exists.
    fn rebuild_rows(&mut self, fallback_to_zero: bool) {
        let key = self.selected_key();
        self.rows.clear();
        for (ci, c) in self.commits.iter().enumerate() {
            self.rows.push(Row::Commit(ci));
            if self.expanded.contains(&c.hash) {
                match self.files.get(&c.hash) {
                    Some(fs) => self.rows.extend((0..fs.len()).map(|fi| Row::File(ci, fi))),
                    None => self.rows.push(Row::Loading(ci)),
                }
            }
        }
        self.selected = match key.as_ref().and_then(|k| self.find_key(k)) {
            Some(i) => i,
            None if fallback_to_zero => 0,
            None => self.selected.min(self.rows.len().saturating_sub(1)),
        };
        self.ensure_visible();
    }

    /// Expand a commit (loading its files on first expand).
    fn expand_commit(&mut self, ci: usize) -> Option<Action> {
        let hash = self.commits.get(ci)?.hash.clone();
        if !self.expanded.insert(hash.clone()) {
            return None;
        }
        self.rebuild_rows(false);
        if self.files.contains_key(&hash) {
            None
        } else {
            Some(Action::LoadCommitFiles(hash))
        }
    }

    fn collapse_commit(&mut self, ci: usize) {
        if let Some(hash) = self.commits.get(ci).map(|c| c.hash.clone()) {
            if self.expanded.remove(&hash) {
                self.rebuild_rows(false);
            }
        }
    }

    /// Click/Enter toggles a commit row.
    fn toggle_commit(&mut self, ci: usize) -> Option<Action> {
        let hash = self.commits.get(ci)?.hash.clone();
        if self.expanded.contains(&hash) {
            self.collapse_commit(ci);
            None
        } else {
            self.expand_commit(ci)
        }
    }

    /// `SelectCommitFile` for the current row, if it is a file row.
    fn select_current(&mut self) -> Option<Action> {
        let Some(Row::File(ci, fi)) = self.rows.get(self.selected).copied() else {
            return None;
        };
        self.active = true;
        let commit = self.commits.get(ci)?.clone();
        let file = self.files.get(&commit.hash)?.get(fi)?.clone();
        Some(Action::SelectCommitFile { commit, file })
    }

    /// Request the next page when the viewport nears the last row.
    fn maybe_page(&mut self) -> Option<Action> {
        if self.exhausted || self.loading_more || self.rows.is_empty() {
            return None;
        }
        if self.rows.len() <= self.scroll + self.view_height + PAGE_NEAR_END {
            self.loading_more = true;
            return Some(Action::LoadHistory {
                skip: self.commits.len(),
            });
        }
        None
    }

    fn move_selection(&mut self, delta: isize) -> Option<Action> {
        if self.rows.is_empty() {
            return None;
        }
        let i = (self.selected as isize + delta).clamp(0, self.rows.len() as isize - 1) as usize;
        if i != self.selected {
            self.selected = i;
            self.ensure_visible();
        }
        self.select_current().or_else(|| self.maybe_page())
    }

    fn goto(&mut self, i: usize) -> Option<Action> {
        if self.rows.is_empty() {
            return None;
        }
        self.selected = i.min(self.rows.len() - 1);
        self.ensure_visible();
        self.select_current().or_else(|| self.maybe_page())
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

/// Compact relative timestamp: `now`, `Nm`, `Nh`, `Nd`, `Nw`, `Nmo`, `Ny`.
pub fn relative_time(ts: i64, now: i64) -> String {
    const MIN: i64 = 60;
    const HOUR: i64 = 60 * MIN;
    const DAY: i64 = 24 * HOUR;
    let d = now - ts;
    if d < MIN {
        "now".to_string()
    } else if d < HOUR {
        format!("{}m", d / MIN)
    } else if d < DAY {
        format!("{}h", d / HOUR)
    } else if d < 7 * DAY {
        format!("{}d", d / DAY)
    } else if d < 5 * 7 * DAY {
        format!("{}w", d / (7 * DAY))
    } else if d < 365 * DAY {
        format!("{}mo", d / (30 * DAY))
    } else {
        format!("{}y", d / (365 * DAY))
    }
}

/// ` ● {subject} {author}` left-aligned, relative date right-aligned, the
/// subject truncated with `…` when it doesn't fit.
fn commit_row_line(
    commit: &Commit,
    expanded: bool,
    now: i64,
    width: usize,
    style: Style,
) -> Line<'static> {
    let date = relative_time(commit.time, now);
    let date_w = date.chars().count();
    let avail = width.saturating_sub(date_w + 1);
    let author = format!(" {}", commit.author);
    let author_w = author.chars().count();
    // ` ● ` marker (3 cols) + subject + dim author.
    let subj_budget = avail.saturating_sub(3 + author_w);
    let mut subject: String = commit.subject.chars().take(subj_budget).collect();
    if commit.subject.chars().count() > subj_budget {
        subject = format!(
            "{}…",
            subject
                .chars()
                .take(subj_budget.saturating_sub(1))
                .collect::<String>()
        );
    }
    let subj_style = if expanded {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    };
    let used = 3 + subject.chars().count() + author_w;
    let pad = avail.saturating_sub(used);
    Line::from(vec![
        Span::styled(" ● ", style.fg(Color::Cyan)),
        Span::styled(subject, subj_style),
        Span::styled(author, style.fg(Color::DarkGray)),
        Span::styled(" ".repeat(pad + 1), style),
        Span::styled(date, style.fg(Color::DarkGray)),
    ])
}

impl Component for History {
    fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Char('g') | KeyCode::Home => self.goto(0),
            KeyCode::Char('G') | KeyCode::End => self.goto(self.rows.len().saturating_sub(1)),
            KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Right => {
                match self.rows.get(self.selected).copied() {
                    Some(Row::Commit(ci)) => self.toggle_commit(ci),
                    Some(Row::File(..)) => Some(Action::Focus(PanelId::DiffView)),
                    _ => None,
                }
            }
            KeyCode::Left => match self.rows.get(self.selected).copied() {
                Some(Row::Commit(ci)) => {
                    self.collapse_commit(ci);
                    None
                }
                Some(Row::File(ci, _)) | Some(Row::Loading(ci)) => {
                    // Collapse the parent commit and select its row.
                    self.collapse_commit(ci);
                    if let Some(i) = self
                        .rows
                        .iter()
                        .position(|r| matches!(r, Row::Commit(c) if *c == ci))
                    {
                        self.selected = i;
                        self.ensure_visible();
                    }
                    None
                }
                None => None,
            },
            _ => None,
        }
    }

    fn handle_mouse(&mut self, ev: MouseEvent, area: Rect) -> Option<Action> {
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // Only clicks strictly inside the borders act on rows.
                if ev.row <= area.y || ev.row >= area.y + area.height - 1 {
                    return None;
                }
                let idx = self.scroll + (ev.row - area.y - 1) as usize;
                match self.rows.get(idx).copied() {
                    Some(Row::Commit(ci)) => {
                        self.selected = idx;
                        self.ensure_visible();
                        self.toggle_commit(ci)
                    }
                    // Already-selected file rows are a no-op only while this
                    // list owns the diff; otherwise they re-activate it.
                    Some(Row::File(..)) if idx == self.selected && self.active => None,
                    Some(Row::File(..)) => {
                        self.selected = idx;
                        self.ensure_visible();
                        self.select_current()
                    }
                    Some(Row::Loading(_)) => {
                        self.selected = idx;
                        self.ensure_visible();
                        None
                    }
                    None => None,
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
                self.maybe_page()
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
            Action::HistoryLoaded { skip, commits } => {
                if *skip == 0 {
                    // Fresh page: drop expansions whose commit vanished, keep
                    // the selection by (hash, path) when possible.
                    let existing: HashSet<&str> = commits.iter().map(|c| c.hash.as_str()).collect();
                    self.expanded.retain(|h| existing.contains(h.as_str()));
                    self.commits = commits.clone();
                    self.exhausted = commits.len() < HISTORY_PAGE;
                    self.loading_more = false;
                    self.rebuild_rows(true);
                } else {
                    self.commits.extend(commits.iter().cloned());
                    self.exhausted = commits.len() < HISTORY_PAGE;
                    self.loading_more = false;
                    self.rebuild_rows(false);
                }
                None
            }
            Action::CommitFilesLoaded { hash, files } => {
                self.files.insert(hash.clone(), files.clone());
                self.rebuild_rows(false);
                None
            }
            // The diff now shows one of this list's commit files.
            Action::SelectCommitFile { .. } => {
                self.active = true;
                None
            }
            // The diff went back to a working-tree file.
            Action::SelectFile(_) => {
                self.active = false;
                None
            }
            _ => None,
        }
    }

    fn hints(&self) -> &'static str {
        "enter expand/open · ←/→ collapse/expand"
    }

    fn render(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        let block = Block::bordered()
            .title("Commits")
            .border_style(border_style(focused));
        let inner = block.inner(area);
        self.view_height = inner.height.max(1) as usize;
        self.ensure_visible();
        f.render_widget(block, area);

        if self.commits.is_empty() {
            f.render_widget(
                Paragraph::new(" No commits yet").style(Style::default().fg(Color::DarkGray)),
                inner,
            );
            return;
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let width = inner.width as usize;
        // Only build the visible slice — commit lists can be long.
        let lines: Vec<Line> = self
            .rows
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(self.view_height)
            .map(|(i, row)| {
                let style = if i == self.selected {
                    selection_style(focused && self.active)
                } else if self.hover == Some(i) {
                    Style::default().bg(Color::Rgb(30, 30, 40))
                } else {
                    Style::default()
                };
                match *row {
                    Row::Commit(ci) => commit_row_line(
                        &self.commits[ci],
                        self.expanded.contains(&self.commits[ci].hash),
                        now,
                        width,
                        style,
                    ),
                    Row::File(ci, fi) => {
                        let hash = &self.commits[ci].hash;
                        match self.files.get(hash).and_then(|fs| fs.get(fi)) {
                            Some(cf) => file_row_line(&cf.path, cf.code, 3, width, style, &[]).0,
                            None => Line::from(Span::styled("   …", style.fg(Color::DarkGray))),
                        }
                    }
                    Row::Loading(_) => {
                        Line::from(Span::styled("   loading…", style.fg(Color::DarkGray)))
                    }
                }
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{FileChange, Section};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn commit(n: usize) -> Commit {
        Commit {
            hash: format!("hash{n:04}"),
            short: format!("sh{n:04}"),
            author: format!("author{n}"),
            time: 1_700_000_000 - (n as i64) * 3600,
            subject: format!("commit subject {n}"),
        }
    }

    fn commits(n: usize) -> Vec<Commit> {
        (0..n).map(commit).collect()
    }

    fn load(h: &mut History, n: usize) {
        h.update(&Action::HistoryLoaded {
            skip: 0,
            commits: commits(n),
        });
    }

    fn file(path: &str) -> CommitFile {
        CommitFile {
            path: path.to_string(),
            orig_path: None,
            code: 'M',
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, ratatui::crossterm::event::KeyModifiers::empty())
    }

    fn click(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: ratatui::crossterm::event::KeyModifiers::empty(),
        }
    }

    /// Rendered area: 40 wide, 8 high -> inner rows y=1..7.
    fn draw(h: &mut History) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(40, 8)).unwrap();
        term.draw(|f| h.render(f, f.area(), true)).unwrap();
        term
    }

    #[test]
    fn relative_time_boundaries() {
        let now = 1_700_000_000i64;
        assert_eq!(relative_time(now, now), "now");
        assert_eq!(relative_time(now - 59, now), "now");
        assert_eq!(relative_time(now - 60, now), "1m");
        assert_eq!(relative_time(now - 3599, now), "59m");
        assert_eq!(relative_time(now - 3600, now), "1h");
        assert_eq!(relative_time(now - 86399, now), "23h");
        assert_eq!(relative_time(now - 86400, now), "1d");
        assert_eq!(relative_time(now - 6 * 86400, now), "6d");
        assert_eq!(relative_time(now - 7 * 86400, now), "1w");
        assert_eq!(relative_time(now - 34 * 86400, now), "4w");
        assert_eq!(relative_time(now - 35 * 86400, now), "1mo");
        assert_eq!(relative_time(now - 364 * 86400, now), "12mo");
        assert_eq!(relative_time(now - 365 * 86400, now), "1y");
        // Future timestamps -> "now".
        assert_eq!(relative_time(now + 9999, now), "now");
    }

    #[test]
    fn renders_commit_rows() {
        let mut h = History::default();
        load(&mut h, 3);
        let term = draw(&mut h);
        let buf = term.backend().buffer().clone();
        let text: String = (0..40).map(|x| buf[(x, 1)].symbol().to_string()).collect();
        assert!(text.contains('●'));
        assert!(text.contains("commit subject 0"));
        assert!(text.contains("author0"));
    }

    #[test]
    fn click_commit_expands_and_loads_files() {
        let mut h = History::default();
        load(&mut h, 3);
        draw(&mut h);
        // First commit row is inner y=1.
        let act = h.handle_mouse(click(2, 1), Rect::new(0, 0, 40, 8));
        assert!(matches!(act, Some(Action::LoadCommitFiles(h)) if h == "hash0000"));
        // While files are pending, a loading row is shown under the commit.
        let term = draw(&mut h);
        let buf = term.backend().buffer().clone();
        let row2: String = (0..40).map(|x| buf[(x, 2)].symbol().to_string()).collect();
        assert!(row2.contains("loading"), "row2 = {row2:?}");
        // Files arrive -> rows contain them.
        h.update(&Action::CommitFilesLoaded {
            hash: "hash0000".to_string(),
            files: vec![file("a.rs"), file("b.rs")],
        });
        let term = draw(&mut h);
        let buf = term.backend().buffer().clone();
        let row2: String = (0..40).map(|x| buf[(x, 2)].symbol().to_string()).collect();
        assert!(row2.contains("a.rs"), "row2 = {row2:?}");
    }

    #[test]
    fn click_file_selects_commit_file() {
        let mut h = History::default();
        load(&mut h, 2);
        h.handle_mouse(click(2, 1), Rect::new(0, 0, 40, 8));
        h.update(&Action::CommitFilesLoaded {
            hash: "hash0000".to_string(),
            files: vec![file("a.rs")],
        });
        // Render once so view_height is real — otherwise the first click's
        // ensure_visible scrolls with a phantom 1-row viewport.
        draw(&mut h);
        let act = h.handle_mouse(click(4, 2), Rect::new(0, 0, 40, 8));
        match act {
            Some(Action::SelectCommitFile { commit, file }) => {
                assert_eq!(commit.hash, "hash0000");
                assert_eq!(file.path, "a.rs");
            }
            other => panic!("expected SelectCommitFile, got {other:?}"),
        }
        // Clicking the same row again while active -> no-op.
        assert!(h
            .handle_mouse(click(4, 2), Rect::new(0, 0, 40, 8))
            .is_none());
        // After a working-file selection broadcast, clicking re-emits.
        h.update(&Action::SelectFile(FileChange {
            path: "w.rs".to_string(),
            orig_path: None,
            section: Section::Unstaged,
            code: 'M',
        }));
        assert!(matches!(
            h.handle_mouse(click(4, 2), Rect::new(0, 0, 40, 8)),
            Some(Action::SelectCommitFile { .. })
        ));
    }

    #[test]
    fn paging_triggers_near_the_end() {
        let mut h = History::default();
        load(&mut h, HISTORY_PAGE);
        let mut term = draw(&mut h); // view_height = 6
                                     // Walk close to the bottom: last trigger when
                                     // scroll + view_height + 20 >= rows.
        let mut emitted = None;
        for _ in 0..HISTORY_PAGE {
            if let Some(a) = h.handle_key(key(KeyCode::Char('j'))) {
                emitted = Some(a);
                break;
            }
        }
        match emitted {
            Some(Action::LoadHistory { skip }) => assert_eq!(skip, HISTORY_PAGE),
            other => panic!("expected LoadHistory, got {other:?}"),
        }
        // While the page is in flight, no duplicate requests.
        assert!(h.handle_key(key(KeyCode::Char('j'))).is_none());
        // A short page exhausts the list.
        h.update(&Action::HistoryLoaded {
            skip: HISTORY_PAGE,
            commits: commits(3)
                .iter()
                .cloned()
                .map(|mut c| {
                    c.hash.push('x');
                    c
                })
                .collect(),
        });
        for _ in 0..20 {
            assert!(!matches!(
                h.handle_key(key(KeyCode::Char('j'))),
                Some(Action::LoadHistory { .. })
            ));
        }
        let _ = &mut term;
    }

    #[test]
    fn refresh_keeps_expanded_commit() {
        let mut h = History::default();
        load(&mut h, 3);
        h.handle_mouse(click(2, 1), Rect::new(0, 0, 40, 8));
        h.update(&Action::CommitFilesLoaded {
            hash: "hash0000".to_string(),
            files: vec![file("a.rs")],
        });
        assert!(h.expanded.contains("hash0000"));
        // Same commits arrive again (tick refresh) -> expansion preserved.
        load(&mut h, 3);
        assert!(h.expanded.contains("hash0000"));
        assert!(h.rows.iter().any(|r| matches!(r, Row::File(0, 0))));
    }
}
