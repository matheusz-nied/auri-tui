use std::collections::HashMap;

use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::action::Action;
use crate::component::Component;
use crate::fs::tree::{Tree, TreeRow};
use crate::fs::{parent, EntryKind};
use crate::git::{FileChange, Section};
use crate::icons::{dir_icon, file_icon, IconStyle};
use crate::text;

use super::hitbox::{button_span, Hitboxes};
use super::{border_style, code_color, selection_style, SCROLL_LINES};

/// VS Code–style file explorer. Directory listings are loaded lazily: an
/// expand emits `LoadDirs` for that dir and every `Refresh` re-lists the
/// visible ones, so created/deleted files show up. Files are decorated with
/// their git status letter; folders containing changes get a colored `●`.
/// Each row shows a colored folder/file-type icon (`crate::icons`).
/// Opening a file emits `OpenFile`; `FileLoaded` marks it as the open file
/// (shown in the main pane while the Explorer view is active).
pub struct FileTree {
    /// Repo directory name, shown in the title.
    root_name: String,
    tree: Tree,
    /// `tree.rows()` cached until the next change.
    rows: Vec<TreeRow>,
    selected: usize,
    hover: Option<usize>,
    scroll: usize,
    view_height: usize,
    hitboxes: Hitboxes,
    /// Path shown by the file viewer (from `FileLoaded`).
    open: Option<String>,
    /// Path -> status letter for changed files and their ancestor dirs.
    decorations: HashMap<String, char>,
    /// Glyph set (`[explorer] icons` preference).
    icons: IconStyle,
}

impl FileTree {
    pub fn new(root_name: impl Into<String>) -> Self {
        Self {
            root_name: root_name.into(),
            tree: Tree::default(),
            rows: Vec::new(),
            selected: 0,
            hover: None,
            scroll: 0,
            view_height: 1,
            hitboxes: Hitboxes::default(),
            open: None,
            decorations: HashMap::new(),
            icons: IconStyle::default(),
        }
    }

    /// Re-flatten the tree, keeping the selection on the same path when it
    /// is still visible (else the same index, clamped).
    fn rebuild(&mut self) {
        let prev = self.selected_row().map(|r| r.path.clone());
        self.rows = self.tree.rows();
        if let Some(i) = prev.and_then(|p| self.index_of(&p)) {
            self.selected = i;
        }
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        self.ensure_visible();
    }

    fn index_of(&self, path: &str) -> Option<usize> {
        self.rows.iter().position(|r| r.path == path)
    }

    fn selected_row(&self) -> Option<&TreeRow> {
        self.rows.get(self.selected)
    }

    fn select(&mut self, i: usize) {
        if self.rows.is_empty() {
            return;
        }
        self.selected = i.min(self.rows.len() - 1);
        self.ensure_visible();
    }

    fn move_selection(&mut self, delta: isize) {
        let last = self.rows.len().saturating_sub(1) as isize;
        self.select((self.selected as isize + delta).clamp(0, last) as usize);
    }

    fn expand(&mut self, path: &str) -> Option<Action> {
        let needs_load = self.tree.expand(path);
        self.rebuild();
        needs_load.then(|| Action::LoadDirs(vec![path.to_string()]))
    }

    fn collapse(&mut self, path: &str) {
        self.tree.collapse(path);
        self.rebuild();
    }

    /// Enter/click: toggle a directory, open a file.
    fn activate(&mut self) -> Option<Action> {
        let row = self.selected_row()?.clone();
        match row.kind {
            EntryKind::Dir if row.expanded => {
                self.collapse(&row.path);
                None
            }
            EntryKind::Dir => self.expand(&row.path),
            EntryKind::File => Some(Action::OpenFile(row.path)),
        }
    }

    /// `l`/→: expand a collapsed dir, step into an expanded one, open a file.
    fn step_in(&mut self) -> Option<Action> {
        let row = self.selected_row()?.clone();
        match row.kind {
            EntryKind::Dir if row.expanded => {
                let first_child = self.rows.get(self.selected + 1);
                if first_child.is_some_and(|c| c.depth > row.depth) {
                    self.select(self.selected + 1);
                }
                None
            }
            _ => self.activate(),
        }
    }

