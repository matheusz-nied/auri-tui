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
use crate::components::file_tree::FileTree;
use crate::components::file_view::FileView;
use crate::components::history::{History, HISTORY_PAGE};
use crate::components::hitbox::Hitboxes;
use crate::components::view_tabs;
use crate::editor::EditorLauncher;
use crate::event::{AppEvent, Events};
use crate::fs::{FileStamp, FsBackend};
use crate::git::{DiffSource, GitBackend, Section};
use crate::layout::{self, Sidebar, SidebarView};
use crate::prefs::{Preferences, PrefsStore};

/// The file shown by `FileView`; `stamp` is its state when last read,
/// compared on refresh to decide whether to re-read it.
#[derive(Debug, Clone, PartialEq)]
struct OpenFile {
    path: String,
    stamp: Option<FileStamp>,
}

/// Gives the real terminal to a child process that may prompt on it (GPG
/// pinentry, an interactive commit hook) and takes it back. The TUI draws
/// in raw mode on the alternate screen, where such a prompt would be
/// unusable and scribble over the frame.
pub trait TerminalHandoff {
    /// Leave raw mode / the alternate screen.
    fn release(&mut self);
    /// Re-enter them; the next frame is redrawn from scratch.
    fn reclaim(&mut self);
}

/// No terminal to hand over (tests).
struct NoHandoff;

impl TerminalHandoff for NoHandoff {
    fn release(&mut self) {}
    fn reclaim(&mut self) {}
}

/// No editor to run (tests): opening one is an error.
struct NoEditor;

impl EditorLauncher for NoEditor {
    fn open(&mut self, _path: &str, _line: Option<usize>) -> Result<()> {
        anyhow::bail!("no editor available")
    }
}

