use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{DefaultTerminal, Frame};

use crate::action::{Action, PanelId};
use crate::component::Component;
use crate::components::changes::Changes;
use crate::components::commit_input::CommitInput;
use crate::components::confirm_dialog::ConfirmDialog;
use crate::components::diff_view::DiffView;
use crate::event::{AppEvent, Events};
use crate::git::{FileChange, GitBackend, Section};
use crate::layout::{self, Sidebar};

/// Owns the components, the focus state, the last-frame layout rects and the
/// action queue. It is the *only* place where `GitBackend` is called: side
/// effects requested by components (`Refresh`, `ToggleStage`, `Commit`,
/// `SelectFile`) are executed here and their results are broadcast back to all
/// components as new actions.
pub struct App {
    git: Box<dyn GitBackend>,
    events: Events,
    /// Panels in focus-cycling order.
    components: Vec<(PanelId, Box<dyn Component>)>,
    /// Modal overlays rendered above the panels. While any overlay reports
    /// `captures_input()`, it receives all input exclusively.
    overlays: Vec<Box<dyn Component>>,
    focus: PanelId,
    /// Panel the mouse cursor is currently over (drives `mouse_leave`).
    hovered: Option<PanelId>,
    /// Full frame area from the last render (overlay mouse coordinates).
    frame_area: Rect,
    queue: VecDeque<Action>,
    /// Rect each panel was rendered into last frame, used for click hit-tests.
    /// Hidden (collapsed-sidebar) panels have no entry.
    rects: HashMap<PanelId, Rect>,
    status_bar: Rect,
    /// Resizable/collapsible left column (commit input + changes list).
    sidebar: Sidebar,
    /// Area above the status bar from the last render; the divider lives in it.
    main_area: Rect,
    /// Last status snapshot, for the "nothing staged" commit check.
    last_status: Vec<FileChange>,
    branch: String,
    selected: Option<FileChange>,
    /// Last diff doc broadcast to components — compared against fresh loads so
    /// unchanged diffs aren't re-sent on every refresh tick.
    last_diff: Option<crate::git::DiffDoc>,
    /// (message, is_error) shown in the status bar.
    message: Option<(String, bool)>,
    /// Last key/mouse event time — periodic refresh is deferred while input
    /// is actively arriving (see `should_refresh`).
    last_input: Option<Instant>,
    /// Only redraw when something changed — idle timeouts skip `draw`.
    dirty: bool,
    running: bool,
}

/// How long after the last input event a `Tick` may trigger `Action::Refresh`.
/// During an input burst (trackpad scroll = dozens of events) the synchronous
/// git calls must not interleave with event handling.
const REFRESH_IDLE: Duration = Duration::from_secs(1);

/// Whether a `Tick` may enqueue `Refresh`: only once input has been quiet for
/// `REFRESH_IDLE` (or no input has ever arrived).
fn should_refresh(last_input: Option<Instant>, now: Instant) -> bool {
    last_input.is_none_or(|t| now.duration_since(t) >= REFRESH_IDLE)
}

impl App {
    pub fn new(git: Box<dyn GitBackend>) -> Self {
        Self {
            git,
            events: Events::default(),
            components: vec![
                (PanelId::CommitInput, Box::new(CommitInput::default())),
                (PanelId::Changes, Box::new(Changes::default())),
                (PanelId::DiffView, Box::new(DiffView::default())),
            ],
            overlays: vec![Box::new(ConfirmDialog::default())],
            focus: PanelId::Changes,
            hovered: None,
            frame_area: Rect::default(),
            queue: VecDeque::new(),
            rects: HashMap::new(),
            status_bar: Rect::default(),
            sidebar: Sidebar::default(),
            main_area: Rect::default(),
            last_status: Vec::new(),
            branch: String::new(),
            selected: None,
            last_diff: None,
            message: None,
            last_input: None,
            dirty: true,
            running: true,
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        self.enqueue(Action::Refresh);
        while self.running {
            if let Some(ev) = self.events.poll_event()? {
                self.handle_event(ev);
                // A burst (trackpad scroll, held key) queues many events:
                // handle them all, then draw once — not one render each.
                for ev in self.events.drain()? {
                    self.handle_event(ev);
                }
            }
            self.dispatch();
            if self.dirty {
                terminal.draw(|f| self.render(f))?;
                self.dirty = false;
            }
        }
        Ok(())
    }

    fn handle_event(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Key(key) => {
                self.last_input = Some(Instant::now());
                self.dirty = true;
                self.on_key(key);
            }
            AppEvent::Mouse(mouse) => {
                self.last_input = Some(Instant::now());
                self.dirty = true;
                self.on_mouse(mouse);
            }
            // The next draw picks up the new size automatically.
            AppEvent::Resize(..) => self.dirty = true,
            AppEvent::Tick => {
                if should_refresh(self.last_input, Instant::now()) {
                    self.enqueue(Action::Refresh);
                }
            }
        }
    }

