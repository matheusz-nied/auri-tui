pub mod cli;
pub mod parse;

use std::path::PathBuf;

use anyhow::Result;

/// Which section of the status list a change belongs to. A single file can
/// appear in both `Staged` and `Unstaged` when it has staged and unstaged
/// modifications. An unmerged path (merge/rebase conflict) appears only in
/// `Conflicted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// Unmerged paths; staging one marks its conflict resolved.
    Conflicted,
    Staged,
    Unstaged,
    Untracked,
}

/// One entry in the changes list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// Repo-relative path (the new path for renames).
    pub path: String,
    /// Original path for renames/copies.
    pub orig_path: Option<String>,
    pub section: Section,
    /// One-letter status for display: M A D R C T, `U` untracked, `!`
    /// conflicted.
    pub code: char,
}

/// The kind of one cell (one side) of a diff row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    Context,
    Removed,
    Added,
}

/// The kind of a whole side-by-side row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Context,
    /// At least one side is a removed/added line; the other side may be an
    /// empty filler (`None`).
    Changed,
    /// Separator row between hunks; the header text is stored in `left`.
    HunkHeader,
}

/// One line of text on one side of a side-by-side diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffCell {
    pub line_no: usize,
    pub text: String,
    pub kind: CellKind,
}

/// A row pairing an old-side line with a new-side line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRow {
    pub left: Option<DiffCell>,
    pub right: Option<DiffCell>,
    pub kind: RowKind,
}

/// A parsed diff document for one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffDoc {
    pub path: String,
    pub rows: Vec<DiffRow>,
    pub binary: bool,
}

/// One commit in the branch history (newest first as returned by `git log`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub short: String,
    pub author: String,
    /// Unix timestamp (committer/author date as reported by `%at`).
    pub time: i64,
    pub subject: String,
}

/// One file touched by a commit (`git diff --name-status <base> <hash>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitFile {
    /// The new path (for renames/copies, the destination).
    pub path: String,
    /// Original path for renames/copies.
    pub orig_path: Option<String>,
    /// One-letter status: A M D R C T.
    pub code: char,
}

/// What the diff pane is currently showing: a working-tree change or a file
/// inside a specific commit. `App` tracks this to decide what to reload on
/// refresh; `DiffView` tracks it so status updates only clear working diffs.
#[derive(Debug, Clone, PartialEq)]
pub enum DiffSource {
    Working(FileChange),
    Commit { hash: String, file: CommitFile },
}

/// The only interface the rest of the app uses to talk to git. Implementations
/// live behind `Box<dyn GitBackend>` inside `App`, so components stay fully
/// decoupled from git and can be tested with a mock backend.
pub trait GitBackend {
    /// The repository toplevel this backend operates on.
    fn root(&self) -> PathBuf;
    fn status(&self) -> Result<Vec<FileChange>>;
    fn diff(&self, file: &FileChange) -> Result<DiffDoc>;
    fn stage(&self, path: &str) -> Result<()>;
    fn unstage(&self, path: &str) -> Result<()>;
    /// Revert a file's changes: delete untracked files, restore unstaged
    /// modifications from the index. Errors on `Staged`/`Conflicted` files.
    fn discard(&self, file: &FileChange) -> Result<()>;
    /// Stage every change except conflicted paths — staging those would
    /// silently mark them resolved; that is `stage(path)`, one at a time.
    fn stage_all(&self) -> Result<()>;
    fn unstage_all(&self) -> Result<()>;
    fn commit(&self, message: &str) -> Result<()>;
    /// Whether `commit` may talk to the terminal — commit signing (GPG
    /// pinentry, ssh passphrase) or commit hooks that can prompt — so the
    /// TUI must hand the terminal over while it runs.
    fn commit_may_prompt(&self) -> bool {
        false
    }
    fn branch(&self) -> Result<String>;
    /// HEAD's full hash; `None` on an unborn branch (repo with no commits).
    fn head(&self) -> Result<Option<String>>;
    /// Commits of HEAD, newest first. Empty on an unborn branch.
    fn log(&self, skip: usize, limit: usize) -> Result<Vec<Commit>>;
    /// Files changed by `hash`, diffed against its first parent (or the
    /// empty tree for a root commit).
    fn commit_files(&self, hash: &str) -> Result<Vec<CommitFile>>;
    /// Full-context diff of `file` between the commit's first parent (or the
    /// empty tree) and the commit itself.
    fn commit_diff(&self, hash: &str, file: &CommitFile) -> Result<DiffDoc>;
    /// Raw staged patch for the AI prompt: the `--stat` summary and the
    /// default-context `diff --cached` output.
    fn staged_patch(&self) -> Result<(String, String)>;
}
