//! Opening a file in the user's editor (`o`). `command` is pure: it picks
//! the editor (`$VISUAL`, `$EDITOR`, else `vi`) and builds its arguments,
//! including a jump to a line for editors whose syntax is known.
//! `EditorLauncher` is what `App` calls — with the terminal handed over —
//! so tests can fake it.

use std::process::Command;

use anyhow::{bail, Context, Result};

/// Editor used when neither `$VISUAL` nor `$EDITOR` is set.
const FALLBACK_EDITOR: &str = "vi";

/// An editor invocation. Run like git does, through
/// `sh -c '<editor> "$@"' <editor> <args>...`, so `editor` may carry its
/// own flags (`code --wait`, `nvim -u NONE`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorCommand {
    pub editor: String,
    pub args: Vec<String>,
}

/// The first non-blank of `visual`/`editor` (else `vi`), opening `path`
/// at `line` (1-based) when the editor's line syntax is known.
pub fn command(
    visual: Option<&str>,
    editor: Option<&str>,
    path: &str,
    line: Option<usize>,
) -> EditorCommand {
    let editor = [visual, editor]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|e| !e.is_empty())
        .unwrap_or(FALLBACK_EDITOR);
    let program = editor
        .split_whitespace()
        .next()
        .and_then(|p| p.rsplit('/').next())
        .unwrap_or_default();
    let args = match line {
        None => vec![path.to_string()],
        Some(n) => match program {
            "vi" | "vim" | "nvim" | "view" | "gvim" | "mvim" | "nano" | "emacs" | "emacsclient"
            | "micro" | "kak" | "joe" | "mg" => {
                vec![format!("+{n}"), path.to_string()]
            }
            "hx" | "helix" | "subl" | "zed" | "zeditor" => vec![format!("{path}:{n}")],
            "code" | "code-insiders" | "codium" | "cursor" | "windsurf" => {
                vec!["--goto".to_string(), format!("{path}:{n}")]
            }
            // Unknown syntax: a stray `+12` could open as a file.
            _ => vec![path.to_string()],
        },
    };
    EditorCommand {
        editor: editor.to_string(),
        args,
    }
}

/// Opens a file in an editor and waits for it to exit. `App` releases the
/// terminal around the call (`TerminalHandoff`).
pub trait EditorLauncher {
    /// `path` is absolute; `line` is 1-based.
    fn open(&mut self, path: &str, line: Option<usize>) -> Result<()>;
}

/// The real launcher: `$VISUAL`/`$EDITOR` through `sh`, on the inherited
/// terminal.
pub struct ShellEditor;

impl EditorLauncher for ShellEditor {
    fn open(&mut self, path: &str, line: Option<usize>) -> Result<()> {
        let visual = std::env::var("VISUAL").ok();
        let editor = std::env::var("EDITOR").ok();
        run(&command(visual.as_deref(), editor.as_deref(), path, line))
    }
}

/// Run `cmd` through `sh` and wait; a non-zero exit is an error.
fn run(cmd: &EditorCommand) -> Result<()> {
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("{} \"$@\"", cmd.editor))
        .arg(&cmd.editor)
        .args(&cmd.args)
        .status()
        .with_context(|| format!("failed to run {}", cmd.editor))?;
    if !status.success() {
        bail!("{} exited with {status}", cmd.editor);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(editor: &str, line: Option<usize>) -> Vec<String> {
        command(None, Some(editor), "/r/a.rs", line).args
    }

    #[test]
    fn visual_wins_then_editor_then_vi() {
        let pick = |v, e| command(v, e, "/r/a.rs", None).editor;
        assert_eq!(pick(Some("nvim"), Some("nano")), "nvim");
        assert_eq!(pick(Some("  "), Some("nano")), "nano");
        assert_eq!(pick(None, Some("")), "vi");
        assert_eq!(pick(None, None), "vi");
    }

    #[test]
    fn line_syntax_follows_the_program() {
        assert_eq!(args("vim", Some(12)), ["+12", "/r/a.rs"]);
        assert_eq!(args("/opt/bin/nvim -u NONE", Some(3)), ["+3", "/r/a.rs"]);
        assert_eq!(args("hx", Some(12)), ["/r/a.rs:12"]);
        assert_eq!(args("code --wait", Some(12)), ["--goto", "/r/a.rs:12"]);
    }

    #[test]
    fn unknown_editor_or_no_line_gets_just_the_path() {
        assert_eq!(args("ed", Some(12)), ["/r/a.rs"]);
        assert_eq!(args("vim", None), ["/r/a.rs"]);
    }

    #[test]
    fn run_keeps_editor_flags_and_quotes_args() {
        // The editor string is shell code, the path one argument even with
        // a space: this runs `test "/r/a b.rs" = "/r/a b.rs"`.
        let ok = command(None, Some("test \"/r/a b.rs\" ="), "/r/a b.rs", None);
        assert!(run(&ok).is_ok());
        let err = run(&command(None, Some("false"), "/r/a.rs", None)).unwrap_err();
        assert!(err.to_string().starts_with("false exited with"), "{err}");
    }
}
