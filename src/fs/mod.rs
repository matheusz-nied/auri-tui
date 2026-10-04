//! Workspace filesystem access for the explorer and the file viewer.
//!
//! Mirrors `git/`: model types plus the `FsBackend` trait that only `App`
//! calls (components never do I/O); `local` is the real implementation,
//! `tree` the pure expand/collapse model the explorer renders. Paths are
//! always repo-relative, `/`-separated, `""` = the root.

pub mod local;
pub mod tree;

use std::time::SystemTime;

use anyhow::{bail, Result};

/// Files bigger than this are shown truncated.
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// How many leading bytes are checked for NUL to detect binary files.
const BINARY_SNIFF: usize = 8000;
pub const TAB_WIDTH: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Dir,
    File,
}

/// One child of a directory. Symlinks report their target's kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    /// Repo-relative path.
    pub path: String,
    pub kind: EntryKind,
}

/// A file's contents prepared for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDoc {
    pub path: String,
    /// Lines with tabs expanded and control chars replaced; empty for
    /// binary files.
    pub lines: Vec<String>,
    pub binary: bool,
    /// Only the first `MAX_FILE_BYTES` were read.
    pub truncated: bool,
    /// The exact contents, for editing — `None` when they can't be edited
    /// safely (binary, truncated or not UTF-8: saving would corrupt them).
    pub source: Option<String>,
}

/// Cheap change detector for an open file: compared on every refresh so
/// the file is only re-read when it changed on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStamp {
    pub modified: Option<SystemTime>,
    pub len: u64,
}

/// What `FsBackend::write_file` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    /// Written; the file's new stamp.
    Written(FileStamp),
    /// Not written: the file changed on disk (or was deleted, `stamp:
    /// None`) since the stamp the caller expected.
    Conflict { stamp: Option<FileStamp> },
}

/// The only interface the rest of the app uses to read the workspace. Lives
/// behind `Box<dyn FsBackend>` inside `App`, next to `GitBackend`.
pub trait FsBackend {
    /// Children of `dir` sorted for display (dirs first, then by name,
    /// case-insensitive), without `.git`. `None` if the directory is gone.
    fn read_dir(&self, dir: &str) -> Result<Option<Vec<DirEntry>>>;
    /// Read (at most `MAX_FILE_BYTES` of) a file for display.
    fn read_file(&self, path: &str) -> Result<FileDoc>;
    /// `None` if the file is gone.
    fn stamp(&self, path: &str) -> Result<Option<FileStamp>>;
    /// Replace a file's contents atomically (permissions kept, symlinks
    /// written through). With `expected`, only if the file's stamp still
    /// equals it — otherwise nothing is written and `Conflict` comes back;
    /// `None` writes unconditionally (overwrite, or recreate a deleted
    /// file).
    fn write_file(
        &self,
        path: &str,
        contents: &str,
        expected: Option<FileStamp>,
    ) -> Result<WriteOutcome>;
}

/// Reject anything that could escape the root: absolute paths, `..`, `.`
/// and empty components (`a//b`, trailing `/`). `""` is the root itself.
pub fn validate_rel(path: &str) -> Result<()> {
    if path.is_empty() {
        return Ok(());
    }
    if path.starts_with('/') || path.contains('\\') {
        bail!("invalid path: {path}");
    }
    if path
        .split('/')
        .any(|c| c.is_empty() || c == "." || c == "..")
    {
        bail!("invalid path: {path}");
    }
    Ok(())
}

