pub mod cli;
pub mod parse;

use anyhow::Result;

/// Which section of the status list a change belongs to. A single file can
/// appear in both `Staged` and `Unstaged` when it has staged and unstaged
/// modifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
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
    /// One-letter status for display: M A D R U ? etc.
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

/// The only interface the rest of the app uses to talk to git. Implementations
/// live behind `Box<dyn GitBackend>` inside `App`, so components stay fully
/// decoupled from git and can be tested with a mock backend.
pub trait GitBackend {
    fn status(&self) -> Result<Vec<FileChange>>;
    fn diff(&self, file: &FileChange) -> Result<DiffDoc>;
    fn stage(&self, path: &str) -> Result<()>;
    fn unstage(&self, path: &str) -> Result<()>;
    fn stage_all(&self) -> Result<()>;
    fn commit(&self, message: &str) -> Result<()>;
    fn branch(&self) -> Result<String>;
}
