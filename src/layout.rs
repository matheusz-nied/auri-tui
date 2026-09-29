use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use serde::{Deserialize, Serialize};

/// Narrowest the sidebar can get while visible.
pub const MIN_SIDEBAR: u16 = 20;
/// Narrowest the diff pane can get before the sidebar clamps.
pub const MIN_MAIN: u16 = 30;
/// Columns added/removed per `grow`/`shrink` key press.
pub const RESIZE_STEP: u16 = 4;
/// Height of the Explorer/Source Control tab bar atop the sidebar: the
/// 1-row tabs plus a blank spacing row.
const TABS_HEIGHT: u16 = 2;
/// Commit input height inside the sidebar.
const COMMIT_HEIGHT: u16 = 4;
/// Minimum height for the changes and history panels.
const MIN_PANEL: u16 = 4;
/// Default share of the space below the commit box that history gets.
const HISTORY_SHARE: u16 = 45;

/// What the sidebar shows, like VS Code's activity bar: the file tree or
/// the git panels (commit input + changes + history).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SidebarView {
    Explorer,
    #[default]
    SourceControl,
}

/// Sidebar layout state: width, visibility, the active view, divider
/// drag/hover and the changes/history split (Source Control view).
/// Pure value type — `App` owns one and re-renders every frame, so all
/// mutation happens through these methods.
#[derive(Debug)]
pub struct Sidebar {
    /// Explicit width; `None` means "35% of the terminal".
    width: Option<u16>,
    /// History panel height; `None` means 45% of the space below the
    /// commit input box.
    history_height: Option<u16>,
    pub visible: bool,
    pub view: SidebarView,
    dragging: bool,
    /// Dragging the horizontal divider between Changes and Commits.
    dragging_split: bool,
    hover_divider: bool,
    hover_split: bool,
}

impl Default for Sidebar {
    fn default() -> Self {
        Self {
            width: None,
            history_height: None,
            visible: true,
            view: SidebarView::SourceControl,
            dragging: false,
            dragging_split: false,
            hover_divider: false,
            hover_split: false,
        }
    }
}

impl Sidebar {
    /// Sidebar width in columns for a `total`-wide main area. `0` when hidden.
    /// Terminals too small for both panes fall back to the 35% default.
    pub fn width_for(&self, total: u16) -> u16 {
        if !self.visible {
            return 0;
        }
        if total < MIN_SIDEBAR + MIN_MAIN {
            return total * 35 / 100;
        }
        self.width
            .unwrap_or(total * 35 / 100)
            .clamp(MIN_SIDEBAR, total - MIN_MAIN)
    }

    fn clamp(&self, w: u16, total: u16) -> u16 {
        if total < MIN_SIDEBAR + MIN_MAIN {
            return w;
        }
        w.clamp(MIN_SIDEBAR, total - MIN_MAIN)
    }

    /// Widen by `RESIZE_STEP`; materializes `width` from the default on first
    /// call, clamped so the main pane keeps `MIN_MAIN` columns.
    pub fn grow(&mut self, total: u16) {
        let base = self.width.unwrap_or(total * 35 / 100);
        self.width = Some(self.clamp(base.saturating_add(RESIZE_STEP), total));
    }

    /// Narrow by `RESIZE_STEP`, clamped to `MIN_SIDEBAR`.
    pub fn shrink(&mut self, total: u16) {
        let base = self.width.unwrap_or(total * 35 / 100);
        self.width = Some(self.clamp(base.saturating_sub(RESIZE_STEP), total));
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
        if !self.visible {
            self.dragging = false;
            self.dragging_split = false;
            self.hover_divider = false;
            self.hover_split = false;
        }
    }

    /// Split the rows below the commit box into (changes, history) heights.
    /// History defaults to 45%; when there's room, both panels keep at least
    /// `MIN_PANEL` rows — below that the space is split evenly.
    fn split_heights(&self, below: u16) -> (u16, u16) {
        if below < 2 * MIN_PANEL {
            let c = below / 2;
            return (c, below - c);
        }
        let h = self.clamp_history(
            self.history_height.unwrap_or(below * HISTORY_SHARE / 100),
            below,
        );
        (below - h, h)
    }