    /// `h`/←: collapse an expanded dir, else jump to the parent folder.
    fn step_out(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        if row.kind == EntryKind::Dir && row.expanded {
            self.collapse(&row.path);
        } else if let Some(i) = self.index_of(parent(&row.path)) {
            self.select(i);
        }
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

    /// Row index under a mouse event strictly inside the borders.
    fn row_at(&self, ev: &MouseEvent, area: Rect) -> Option<usize> {
        if ev.row <= area.y || ev.row + 1 >= area.y + area.height {
            return None;
        }
        let idx = self.scroll + (ev.row - area.y - 1) as usize;
        (idx < self.rows.len()).then_some(idx)
    }
}

/// Status letter per changed path plus an aggregate for each ancestor dir:
/// the shared letter when all changes below agree, else `M`. The working
/// tree state wins over the index (a staged+modified file shows its
/// unstaged letter).
pub fn decorations(files: &[FileChange]) -> HashMap<String, char> {
    let mut by_file: HashMap<&str, char> = HashMap::new();
    for f in files.iter().filter(|f| f.section == Section::Staged) {
        by_file.insert(&f.path, f.code);
    }
    for f in files.iter().filter(|f| f.section != Section::Staged) {
        by_file.insert(&f.path, f.code);
    }
    let mut out = HashMap::new();
    for (path, code) in by_file {
        let mut dir = parent(path);
        while !dir.is_empty() {
            out.entry(dir.to_string())
                .and_modify(|c| {
                    // A conflict anywhere below wins; other mixes read `M`.
                    if *c == '!' || code == '!' {
                        *c = '!';
                    } else if *c != code {
                        *c = 'M';
                    }
                })
                .or_insert(code);
            dir = parent(dir);
        }
        out.insert(path.to_string(), code);
    }
    out
}

/// ` {indent}{chevron}{icon} {name}` with the decoration in the last column
/// (file letter or `●` for folders); the name is cut with `…` when it
/// doesn't fit. The icon keeps its type color; the name takes the git color.
fn tree_row_line(
    row: &TreeRow,
    deco: Option<char>,
    icons: IconStyle,
    width: usize,
    style: Style,
) -> Line<'static> {
    let chevron = match (row.kind, row.expanded) {
        (EntryKind::Dir, true) => "▾ ",
        (EntryKind::Dir, false) => "▸ ",
        (EntryKind::File, _) => "  ",
    };
    let icon = match row.kind {
        EntryKind::Dir => dir_icon(icons, row.expanded),
        EntryKind::File => file_icon(icons, &row.name),
    };
    let indent = format!(" {}{chevron}", "  ".repeat(row.depth));
    let indent = text::truncate(&indent, width);
    let indent_w = text::width(&indent);
    let icon_text = text::truncate(&format!("{} ", icon.glyph), width - indent_w);
    let prefix_w = indent_w + text::width(&icon_text);
    // Name budget: minus the decoration column and one space before it.
    let budget = width.saturating_sub(prefix_w + 2);
    let name = text::ellipsize(&row.name, budget);
    let name_style = match deco {
        Some(c) => style.fg(code_color(c)),
        None => style,
    };
    let mark = match (deco, row.kind) {
        (Some(_), EntryKind::Dir) => "●".to_string(),
        (Some(c), EntryKind::File) => c.to_string(),
        (None, _) => String::new(),
    };
    let used = prefix_w + text::width(&name) + text::width(&mark);
    let pad = width.saturating_sub(used);
    Line::from(vec![
        Span::styled(indent, style.fg(Color::DarkGray)),
        Span::styled(icon_text, style.fg(icon.color)),
        Span::styled(name, name_style),
        Span::styled(" ".repeat(pad), style),
        Span::styled(mark, name_style),
    ])
}