    fn enqueue(&mut self, action: Action) {
        self.queue.push_back(action);
    }

    /// Drain the queue: broadcast every action to all components (`update`)
    /// and execute the side-effecting ones locally. Actions emitted along the
    /// way are processed in order. Any processed action marks the frame dirty.
    fn dispatch(&mut self) {
        if self.queue.is_empty() {
            return;
        }
        self.dirty = true;
        while let Some(action) = self.queue.pop_front() {
            for (_, comp) in &mut self.components {
                if let Some(next) = comp.update(&action) {
                    self.queue.push_back(next);
                }
            }
            for overlay in &mut self.overlays {
                if let Some(next) = overlay.update(&action) {
                    self.queue.push_back(next);
                }
            }
            self.execute(action);
        }
    }

    /// App-side handling: focus changes and git side effects.
    fn execute(&mut self, action: Action) {
        match action {
            Action::Quit => self.running = false,
            Action::FocusNext => self.cycle_focus(1),
            Action::FocusPrev => self.cycle_focus(-1),
            Action::Focus(id) => {
                if self.panel_visible(id) {
                    self.focus = id;
                }
            }
            Action::Refresh => {
                match self.git.status() {
                    Ok(files) => self.enqueue(Action::StatusLoaded(files)),
                    Err(e) => self.enqueue(Action::Error(format!("status: {e}"))),
                }
                match self.git.branch() {
                    Ok(b) => self.enqueue(Action::BranchLoaded(b)),
                    Err(e) => self.enqueue(Action::Error(format!("branch: {e}"))),
                }
            }
            Action::StatusLoaded(files) => {
                // The broadcast above ran first, so if `Changes` had to move
                // the selection (file committed/moved/first load) its
                // SelectFile is already queued — that action loads the new
                // file's diff, so reloading the old one would be stale work.
                let select_queued = self
                    .queue
                    .iter()
                    .any(|a| matches!(a, Action::SelectFile(_)));
                self.last_status = files;
                if !select_queued {
                    self.reload_selected_diff();
                }
            }
            Action::SelectFile(file) => {
                self.selected = Some(file.clone());
                match self.git.diff(&file) {
                    Ok(doc) => {
                        self.last_diff = Some(doc.clone());
                        self.enqueue(Action::DiffLoaded(doc));
                    }
                    Err(e) => self.enqueue(Action::Error(format!("diff: {e}"))),
                }
            }
            Action::ToggleStage(file) => {
                let result = if file.section == Section::Staged {
                    self.git.unstage(&file.path)
                } else {
                    self.git.stage(&file.path)
                };
                match result {
                    // Changes re-selects the file in its new section on the
                    // next StatusLoaded, which emits a fresh SelectFile.
                    Ok(()) => self.enqueue(Action::Refresh),
                    Err(e) => self.enqueue(Action::Error(format!("stage: {e}"))),
                }
            }
            Action::StageAll => match self.git.stage_all() {
                Ok(()) => self.enqueue(Action::Refresh),
                Err(e) => self.enqueue(Action::Error(format!("stage: {e}"))),
            },
            Action::UnstageAll => match self.git.unstage_all() {
                Ok(()) => self.enqueue(Action::Refresh),
                Err(e) => self.enqueue(Action::Error(format!("unstage: {e}"))),
            },
            Action::Discard(file) => match self.git.discard(&file) {
                Ok(()) => self.enqueue(Action::Refresh),
                Err(e) => self.enqueue(Action::Error(format!("discard: {e}"))),
            },
            Action::Commit(msg) => {
                let staged = self
                    .last_status
                    .iter()
                    .any(|f| f.section == Section::Staged);
                if !staged {
                    self.enqueue(Action::Error("Nothing staged".to_string()));
                } else {
                    match self.git.commit(&msg) {
                        Ok(()) => {
                            self.enqueue(Action::CommitDone);
                            self.enqueue(Action::Refresh);
                        }
                        Err(e) => self.enqueue(Action::Error(format!("commit: {e}"))),
                    }
                }
            }
            Action::CommitDone => {
                self.message = Some(("Committed".to_string(), false));
            }
            Action::Error(msg) => {
                self.message = Some((msg, true));
            }
            Action::BranchLoaded(branch) => self.branch = branch,
            // Handled entirely by components via `update`: Confirm opens the
            // overlay, DiffPrev/DiffNext scroll the diff view.
            Action::DiffLoaded(_)
            | Action::DiffReloaded(_)
            | Action::Confirm { .. }
            | Action::DiffPrevChange
            | Action::DiffNextChange => {}
        }
    }