    fn clamp_history(&self, h: u16, below: u16) -> u16 {
        if below < 2 * MIN_PANEL {
            return below / 2;
        }
        h.clamp(MIN_PANEL, below - MIN_PANEL)
    }

    /// The history panel's top border row — the horizontal drag target.
    /// Only the Source Control view has one.
    fn split_row(&self, main: Rect) -> Option<u16> {
        let w = self.width_for(main.width);
        let body = body(main);
        if w == 0 || body.height <= COMMIT_HEIGHT || self.view != SidebarView::SourceControl {
            return None;
        }
        let below = body.height - COMMIT_HEIGHT;
        Some(body.y + COMMIT_HEIGHT + self.split_heights(below).0)
    }

    /// Whether the cursor is on the changes/history divider row, inside the
    /// sidebar's column range.
    fn on_split(&self, ev: &MouseEvent, main: Rect) -> bool {
        let w = self.width_for(main.width);
        match self.split_row(main) {
            Some(row) => ev.row == row && ev.column >= main.x && ev.column < main.x + w,
            None => false,
        }
    }

    /// The two columns forming the drag target: the sidebar's right border
    /// and the diff pane's left border, within `main`'s rows.
    fn divider_cols(&self, main: Rect) -> Option<(u16, u16)> {
        let w = self.width_for(main.width);
        if w == 0 || w >= main.width {
            return None;
        }
        Some((main.x + w - 1, main.x + w))
    }

    fn on_divider(&self, ev: &MouseEvent, main: Rect) -> bool {
        let Some((a, b)) = self.divider_cols(main) else {
            return false;
        };
        (ev.column == a || ev.column == b) && ev.row >= main.y && ev.row < main.y + main.height
    }

    /// Route a mouse event against the dividers. Returns `true` when
    /// consumed — the caller must not forward it to any component.
    pub fn on_mouse(&mut self, ev: &MouseEvent, main: Rect) -> bool {
        use MouseEventKind as K;
        match ev.kind {
            K::Down(MouseButton::Left) if self.on_divider(ev, main) => {
                self.dragging = true;
                true
            }
            K::Down(MouseButton::Left) if self.on_split(ev, main) => {
                self.dragging_split = true;
                true
            }
            K::Drag(MouseButton::Left) if self.dragging => {
                let w = ev.column.saturating_sub(main.x).saturating_add(1);
                self.width = Some(self.clamp(w, main.width));
                true
            }
            K::Drag(MouseButton::Left) if self.dragging_split => {
                let body = body(main);
                let below = body.height.saturating_sub(COMMIT_HEIGHT.min(body.height));
                let h = (body.y + body.height).saturating_sub(ev.row);
                self.history_height = Some(self.clamp_history(h, below));
                true
            }
            K::Up(MouseButton::Left) if self.dragging || self.dragging_split => {
                self.dragging = false;
                self.dragging_split = false;
                true
            }
            K::Moved => {
                self.hover_divider = self.on_divider(ev, main);
                self.hover_split = !self.hover_divider && self.on_split(ev, main);
                false
            }
            _ => false,
        }
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging || self.dragging_split
    }

    /// A divider is hovered or being dragged — draw it highlighted.
    pub fn divider_active(&self) -> bool {
        self.hover_divider || self.dragging || self.hover_split || self.dragging_split
    }

    /// Persisted form of the sidebar geometry (raw values — clamping happens
    /// at render time, so these are exactly what the user last set).
    pub fn to_prefs(&self) -> crate::prefs::LayoutPrefs {
        crate::prefs::LayoutPrefs {
            sidebar_width: self.width,
            sidebar_visible: self.visible,
            history_height: self.history_height,
            sidebar_view: self.view,
        }
    }

