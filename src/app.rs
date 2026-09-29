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
use crate::ai::{self, AiOutcome, AiProvider, AiRunner, ProcessRunner};
use crate::component::Component;
use crate::components::changes::Changes;
use crate::components::commit_input::CommitInput;
use crate::components::confirm_dialog::ConfirmDialog;
use crate::components::diff_view::DiffView;
use crate::components::history::{History, HISTORY_PAGE};
use crate::event::{AppEvent, Events};
use crate::git::{DiffSource, GitBackend, Section};
use crate::layout::{self, Sidebar};
use crate::prefs::{Preferences, PrefsStore};

/// Owns the components, the focus state, the last-frame layout rects and the
/// action queue. It is the *only* place where `GitBackend` is called: side
/// effects requested by components (`Refresh`, `ToggleStage`, `Commit`,
/// `SelectFile`) are executed here and their results are broadcast back to all
/// components as new actions.
pub struct App {
    git: Box<dyn GitBackend>,
    /// Background AI commit-message generation; polled once per loop.
    ai: Box<dyn AiRunner>,
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
    last_status: Vec<crate::git::FileChange>,
    branch: String,
    /// What the diff pane shows: a working-tree file or a commit file.
    selected: Option<DiffSource>,
    /// HEAD hash from the last refresh — history reloads only when it moves.
    last_head: Option<String>,
    /// Whether `head()` has been fetched at least once.
    seen_head: bool,
    /// Last diff doc broadcast to components — compared against fresh loads so
    /// unchanged diffs aren't re-sent on every refresh tick.
    last_diff: Option<crate::git::DiffDoc>,
    /// Loaded preferences; `App` is the only writer (via `sync_prefs`).
    prefs: Preferences,
    /// Where prefs are persisted.
    store: Box<dyn PrefsStore>,
    /// `false` when the initial `load()` failed — never save over a file we
    /// couldn't parse.
    prefs_writable: bool,
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
    pub fn new(git: Box<dyn GitBackend>, store: Box<dyn PrefsStore>) -> Self {
        let mut app = Self {
            git,
            ai: Box::new(ProcessRunner::default()),
            events: Events::default(),
            components: vec![
                (PanelId::CommitInput, Box::new(CommitInput::default())),
                (PanelId::Changes, Box::new(Changes::default())),
                (PanelId::History, Box::new(History::default())),
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
            last_head: None,
            seen_head: false,
            last_diff: None,
            prefs: Preferences::default(),
            store,
            prefs_writable: true,
            message: None,
            last_input: None,
            dirty: true,
            running: true,
        };
        match app.store.load() {
            Ok(prefs) => {
                app.sidebar.apply_prefs(&prefs.layout);
                app.prefs = prefs;
            }
            // Unreadable/corrupt prefs file: run with defaults and never
            // overwrite the file the user may want to inspect.
            Err(e) => {
                app.prefs_writable = false;
                app.enqueue(Action::Error(format!(
                    "preferences: {e} — using defaults, changes won't be saved"
                )));
            }
        }
        app.enqueue(Action::PreferencesChanged(app.prefs.clone()));
        app
    }

    /// Test hook: swap in a fake AI runner.
    pub fn with_ai_runner(mut self, r: Box<dyn AiRunner>) -> Self {
        self.ai = r;
        self
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
            self.poll_ai();
            self.dispatch();
            self.sync_prefs();
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
                // History reloads only when HEAD moved (new commit, rebase,
                // checkout...) — not on every tick.
                match self.git.head() {
                    Ok(head) if !self.seen_head || self.last_head != head => {
                        self.seen_head = true;
                        self.last_head = head;
                        self.enqueue(Action::LoadHistory { skip: 0 });
                    }
                    Ok(_) => {}
                    Err(e) => self.enqueue(Action::Error(format!("head: {e}"))),
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
                    .any(|a| matches!(a, Action::SelectFile(_) | Action::SelectCommitFile { .. }));
                self.last_status = files;
                if !select_queued {
                    self.reload_selected_diff();
                }
            }
            Action::SelectFile(file) => {
                self.selected = Some(DiffSource::Working(file.clone()));
                match self.git.diff(&file) {
                    Ok(doc) => {
                        self.last_diff = Some(doc.clone());
                        self.enqueue(Action::DiffLoaded(doc));
                    }
                    Err(e) => self.enqueue(Action::Error(format!("diff: {e}"))),
                }
            }
            Action::SelectCommitFile { commit, file } => {
                self.selected = Some(DiffSource::Commit {
                    hash: commit.hash.clone(),
                    file: file.clone(),
                });
                match self.git.commit_diff(&commit.hash, &file) {
                    Ok(doc) => {
                        self.last_diff = Some(doc.clone());
                        self.enqueue(Action::DiffLoaded(doc));
                    }
                    Err(e) => self.enqueue(Action::Error(format!("diff: {e}"))),
                }
            }
            Action::LoadHistory { skip } => match self.git.log(skip, HISTORY_PAGE) {
                Ok(commits) => self.enqueue(Action::HistoryLoaded { skip, commits }),
                Err(e) => self.enqueue(Action::Error(format!("log: {e}"))),
            },
            Action::LoadCommitFiles(hash) => match self.git.commit_files(&hash) {
                Ok(files) => self.enqueue(Action::CommitFilesLoaded { hash, files }),
                Err(e) => self.enqueue(Action::Error(format!("commit files: {e}"))),
            },
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
            Action::GenerateCommitMessage => self.start_ai_generation(),
            Action::CancelCommitMessage => self.ai.cancel(),
            Action::ToggleAiProvider => self.toggle_ai_provider(),
            Action::Error(msg) => {
                self.message = Some((msg, true));
            }
            Action::BranchLoaded(branch) => self.branch = branch,
            // Handled entirely by components via `update`: Confirm opens the
            // overlay, DiffPrev/DiffNext scroll the diff view, the *Loaded
            // data actions feed the panels.
            Action::DiffLoaded(_)
            | Action::DiffReloaded(_)
            | Action::Confirm { .. }
            | Action::DiffPrevChange
            | Action::DiffNextChange
            | Action::HistoryLoaded { .. }
            | Action::CommitFilesLoaded { .. }
            | Action::CommitMessageGenerating { .. }
            | Action::CommitMessageGenerated(_)
            | Action::CommitMessageFailed
            | Action::PreferencesChanged(_) => {}
        }
    }