impl Component for FileTree {
    fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::PageDown => self.move_selection(self.view_height as isize),
            KeyCode::PageUp => self.move_selection(-(self.view_height as isize)),
            KeyCode::Char('g') | KeyCode::Home => self.select(0),
            KeyCode::Char('G') | KeyCode::End => self.select(self.rows.len().saturating_sub(1)),
            KeyCode::Enter | KeyCode::Char(' ') => return self.activate(),
            KeyCode::Char('l') | KeyCode::Right => return self.step_in(),
            KeyCode::Char('h') | KeyCode::Left => self.step_out(),
            _ => {}
        }
        None
    }

    fn handle_mouse(&mut self, ev: MouseEvent, area: Rect) -> Option<Action> {
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(action) = self.hitboxes.hit(ev.column, ev.row) {
                    return Some(action);
                }
                let idx = self.row_at(&ev, area)?;
                // Re-clicking the open file would reload it and reset the
                // viewer's scroll — no-op instead.
                if self.open.as_deref() == Some(self.rows[idx].path.as_str()) {
                    self.select(idx);
                    return None;
                }
                self.select(idx);
                self.activate()
            }
            MouseEventKind::Moved => {
                self.hover = self.row_at(&ev, area);
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
            // Re-list everything on screen (the root included — this is also
            // how the first listing is requested).
            Action::Refresh => return Some(Action::LoadDirs(self.tree.visible_dirs())),
            Action::DirLoaded { path, entries } => {
                if self.tree.set_children(path, entries.clone()) {
                    self.rebuild();
                }
            }
            Action::StatusLoaded(files) => self.decorations = decorations(files),
            Action::PreferencesChanged(prefs) => self.icons = prefs.explorer.icons,
            Action::ExplorerCollapseAll => {
                self.tree.collapse_all();
                self.rebuild();
            }
            Action::FileLoaded(doc) => {
                self.open = Some(doc.path.clone());
                if let Some(i) = self.index_of(&doc.path) {
                    self.select(i);
                }
            }
            _ => {}
        }
        None
    }

    fn hints(&self) -> &'static str {
        "enter open/toggle · h/l collapse/expand"
    }

    fn render(&mut self, f: &mut Frame, area: Rect, focused: bool) {
        self.hitboxes.clear();
        let block = Block::bordered()
            .title(format!(" Explorer · {} ", self.root_name))
            .border_style(border_style(focused));
        let inner = block.inner(area);
        self.view_height = inner.height.max(1) as usize;
        self.ensure_visible();
        f.render_widget(block, area);

        // Toolbar over the top border, right-aligned like the diff's.
        let toolbar = [("⊟", Action::ExplorerCollapseAll), ("↻", Action::Refresh)];
        let mut bx = area.x + area.width.saturating_sub(2);
        for (glyph, action) in toolbar.into_iter().rev() {
            if bx < area.x + 3 {
                break;
            }
            bx -= 3;
            let rect = Rect::new(bx, area.y, 3, 1);
            f.render_widget(
                button_span(glyph, Style::default().fg(Color::DarkGray)),
                rect,
            );
            self.hitboxes.push(rect, action);
        }

        if self.rows.is_empty() {
            let hint = Paragraph::new(" Empty folder").style(Style::default().fg(Color::DarkGray));
            f.render_widget(hint, inner);
            return;
        }
        let width = inner.width as usize;
        let lines: Vec<Line> = self
            .rows
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(self.view_height)
            .map(|(i, row)| {
                let style = if i == self.selected {
                    selection_style(focused)
                } else if self.hover == Some(i) {
                    Style::default().bg(Color::Rgb(35, 35, 45))
                } else {
                    Style::default()
                };
                let deco = self.decorations.get(&row.path).copied();
                tree_row_line(row, deco, self.icons, width, style)
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::{DirEntry, FileDoc};
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;
    use ratatui::Terminal;

    fn entry(path: &str, kind: EntryKind) -> DirEntry {
        DirEntry {
            name: path.rsplit('/').next().unwrap().to_string(),
            path: path.to_string(),
            kind,
        }
    }

    fn loaded(path: &str, entries: Vec<DirEntry>) -> Action {
        Action::DirLoaded {
            path: path.to_string(),
            entries,
        }
    }

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    /// Root: `src/` + `README.md`; `src` lists `src/app.rs`.
    fn tree() -> FileTree {
        let mut t = FileTree::new("repo");
        t.view_height = 10;
        t.update(&loaded(
            "",
            vec![
                entry("src", EntryKind::Dir),
                entry("README.md", EntryKind::File),
            ],
        ));
        t
    }

    fn doc(path: &str) -> FileDoc {
        FileDoc {
            path: path.to_string(),
            lines: vec![],
            binary: false,
            truncated: false,
        }
    }

    #[test]
    fn refresh_requests_visible_dirs() {
        let mut t = FileTree::new("repo");
        assert!(matches!(
            t.update(&Action::Refresh),
            Some(Action::LoadDirs(d)) if d == [""]
        ));
    }

    #[test]
    fn keyboard_expand_open_and_collapse() {
        let mut t = tree();
        // Expanding an unloaded dir asks App for its listing.
        let act = t.handle_key(key(KeyCode::Char('l')));
        assert!(matches!(act, Some(Action::LoadDirs(d)) if d == ["src"]));
        t.update(&loaded("src", vec![entry("src/app.rs", EntryKind::File)]));
        assert_eq!(t.rows.len(), 3);
        // l on an expanded dir steps into it; l on a file opens it.
        assert!(t.handle_key(key(KeyCode::Char('l'))).is_none());
        assert_eq!(t.selected_row().unwrap().path, "src/app.rs");
        assert!(matches!(
            t.handle_key(key(KeyCode::Char('l'))),
            Some(Action::OpenFile(p)) if p == "src/app.rs"
        ));
        // h on a file jumps to its folder, h again collapses it.
        t.handle_key(key(KeyCode::Char('h')));
        assert_eq!(t.selected_row().unwrap().path, "src");
        t.handle_key(key(KeyCode::Char('h')));
        assert_eq!(t.rows.len(), 2);
        // Enter toggles back open without a reload (already listed).
        assert!(t.handle_key(key(KeyCode::Enter)).is_none());
        assert_eq!(t.rows.len(), 3);
    }

    #[test]
    fn selection_follows_path_across_reloads() {
        let mut t = tree();
        t.handle_key(key(KeyCode::Char('j')));
        assert_eq!(t.selected_row().unwrap().path, "README.md");
        // A new file sorted before it doesn't move the selection.
        t.update(&loaded(
            "",
            vec![
                entry("src", EntryKind::Dir),
                entry("AGENTS.md", EntryKind::File),
                entry("README.md", EntryKind::File),
            ],
        ));
        assert_eq!(t.selected_row().unwrap().path, "README.md");
    }

    #[test]
    fn loaded_file_becomes_open_and_selected() {
        let mut t = tree();
        t.update(&Action::FileLoaded(doc("README.md")));
        assert_eq!(t.open.as_deref(), Some("README.md"));
        assert_eq!(t.selected_row().unwrap().path, "README.md");
    }

    #[test]
    fn clicks_toggle_dirs_open_files_and_hit_toolbar() {
        let mut t = tree();
        let area = Rect::new(0, 0, 30, 8);
        let mut term = Terminal::new(TestBackend::new(30, 8)).unwrap();
        term.draw(|f| t.render(f, area, true)).unwrap();
        let click = |col, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        };
        // Row 1 = `src` (first row inside the border).
        assert!(matches!(
            t.handle_mouse(click(5, 1), area),
            Some(Action::LoadDirs(_))
        ));
        t.update(&loaded("src", vec![entry("src/app.rs", EntryKind::File)]));
        assert!(matches!(
            t.handle_mouse(click(5, 2), area),
            Some(Action::OpenFile(p)) if p == "src/app.rs"
        ));
        t.update(&Action::FileLoaded(doc("src/app.rs")));
        // Clicking the open file again is a no-op.
        assert!(t.handle_mouse(click(5, 2), area).is_none());
        // Toolbar: ⊟ at cols 22..25, ↻ at 25..28.
        term.draw(|f| t.render(f, area, true)).unwrap();
        assert!(matches!(
            t.handle_mouse(click(23, 0), area),
            Some(Action::ExplorerCollapseAll)
        ));
        t.update(&Action::ExplorerCollapseAll);
        assert_eq!(t.rows.len(), 2);
    }

    #[test]
    fn decorations_aggregate_into_folders() {
        let fc = |path: &str, section, code| FileChange {
            path: path.to_string(),
            orig_path: None,
            section,
            code,
        };
        let d = decorations(&[
            fc("src/a.rs", Section::Staged, 'A'),
            fc("src/a.rs", Section::Unstaged, 'M'),
            fc("src/ui/new.rs", Section::Untracked, 'U'),
            fc("docs/x.md", Section::Untracked, 'U'),
        ]);
        assert_eq!(d["src/a.rs"], 'M', "working tree wins over index");
        assert_eq!(d["src/ui/new.rs"], 'U');
        assert_eq!(d["src/ui"], 'U');
        assert_eq!(d["src"], 'M', "mixed changes aggregate to M");
        assert_eq!(d["docs"], 'U');
        assert!(!d.contains_key(""));

        let d = decorations(&[
            fc("src/a.rs", Section::Unstaged, 'M'),
            fc("src/b.rs", Section::Conflicted, '!'),
            fc("src/c.rs", Section::Untracked, 'U'),
        ]);
        assert_eq!(d["src/b.rs"], '!');
        assert_eq!(d["src"], '!', "a conflict below marks the folder");
    }

    fn row(path: &str, depth: usize, kind: EntryKind, expanded: bool) -> TreeRow {
        TreeRow {
            path: path.to_string(),
            name: path.rsplit('/').next().unwrap().to_string(),
            depth,
            kind,
            expanded,
        }
    }

    fn text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn row_line_truncates_and_marks() {
        let r = row("src/very_long_file_name.rs", 1, EntryKind::File, false);
        let line = tree_row_line(&r, Some('M'), IconStyle::Text, 19, Style::default());
        assert_eq!(text(&line), "     rs very_lon… M");
        assert_eq!(text(&line).chars().count(), 19);
    }

    #[test]
    fn row_line_measures_wide_names_in_columns() {
        let r = row("docs/日本語のファイル名.md", 1, EntryKind::File, false);
        let line = tree_row_line(&r, Some('A'), IconStyle::Text, 19, Style::default());
        assert_eq!(crate::text::width(&text(&line)), 19);
        assert!(text(&line).ends_with("… A"), "{:?}", text(&line));
    }

    #[test]
    fn row_line_icons_align_and_keep_their_color() {
        let dir = row("src", 0, EntryKind::Dir, true);
        let file = row("README.md", 0, EntryKind::File, false);
        let d = tree_row_line(&dir, None, IconStyle::Text, 20, Style::default());
        let f = tree_row_line(&file, Some('M'), IconStyle::Text, 20, Style::default());
        assert!(text(&d).starts_with(" ▾ ▰  src"));
        assert!(text(&f).starts_with("   M↓ README.md"));
        // Names start in the same column; the icon keeps its type color
        // while the name takes the git status color.
        assert_eq!(
            text(&d).find("src").map(|i| text(&d)[..i].chars().count()),
            Some(6)
        );
        assert_eq!(
            text(&f)
                .find("README")
                .map(|i| text(&f)[..i].chars().count()),
            Some(6)
        );
        assert_eq!(
            f.spans[1].style.fg,
            Some(file_icon(IconStyle::Text, "README.md").color)
        );
        assert_eq!(f.spans[2].style.fg, Some(code_color('M')));
        // Nerd style swaps only the glyph.
        let n = tree_row_line(&file, None, IconStyle::Nerd, 20, Style::default());
        assert!(text(&n).starts_with("   \u{e73e}  README.md"));
    }

    #[test]
    fn icon_style_follows_preferences() {
        let mut t = tree();
        let mut prefs = crate::prefs::Preferences::default();
        prefs.explorer.icons = IconStyle::Nerd;
        t.update(&Action::PreferencesChanged(prefs));
        let area = Rect::new(0, 0, 30, 5);
        let mut term = Terminal::new(TestBackend::new(30, 5)).unwrap();
        term.draw(|f| t.render(f, area, true)).unwrap();
        // Row 1 = `src` (collapsed): " ▸ " then the closed-folder glyph.
        assert_eq!(term.backend().buffer()[(4, 1)].symbol(), "\u{f07b}");
    }
}