    /// Reload the diff of the currently selected file, if it still exists in
    /// `last_status`. Broadcasts `DiffReloaded` only when the doc changed.
    fn reload_selected_diff(&mut self) {
        let Some(sel) = &self.selected else {
            return;
        };
        let still_there = self
            .last_status
            .iter()
            .any(|f| f.path == sel.path && f.section == sel.section);
        if !still_there {
            return;
        }
        match self.git.diff(sel) {
            Ok(doc) if self.last_diff.as_ref() != Some(&doc) => {
                self.last_diff = Some(doc.clone());
                self.enqueue(Action::DiffReloaded(doc));
            }
            Ok(_) => {}
            Err(e) => self.enqueue(Action::Error(format!("diff: {e}"))),
        }
    }

    /// Whether a panel is on screen (only the diff survives a hidden sidebar).
    fn panel_visible(&self, id: PanelId) -> bool {
        self.sidebar.visible || id == PanelId::DiffView
    }

    fn cycle_focus(&mut self, delta: isize) {
        let len = self.components.len() as isize;
        let cur = self
            .components
            .iter()
            .position(|(id, _)| *id == self.focus)
            .unwrap_or(0) as isize;
        let mut next = (cur + delta).rem_euclid(len);
        // Skip hidden panels (collapsed sidebar).
        for _ in 0..len {
            if self.panel_visible(self.components[next as usize].0) {
                break;
            }
            next = (next + delta).rem_euclid(len);
        }
        self.focus = self.components[next as usize].0;
    }

