//! `GitBackend` implementation that shells out to the `git` binary.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use anyhow::{bail, Context, Result};

use super::parse::{parse_diff, parse_log, parse_name_status, parse_status};
use super::{Commit, CommitFile, DiffDoc, FileChange, GitBackend, Section};

/// Context size for diffs: effectively the whole file, like VS Code.
const FULL_CONTEXT: &str = "-U100000";

/// Hooks `git commit` runs; any of them may prompt on the terminal.
const COMMIT_HOOKS: [&str; 4] = [
    "pre-commit",
    "prepare-commit-msg",
    "commit-msg",
    "post-commit",
];

/// A file git would run as a hook.
fn is_executable(path: &Path) -> bool {
    let Ok(meta) = path.metadata() else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// `git -C <dir>` without optional locks: the periodic `status` must not
/// take `index.lock` to refresh the index, or it races git commands the
/// user runs in another terminal ("index.lock exists").
fn git_command(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("--no-optional-locks").arg("-C").arg(dir);
    cmd
}

/// Talks to git by spawning the `git` CLI in a fixed repository root.
pub struct CliGit {
    root: PathBuf,
}

impl CliGit {
    /// `root` should be the repo toplevel (e.g. from `git rev-parse
    /// --show-toplevel`).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Run `git -C <root> <args>` and return the raw output.
    /// `ok_codes` lists exit codes treated as success besides 0 (used for
    /// `diff --no-index`, which exits 1 when files differ).
    fn run(&self, args: &[&str], ok_codes: &[i32]) -> Result<Output> {
        let output = git_command(&self.root)
            .args(args)
            .output()
            .context("failed to spawn git")?;
        Self::check(args, ok_codes, output)
    }

    /// `run` with `input` written to git's stdin.
    fn run_with_stdin(&self, args: &[&str], input: &[u8]) -> Result<Output> {
        let mut child = git_command(&self.root)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn git")?;
        // Dropping the handle after the write closes git's stdin.
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(input).context("writing to git")?;
        }
        let output = child.wait_with_output().context("waiting for git")?;
        Self::check(args, &[], output)
    }

    fn check(args: &[&str], ok_codes: &[i32], output: Output) -> Result<Output> {
        let ok = output.status.success()
            || output
                .status
                .code()
                .is_some_and(|code| ok_codes.contains(&code));
        if !ok {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("git {} failed: {}", args.join(" "), stderr.trim());
        }
        Ok(output)
    }

    /// Revision to diff a commit against: its first parent, or the empty
    /// tree object for a root commit (`git hash-object -t tree /dev/null`
    /// produces the right hash for sha256 repos too).
    fn commit_base(&self, hash: &str) -> Result<String> {
        let parent = format!("{hash}^1");
        let out = self.run(&["rev-parse", "--verify", "-q", &parent], &[1])?;
        if out.status.success() {
            return Ok(parent);
        }
        let out = self.run(&["hash-object", "-t", "tree", "/dev/null"], &[])?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

impl GitBackend for CliGit {
    fn root(&self) -> PathBuf {
        self.root.clone()
    }

    fn status(&self) -> Result<Vec<FileChange>> {
        let out = self.run(
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
            &[],
        )?;
        Ok(parse_status(&out.stdout))
    }

    fn diff(&self, file: &FileChange) -> Result<DiffDoc> {
        let output = match file.section {
            Section::Staged => {
                let mut args = vec![
                    "diff",
                    "--cached",
                    "--no-color",
                    "--no-ext-diff",
                    FULL_CONTEXT,
                    "--",
                ];
                if let Some(orig) = &file.orig_path {
                    args.push(orig);
                }
                args.push(&file.path);
                self.run(&args, &[])?
            }
            Section::Unstaged => self.run(
                &[
                    "diff",
                    "--no-color",
                    "--no-ext-diff",
                    FULL_CONTEXT,
                    "--",
                    &file.path,
                ],
                &[],
            )?,
            // The index holds the conflict stages, so a plain `diff` would be
            // a combined (`--cc`) diff; show the working file (conflict
            // markers included) against HEAD instead.
            Section::Conflicted if self.head()?.is_some() => self.run(
                &[
                    "diff",
                    "HEAD",
                    "--no-color",
                    "--no-ext-diff",
                    FULL_CONTEXT,
                    "--",
                    &file.path,
                ],
                &[],
            )?,
            // `diff --no-index` exits 1 when the files differ — that is the
            // normal case, not an error.
            Section::Untracked | Section::Conflicted => self.run(
                &[
                    "diff",
                    "--no-index",
                    "--no-color",
                    FULL_CONTEXT,
                    "--",
                    "/dev/null",
                    &file.path,
                ],
                &[1],
            )?,
        };
        let text = String::from_utf8_lossy(&output.stdout);
        Ok(parse_diff(&file.path, &text))
    }

    fn stage(&self, path: &str) -> Result<()> {
        self.run(&["add", "--", path], &[])?;
        Ok(())
    }

    fn unstage(&self, path: &str) -> Result<()> {
        // `git restore --staged` needs a HEAD commit; on an unborn branch
        // (repo with no commits) fall back to removing the index entry.
        let head = self.run(&["rev-parse", "--verify", "-q", "HEAD"], &[1])?;
        if head.status.success() {
            self.run(&["restore", "--staged", "--", path], &[])?;
        } else {
            self.run(&["rm", "--cached", "-q", "--", path], &[])?;
        }
        Ok(())
    }

    fn discard(&self, file: &FileChange) -> Result<()> {
        match file.section {
            Section::Untracked => {
                self.run(&["clean", "-f", "-q", "--", &file.path], &[])?;
            }
            Section::Unstaged => {
                // Restores worktree content from the index; works on unborn
                // branches too since no HEAD lookup is needed.
                self.run(&["restore", "--worktree", "--", &file.path], &[])?;
            }
            Section::Staged | Section::Conflicted => {
                bail!("discard is only for unstaged changes")
            }
        }
        Ok(())
    }

    fn stage_all(&self) -> Result<()> {
        let status = self.status()?;
        if !status.iter().any(|f| f.section == Section::Conflicted) {
            self.run(&["add", "-A"], &[])?;
            return Ok(());
        }
        // `add -A` stages unmerged paths even when a pathspec excludes
        // them, so name every other changed path explicitly (NUL-separated
        // on stdin: no argv length limit, no pathspec magic).
        let mut input = Vec::new();
        for f in status
            .iter()
            .filter(|f| matches!(f.section, Section::Unstaged | Section::Untracked))
        {
            input.extend_from_slice(f.path.as_bytes());
            input.push(0);
        }
        if !input.is_empty() {
            self.run_with_stdin(
                &[
                    "--literal-pathspecs",
                    "add",
                    "-A",
                    "--pathspec-from-file=-",
                    "--pathspec-file-nul",
                ],
                &input,
            )?;
        }
        Ok(())
    }

    fn unstage_all(&self) -> Result<()> {
        // `git reset` needs a HEAD commit; on an unborn branch remove every
        // index entry instead.
        let head = self.run(&["rev-parse", "--verify", "-q", "HEAD"], &[1])?;
        if head.status.success() {
            self.run(&["reset", "-q"], &[])?;
        } else {
            self.run(&["rm", "--cached", "-r", "-q", "--", "."], &[])?;
        }
        Ok(())
    }

    fn commit(&self, message: &str) -> Result<()> {
        self.run(&["commit", "-m", message], &[])?;
        Ok(())
    }

    fn commit_may_prompt(&self) -> bool {
        let signs = self
            .run(&["config", "--bool", "commit.gpgsign"], &[1])
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "true");
        // Relative to the root (`-C`), or absolute (`core.hooksPath`).
        let hooks = self
            .run(&["rev-parse", "--git-path", "hooks"], &[])
            .map(|o| self.root.join(String::from_utf8_lossy(&o.stdout).trim()));
        signs || hooks.is_ok_and(|dir| COMMIT_HOOKS.iter().any(|h| is_executable(&dir.join(h))))
    }

    fn branch(&self) -> Result<String> {
        // Detached HEAD: fall back to the short commit hash.
        if let Ok(out) = self.run(&["symbolic-ref", "--short", "-q", "HEAD"], &[1]) {
            let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !name.is_empty() {
                return Ok(name);
            }
        }
        let out = self.run(&["rev-parse", "--short", "HEAD"], &[])?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn head(&self) -> Result<Option<String>> {
        let out = self.run(&["rev-parse", "--verify", "-q", "HEAD"], &[1])?;
        if out.status.success() {
            let hash = String::from_utf8_lossy(&out.stdout).trim().to_string();
            Ok((!hash.is_empty()).then_some(hash))
        } else {
            Ok(None)
        }
    }

    fn log(&self, skip: usize, limit: usize) -> Result<Vec<Commit>> {
        // No HEAD on an unborn branch — no history yet.
        if self.head()?.is_none() {
            return Ok(Vec::new());
        }
        let out = self.run(
            &[
                "log",
                "-z",
                "--format=%H%x1f%h%x1f%an%x1f%at%x1f%s",
                &format!("--skip={skip}"),
                "-n",
                &limit.to_string(),
                "HEAD",
            ],
            &[],
        )?;
        Ok(parse_log(&out.stdout))
    }

    fn commit_files(&self, hash: &str) -> Result<Vec<CommitFile>> {
        let base = self.commit_base(hash)?;
        let out = self.run(
            &[
                "diff",
                "--name-status",
                "-z",
                "-M",
                "--no-color",
                &base,
                hash,
            ],
            &[],
        )?;
        Ok(parse_name_status(&out.stdout))
    }

    fn commit_diff(&self, hash: &str, file: &CommitFile) -> Result<DiffDoc> {
        let base = self.commit_base(hash)?;
        let mut args = vec![
            "diff",
            "--no-color",
            "--no-ext-diff",
            FULL_CONTEXT,
            "-M",
            base.as_str(),
            hash,
            "--",
        ];
        if let Some(orig) = &file.orig_path {
            args.push(orig);
        }
        args.push(&file.path);
        let out = self.run(&args, &[])?;
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(parse_diff(&file.path, &text))
    }

    fn staged_patch(&self) -> Result<(String, String)> {
        let stat = self.run(
            &["diff", "--cached", "--no-color", "--no-ext-diff", "--stat"],
            &[],
        )?;
        let diff = self.run(&["diff", "--cached", "--no-color", "--no-ext-diff"], &[])?;
        Ok((
            String::from_utf8_lossy(&stat.stdout).into_owned(),
            String::from_utf8_lossy(&diff.stdout).into_owned(),
        ))
    }
}

/// Resolve the toplevel of the repo containing `path`. Errors when `path` is
/// not inside a git repository (or git can't be spawned).
pub fn resolve_toplevel(path: &Path) -> Result<PathBuf> {
    let output = git_command(path)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("failed to spawn git — is it installed?")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "'{}' is not inside a git repository: {}",
            path.display(),
            stderr.trim()
        );
    }
    let top = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(PathBuf::from(top))
}
