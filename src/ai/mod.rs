//! AI-generated commit messages.
//!
//! `App` (the only side-effecting layer) gathers the staged diff, builds a
//! prompt with [`build_prompt`] and spawns an already-installed AI CLI —
//! `codex` or `opencode` — non-interactively via [`AiRunner`]. Both CLIs read
//! the prompt on stdin and print only the reply on stdout, so the first
//! cleaned line becomes the commit message input's content (never committed
//! automatically). [`ProcessRunner`] does the spawn on background threads and
//! is polled once per app loop — the UI never blocks.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// Which installed AI CLI generates commit messages. Persisted in `[ai]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AiProvider {
    #[default]
    Codex,
    Opencode,
}

impl AiProvider {
    /// Lowercase display name (as written in preferences.toml).
    pub fn name(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Opencode => "opencode",
        }
    }
}

/// `[ai]` preferences section — all keys optional in the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiPrefs {
    pub provider: AiProvider,
    pub codex_model: String,
    pub codex_reasoning_effort: String,
    pub opencode_model: String,
    /// Kill the child after this long without a finished reply.
    pub timeout_secs: u64,
    /// Cap on diff chars sent to the CLI (see `build_prompt`).
    pub max_diff_chars: usize,
}

impl Default for AiPrefs {
    fn default() -> Self {
        Self {
            provider: AiProvider::Codex,
            codex_model: "gpt-6-luna".to_string(),
            codex_reasoning_effort: "low".to_string(),
            opencode_model: "deepseek/deepseek-flash".to_string(),
            timeout_secs: 60,
            max_diff_chars: 20000,
        }
    }
}

/// Everything needed to spawn one generation: argv, working dir, deadline.
#[derive(Debug, Clone)]
pub struct AiCommand {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub timeout: Duration,
}

/// The exact non-interactive argv for the configured provider. Both read the
/// prompt from stdin and print only the reply on stdout.
pub fn command(prefs: &AiPrefs, root: &Path) -> AiCommand {
    let root = root.to_string_lossy().into_owned();
    match prefs.provider {
        AiProvider::Codex => AiCommand {
            program: "codex".to_string(),
            args: vec![
                "exec".to_string(),
                "--ephemeral".to_string(),
                "--skip-git-repo-check".to_string(),
                "-s".to_string(),
                "read-only".to_string(),
                "-C".to_string(),
                root.clone(),
                "-m".to_string(),
                prefs.codex_model.clone(),
                "-c".to_string(),
                format!(
                    "model_reasoning_effort=\"{}\"",
                    prefs.codex_reasoning_effort
                ),
                "-".to_string(),
            ],
            cwd: PathBuf::from(root),
            timeout: Duration::from_secs(prefs.timeout_secs),
        },
        AiProvider::Opencode => AiCommand {
            program: "opencode".to_string(),
            args: vec![
                "run".to_string(),
                "--pure".to_string(),
                "-m".to_string(),
                prefs.opencode_model.clone(),
                "--dir".to_string(),
                root.clone(),
            ],
            cwd: PathBuf::from(root),
            timeout: Duration::from_secs(prefs.timeout_secs),
        },
    }
}

/// The stdin prompt. `diff` longer than `max_diff_chars` chars is cut on a
/// char boundary and marked `[diff truncated]` so the CLI knows it saw a
/// partial patch.
pub fn build_prompt(
    recent_subjects: &[String],
    stat: &str,
    diff: &str,
    max_diff_chars: usize,
) -> String {
    let subjects = if recent_subjects.is_empty() {
        "(none)".to_string()
    } else {
        recent_subjects
            .iter()
            .map(|s| format!("- {s}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let diff = truncate_chars(diff, max_diff_chars);
    format!(
        "You are generating a git commit message. Do NOT run any commands or tools and do NOT edit files; answer only from the information below.\n\n\
         Write a single-line commit subject (max 72 characters) describing the staged changes. Match the style and language of the recent commits (e.g. use Conventional Commits prefixes if they do). Output ONLY the subject line: no quotes, no code fences, no explanation.\n\n\
         Recent commits:\n{subjects}\n\n\
         Staged changes (stat):\n{stat}\n\n\
         Staged diff:\n{diff}"
    )
}

/// Cut `s` at `max` chars, appending a truncation marker line when it did.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{cut}\n[diff truncated]")
}

/// Extract the commit subject from raw CLI stdout: strip ANSI escapes, skip
/// code fences and empty lines, then clean the first surviving line (prefix,
/// surrounding quotes, whitespace). `None` when nothing usable came back.
pub fn clean_output(raw: &str) -> Option<String> {
    let raw = strip_ansi(raw);
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("```") {
            continue;
        }
        let mut s = line;
        for prefix in ["commit message:", "subject:"] {
            match s.get(..prefix.len()) {
                Some(head) if head.eq_ignore_ascii_case(prefix) => {
                    s = s[prefix.len()..].trim();
                    break;
                }
                _ => {}
            }
        }
        s = strip_surrounding_quote(s).trim();
        if !s.is_empty() {
            return Some(s.to_string());
        }
    }
    None
}

/// Remove ANSI escape sequences (CSI colors, OSC hyperlinks, two-char codes).
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: ESC [ <params> <final byte in @..=~>
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: ESC ] <payload> terminated by BEL or ESC \
            Some(']') => {
                let mut prev_esc = false;
                for c in chars.by_ref() {
                    if c == '\x07' || (prev_esc && c == '\\') {
                        break;
                    }
                    prev_esc = c == '\x1b';
                }
            }
            // Charset designators ESC ( X / ESC ) X eat one more char.
            Some('(') | Some(')') => {
                chars.next();
            }
            // Any other single-char escape is already consumed.
            _ => {}
        }
    }
    out
}