    /// `b` — hide/show the sidebar, moving focus to the diff if it pointed at
    /// a now-hidden panel.
    fn toggle_sidebar(&mut self) {
        self.sidebar.toggle();
        if !self.sidebar.visible && matches!(self.focus, PanelId::CommitInput | PanelId::Changes) {
            self.focus = PanelId::DiffView;
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        // Any key press dismisses the last info/error message.
        self.message = None;
        // Ctrl-C always quits, even while a modal overlay is open.
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.enqueue(Action::Quit);
            return;
        }
        // A modal overlay owns all input while it captures.
        if let Some(overlay) = self.overlays.iter_mut().find(|o| o.captures_input()) {
            if let Some(action) = overlay.handle_key(key) {
                self.enqueue(action);
            }
            return;
        }
        let commit_focused = self.focus == PanelId::CommitInput;
        // When the commit input is focused, only these keys are global —
        // everything else is text input.
        let global = if commit_focused {
            matches!(key.code, KeyCode::Tab | KeyCode::BackTab | KeyCode::Esc)
                || key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c')
        } else {
            true
        };
        if global {
            match (key.code, key.modifiers) {
                (KeyCode::Char('c'), m) if m.contains(KeyModifiers::CONTROL) => {
                    self.enqueue(Action::Quit);
                    return;
                }
                (KeyCode::Tab, _) => {
                    self.enqueue(Action::FocusNext);
                    return;
                }
                (KeyCode::BackTab, _) => {
                    self.enqueue(Action::FocusPrev);
                    return;
                }
                (KeyCode::Esc, _) if commit_focused => {
                    self.enqueue(Action::FocusNext);
                    return;
                }
                _ if !commit_focused => match key.code {
                    KeyCode::Char('q') => {
                        self.enqueue(Action::Quit);
                        return;
                    }
                    KeyCode::Char('r') => {
                        self.enqueue(Action::Refresh);
                        return;
                    }
                    KeyCode::Char('b') => {
                        self.toggle_sidebar();
                        return;
                    }
                    KeyCode::Char('[') => {
                        self.sidebar.shrink(self.main_area.width);
                        return;
                    }
                    KeyCode::Char(']') => {
                        self.sidebar.grow(self.main_area.width);
                        return;
                    }
                    // Reveal the sidebar when focusing a panel inside it.
                    KeyCode::Char('c') => {
                        self.sidebar.visible = true;
                        self.enqueue(Action::Focus(PanelId::CommitInput));
                        return;
                    }
                    KeyCode::Char('1') => {
                        self.sidebar.visible = true;
                        self.enqueue(Action::Focus(PanelId::Changes));
                        return;
                    }
                    KeyCode::Char('2') => {
                        self.enqueue(Action::Focus(PanelId::DiffView));
                        return;
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        if let Some((_, comp)) = self.components.iter_mut().find(|(id, _)| *id == self.focus) {
            if let Some(action) = comp.handle_key(key) {
                self.enqueue(action);
            }
        }
    }

    fn on_mouse(&mut self, ev: ratatui::crossterm::event::MouseEvent) {
        use ratatui::crossterm::event::MouseEventKind;
        let pos = (ev.column, ev.row);
        // Events go to the panel under the cursor (e.g. wheel over the diff
        // scrolls it while the changes list keeps focus); only button presses
        // move focus. Events outside all panels are ignored.
        // A modal overlay owns all input while it captures.
        if let Some(overlay) = self.overlays.iter_mut().find(|o| o.captures_input()) {
            if let Some(action) = overlay.handle_mouse(ev, self.frame_area) {
                self.enqueue(action);
            }
            return;
        }
        // The divider swallows presses/drags/releases that resize the sidebar;
        // everything else falls through to normal panel routing.
        if self.sidebar.on_mouse(&ev, self.main_area) {
            return;
        }
        let under = self
            .rects
            .iter()
            .find(|(_, r)| r.contains(pos.into()))
            .map(|(id, _)| *id);
        if under != self.hovered {
            if let Some(prev) = self.hovered {
                if let Some((_, comp)) = self.components.iter_mut().find(|(id, _)| *id == prev) {
                    comp.mouse_leave();
                }
            }
            self.hovered = under;
        }
        let Some((id, area)) = under.and_then(|id| self.rects.get(&id).copied().map(|r| (id, r)))
        else {
            return;
        };
        if matches!(ev.kind, MouseEventKind::Down(_)) {
            self.focus = id;
        }
        if let Some((_, comp)) = self.components.iter_mut().find(|(cid, _)| *cid == id) {
            if let Some(action) = comp.handle_mouse(ev, area) {
                self.enqueue(action);
            }
        }
    }

    fn render(&mut self, f: &mut Frame) {
        let area = f.area();
        self.frame_area = area;
        let vertical = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
        self.main_area = vertical[0];
        self.status_bar = vertical[1];
        let pr = layout::compute(vertical[0], &self.sidebar);

        self.rects.clear();
        if let Some(r) = pr.commit {
            self.rects.insert(PanelId::CommitInput, r);
        }
        if let Some(r) = pr.changes {
            self.rects.insert(PanelId::Changes, r);
        }
        self.rects.insert(PanelId::DiffView, pr.diff);

        for (id, comp) in &mut self.components {
            if let Some(rect) = self.rects.get(id).copied() {
                comp.render(f, rect, *id == self.focus);
            }
        }
        // Recolor both divider border columns while hovered/dragged so the
        // user can see it is draggable.
        if self.sidebar.divider_active() {
            if let Some((a, b)) = pr.divider {
                for col in [a, b] {
                    for row in self.main_area.y..self.main_area.y + self.main_area.height {
                        if let Some(cell) = f.buffer_mut().cell_mut((col, row)) {
                            cell.set_fg(Color::Cyan);
                        }
                    }
                }
            }
        }
        // Overlays draw on top of the panels with the whole frame available.
        for overlay in &mut self.overlays {
            overlay.render(f, area, overlay.captures_input());
        }
        self.render_status_bar(f);
    }

    fn render_status_bar(&self, f: &mut Frame) {
        let area = self.status_bar;
        let branch = if self.branch.is_empty() {
            "-".to_string()
        } else {
            self.branch.clone()
        };
        // Global keys minus `q` while typing a commit message (q is text
        // there), then the focused panel's own hints.
        let global = if self.focus == PanelId::CommitInput {
            "tab focus · r refresh"
        } else {
            "q quit · tab focus · r refresh · b sidebar · [/] resize"
        };
        let panel_hints = self
            .components
            .iter()
            .find(|(id, _)| *id == self.focus)
            .map(|(_, c)| c.hints())
            .unwrap_or("");
        let keys = if panel_hints.is_empty() {
            global.to_string()
        } else {
            format!("{global} · {panel_hints}")
        };
        let mut spans = vec![
            Span::styled(format!(" {branch}"), Style::default().fg(Color::Cyan)),
            Span::styled(format!("  {keys}"), Style::default().fg(Color::DarkGray)),
        ];
        if let Some((msg, is_error)) = &self.message {
            let style = if *is_error {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::Green)
            };
            spans.push(Span::styled(format!("  {msg}"), style));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_refresh_is_deferred_during_input() {
        let now = Instant::now();
        // No input yet -> refresh is allowed.
        assert!(should_refresh(None, now));
        // Input within the idle window defers the refresh.
        assert!(!should_refresh(Some(now), now));
        assert!(!should_refresh(Some(now - Duration::from_millis(999)), now));
        // Once input has been quiet for REFRESH_IDLE, refresh again.
        assert!(should_refresh(Some(now - REFRESH_IDLE), now));
        assert!(should_refresh(Some(now - Duration::from_secs(10)), now));
    }
}