/// Owns the components, the focus state, the last-frame layout rects and the
/// action queue. It is the *only* place where `GitBackend` and `FsBackend`
/// are called: side effects requested by components (`Refresh`,
/// `ToggleStage`, `Commit`, `SelectFile`, `LoadDirs`, `OpenFile`) are
/// executed here and their results are broadcast back to all components as
/// new actions.
pub struct App {
    git: Box<dyn GitBackend>,
    /// Workspace reads for the explorer and the file viewer.
    fs: Box<dyn FsBackend>,
    /// Background AI commit-message generation; polled once per loop.
    ai: Box<dyn AiRunner>,
    /// Hands the terminal to `git commit` when it may prompt, and to the
    /// editor.
    terminal: Box<dyn TerminalHandoff>,
    /// Runs `$VISUAL`/`$EDITOR` for `OpenInEditor`.
    editor: Box<dyn EditorLauncher>,
    /// The screen was used by someone else: clear before the next draw.
    needs_clear: bool,
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
    /// Resizable/collapsible left column (Explorer or Source Control view).
    sidebar: Sidebar,
    /// Area above the status bar from the last render; the divider lives in it.
    main_area: Rect,
    /// Last status snapshot, for the "nothing staged" commit check.
    last_status: Vec<crate::git::FileChange>,
    branch: String,
    /// What `DiffView` shows: a working-tree file or a commit file.
    diff: Option<DiffSource>,
    /// What `FileView` shows. The main pane follows the sidebar view:
    /// Source Control -> diff, Explorer -> file; both are kept.
    file: Option<OpenFile>,
    /// Explorer/Source Control tab buttons drawn last frame.
    tab_hits: Hitboxes,
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
    /// (message, is_error) shown in the status bar; set it with `notify`.
    /// Cleared by the next key press or after `MESSAGE_TTL`/`ERROR_TTL`.
    message: Option<(String, bool)>,
    /// When `message` was set.
    message_at: Instant,
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

/// How long an info message stays in the status bar.
const MESSAGE_TTL: Duration = Duration::from_secs(4);
/// Errors stay longer — they may need reading.
const ERROR_TTL: Duration = Duration::from_secs(10);

/// Whether a `Tick` may enqueue `Refresh`: only once input has been quiet for
/// `REFRESH_IDLE` (or no input has ever arrived).
fn should_refresh(last_input: Option<Instant>, now: Instant) -> bool {
    last_input.is_none_or(|t| now.duration_since(t) >= REFRESH_IDLE)
}

impl App {
    pub fn new(
        git: Box<dyn GitBackend>,
        fs: Box<dyn FsBackend>,
        store: Box<dyn PrefsStore>,
    ) -> Self {
        let root_name = git
            .root()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut app = Self {
            git,
            fs,
            ai: Box::new(ProcessRunner::default()),
            terminal: Box::new(NoHandoff),
            editor: Box::new(NoEditor),
            needs_clear: false,
            events: Events::default(),
            components: vec![
                (PanelId::CommitInput, Box::new(CommitInput::default())),
                (PanelId::Changes, Box::new(Changes::default())),
                (PanelId::History, Box::new(History::default())),
                (PanelId::Explorer, Box::new(FileTree::new(root_name))),
                (PanelId::DiffView, Box::new(DiffView::default())),
                (PanelId::FileView, Box::new(FileView::default())),
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
            diff: None,
            file: None,
            tab_hits: Hitboxes::default(),
            last_head: None,
            seen_head: false,
            last_diff: None,
            prefs: Preferences::default(),
            store,
            prefs_writable: true,
            message: None,
            message_at: Instant::now(),
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
        // A persisted Explorer view starts focused on the tree.
        app.ensure_focus_visible();
        app.enqueue(Action::PreferencesChanged(app.prefs.clone()));
        app
    }

    /// Set how the terminal is handed to prompting child processes (the
    /// default does nothing — fine without a real terminal).
    pub fn with_terminal_handoff(mut self, t: Box<dyn TerminalHandoff>) -> Self {
        self.terminal = t;
        self
    }

    /// Set how files are opened for editing (the default refuses — there
    /// is no terminal to give an editor in tests).
    pub fn with_editor(mut self, e: Box<dyn EditorLauncher>) -> Self {
        self.editor = e;
        self
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
            self.expire_message(Instant::now());
            self.dispatch();
            self.sync_prefs();
            if self.needs_clear {
                // Forget the last frame: the screen holds someone else's
                // output now, and only a full redraw replaces it.
                terminal.clear()?;
                self.needs_clear = false;
                self.dirty = true;
            }
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
                self.reload_open_file();
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
                self.diff = Some(DiffSource::Working(file.clone()));
                match self.git.diff(&file) {
                    Ok(doc) => {
                        self.last_diff = Some(doc.clone());
                        self.enqueue(Action::DiffLoaded(doc));
                    }
                    Err(e) => self.enqueue(Action::Error(format!("diff: {e}"))),
                }
            }
            Action::SelectCommitFile { commit, file } => {
                self.diff = Some(DiffSource::Commit {
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
            Action::SetSidebarView(view) => self.set_view(view),
            Action::LoadDirs(dirs) => {
                for dir in dirs {
                    match self.fs.read_dir(&dir) {
                        Ok(Some(entries)) => self.enqueue(Action::DirLoaded { path: dir, entries }),
                        // Vanished: the parent's fresh listing drops it.
                        Ok(None) => {}
                        Err(e) => self.enqueue(Action::Error(format!("explorer: {e}"))),
                    }
                }
            }
            Action::OpenFile(path) => {
                // Stamp first: a write racing the read just causes one extra
                // reload on the next refresh.
                let stamp = self.fs.stamp(&path).ok().flatten();
                match self.fs.read_file(&path) {
                    Ok(doc) => {
                        self.file = Some(OpenFile { path, stamp });
                        self.enqueue(Action::FileLoaded(doc));
                    }
                    Err(e) => self.enqueue(Action::Error(format!("open: {e}"))),
                }
            }
            Action::OpenInEditor { path, line } => self.open_in_editor(&path, line),
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
                    let handoff = self.git.commit_may_prompt();
                    if handoff {
                        self.terminal.release();
                    }
                    let result = self.git.commit(&msg);
                    if handoff {
                        self.terminal.reclaim();
                        self.needs_clear = true;
                    }
                    match result {
                        Ok(()) => {
                            self.enqueue(Action::CommitDone);
                            self.enqueue(Action::Refresh);
                        }
                        Err(e) => self.enqueue(Action::Error(format!("commit: {e}"))),
                    }
                }
            }
            Action::CommitDone => {
                self.notify("Committed", false);
            }
            Action::GenerateCommitMessage => self.start_ai_generation(),
            Action::CancelCommitMessage => self.ai.cancel(),
            Action::ToggleAiProvider => self.toggle_ai_provider(),
            Action::Error(msg) => {
                self.notify(msg, true);
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
            | Action::DirLoaded { .. }
            | Action::FileLoaded(_)
            | Action::FileReloaded(_)
            | Action::ExplorerCollapseAll
            | Action::PreferencesChanged(_) => {}
        }
    }

    /// Show `msg` in the status bar (red when `is_error`).
    fn notify(&mut self, msg: impl Into<String>, is_error: bool) {
        self.message = Some((msg.into(), is_error));
        self.message_at = Instant::now();
    }

    /// Drop the status message once it has been shown for its TTL. Called
    /// once per loop iteration (at least every event-poll timeout).
    fn expire_message(&mut self, now: Instant) {
        let Some((_, is_error)) = &self.message else {
            return;
        };
        let ttl = if *is_error { ERROR_TTL } else { MESSAGE_TTL };
        if now.duration_since(self.message_at) >= ttl {
            self.message = None;
            self.dirty = true;
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
                self.notify("Generation cancelled", false);
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
                self.notify(
                    format!("Generating commit message with {provider}… (Esc to cancel)"),
                    false,
                );
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
        self.notify(
            format!("AI provider: {}", self.prefs.ai.provider.name()),
            false,
        );
    }

    /// Run the editor on `path` with the terminal handed over, then refresh
    /// so the edit shows up in the status, diff and file viewer.
    fn open_in_editor(&mut self, path: &str, line: Option<usize>) {
        match self.fs.stamp(path) {
            Ok(Some(_)) => {}
            Ok(None) => {
                self.enqueue(Action::Error(format!("edit: {path} no longer exists")));
                return;
            }
            Err(e) => {
                self.enqueue(Action::Error(format!("edit: {e}")));
                return;
            }
        }
        let abs = self.git.root().join(path);
        self.terminal.release();
        let result = self.editor.open(&abs.to_string_lossy(), line);
        self.terminal.reclaim();
        self.needs_clear = true;
        if let Err(e) = result {
            self.enqueue(Action::Error(format!("edit: {e}")));
        }
        // Even a failed editor may have saved.
        self.enqueue(Action::Refresh);
    }

    /// Reload the diff of the currently selected file, if it still exists in
    /// `last_status`. Broadcasts `DiffReloaded` only when the doc changed.
    /// Commit diffs are immutable — never reloaded.
    fn reload_selected_diff(&mut self) {
        let Some(DiffSource::Working(sel)) = &self.diff else {
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

    /// Re-read the open file when its stamp changed on disk. A deleted
    /// file keeps showing its last contents.
    fn reload_open_file(&mut self) {
        let Some(OpenFile { path, stamp }) = &self.file else {
            return;
        };
        let (path, old) = (path.clone(), *stamp);
        match self.fs.stamp(&path) {
            Ok(Some(new)) if Some(new) != old => match self.fs.read_file(&path) {
                Ok(doc) => {
                    self.file = Some(OpenFile {
                        path,
                        stamp: Some(new),
                    });
                    self.enqueue(Action::FileReloaded(doc));
                }
                Err(e) => self.enqueue(Action::Error(format!("open: {e}"))),
            },
            Ok(_) => {}
            Err(e) => self.enqueue(Action::Error(format!("open: {e}"))),
        }
    }

    /// The panel that owns the main pane — it follows the sidebar view
    /// (also while the sidebar is hidden).
    fn main_panel(&self) -> PanelId {
        match self.sidebar.view {
            SidebarView::Explorer => PanelId::FileView,
            SidebarView::SourceControl => PanelId::DiffView,
        }
    }

    /// Switch the sidebar view (tab click / `SetSidebarView`), revealing the
    /// sidebar. Focus on the main pane stays on the main pane; focus on a
    /// sidebar panel moves to the new view's list.
    fn set_view(&mut self, view: SidebarView) {
        let on_main = matches!(self.focus, PanelId::DiffView | PanelId::FileView);
        self.sidebar.visible = true;
        self.sidebar.view = view;
        if on_main {
            self.focus = self.main_panel();
        }
        self.ensure_focus_visible();
    }

    /// Whether a panel is on screen: sidebar panels only in their view while
    /// the sidebar is shown, main-pane panels only while they own it.
    fn panel_visible(&self, id: PanelId) -> bool {
        let view = self.sidebar.visible.then_some(self.sidebar.view);
        match id {
            PanelId::CommitInput | PanelId::Changes | PanelId::History => {
                view == Some(SidebarView::SourceControl)
            }
            PanelId::Explorer => view == Some(SidebarView::Explorer),
            PanelId::DiffView | PanelId::FileView => id == self.main_panel(),
        }
    }

    /// Move focus off a panel that just disappeared: to the sidebar view's
    /// main list, or to the main pane when the sidebar is hidden.
    fn ensure_focus_visible(&mut self) {
        if self.panel_visible(self.focus) {
            return;
        }
        self.focus = match (self.sidebar.visible, self.sidebar.view) {
            (true, SidebarView::Explorer) => PanelId::Explorer,
            (true, SidebarView::SourceControl) => PanelId::Changes,
            (false, _) => self.main_panel(),
        };
    }

    /// Show the sidebar in `view` and focus `panel` inside it.
    fn show_view(&mut self, view: SidebarView, panel: PanelId) {
        self.set_view(view);
        self.focus = panel;
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

    /// `b` — hide/show the sidebar, moving focus to the main pane if it
    /// pointed at a now-hidden panel.
    fn toggle_sidebar(&mut self) {
        self.sidebar.toggle();
        self.ensure_focus_visible();
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
                    // Reveal the sidebar (in the right view) when focusing a
                    // panel inside it.
                    KeyCode::Char('e') => {
                        self.show_view(SidebarView::Explorer, PanelId::Explorer);
                        return;
                    }
                    KeyCode::Char('c') => {
                        self.show_view(SidebarView::SourceControl, PanelId::CommitInput);
                        return;
                    }
                    KeyCode::Char('1') => {
                        self.show_view(SidebarView::SourceControl, PanelId::Changes);
                        return;
                    }
                    KeyCode::Char('3') => {
                        self.show_view(SidebarView::SourceControl, PanelId::History);
                        return;
                    }
                    KeyCode::Char('2') => {
                        self.enqueue(Action::Focus(self.main_panel()));
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
        // Explorer / Source Control tabs atop the sidebar.
        if matches!(ev.kind, MouseEventKind::Down(_)) {
            if let Some(action) = self.tab_hits.hit(ev.column, ev.row) {
                self.enqueue(action);
                return;
            }
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
        if let Some(r) = pr.explorer {
            self.rects.insert(PanelId::Explorer, r);
        }
        self.rects.insert(self.main_panel(), pr.diff);

        for (id, comp) in &mut self.components {
            if let Some(rect) = self.rects.get(id).copied() {
                comp.render(f, rect, *id == self.focus);
            }
        }
        self.tab_hits.clear();
        if let Some(r) = pr.tabs {
            view_tabs::render(f, r, self.sidebar.view, &mut self.tab_hits);
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
            "q quit · tab focus · r refresh · e files · 1 git · b sidebar · [/] resize"
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
    use crate::fs::{DirEntry, EntryKind, FileDoc};
    use crate::git::{Commit, CommitFile, DiffDoc, FileChange};
    use crate::prefs::{FileStore, LayoutPrefs, MemoryStore};
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;

    /// Minimal backend so `App` can be constructed without a repo.
    #[derive(Default)]
    struct FakeGit {
        /// When set, commits "may prompt" and are recorded here.
        prompting: Option<CallLog>,
    }

    type CallLog = Rc<RefCell<Vec<&'static str>>>;

    /// Records handoffs into the same log as `FakeGit`'s commits.
    struct FakeHandoff(CallLog);

    impl TerminalHandoff for FakeHandoff {
        fn release(&mut self) {
            self.0.borrow_mut().push("release");
        }
        fn reclaim(&mut self) {
            self.0.borrow_mut().push("reclaim");
        }
    }

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
            if let Some(log) = &self.prompting {
                log.borrow_mut().push("commit");
            }
            Ok(())
        }
        fn commit_may_prompt(&self) -> bool {
            self.prompting.is_some()
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

    /// In-memory workspace: directory listings plus file contents with a
    /// version number that stands in for the on-disk stamp.
    #[derive(Clone, Default)]
    struct FakeFs {
        dirs: Rc<RefCell<HashMap<String, Vec<DirEntry>>>>,
        files: Rc<RefCell<HashMap<String, (String, u64)>>>,
    }

    impl FakeFs {
        /// Root with `src/` and `README.md`, `src/` holding `main.rs`.
        fn sample() -> Self {
            let fs = FakeFs::default();
            let e = |path: &str, kind| DirEntry {
                name: path.rsplit('/').next().unwrap().to_string(),
                path: path.to_string(),
                kind,
            };
            fs.dirs.borrow_mut().insert(
                String::new(),
                vec![e("src", EntryKind::Dir), e("README.md", EntryKind::File)],
            );
            fs.dirs
                .borrow_mut()
                .insert("src".to_string(), vec![e("src/main.rs", EntryKind::File)]);
            fs.write("README.md", "hello readme");
            fs.write("src/main.rs", "fn main() {}");
            fs
        }

        fn write(&self, path: &str, text: &str) {
            let mut files = self.files.borrow_mut();
            let version = files.get(path).map_or(0, |(_, v)| v + 1);
            files.insert(path.to_string(), (text.to_string(), version));
        }
    }

    impl FsBackend for FakeFs {
        fn read_dir(&self, dir: &str) -> Result<Option<Vec<DirEntry>>> {
            Ok(self.dirs.borrow().get(dir).cloned())
        }
        fn read_file(&self, path: &str) -> Result<FileDoc> {
            match self.files.borrow().get(path) {
                Some((text, _)) => Ok(crate::fs::decode_file(path, text.as_bytes(), false)),
                None => anyhow::bail!("{path}: not found"),
            }
        }
        fn stamp(&self, path: &str) -> Result<Option<FileStamp>> {
            Ok(self.files.borrow().get(path).map(|(_, v)| FileStamp {
                modified: None,
                len: *v,
            }))
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
        App::new(
            Box::new(FakeGit::default()),
            Box::new(FakeFs::default()),
            Box::new(store),
        )
    }

    /// App over `FakeFs::sample()` with the first refresh (root listing)
    /// already processed.
    fn explorer_app(store: MemoryStore) -> (App, FakeFs) {
        let fs = FakeFs::sample();
        let mut app = App::new(
            Box::new(FakeGit::default()),
            Box::new(fs.clone()),
            Box::new(store),
        );
        app.enqueue(Action::Refresh);
        app.dispatch();
        (app, fs)
    }

    /// Render the whole app into a 120x30 test terminal and return its text.
    fn screen(app: &mut App) -> String {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
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
        text
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
        let mut app = App::new(
            Box::new(FakeGit::default()),
            Box::new(FakeFs::default()),
            Box::new(FileStore::new(path.clone())),
        );
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

    #[test]
    fn explorer_lists_root_and_opens_file_in_viewer() {
        let (mut app, _fs) = explorer_app(MemoryStore::new(None));
        app.on_key(KeyEvent::from(KeyCode::Char('e')));
        assert_eq!(app.focus, PanelId::Explorer);
        let text = screen(&mut app);
        assert!(text.contains("Explorer · repo"), "{text}");
        assert!(
            text.contains("▸ ▰  src") && text.contains("M↓ README.md"),
            "{text}"
        );

        app.enqueue(Action::OpenFile("README.md".to_string()));
        app.dispatch();
        assert_eq!(app.main_panel(), PanelId::FileView);
        let text = screen(&mut app);
        assert!(text.contains("1 hello readme"), "{text}");
        assert!(app.rects.contains_key(&PanelId::FileView));
        assert!(!app.rects.contains_key(&PanelId::DiffView));

        // A diff selected meanwhile doesn't take the pane from the file.
        app.enqueue(Action::SelectFile(staged_file()));
        app.dispatch();
        assert_eq!(app.main_panel(), PanelId::FileView);
    }

    #[test]
    fn main_pane_follows_the_view_and_keeps_both_contents() {
        let (mut app, _fs) = explorer_app(MemoryStore::new(None));
        app.on_key(KeyEvent::from(KeyCode::Char('e')));
        app.enqueue(Action::OpenFile("README.md".to_string()));
        app.dispatch();
        app.on_key(KeyEvent::from(KeyCode::Char('1')));
        assert_eq!(app.main_panel(), PanelId::DiffView);
        screen(&mut app);
        assert!(app.rects.contains_key(&PanelId::DiffView));
        assert!(!app.rects.contains_key(&PanelId::FileView));
        // Back in the Explorer the file is still there.
        app.on_key(KeyEvent::from(KeyCode::Char('e')));
        assert!(screen(&mut app).contains("1 hello readme"));
    }

    #[test]
    fn tab_bar_click_switches_views() {
        let (mut app, _fs) = explorer_app(MemoryStore::new(None));
        // 120 cols: sidebar = 42, tabs = two 21-col halves on row 0 (row 1
        // is spacing).
        let text = screen(&mut app);
        let labels = text.lines().next().unwrap();
        assert!(
            labels.contains("Explorer") && labels.contains("Source Control"),
            "{text}"
        );
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 5, 0));
        app.dispatch();
        assert_eq!(app.sidebar.view, SidebarView::Explorer);
        assert_eq!(app.focus, PanelId::Explorer, "focus leaves hidden Changes");
        screen(&mut app);
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 30, 0));
        app.dispatch();
        assert_eq!(app.sidebar.view, SidebarView::SourceControl);
        assert_eq!(app.focus, PanelId::Changes);
    }

    #[test]
    fn expanding_a_folder_loads_it_through_app() {
        let (mut app, _fs) = explorer_app(MemoryStore::new(None));
        app.on_key(KeyEvent::from(KeyCode::Char('e')));
        app.on_key(KeyEvent::from(KeyCode::Char('l'))); // expand `src`
        app.dispatch();
        let text = screen(&mut app);
        assert!(
            text.contains("▾ ▰  src") && text.contains("rs main.rs"),
            "{text}"
        );
    }

    #[test]
    fn open_file_reloads_only_when_changed_on_disk() {
        let (mut app, fs) = explorer_app(MemoryStore::new(None));
        app.on_key(KeyEvent::from(KeyCode::Char('e')));
        app.enqueue(Action::OpenFile("README.md".to_string()));
        app.dispatch();
        fs.write("README.md", "edited elsewhere");
        app.enqueue(Action::Refresh);
        app.dispatch();
        let text = screen(&mut app);
        assert!(text.contains("1 edited elsewhere"), "{text}");
        // Deleted: the last contents stay, no error.
        fs.files.borrow_mut().clear();
        app.enqueue(Action::Refresh);
        app.dispatch();
        assert!(screen(&mut app).contains("1 edited elsewhere"));
        assert!(app.message.is_none(), "{:?}", app.message);
    }

    #[test]
    fn prompting_commit_runs_with_the_terminal_handed_over() {
        let log = CallLog::default();
        let git = FakeGit {
            prompting: Some(log.clone()),
        };
        let mut app = App::new(
            Box::new(git),
            Box::new(FakeFs::default()),
            Box::new(MemoryStore::new(None)),
        )
        .with_terminal_handoff(Box::new(FakeHandoff(log.clone())));
        app.last_status = vec![staged_file()];
        app.enqueue(Action::Commit("msg".to_string()));
        app.dispatch();
        assert_eq!(*log.borrow(), ["release", "commit", "reclaim"]);
        assert!(
            app.needs_clear,
            "the next frame must be redrawn from scratch"
        );
    }

    #[test]
    fn plain_commit_keeps_the_terminal() {
        let log = CallLog::default();
        let mut app =
            app(MemoryStore::new(None)).with_terminal_handoff(Box::new(FakeHandoff(log.clone())));
        app.last_status = vec![staged_file()];
        app.enqueue(Action::Commit("msg".to_string()));
        app.dispatch();
        assert!(log.borrow().is_empty());
        assert!(!app.needs_clear);
        assert!(matches!(&app.message, Some((m, false)) if m == "Committed"));
    }

    /// (path, line) of each editor run.
    type Opened = Rc<RefCell<Vec<(String, Option<usize>)>>>;

    /// Records each open in the handoff log and "saves" the file.
    struct FakeEditor {
        log: CallLog,
        fs: FakeFs,
        opened: Opened,
    }

    impl EditorLauncher for FakeEditor {
        fn open(&mut self, path: &str, line: Option<usize>) -> Result<()> {
            self.log.borrow_mut().push("edit");
            self.opened.borrow_mut().push((path.to_string(), line));
            self.fs.write("src/main.rs", "fn main() { edited() }");
            Ok(())
        }
    }

    fn editor_app() -> (App, CallLog, Opened) {
        let (app, fs) = explorer_app(MemoryStore::new(None));
        let log = CallLog::default();
        let opened = Rc::default();
        let app = app
            .with_terminal_handoff(Box::new(FakeHandoff(log.clone())))
            .with_editor(Box::new(FakeEditor {
                log: log.clone(),
                fs,
                opened: Rc::clone(&opened),
            }));
        (app, log, opened)
    }

    #[test]
    fn editor_runs_with_the_terminal_handed_over_then_refreshes() {
        let (mut app, log, opened) = editor_app();
        app.on_key(KeyEvent::from(KeyCode::Char('e')));
        app.enqueue(Action::OpenFile("src/main.rs".to_string()));
        app.dispatch();
        // `o` in the file viewer edits at the top visible line.
        app.on_key(KeyEvent::from(KeyCode::Char('2')));
        app.dispatch();
        app.on_key(KeyEvent::from(KeyCode::Char('o')));
        app.dispatch();
        assert_eq!(*log.borrow(), ["release", "edit", "reclaim"]);
        assert_eq!(
            *opened.borrow(),
            [("/repo/src/main.rs".to_string(), Some(1))]
        );
        assert!(
            app.needs_clear,
            "the next frame must be redrawn from scratch"
        );
        // The refresh after the editor exits picks up the saved file.
        assert!(screen(&mut app).contains("edited()"));
    }

    #[test]
    fn editing_a_missing_file_errors_without_running_the_editor() {
        let (mut app, log, _) = editor_app();
        app.enqueue(Action::OpenInEditor {
            path: "gone.rs".to_string(),
            line: None,
        });
        app.dispatch();
        assert!(log.borrow().is_empty());
        assert!(
            matches!(&app.message, Some((m, true)) if m.contains("no longer exists")),
            "{:?}",
            app.message
        );
    }

    #[test]
    fn status_messages_expire() {
        let (mut app, _fs) = explorer_app(MemoryStore::new(None));
        app.enqueue(Action::CommitDone);
        app.dispatch();
        let t0 = app.message_at;
        app.expire_message(t0 + MESSAGE_TTL - Duration::from_millis(1));
        assert!(matches!(&app.message, Some((m, false)) if m == "Committed"));
        app.expire_message(t0 + MESSAGE_TTL);
        assert!(app.message.is_none());
        // Errors outlive info messages.
        app.enqueue(Action::Error("boom".to_string()));
        app.dispatch();
        let t0 = app.message_at;
        app.expire_message(t0 + MESSAGE_TTL);
        assert!(app.message.is_some());
        app.expire_message(t0 + ERROR_TTL);
        assert!(app.message.is_none());
    }

    #[test]
    fn failed_open_keeps_the_previous_file() {
        let (mut app, _fs) = explorer_app(MemoryStore::new(None));
        app.on_key(KeyEvent::from(KeyCode::Char('e')));
        app.enqueue(Action::OpenFile("README.md".to_string()));
        app.enqueue(Action::OpenFile("missing.rs".to_string()));
        app.dispatch();
        assert!(matches!(&app.message, Some((m, true)) if m.contains("missing.rs")));
        assert_eq!(app.file.as_ref().unwrap().path, "README.md");
        assert!(screen(&mut app).contains("1 hello readme"));
    }

    #[test]
    fn focus_follows_views_and_main_pane() {
        let (mut app, _fs) = explorer_app(MemoryStore::new(None));
        app.on_key(KeyEvent::from(KeyCode::Char('2')));
        app.dispatch();
        assert_eq!(app.focus, PanelId::DiffView);
        // Focus on the main pane stays on it when the view (tab) switches.
        app.enqueue(Action::SetSidebarView(SidebarView::Explorer));
        app.dispatch();
        assert_eq!(app.focus, PanelId::FileView);
        // Tab cycling skips the hidden Source Control panels.
        app.on_key(KeyEvent::from(KeyCode::Tab));
        app.dispatch();
        assert_eq!(app.focus, PanelId::Explorer);
        app.on_key(KeyEvent::from(KeyCode::Tab));
        app.dispatch();
        assert_eq!(app.focus, PanelId::FileView);
        // `e`/`1` focus the view's list.
        app.on_key(KeyEvent::from(KeyCode::Char('e')));
        assert_eq!(app.focus, PanelId::Explorer);
        app.on_key(KeyEvent::from(KeyCode::Char('1')));
        assert_eq!(app.focus, PanelId::Changes);
        assert_eq!(app.sidebar.view, SidebarView::SourceControl);
    }

    #[test]
    fn sidebar_view_persists_and_restores_focus() {
        let store = MemoryStore::new(None);
        let (mut app, _fs) = explorer_app(store.clone());
        app.on_key(KeyEvent::from(KeyCode::Char('e')));
        app.sync_prefs();
        assert_eq!(
            store.saved().unwrap().layout.sidebar_view,
            SidebarView::Explorer
        );
        let (app, _fs) = explorer_app(store);
        assert_eq!(app.sidebar.view, SidebarView::Explorer);
        assert_eq!(app.focus, PanelId::Explorer);
    }
}