    /// Persist the sidebar layout when it changed and no drag is in flight.
    /// Called once per loop iteration after `dispatch`.
    fn sync_prefs(&mut self) {
        let cur = self.sidebar.to_prefs();
        if self.sidebar.is_dragging() || cur == self.prefs.layout {
            return;
        }
        self.prefs.layout = cur;
        self.save_prefs();
    }

    /// Write `self.prefs` through the store (skipped entirely when the
    /// initial load failed — a corrupt file is never overwritten) and
    /// broadcast `PreferencesChanged`.
    fn save_prefs(&mut self) {
        if self.prefs_writable {
            if let Err(e) = self.store.save(&self.prefs) {
                self.enqueue(Action::Error(format!("saving preferences: {e}")));
            }
        }
        self.enqueue(Action::PreferencesChanged(self.prefs.clone()));
    }

    /// Non-blocking check on the background AI generation; a finished
    /// outcome becomes broadcast actions. Called once per loop iteration.
    fn poll_ai(&mut self) {
        let Some(outcome) = self.ai.poll() else {
            return;
        };
        match outcome {
            AiOutcome::Done(raw) => match ai::clean_output(&raw) {
                Some(msg) => self.enqueue(Action::CommitMessageGenerated(msg)),
                None => {
                    self.enqueue(Action::CommitMessageFailed);
                    self.enqueue(Action::Error("AI returned an empty message".to_string()));
                }
            },
            AiOutcome::Failed(e) => {
                self.enqueue(Action::CommitMessageFailed);
                self.enqueue(Action::Error(e));
            }
            AiOutcome::Cancelled => {
                self.enqueue(Action::CommitMessageFailed);
                self.message = Some(("Generation cancelled".to_string(), false));
            }
        }
    }

    /// `GenerateCommitMessage`: gather the staged diff and recent subjects,
    /// then spawn the configured AI CLI in the background.
    fn start_ai_generation(&mut self) {
        if self.ai.is_running() {
            return;
        }
        let staged = self
            .last_status
            .iter()
            .any(|f| f.section == Section::Staged);
        if !staged {
            self.enqueue(Action::Error("Nothing staged".to_string()));
            return;
        }
        let prompt = match self.git.staged_patch().and_then(|(stat, diff)| {
            let subjects = self
                .git
                .log(0, 10)?
                .into_iter()
                .map(|c| c.subject)
                .collect::<Vec<_>>();
            Ok(ai::build_prompt(
                &subjects,
                &stat,
                &diff,
                self.prefs.ai.max_diff_chars,
            ))
        }) {
            Ok(p) => p,
            Err(e) => {
                self.enqueue(Action::Error(format!("ai: {e}")));
                return;
            }
        };
        let cmd = ai::command(&self.prefs.ai, &self.git.root());
        match self.ai.start(cmd, prompt) {
            Ok(()) => {
                let provider = self.ai_provider_label();
                self.enqueue(Action::CommitMessageGenerating {
                    provider: provider.clone(),
                });
                self.message = Some((
                    format!("Generating commit message with {provider}… (Esc to cancel)"),
                    false,
                ));
            }
            Err(e) => self.enqueue(Action::Error(format!("{e}"))),
        }
    }