/// `dir` + `name` as a repo-relative path.
pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// Parent directory of a repo-relative path (`""` for top-level entries).
pub fn parent(path: &str) -> &str {
    path.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

/// Explorer order: directories first, then case-insensitive name (exact
/// name as tiebreak so the order is total).
pub fn sort_entries(entries: &mut [DirEntry]) {
    entries.sort_by(|a, b| {
        (a.kind != EntryKind::Dir)
            .cmp(&(b.kind != EntryKind::Dir))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// Turn raw bytes into a displayable document: NUL in the first
/// `BINARY_SNIFF` bytes = binary; otherwise lossy UTF-8 split into lines
/// (`\n` or `\r\n`), tabs expanded to `TAB_WIDTH` stops and other control
/// characters replaced by `�` so they stay visible and one column wide.
pub fn decode_file(path: &str, bytes: &[u8], truncated: bool) -> FileDoc {
    let binary = bytes[..bytes.len().min(BINARY_SNIFF)].contains(&0);
    let source = (!binary && !truncated)
        .then(|| std::str::from_utf8(bytes).ok().map(str::to_string))
        .flatten();
    let lines = if binary {
        Vec::new()
    } else {
        let text = String::from_utf8_lossy(bytes);
        let mut lines: Vec<String> = text
            .split('\n')
            .map(|l| display_line(l.strip_suffix('\r').unwrap_or(l)))
            .collect();
        // A trailing newline terminates the last line, it doesn't start one
        // (and an empty file has no lines at all).
        if text.is_empty() || text.ends_with('\n') {
            lines.pop();
        }
        lines
    };
    FileDoc {
        path: path.to_string(),
        lines,
        binary,
        truncated,
        source,
    }
}

fn display_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut col = 0;
    for c in line.chars() {
        if c == '\t' {
            let n = TAB_WIDTH - col % TAB_WIDTH;
            out.extend(std::iter::repeat_n(' ', n));
            col += n;
        } else {
            out.push(if c.is_control() { '\u{FFFD}' } else { c });
            col += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, kind: EntryKind) -> DirEntry {
        DirEntry {
            name: name.to_string(),
            path: name.to_string(),
            kind,
        }
    }

    #[test]
    fn validate_rel_rejects_escapes() {
        for ok in ["", "a", "a/b.rs", ".gitignore", "a/.hidden/..x"] {
            assert!(validate_rel(ok).is_ok(), "{ok}");
        }
        for bad in ["/etc", "..", "a/../b", "./a", "a//b", "a/", "a\\b"] {
            assert!(validate_rel(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn join_and_parent() {
        assert_eq!(join("", "a"), "a");
        assert_eq!(join("a/b", "c"), "a/b/c");
        assert_eq!(parent("a/b/c"), "a/b");
        assert_eq!(parent("a"), "");
    }

    #[test]
    fn sort_puts_dirs_first_then_case_insensitive() {
        let mut v = vec![
            entry("b.rs", EntryKind::File),
            entry("Src", EntryKind::Dir),
            entry("A.md", EntryKind::File),
            entry("docs", EntryKind::Dir),
            entry("a.md", EntryKind::File),
        ];
        sort_entries(&mut v);
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["docs", "Src", "A.md", "a.md", "b.rs"]);
    }

    #[test]
    fn decode_splits_lines_and_expands_tabs() {
        let doc = decode_file("f", b"a\tb\r\n\tx\n\x1b[31m\n", false);
        assert!(!doc.binary);
        assert_eq!(doc.lines, ["a   b", "    x", "\u{FFFD}[31m"]);
        // No trailing newline: last line kept.
        assert_eq!(decode_file("f", b"x\ny", false).lines, ["x", "y"]);
        assert!(decode_file("f", b"", false).lines.is_empty());
    }

    #[test]
    fn decode_detects_binary() {
        let doc = decode_file("f", b"PNG\0\x01\x02", false);
        assert!(doc.binary);
        assert!(doc.lines.is_empty());
        assert_eq!(doc.source, None);
    }

    #[test]
    fn source_is_exact_and_only_for_editable_text() {
        let doc = decode_file("f", b"a\tb\r\n", false);
        assert_eq!(doc.source.as_deref(), Some("a\tb\r\n"));
        assert_eq!(decode_file("f", b"abc", true).source, None);
        assert_eq!(decode_file("f", b"caf\xe9", false).source, None);
    }
}
