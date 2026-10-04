//! `FsBackend` over the real filesystem, rooted at the repository toplevel.

use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use super::{
    decode_file, join, sort_entries, validate_rel, DirEntry, EntryKind, FileDoc, FileStamp,
    FsBackend, WriteOutcome, MAX_FILE_BYTES,
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

    /// Write a temporary file next to the target, then rename it over the
    /// target: a crash or full disk mid-save never leaves a half-written
    /// file. A symlink is resolved first so the link itself survives.
    fn write_file(
        &self,
        path: &str,
        contents: &str,
        expected: Option<FileStamp>,
    ) -> Result<WriteOutcome> {
        if path.is_empty() {
            bail!("can't save the root directory");
        }
        if let Some(want) = expected {
            let now = self.stamp(path)?;
            if now != Some(want) {
                return Ok(WriteOutcome::Conflict { stamp: now });
            }
        }
        let abs = self.abs(path)?;
        let target = match fs::symlink_metadata(&abs) {
            Ok(m) if m.file_type().is_symlink() => {
                fs::canonicalize(&abs).with_context(|| format!("saving {path}: broken link"))?
            }
            _ => abs,
        };
        let (Some(dir), Some(name)) = (target.parent(), target.file_name()) else {
            bail!("saving {path}: invalid path");
        };
        let tmp = dir.join(format!(
            ".{}.auri-save-{}",
            name.to_string_lossy(),
            std::process::id()
        ));
        // Keep the mode (an executable script stays executable).
        let perms = fs::metadata(&target).ok().map(|m| m.permissions());
        let written = (|| -> std::io::Result<()> {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(contents.as_bytes())?;
            file.sync_all()?;
            if let Some(perms) = perms {
                fs::set_permissions(&tmp, perms)?;
            }
            fs::rename(&tmp, &target)
        })();
        if let Err(e) = written {
            let _ = fs::remove_file(&tmp);
            return Err(e).with_context(|| format!("saving {path}"));
        }
        match self.stamp(path)? {
            Some(stamp) => Ok(WriteOutcome::Written(stamp)),
            None => bail!("saving {path}: file vanished right after writing"),
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

    #[test]
    fn write_file_replaces_contents_and_returns_the_new_stamp() {
        let (d, fs) = setup();
        let before = fs.stamp("README.md").unwrap();
        let WriteOutcome::Written(stamp) = fs.write_file("README.md", "# new\r\n", before).unwrap()
        else {
            panic!("conflict")
        };
        assert_eq!(
            std::fs::read(d.path().join("README.md")).unwrap(),
            b"# new\r\n"
        );
        assert_eq!(fs.stamp("README.md").unwrap(), Some(stamp));
        // No temporary file left behind.
        let names: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert!(names.iter().all(|n| !n.contains("auri-save")), "{names:?}");
    }

    #[test]
    fn write_file_refuses_when_changed_on_disk_unless_forced() {
        let (d, fs) = setup();
        let opened = fs.stamp("README.md").unwrap();
        std::fs::write(d.path().join("README.md"), "# someone else, longer\n").unwrap();
        let out = fs.write_file("README.md", "mine", opened).unwrap();
        assert!(matches!(out, WriteOutcome::Conflict { stamp: Some(_) }));
        assert_eq!(
            std::fs::read_to_string(d.path().join("README.md")).unwrap(),
            "# someone else, longer\n"
        );
        std::fs::remove_file(d.path().join("README.md")).unwrap();
        let out = fs.write_file("README.md", "mine", opened).unwrap();
        assert_eq!(out, WriteOutcome::Conflict { stamp: None });
        // Forced: recreated.
        assert!(matches!(
            fs.write_file("README.md", "mine", None).unwrap(),
            WriteOutcome::Written(_)
        ));
        assert_eq!(
            std::fs::read_to_string(d.path().join("README.md")).unwrap(),
            "mine"
        );
        assert!(fs.write_file("../escape", "x", None).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn write_file_keeps_mode_and_symlinks() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let (d, fs) = setup();
        let script = d.path().join("run.sh");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        fs.write_file("run.sh", "#!/bin/sh\necho hi\n", None)
            .unwrap();
        let mode = std::fs::metadata(&script).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);

        symlink("README.md", d.path().join("link.md")).unwrap();
        fs.write_file("link.md", "via link", None).unwrap();
        assert!(std::fs::symlink_metadata(d.path().join("link.md"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_to_string(d.path().join("README.md")).unwrap(),
            "via link"
        );
    }
}