    /// "codex (gpt-6-luna)"-style label for status/progress messages.
    fn ai_provider_label(&self) -> String {
        let ai = &self.prefs.ai;
        match ai.provider {
            AiProvider::Codex => format!("codex ({})", ai.codex_model),
            AiProvider::Opencode => format!("opencode ({})", ai.opencode_model),
        }
    }

    /// `ToggleAiProvider`: flip the provider and persist it.
    fn toggle_ai_provider(&mut self) {
        self.prefs.ai.provider = match self.prefs.ai.provider {
            AiProvider::Codex => AiProvider::Opencode,
            AiProvider::Opencode => AiProvider::Codex,
        };
        self.save_prefs();
        self.message = Some((
            format!("AI provider: {}", self.prefs.ai.provider.name()),
            false,
        ));
    }

    /// Reload the diff of the currently selected file, if it still exists in
    /// `last_status`. Broadcasts `DiffReloaded` only when the doc changed.
    /// Commit diffs are immutable — never reloaded.
    fn reload_selected_diff(&mut self) {
        let Some(DiffSource::Working(sel)) = &self.selected else {
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
        if !self.sidebar.visible && self.focus != PanelId::DiffView {
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
        // AI keys work from every panel — including while typing a message.
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('g') => {
                    self.enqueue(Action::GenerateCommitMessage);
                    return;
                }
                KeyCode::Char('t') => {
                    self.enqueue(Action::ToggleAiProvider);
                    return;
                }
                _ => {}
            }
        }
        // Esc cancels an in-flight generation before its other meanings.
        if key.code == KeyCode::Esc && self.ai.is_running() {
            self.enqueue(Action::CancelCommitMessage);
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
                    KeyCode::Char('3') => {
                        self.sidebar.visible = true;
                        self.enqueue(Action::Focus(PanelId::History));
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
        if let Some(r) = pr.history {
            self.rects.insert(PanelId::History, r);
        }
        self.rects.insert(PanelId::DiffView, pr.diff);

        for (id, comp) in &mut self.components {
            if let Some(rect) = self.rects.get(id).copied() {
                comp.render(f, rect, *id == self.focus);
            }
        }
        // Recolor the divider columns and the changes/history split row while
        // hovered/dragged so the user can see they are draggable.
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
            if let (Some(row), Some(panel)) = (pr.split_row, pr.changes) {
                for col in panel.x..panel.x + panel.width {
                    if let Some(cell) = f.buffer_mut().cell_mut((col, row)) {
                        cell.set_fg(Color::Cyan);
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
    use crate::ai::{AiCommand, AiRunner};
    use crate::git::{Commit, CommitFile, DiffDoc, FileChange};
    use crate::prefs::{FileStore, LayoutPrefs, MemoryStore};
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;

    /// Minimal backend so `App` can be constructed without a repo.
    struct FakeGit;

    impl GitBackend for FakeGit {
        fn root(&self) -> PathBuf {
            PathBuf::from("/repo")
        }
        fn status(&self) -> Result<Vec<FileChange>> {
            Ok(vec![])
        }
        fn diff(&self, _file: &FileChange) -> Result<DiffDoc> {
            Ok(DiffDoc {
                path: String::new(),
                rows: vec![],
                binary: false,
            })
        }
        fn stage(&self, _path: &str) -> Result<()> {
            Ok(())
        }
        fn unstage(&self, _path: &str) -> Result<()> {
            Ok(())
        }
        fn discard(&self, _file: &FileChange) -> Result<()> {
            Ok(())
        }
        fn stage_all(&self) -> Result<()> {
            Ok(())
        }
        fn unstage_all(&self) -> Result<()> {
            Ok(())
        }
        fn commit(&self, _message: &str) -> Result<()> {
            Ok(())
        }
        fn branch(&self) -> Result<String> {
            Ok("main".to_string())
        }
        fn head(&self) -> Result<Option<String>> {
            Ok(None)
        }
        fn log(&self, _skip: usize, _limit: usize) -> Result<Vec<Commit>> {
            Ok(vec![])
        }
        fn commit_files(&self, _hash: &str) -> Result<Vec<CommitFile>> {
            Ok(vec![])
        }
        fn commit_diff(&self, _hash: &str, _file: &CommitFile) -> Result<DiffDoc> {
            self.diff(&FileChange {
                path: String::new(),
                orig_path: None,
                section: Section::Unstaged,
                code: 'M',
            })
        }
        fn staged_patch(&self) -> Result<(String, String)> {
            Ok((
                " f.txt | 2 +-".to_string(),
                "diff --git a/f.txt b/f.txt\n+STAGED_PATCH".to_string(),
            ))
        }
    }

    /// Records what `start` received and lets the test inject an outcome.
    #[derive(Clone, Default)]
    struct FakeRunner {
        state: Rc<RefCell<FakeAiState>>,
    }

    #[derive(Default)]
    struct FakeAiState {
        running: bool,
        started: Option<(AiCommand, String)>,
        outcome: Option<AiOutcome>,
    }

    impl AiRunner for FakeRunner {
        fn start(&mut self, cmd: AiCommand, stdin: String) -> Result<()> {
            if self.state.borrow().running {
                anyhow::bail!("already running");
            }
            let mut s = self.state.borrow_mut();
            s.running = true;
            s.started = Some((cmd, stdin));
            Ok(())
        }
        fn poll(&mut self) -> Option<AiOutcome> {
            let mut s = self.state.borrow_mut();
            let outcome = s.outcome.take();
            if outcome.is_some() {
                s.running = false;
            }
            outcome
        }
        fn cancel(&mut self) {
            let mut s = self.state.borrow_mut();
            if s.running {
                s.outcome = Some(AiOutcome::Cancelled);
            }
        }
        fn is_running(&self) -> bool {
            self.state.borrow().running
        }
    }

    fn staged_file() -> FileChange {
        FileChange {
            path: "f.txt".to_string(),
            orig_path: None,
            section: Section::Staged,
            code: 'M',
        }
    }

    fn app(store: MemoryStore) -> App {
        App::new(Box::new(FakeGit), Box::new(store))
    }

    fn mouse(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: col,
            row,
            modifiers: KeyModifiers::empty(),
        }
    }

    #[test]
    fn prefs_loaded_applies_sidebar_width() {
        let store = MemoryStore::new(Some(Preferences {
            layout: LayoutPrefs {
                sidebar_width: Some(50),
                ..Default::default()
            },
            ..Default::default()
        }));
        let app = app(store);
        assert_eq!(app.sidebar.width_for(200), 50);
    }

    #[test]
    fn resize_key_persists_width() {
        let store = MemoryStore::new(Some(Preferences {
            layout: LayoutPrefs {
                sidebar_width: Some(50),
                ..Default::default()
            },
            ..Default::default()
        }));
        let mut app = app(store.clone());
        app.main_area = Rect::new(0, 0, 200, 30);
        app.dispatch(); // flush the startup queue
        app.on_key(KeyEvent::from(KeyCode::Char(']')));
        app.dispatch();
        app.sync_prefs();
        assert_eq!(
            store.saved().unwrap().layout.sidebar_width,
            Some(54),
            "] on width 50 must persist 54"
        );
    }

    #[test]
    fn drags_save_once_on_release() {
        let store = MemoryStore::new(None);
        let mut app = app(store.clone());
        app.main_area = Rect::new(0, 0, 200, 30);
        // Press on the divider (default width 35% of 200 = 70 -> cols 69/70).
        assert!(app.sidebar.on_mouse(
            &mouse(MouseEventKind::Down(MouseButton::Left), 69, 5),
            app.main_area
        ));
        app.sync_prefs();
        assert_eq!(store.save_count(), 0, "nothing saved mid-press");
        assert!(app.sidebar.on_mouse(
            &mouse(MouseEventKind::Drag(MouseButton::Left), 90, 5),
            app.main_area
        ));
        app.sync_prefs();
        assert_eq!(store.save_count(), 0, "nothing saved mid-drag");
        assert!(app.sidebar.on_mouse(
            &mouse(MouseEventKind::Up(MouseButton::Left), 90, 5),
            app.main_area
        ));
        app.sync_prefs();
        assert_eq!(store.save_count(), 1, "one save after release");
        assert_eq!(store.saved().unwrap().layout.sidebar_width, Some(91));
    }

    #[test]
    fn failed_load_never_saves() {
        let store = MemoryStore::failing_load("corrupt prefs");
        let mut app = app(store.clone());
        app.main_area = Rect::new(0, 0, 200, 30);
        app.dispatch();
        assert!(
            matches!(&app.message, Some((m, true)) if m.contains("preferences")),
            "load error should surface in the status bar: {:?}",
            app.message
        );
        app.on_key(KeyEvent::from(KeyCode::Char(']')));
        app.dispatch();
        app.sync_prefs();
        assert_eq!(store.save_count(), 0);
    }

    #[test]
    fn invalid_prefs_file_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.toml");
        std::fs::write(&path, "[[[bad").unwrap();
        let mut app = App::new(Box::new(FakeGit), Box::new(FileStore::new(path.clone())));
        app.main_area = Rect::new(0, 0, 200, 30);
        app.dispatch();
        app.on_key(KeyEvent::from(KeyCode::Char(']')));
        app.dispatch();
        app.sync_prefs();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[[[bad");
    }

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

    /// An App with a FakeRunner and, when `staged`, a staged change loaded.
    fn ai_app(staged: bool) -> (App, FakeRunner) {
        let runner = FakeRunner::default();
        let mut app = app(MemoryStore::new(None)).with_ai_runner(Box::new(runner.clone()));
        app.dispatch(); // flush the startup queue
        if staged {
            app.enqueue(Action::StatusLoaded(vec![staged_file()]));
            app.dispatch();
        }
        (app, runner)
    }

    #[test]
    fn generate_with_nothing_staged_errors_without_starting() {
        let (mut app, runner) = ai_app(false);
        app.enqueue(Action::GenerateCommitMessage);
        app.dispatch();
        assert!(
            matches!(&app.message, Some((m, true)) if m == "Nothing staged"),
            "{:?}",
            app.message
        );
        assert!(runner.state.borrow().started.is_none());
    }

    #[test]
    fn generate_starts_runner_with_staged_patch() {
        let (mut app, runner) = ai_app(true);
        app.enqueue(Action::GenerateCommitMessage);
        app.dispatch();
        let s = runner.state.borrow();
        let (cmd, stdin) = s.started.as_ref().expect("runner should start");
        assert_eq!(cmd.program, "codex");
        assert!(stdin.contains("+STAGED_PATCH"), "stdin: {stdin}");
        drop(s);
        assert!(
            matches!(&app.message, Some((m, false)) if m.contains("Generating commit message")),
            "{:?}",
            app.message
        );
    }

    #[test]
    fn generated_message_fills_commit_input() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let (mut app, runner) = ai_app(true);
        app.enqueue(Action::GenerateCommitMessage);
        app.dispatch();
        runner.state.borrow_mut().outcome =
            Some(AiOutcome::Done("```\nfeat: add x\n```\n".to_string()));
        app.poll_ai();
        app.dispatch();
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        term.draw(|f| app.render(f)).unwrap();
        let buf = term.backend().buffer();
        let mut text = String::new();
        for y in buf.area.top()..buf.area.bottom() {
            for x in buf.area.left()..buf.area.right() {
                text.push_str(buf[(x, y)].symbol());
            }
            text.push('\n');
        }
        assert!(text.contains("feat: add x"), "buffer:\n{text}");
    }

    #[test]
    fn failed_generation_shows_error_status() {
        let (mut app, runner) = ai_app(true);
        app.enqueue(Action::GenerateCommitMessage);
        app.dispatch();
        runner.state.borrow_mut().outcome = Some(AiOutcome::Failed("boom".to_string()));
        app.poll_ai();
        app.dispatch();
        assert!(
            matches!(&app.message, Some((m, true)) if m == "boom"),
            "{:?}",
            app.message
        );
    }

    #[test]
    fn esc_cancels_running_generation() {
        let (mut app, runner) = ai_app(true);
        app.enqueue(Action::GenerateCommitMessage);
        app.dispatch();
        assert!(runner.state.borrow().running);
        app.on_key(KeyEvent::from(KeyCode::Esc));
        app.dispatch(); // CancelCommitMessage -> runner.cancel()
        app.poll_ai();
        app.dispatch(); // CommitMessageFailed
        assert!(!runner.state.borrow().running);
        assert!(
            matches!(&app.message, Some((m, false)) if m == "Generation cancelled"),
            "{:?}",
            app.message
        );
    }

    #[test]
    fn toggle_provider_persists() {
        let store = MemoryStore::new(None);
        let mut app = app(store.clone());
        app.dispatch();
        app.enqueue(Action::ToggleAiProvider);
        app.dispatch();
        assert_eq!(
            store.saved().unwrap().ai.provider,
            AiProvider::Opencode,
            "provider should persist as opencode"
        );
        assert!(
            matches!(&app.message, Some((m, false)) if m == "AI provider: opencode"),
            "{:?}",
            app.message
        );
    }
}