    /// Restore geometry loaded from disk.
    pub fn apply_prefs(&mut self, p: &crate::prefs::LayoutPrefs) {
        self.width = p.sidebar_width;
        self.visible = p.sidebar_visible;
        self.history_height = p.history_height;
        self.view = p.sidebar_view;
    }
}

/// The sidebar rows below the tab bar (full `main` width — callers cut
/// the sidebar's columns themselves).
fn body(main: Rect) -> Rect {
    let t = TABS_HEIGHT.min(main.height);
    Rect::new(main.x, main.y + t, main.width, main.height - t)
}

/// Screen rects for the panels plus divider hit zones: the vertical
/// `(sidebar right border, main pane left border)` columns and the changes/
/// history split row. Only the active sidebar view's panels get rects.
pub struct PanelRects {
    /// The Explorer/Source Control tab bar (sidebar's top rows).
    pub tabs: Option<Rect>,
    pub commit: Option<Rect>,
    pub changes: Option<Rect>,
    pub history: Option<Rect>,
    pub explorer: Option<Rect>,
    /// The main pane (diff or file viewer).
    pub diff: Rect,
    pub divider: Option<(u16, u16)>,
    /// Row of the history panel's top border (the horizontal drag target).
    pub split_row: Option<u16>,
}

/// Split `main` (the frame minus the status bar) into sidebar + diff pane.
pub fn compute(main: Rect, sidebar: &Sidebar) -> PanelRects {
    let w = sidebar.width_for(main.width);
    if w == 0 {
        return PanelRects {
            tabs: None,
            commit: None,
            changes: None,
            history: None,
            explorer: None,
            diff: main,
            divider: None,
            split_row: None,
        };
    }
    // Keep at least one column for the diff pane.
    let w = w.min(main.width.saturating_sub(1).max(1));
    let diff = Rect::new(main.x + w, main.y, main.width - w, main.height);
    let divider = Some((main.x + w - 1, main.x + w));
    let body = body(main);
    let tabs = Some(Rect::new(main.x, main.y, w, main.height - body.height));
    if sidebar.view == SidebarView::Explorer {
        return PanelRects {
            tabs,
            commit: None,
            changes: None,
            history: None,
            explorer: Some(Rect::new(body.x, body.y, w, body.height)),
            diff,
            divider,
            split_row: None,
        };
    }
    let commit_h = body.height.min(COMMIT_HEIGHT);
    let below = body.height - commit_h;
    let (changes_h, history_h) = sidebar.split_heights(below);
    let commit = Rect::new(body.x, body.y, w, commit_h);
    let changes = Rect::new(body.x, body.y + commit_h, w, changes_h);
    let history = Rect::new(body.x, changes.y + changes_h, w, history_h);
    PanelRects {
        tabs,
        commit: Some(commit),
        changes: Some(changes),
        history: Some(history),
        explorer: None,
        diff,
        divider,
        split_row: Some(history.y),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};

    fn mouse(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: col,
            row,
            modifiers: KeyModifiers::empty(),
        }
    }

    fn visible() -> Sidebar {
        Sidebar::default()
    }

    #[test]
    fn default_width_is_35_percent() {
        assert_eq!(visible().width_for(100), 35);
        assert_eq!(visible().width_for(60), 21);
    }

    #[test]
    fn width_clamps_to_min_and_leaves_min_main() {
        let mut s = visible();
        s.width = Some(5);
        assert_eq!(s.width_for(100), MIN_SIDEBAR);
        s.width = Some(90);
        assert_eq!(s.width_for(100), 100 - MIN_MAIN);
    }

    #[test]
    fn tiny_terminal_falls_back_to_35_percent() {
        let mut s = visible();
        s.width = Some(90);
        // 40 < MIN_SIDEBAR + MIN_MAIN: no clamping possible, keep the ratio.
        assert_eq!(s.width_for(40), 14);
    }

    #[test]
    fn grow_and_shrink_from_default_with_clamps() {
        let mut s = visible();
        s.grow(100);
        assert_eq!(s.width_for(100), 39);
        s.shrink(100);
        s.shrink(100);
        assert_eq!(s.width_for(100), 31);
        for _ in 0..10 {
            s.shrink(100);
        }
        assert_eq!(s.width_for(100), MIN_SIDEBAR);
        for _ in 0..30 {
            s.grow(100);
        }
        assert_eq!(s.width_for(100), 100 - MIN_MAIN);
    }

    #[test]
    fn hidden_sidebar_gives_diff_the_whole_main() {
        let mut s = visible();
        s.toggle();
        let main = Rect::new(0, 0, 100, 20);
        assert_eq!(s.width_for(100), 0);
        let pr = compute(main, &s);
        assert!(
            pr.tabs.is_none()
                && pr.commit.is_none()
                && pr.changes.is_none()
                && pr.history.is_none()
                && pr.explorer.is_none()
                && pr.divider.is_none()
                && pr.split_row.is_none()
        );
        assert_eq!(pr.diff, main);
    }

    #[test]
    fn compute_splits_sidebar_into_three_panels() {
        let s = visible();
        let main = Rect::new(0, 0, 100, 25);
        let pr = compute(main, &s);
        // 2-row tab bar, then 19 rows below the commit box: history gets 45% = 8.
        assert_eq!(pr.tabs, Some(Rect::new(0, 0, 35, 2)));
        assert_eq!(pr.commit, Some(Rect::new(0, 2, 35, 4)));
        assert_eq!(pr.changes, Some(Rect::new(0, 6, 35, 11)));
        assert_eq!(pr.history, Some(Rect::new(0, 17, 35, 8)));
        assert_eq!(pr.diff, Rect::new(35, 0, 65, 25));
        assert_eq!(pr.divider, Some((34, 35)));
        assert_eq!(pr.split_row, Some(17));
    }

    #[test]
    fn explorer_view_gives_the_tree_the_whole_sidebar() {
        let mut s = visible();
        s.view = SidebarView::Explorer;
        let main = Rect::new(0, 0, 100, 24);
        let pr = compute(main, &s);
        assert_eq!(pr.tabs, Some(Rect::new(0, 0, 35, 2)));
        assert_eq!(pr.explorer, Some(Rect::new(0, 2, 35, 22)));
        assert!(pr.commit.is_none() && pr.changes.is_none() && pr.history.is_none());
        assert_eq!(pr.diff, Rect::new(35, 0, 65, 24));
        assert_eq!(pr.divider, Some((34, 35)));
        assert_eq!(pr.split_row, None);
        // No split-row drag in the explorer: row 15 is just a tree row.
        assert!(!s.on_mouse(
            &mouse(MouseEventKind::Down(MouseButton::Left), 10, 15),
            main
        ));
    }

    #[test]
    fn split_clamps_both_panels_to_four_rows() {
        let mut s = visible();
        let main = Rect::new(0, 0, 100, 18); // below = 12
        s.history_height = Some(11); // would leave changes = 1
        let pr = compute(main, &s);
        // clamped: history <= 12-4 = 8
        assert_eq!(pr.changes.unwrap().height, 4);
        assert_eq!(pr.history.unwrap().height, 8);
        // Tiny space: even split.
        let main = Rect::new(0, 0, 100, 12); // below = 6
        let pr = compute(main, &s);
        assert_eq!(pr.changes.unwrap().height, 3);
        assert_eq!(pr.history.unwrap().height, 3);
    }

    #[test]
    fn drag_on_split_row_resizes_history() {
        let mut s = visible();
        let main = Rect::new(0, 0, 100, 25);
        // split_row = 17 (see compute test); press on it inside the sidebar.
        assert!(s.on_mouse(
            &mouse(MouseEventKind::Down(MouseButton::Left), 10, 17),
            main
        ));
        assert!(s.is_dragging());
        // Drag up to row 13: history_height = bottom(25) - 13 = 12.
        assert!(s.on_mouse(
            &mouse(MouseEventKind::Drag(MouseButton::Left), 10, 13),
            main
        ));
        assert!(s.on_mouse(&mouse(MouseEventKind::Up(MouseButton::Left), 10, 13), main));
        let pr = compute(main, &s);
        assert_eq!(pr.history.unwrap().height, 12);
        assert_eq!(pr.changes.unwrap().height, 7);
        // Down off the split row is not consumed.
        let mut s2 = visible();
        assert!(!s2.on_mouse(
            &mouse(MouseEventKind::Down(MouseButton::Left), 10, 16),
            main
        ));
        // Hover on the split row marks the divider active.
        assert!(!s2.on_mouse(&mouse(MouseEventKind::Moved, 10, 17), main));
        assert!(s2.divider_active());
    }

    #[test]
    fn drag_on_divider_resizes_and_each_event_is_consumed() {
        let mut s = visible();
        let main = Rect::new(0, 0, 100, 20);
        // Down on the sidebar's right border column (34).
        assert!(s.on_mouse(&mouse(MouseEventKind::Down(MouseButton::Left), 34, 5), main));
        assert!(s.is_dragging());
        // Drag to column 50 -> width = 50 - 0 + 1 = 51.
        assert!(s.on_mouse(&mouse(MouseEventKind::Drag(MouseButton::Left), 50, 5), main));
        // Release ends the drag, also consumed even off the divider.
        assert!(s.on_mouse(&mouse(MouseEventKind::Up(MouseButton::Left), 60, 5), main));
        assert!(!s.is_dragging());
        assert_eq!(s.width_for(100), 51);
    }

    #[test]
    fn drag_is_clamped() {
        let mut s = visible();
        let main = Rect::new(0, 0, 100, 20);
        s.on_mouse(&mouse(MouseEventKind::Down(MouseButton::Left), 35, 5), main);
        s.on_mouse(&mouse(MouseEventKind::Drag(MouseButton::Left), 3, 5), main);
        assert_eq!(s.width_for(100), MIN_SIDEBAR);
        s.on_mouse(&mouse(MouseEventKind::Drag(MouseButton::Left), 95, 5), main);
        assert_eq!(s.width_for(100), 100 - MIN_MAIN);
    }

    #[test]
    fn down_elsewhere_is_not_consumed() {
        let mut s = visible();
        let main = Rect::new(0, 0, 100, 20);
        assert!(!s.on_mouse(&mouse(MouseEventKind::Down(MouseButton::Left), 10, 5), main));
        assert!(!s.is_dragging());
        // Down on the divider is also ignored while hidden.
        s.toggle();
        assert!(!s.on_mouse(&mouse(MouseEventKind::Down(MouseButton::Left), 34, 5), main));
    }

    #[test]
    fn moved_over_divider_sets_active_not_consumed() {
        let mut s = visible();
        let main = Rect::new(0, 0, 100, 20);
        assert!(!s.on_mouse(&mouse(MouseEventKind::Moved, 34, 5), main));
        assert!(s.divider_active());
        assert!(!s.on_mouse(&mouse(MouseEventKind::Moved, 10, 5), main));
        assert!(!s.divider_active());
    }

    #[test]
    fn prefs_round_trip() {
        let mut s = visible();
        s.width = Some(44);
        s.visible = false;
        s.history_height = Some(7);
        s.view = SidebarView::Explorer;
        let p = s.to_prefs();
        assert_eq!(p.sidebar_width, Some(44));
        assert_eq!(p.sidebar_view, SidebarView::Explorer);
        assert!(!p.sidebar_visible);
        assert_eq!(p.history_height, Some(7));
        let mut s2 = visible();
        s2.apply_prefs(&p);
        assert_eq!(s2.to_prefs(), p);
        // Defaults apply cleanly to a fresh sidebar.
        let mut s3 = visible();
        s3.apply_prefs(&crate::prefs::LayoutPrefs::default());
        assert_eq!(s3.width_for(100), 35);
    }
}
