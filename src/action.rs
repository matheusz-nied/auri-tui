use std::sync::Arc;

use crate::fs::{DirEntry, FileDoc};
use crate::git::{Commit, CommitFile, DiffDoc, FileChange};
use crate::keymap::HelpSection;
use crate::layout::SidebarView;
use crate::prefs::Preferences;

/// Identifies a panel so focus and actions can be routed to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PanelId {
    CommitInput,
    Changes,
    History,
    /// File tree (sidebar Explorer view).
    Explorer,
    DiffView,
    /// Read-only file viewer; shares the main pane with `DiffView`.
    FileView,
}

impl PanelId {
    /// The panel's name on the help screen.
    pub fn name(self) -> &'static str {
        match self {
            PanelId::CommitInput => "Commit box",
            PanelId::Changes => "Changes",
            PanelId::History => "History",
            PanelId::Explorer => "Explorer",
            PanelId::DiffView => "Diff",
            PanelId::FileView => "File viewer",
        }
    }
}

/// The single message type that flows through the app.
///
/// Architecture rule: components never talk to git (or any other backend)
/// directly. They emit `Action`s (e.g. `SelectFile`, `ToggleStage`) and react
/// to broadcast `Action`s (e.g. `StatusLoaded`, `DiffLoaded`, `Error`) in
/// [`crate::component::Component::update`]. `App` owns a `Box<dyn GitBackend>`
/// and is the only place where side effects are executed; their results are
/// re-injected into the queue as new actions.
///
/// To add a new panel:
/// 1. Add a `PanelId` variant, implement `Component` for your widget and
///    register it in `App::new`.
/// 2. If it needs data, add an `XxxLoaded` action and produce it in
///    `App::execute` (or from another component).
#[derive(Debug, Clone)]
pub enum Action {
    Quit,
    /// Reload git status and branch name (side effect, executed by `App`).
    Refresh,
    FocusNext,
    FocusPrev,
    Focus(PanelId),
    /// A file was selected; `App` loads its diff and broadcasts `DiffLoaded`.
    SelectFile(FileChange),
    /// Fresh status data broadcast to all components.
    StatusLoaded(Vec<FileChange>),
    /// Fresh diff data broadcast to all components. Sent in response to
    /// `SelectFile`; receivers reset their scroll to the first change.
    /// Shared (`Arc`): a whole-file diff can be large.
    DiffLoaded(Arc<DiffDoc>),
    /// Refreshed diff for the *same* selected file (e.g. after a periodic
    /// refresh noticed the file changed on disk). Receivers update the
    /// content but keep their scroll position.
    DiffReloaded(Arc<DiffDoc>),
    /// Current branch (or detached HEAD short hash) broadcast after refresh.
    BranchLoaded(String),
    /// Stage if the change is not staged, unstage if it is (side effect).
    ToggleStage(FileChange),
    StageAll,
    UnstageAll,
    /// Throw away a file's changes: `git restore --worktree` for unstaged
    /// files, `git clean` for untracked ones (side effect).
    Discard(FileChange),
    /// Ask the user before running `then`: opens the confirmation overlay.
    /// `App` itself ignores this — `ConfirmDialog` picks it up via `update`.
    Confirm {
        prompt: String,
        confirm_label: String,
        then: Box<Action>,
    },
    /// Scroll the diff to the previous/next changed block (handled by
    /// `DiffView::update`; `App` ignores them).
    DiffPrevChange,
    DiffNextChange,
    /// Fetch a page of commits (`skip` = how many commits are already shown).
    /// Executed by `App`.
    LoadHistory {
        skip: usize,
    },
    /// A page of commits for the History panel.
    HistoryLoaded {
        skip: usize,
        commits: Vec<Commit>,
    },
    /// Fetch the files changed by a commit (side effect, executed by `App`).
    LoadCommitFiles(String),
    /// Files of an expanded commit row.
    CommitFilesLoaded {
        hash: String,
        files: Vec<CommitFile>,
    },
    /// Open one file's diff inside a commit; `App` runs `commit_diff` and
    /// broadcasts `DiffLoaded` (fresh — scroll resets to the first change).
    SelectCommitFile {
        commit: Commit,
        file: CommitFile,
    },
    /// Switch the sidebar between Explorer and Source Control (tab bar
    /// click); the main pane follows: file viewer / diff. Executed by `App`.
    SetSidebarView(SidebarView),
    /// List these directories (side effect, executed by `App`). Emitted by
    /// the explorer on expand and, for every visible dir, on each `Refresh`.
    LoadDirs(Vec<String>),
    /// One directory's sorted children. Vanished dirs produce nothing.
    DirLoaded {
        path: String,
        entries: Vec<DirEntry>,
    },
    /// Open a workspace file in the file viewer; `App` reads it, makes the
    /// viewer own the main pane and broadcasts `FileLoaded`.
    OpenFile(String),
    /// Edit a workspace file in `$VISUAL`/`$EDITOR`, at `line` (1-based)
    /// when given. `App` hands the terminal over, waits for the editor to
    /// exit, then refreshes.
    OpenInEditor {
        path: String,
        line: Option<usize>,
    },
    /// A freshly opened file (receivers reset their scroll).
    FileLoaded(FileDoc),
    /// The open file changed on disk (receivers keep their scroll).
    FileReloaded(FileDoc),
    /// Collapse every explorer folder (toolbar button; `App` ignores it).
    ExplorerCollapseAll,
    /// Commit the staged changes with this message (side effect). `amend`
    /// replaces the last commit instead and needs nothing staged.
    Commit {
        message: String,
        amend: bool,
    },
    /// A commit (or amend) succeeded — the input clears and leaves amend
    /// mode.
    CommitDone {
        amended: bool,
    },
    /// Switch the commit input's amend mode (`Ctrl-A` or its toggle;
    /// handled by `CommitInput`, which may ask for `LoadLastCommitMessage`).
    ToggleAmend,
    /// Fetch the last commit's full message to prefill an amend (side
    /// effect, executed by `App`).
    LoadLastCommitMessage,
    LastCommitMessageLoaded(String),
    /// Generate a commit message from the staged diff with the configured AI
    /// CLI (side effect, run by `App` in the background). The result only
    /// fills the input via `CommitMessageGenerated` — it never commits.
    GenerateCommitMessage,
    /// Kill the in-flight generation; the `Cancelled` outcome still flows
    /// back through `App` as `CommitMessageFailed`.
    CancelCommitMessage,
    /// Broadcast when a generation starts; `provider` labels the CLI+model
    /// (e.g. "codex (gpt-6-luna)") so the input can show progress.
    CommitMessageGenerating {
        provider: String,
    },
    /// The cleaned one-line message produced by the AI.
    CommitMessageGenerated(String),
    /// Generation ended without a message — `CommitInput` clears its busy
    /// state; the reason (error text or "cancelled") is in the status bar.
    CommitMessageFailed,
    /// Switch the AI provider codex<->opencode and persist it (side effect).
    ToggleAiProvider,
    /// Preferences were (re)loaded — at startup and after each persisted
    /// change. Broadcast so components can read their section.
    PreferencesChanged(Preferences),
    /// Open the key reference overlay with these sections (`?`; built by
    /// `App` from `keymap` and every panel's `hints`).
    ShowHelp(Vec<HelpSection>),
    Error(String),
}