/// Strip one pair of surrounding `"`, `'` or backticks.
fn strip_surrounding_quote(s: &str) -> &str {
    for q in ['"', '\'', '`'] {
        if s.len() >= 2 * q.len_utf8() && s.starts_with(q) && s.ends_with(q) {
            return &s[q.len_utf8()..s.len() - q.len_utf8()];
        }
    }
    s
}

/// The last `max` chars of `s` (char-boundary safe) — for stderr tails.
fn last_chars(s: &str, max: usize) -> &str {
    let count = s.chars().count();
    if count <= max {
        return s;
    }
    let byte_idx = s
        .char_indices()
        .nth(count - max)
        .map(|(i, _)| i)
        .unwrap_or(0);
    &s[byte_idx..]
}

/// How one generation ended. `Done` carries raw stdout for `clean_output`.
#[derive(Debug)]
pub enum AiOutcome {
    Done(String),
    Failed(String),
    Cancelled,
}

/// Spawn-and-poll seam so `App` stays synchronous and tests can inject
/// outcomes. One generation at a time.
pub trait AiRunner {
    /// Begin a run; `stdin` is the prompt fed to the child. `Err` when a run
    /// is already in flight or the program can't be spawned.
    fn start(&mut self, cmd: AiCommand, stdin: String) -> Result<()>;
    /// The finished outcome, if any — never blocks.
    fn poll(&mut self) -> Option<AiOutcome>;
    /// Ask the running child to stop; the outcome arrives via `poll`.
    fn cancel(&mut self);
    fn is_running(&self) -> bool;
}

/// Real runner: the child plus a stdin writer, two pipe drainers and a
/// supervisor thread that watches the cancel flag and the deadline.
#[derive(Default)]
pub struct ProcessRunner {
    cancel: Option<Arc<AtomicBool>>,
    rx: Option<mpsc::Receiver<AiOutcome>>,
}

