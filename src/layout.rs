use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

/// Narrowest the sidebar can get while visible.
pub const MIN_SIDEBAR: u16 = 20;
/// Narrowest the diff pane can get before the sidebar clamps.
pub const MIN_MAIN: u16 = 30;
/// Columns added/removed per `grow`/`shrink` key press.
pub const RESIZE_STEP: u16 = 4;
/// Commit input height inside the sidebar.
const COMMIT_HEIGHT: u16 = 4;

/// Sidebar (commit input + changes list) layout state: width, visibility and
/// divider drag/hover. Pure value type — `App` owns one and re-renders every
/// frame, so all mutation happens through these methods.
#[derive(Debug)]
pub struct Sidebar {
    /// Explicit width; `None` means "35% of the terminal".
    width: Option<u16>,
    pub visible: bool,
    dragging: bool,
    hover_divider: bool,
}

impl Default for Sidebar {
    fn default() -> Self {
        Self {
            width: None,
            visible: true,
            dragging: false,
            hover_divider: false,
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
            self.hover_divider = false;
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

    /// Route a mouse event against the divider. Returns `true` when consumed —
    /// the caller must not forward it to any component.
    pub fn on_mouse(&mut self, ev: &MouseEvent, main: Rect) -> bool {
        use MouseEventKind as K;
        match ev.kind {
            K::Down(MouseButton::Left) if self.on_divider(ev, main) => {
                self.dragging = true;
                true
            }
            K::Drag(MouseButton::Left) if self.dragging => {
                let w = ev.column.saturating_sub(main.x).saturating_add(1);
                self.width = Some(self.clamp(w, main.width));
                true
            }
            K::Up(MouseButton::Left) if self.dragging => {
                self.dragging = false;
                true
            }
            K::Moved => {
                self.hover_divider = self.on_divider(ev, main);
                false
            }
            _ => false,
        }
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// Divider is hovered or being dragged — draw it highlighted.
    pub fn divider_active(&self) -> bool {
        self.hover_divider || self.dragging
    }
}

/// Screen rects for the three panels plus the divider hit-zone columns
/// `(sidebar right border, diff left border)`.
pub struct PanelRects {
    pub commit: Option<Rect>,
    pub changes: Option<Rect>,
    pub diff: Rect,
    pub divider: Option<(u16, u16)>,
}

/// Split `main` (the frame minus the status bar) into sidebar + diff pane.
pub fn compute(main: Rect, sidebar: &Sidebar) -> PanelRects {
    let w = sidebar.width_for(main.width);
    if w == 0 {
        return PanelRects {
            commit: None,
            changes: None,
            diff: main,
            divider: None,
        };
    }
    // Keep at least one column for the diff pane.
    let w = w.min(main.width.saturating_sub(1).max(1));
    let commit_h = main.height.min(COMMIT_HEIGHT);
    let commit = Rect::new(main.x, main.y, w, commit_h);
    let changes = Rect::new(main.x, main.y + commit_h, w, main.height - commit_h);
    let diff = Rect::new(main.x + w, main.y, main.width - w, main.height);
    PanelRects {
        commit: Some(commit),
        changes: Some(changes),
        diff,
        divider: Some((main.x + w - 1, main.x + w)),
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
        assert!(pr.commit.is_none() && pr.changes.is_none() && pr.divider.is_none());
        assert_eq!(pr.diff, main);
    }

    #[test]
    fn compute_splits_sidebar_commit_and_changes() {
        let s = visible();
        let main = Rect::new(0, 0, 100, 20);
        let pr = compute(main, &s);
        assert_eq!(pr.commit, Some(Rect::new(0, 0, 35, 4)));
        assert_eq!(pr.changes, Some(Rect::new(0, 4, 35, 16)));
        assert_eq!(pr.diff, Rect::new(35, 0, 65, 20));
        assert_eq!(pr.divider, Some((34, 35)));
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
}
