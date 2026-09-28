use crate::git::{DiffDoc, FileChange};

/// Identifies a panel so focus and actions can be routed to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PanelId {
    CommitInput,
    Changes,
    DiffView,
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
    DiffLoaded(DiffDoc),
    /// Refreshed diff for the *same* selected file (e.g. after a periodic
    /// refresh noticed the file changed on disk). Receivers update the
    /// content but keep their scroll position.
    DiffReloaded(DiffDoc),
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
    /// Commit the staged changes with this message (side effect).
    Commit(String),
    CommitDone,
    Error(String),
}