impl AiRunner for ProcessRunner {
    fn start(&mut self, cmd: AiCommand, stdin: String) -> Result<()> {
        if self.is_running() {
            bail!("AI generation already running");
        }
        let program = cmd.program.clone();
        let mut child = Command::new(&cmd.program)
            .args(&cmd.args)
            .current_dir(&cmd.cwd)
            .env("NO_COLOR", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    anyhow::anyhow!("{} not found in PATH", cmd.program)
                } else {
                    anyhow::anyhow!(e).context(format!("{} failed to start", cmd.program))
                }
            })?;

        // Feed the prompt, then drop the handle so the child sees EOF.
        if let Some(mut w) = child.stdin.take() {
            thread::spawn(move || {
                let _ = w.write_all(stdin.as_bytes());
            });
        }
        // Drain both pipes concurrently so a verbose child can't block on a
        // full pipe buffer while we wait for it to exit.
        let out_thread = child.stdout.take().map(|mut p| {
            thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = p.read_to_end(&mut buf);
                buf
            })
        });
        let err_thread = child.stderr.take().map(|mut p| {
            thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = p.read_to_end(&mut buf);
                buf
            })
        });

        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let flag = cancel.clone();
        let timeout = cmd.timeout;
        thread::spawn(move || {
            let deadline = Instant::now() + timeout;
            let outcome = loop {
                if flag.load(Ordering::Relaxed) {
                    let _ = child.kill();
                    let _ = child.wait();
                    break AiOutcome::Cancelled;
                }
                match child.try_wait() {
                    Ok(Some(status)) => {
                        let stdout = join_bytes(out_thread);
                        let stderr = join_bytes(err_thread);
                        if status.success() {
                            break AiOutcome::Done(String::from_utf8_lossy(&stdout).into_owned());
                        }
                        let tail = strip_ansi(&String::from_utf8_lossy(&stderr));
                        let tail = last_chars(tail.trim(), 300).to_string();
                        break AiOutcome::Failed(if tail.is_empty() {
                            format!("{program} exited with {status}")
                        } else {
                            format!("{program}: {tail}")
                        });
                    }
                    Ok(None) if Instant::now() >= deadline => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break AiOutcome::Failed(format!(
                            "{program} timed out after {}s",
                            timeout.as_secs()
                        ));
                    }
                    Ok(None) => thread::sleep(Duration::from_millis(50)),
                    Err(e) => break AiOutcome::Failed(format!("{program}: {e}")),
                }
            };
            let _ = tx.send(outcome);
        });
        self.cancel = Some(cancel);
        self.rx = Some(rx);
        Ok(())
    }

    fn poll(&mut self) -> Option<AiOutcome> {
        let rx = self.rx.as_ref()?;
        match rx.try_recv() {
            Ok(outcome) => {
                self.rx = None;
                self.cancel = None;
                Some(outcome)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.rx = None;
                self.cancel = None;
                Some(AiOutcome::Failed("AI worker died".to_string()))
            }
        }
    }

    fn cancel(&mut self) {
        if let Some(flag) = &self.cancel {
            flag.store(true, Ordering::Relaxed);
        }
    }

    fn is_running(&self) -> bool {
        self.rx.is_some()
    }
}

