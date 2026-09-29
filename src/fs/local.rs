//! `FsBackend` over the real filesystem, rooted at the repository toplevel.

use std::fs;
use std::io::{ErrorKind, Read};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use super::{
    decode_file, join, sort_entries, validate_rel, DirEntry, EntryKind, FileDoc, FileStamp,
    FsBackend, MAX_FILE_BYTES,
};

/// Reads the workspace under a fixed root. Every path is validated with
/// `validate_rel` first, so nothing outside the root is addressable (except
/// through symlinks inside it, which are followed like VS Code does — the
/// explorer only expands on demand, so link cycles can't recurse).
pub struct LocalFs {
    root: PathBuf,
}

impl LocalFs {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn abs(&self, rel: &str) -> Result<PathBuf> {
        validate_rel(rel)?;
        Ok(if rel.is_empty() {
            self.root.clone()
        } else {
            self.root.join(rel)
        })
    }
}

impl FsBackend for LocalFs {
    fn read_dir(&self, dir: &str) -> Result<Option<Vec<DirEntry>>> {
        let abs = self.abs(dir)?;
        let iter = match fs::read_dir(&abs) {
            Ok(it) => it,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading {}", abs.display())),
        };
        let mut entries = Vec::new();
        for item in iter {
            let item = item.with_context(|| format!("reading {}", abs.display()))?;
            let name = item.file_name().to_string_lossy().into_owned();
            if name == ".git" {
                continue;
            }
            // `file_type` doesn't follow symlinks; `metadata` does (a broken
            // link falls back to File, and opening it reports the error).
            let is_dir = match item.file_type() {
                Ok(t) if t.is_symlink() => fs::metadata(item.path()).is_ok_and(|m| m.is_dir()),
                Ok(t) => t.is_dir(),
                Err(_) => false,
            };
            entries.push(DirEntry {
                path: join(dir, &name),
                name,
                kind: if is_dir {
                    EntryKind::Dir
                } else {
                    EntryKind::File
                },
            });
        }
        sort_entries(&mut entries);
        Ok(Some(entries))
    }

    fn read_file(&self, path: &str) -> Result<FileDoc> {
        let abs = self.abs(path)?;
        let file = fs::File::open(&abs).with_context(|| format!("opening {path}"))?;
        let meta = file.metadata().with_context(|| format!("reading {path}"))?;
        if meta.is_dir() {
            bail!("{path} is a directory");
        }
        let mut bytes = Vec::new();
        file.take(MAX_FILE_BYTES)
            .read_to_end(&mut bytes)
            .with_context(|| format!("reading {path}"))?;
        Ok(decode_file(path, &bytes, meta.len() > MAX_FILE_BYTES))
    }

    fn stamp(&self, path: &str) -> Result<Option<FileStamp>> {
        let abs = self.abs(path)?;
        match fs::metadata(&abs) {
            Ok(m) => Ok(Some(FileStamp {
                modified: m.modified().ok(),
                len: m.len(),
            })),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {path}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, LocalFs) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("src/nested")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(root.join("README.md"), "# hi\n").unwrap();
        fs::write(root.join("bin.dat"), [0u8, 1, 2]).unwrap();
        let fs = LocalFs::new(root);
        (dir, fs)
    }

    #[test]
    fn read_dir_sorts_and_hides_git() {
        let (_d, fs) = setup();
        let root = fs.read_dir("").unwrap().unwrap();
        let names: Vec<_> = root.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["src", "bin.dat", "README.md"]);
        let src = fs.read_dir("src").unwrap().unwrap();
        assert_eq!(src[0].path, "src/nested");
        assert_eq!(src[0].kind, EntryKind::Dir);
        assert_eq!(src[1].path, "src/main.rs");
        assert_eq!(fs.read_dir("gone").unwrap(), None);
    }

    #[test]
    fn read_file_text_binary_and_errors() {
        let (_d, fs) = setup();
        let doc = fs.read_file("src/main.rs").unwrap();
        assert_eq!(doc.lines, ["fn main() {}"]);
        assert!(!doc.truncated);
        assert!(fs.read_file("bin.dat").unwrap().binary);
        assert!(fs.read_file("src").is_err());
        assert!(fs.read_file("../etc/passwd").is_err());
        assert!(fs.read_file("missing").is_err());
    }

    #[test]
    fn large_file_is_truncated() {
        let (d, fs) = setup();
        let big = "x\n".repeat(MAX_FILE_BYTES as usize / 2 + 10);
        std::fs::write(d.path().join("big.txt"), big).unwrap();
        let doc = fs.read_file("big.txt").unwrap();
        assert!(doc.truncated);
        assert_eq!(doc.lines.len(), MAX_FILE_BYTES as usize / 2);
    }

    #[test]
    fn stamp_changes_with_content_and_is_none_when_gone() {
        let (d, fs) = setup();
        let a = fs.stamp("README.md").unwrap().unwrap();
        std::fs::write(d.path().join("README.md"), "# longer title\n").unwrap();
        let b = fs.stamp("README.md").unwrap().unwrap();
        assert_ne!(a, b);
        std::fs::remove_file(d.path().join("README.md")).unwrap();
        assert_eq!(fs.stamp("README.md").unwrap(), None);
    }
}