fn join_bytes(h: Option<thread::JoinHandle<Vec<u8>>>) -> Vec<u8> {
    h.and_then(|h| h.join().ok()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_prompt_has_subjects_stat_and_diff() {
        let subjects = vec!["feat: a".to_string(), "fix: b".to_string()];
        let p = build_prompt(&subjects, "1 file changed", "diff --git a/x", 20000);
        assert!(p.contains("- feat: a\n- fix: b"));
        assert!(p.contains("Staged changes (stat):\n1 file changed"));
        assert!(p.contains("Staged diff:\ndiff --git a/x"));
        assert!(!p.contains("[diff truncated]"));
    }

    #[test]
    fn build_prompt_none_marker_when_no_subjects() {
        let p = build_prompt(&[], "stat", "diff", 20000);
        assert!(p.contains("Recent commits:\n(none)"));
    }

    #[test]
    fn build_prompt_truncates_on_char_boundary() {
        // 'é' is multibyte; a byte-slice cut would panic — this must not.
        let diff = "é".repeat(50);
        let p = build_prompt(&[], "s", &diff, 10);
        assert!(p.contains("[diff truncated]"));
        let cut = p.rsplit("Staged diff:\n").next().unwrap();
        assert_eq!(cut, format!("{}\n[diff truncated]", "é".repeat(10)));
    }

    #[test]
    fn clean_output_skips_fences_and_takes_first_line() {
        let raw = "```\nfeat: add x\n```\nignored second line";
        assert_eq!(clean_output(raw).as_deref(), Some("feat: add x"));
    }

    #[test]
    fn clean_output_quotes_prefix_ansi_and_empty() {
        assert_eq!(clean_output("\"feat: x\"").as_deref(), Some("feat: x"));
        assert_eq!(clean_output("`fix: y`").as_deref(), Some("fix: y"));
        assert_eq!(clean_output("'chore: z'").as_deref(), Some("chore: z"));
        assert_eq!(
            clean_output("Commit message: feat: x").as_deref(),
            Some("feat: x")
        );
        assert_eq!(
            clean_output("subject: 'feat: x'").as_deref(),
            Some("feat: x")
        );
        assert_eq!(
            clean_output("\x1b[32mfeat: colored\x1b[0m").as_deref(),
            Some("feat: colored")
        );
        assert_eq!(
            clean_output("  \n\nfeat: after blanks\nsecond").as_deref(),
            Some("feat: after blanks")
        );
        assert_eq!(clean_output("   \n  \n```\n```").as_deref(), None);
        assert_eq!(clean_output("").as_deref(), None);
        assert_eq!(clean_output("\"\"").as_deref(), None);
    }

    #[test]
    fn command_argv_codex() {
        let prefs = AiPrefs::default();
        let cmd = command(&prefs, Path::new("/repo"));
        assert_eq!(cmd.program, "codex");
        assert_eq!(
            cmd.args,
            vec![
                "exec",
                "--ephemeral",
                "--skip-git-repo-check",
                "-s",
                "read-only",
                "-C",
                "/repo",
                "-m",
                "gpt-6-luna",
                "-c",
                "model_reasoning_effort=\"low\"",
                "-",
            ]
        );
        assert_eq!(cmd.cwd, PathBuf::from("/repo"));
        assert_eq!(cmd.timeout, Duration::from_secs(60));
    }

    #[test]
    fn command_argv_opencode() {
        let prefs = AiPrefs {
            provider: AiProvider::Opencode,
            ..Default::default()
        };
        let cmd = command(&prefs, Path::new("/repo"));
        assert_eq!(cmd.program, "opencode");
        assert_eq!(
            cmd.args,
            vec![
                "run",
                "--pure",
                "-m",
                "deepseek/deepseek-flash",
                "--dir",
                "/repo"
            ]
        );
    }

    #[test]
    fn ai_section_parses_and_defaults() {
        let prefs: crate::prefs::Preferences =
            toml::from_str("[ai]\nprovider = \"opencode\"\n").unwrap();
        assert_eq!(prefs.ai.provider, AiProvider::Opencode);
        assert_eq!(prefs.ai.codex_model, "gpt-6-luna");
        // A file with no [ai] section keeps all defaults.
        let prefs: crate::prefs::Preferences = toml::from_str("").unwrap();
        assert_eq!(prefs.ai, AiPrefs::default());
    }

    #[cfg(unix)]
    mod process {
        use super::*;

        fn sh(script: &str, timeout: Duration) -> AiCommand {
            AiCommand {
                program: "sh".to_string(),
                args: vec!["-c".to_string(), script.to_string()],
                cwd: PathBuf::from("/tmp"),
                timeout,
            }
        }

        fn wait_outcome(r: &mut ProcessRunner) -> AiOutcome {
            for _ in 0..200 {
                if let Some(o) = r.poll() {
                    return o;
                }
                thread::sleep(Duration::from_millis(25));
            }
            panic!("runner produced no outcome within 5s");
        }

        #[test]
        fn success_returns_stdout_and_stdin_is_delivered() {
            let mut r = ProcessRunner::default();
            // Echo stdin back so the test proves the prompt reached the child.
            r.start(
                sh("cat; echo 'feat: x'", Duration::from_secs(10)),
                "PROMPT".into(),
            )
            .unwrap();
            assert!(r.is_running());
            match wait_outcome(&mut r) {
                AiOutcome::Done(out) => {
                    assert!(out.contains("PROMPT"), "stdin missing: {out:?}");
                    assert!(out.contains("feat: x"));
                }
                o => panic!("expected Done, got {o:?}"),
            }
            assert!(!r.is_running());
        }

        #[test]
        fn non_zero_exit_fails_with_stderr() {
            let mut r = ProcessRunner::default();
            r.start(
                sh(
                    "echo 'oops went wrong' >&2; exit 1",
                    Duration::from_secs(10),
                ),
                String::new(),
            )
            .unwrap();
            match wait_outcome(&mut r) {
                AiOutcome::Failed(e) => assert!(e.contains("oops went wrong"), "{e}"),
                o => panic!("expected Failed, got {o:?}"),
            }
        }

        #[test]
        fn timeout_fails() {
            let mut r = ProcessRunner::default();
            r.start(sh("sleep 5", Duration::from_millis(200)), String::new())
                .unwrap();
            match wait_outcome(&mut r) {
                AiOutcome::Failed(e) => assert!(e.contains("timed out"), "{e}"),
                o => panic!("expected Failed, got {o:?}"),
            }
        }

        #[test]
        fn cancel_reports_cancelled() {
            let mut r = ProcessRunner::default();
            r.start(sh("sleep 5", Duration::from_secs(30)), String::new())
                .unwrap();
            r.cancel();
            match wait_outcome(&mut r) {
                AiOutcome::Cancelled => {}
                o => panic!("expected Cancelled, got {o:?}"),
            }
        }

        #[test]
        fn missing_program_errors_on_start() {
            let mut r = ProcessRunner::default();
            let cmd = AiCommand {
                program: "auri-definitely-not-installed".to_string(),
                args: vec![],
                cwd: PathBuf::from("/tmp"),
                timeout: Duration::from_secs(1),
            };
            let err = r.start(cmd, String::new()).unwrap_err();
            assert!(err.to_string().contains("not found in PATH"), "{err}");
            assert!(!r.is_running());
        }
    }
}
