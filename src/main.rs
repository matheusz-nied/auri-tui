use std::io::stdout;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use auri_tui::app::{App, TerminalHandoff};
use auri_tui::clipboard::Osc52;
use auri_tui::fs::local::LocalFs;
use auri_tui::git::cli::{find_toplevel, git_dirs, CliGit};
use auri_tui::prefs::{FileStore, MemoryStore, PrefsStore};
use auri_tui::watch::{FsWatcher, RepoWatcher};
use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};

const USAGE: &str = "\
auri — browse files and git diffs side by side, right in your terminal

Usage: auri [PATH]
       auri update

Arguments:
  [PATH]  Folder or path inside a git repository (default: current directory)

Commands:
  update  Install the latest release in place of this binary

Options:
  -h, --help     Print help
  -V, --version  Print version

Inside auri: e = files, c = source control, 2 = main pane,
i = edit the open file (Esc stops, Ctrl-S saves), b = toggle sidebar,
Tab = next panel, q = quit, ? = all keys.";

fn main() -> Result<()> {
    let arg = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
    match arg.as_str() {
        "-h" | "--help" => {
            println!("{USAGE}");
            return Ok(());
        }
        "-V" | "--version" => {
            println!("auri {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        // A directory named `update` still opens as `auri ./update`.
        "update" => return auri_tui::update::run(),
        _ => {}
    }
    let (root, has_git) = workspace(&PathBuf::from(arg))?;

    install_panic_hook();
    let mut terminal = ratatui::init();
    // Bracketed paste: a pasted newline must not act as Enter (commit).
    execute!(stdout(), EnableMouseCapture, EnableBracketedPaste)?;

    // No home directory (nothing to derive a config path from) -> prefs are
    // in-memory only for this session.
    let store: Box<dyn PrefsStore> = match FileStore::default_path() {
        Some(path) => Box::new(FileStore::new(path)),
        None => Box::new(MemoryStore::new(None)),
    };
    // Refresh when the repo changes; without a watcher, poll every 2 s.
    let watcher = if has_git {
        git_dirs(&root)
            .and_then(|(git_dir, common_dir)| FsWatcher::start(&root, &git_dir, &common_dir))
    } else {
        FsWatcher::start_workspace(&root)
    }
    .map(|w| Box::new(w) as Box<dyn RepoWatcher>);
    // Two stateless git backends: one for the worker thread (status,
    // diffs, staging…), one for the few calls the UI thread makes itself.
    let fs = Box::new(LocalFs::new(root.clone()));
    let app = if has_git {
        App::new(Box::new(CliGit::new(root.clone())), fs, store)
    } else {
        App::without_git(Box::new(CliGit::new(root.clone())), fs, store)
    };
    let mut app = app
        .with_git_worker(Box::new(CliGit::new(root)))
        .with_terminal_handoff(Box::new(Crossterm))
        .with_clipboard(Box::new(Osc52))
        .with_watcher(watcher);
    let result = app.run(&mut terminal);

    let _ = execute!(stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    result
}

fn workspace(path: &std::path::Path) -> Result<(PathBuf, bool)> {
    let folder = path
        .canonicalize()
        .with_context(|| format!("cannot open '{}'", path.display()))?;
    if !folder.is_dir() {
        bail!("'{}' is not a directory", path.display());
    }
    match find_toplevel(&folder)? {
        Some(root) => Ok((root, true)),
        None => Ok((folder, false)),
    }
}

/// Undoes/redoes what `ratatui::init` + `EnableMouseCapture` set up, so a
/// prompting `git commit` (GPG pinentry, hooks) gets a normal
/// terminal.
struct Crossterm;

impl TerminalHandoff for Crossterm {
    fn release(&mut self) {
        let _ = execute!(
            stdout(),
            DisableMouseCapture,
            DisableBracketedPaste,
            LeaveAlternateScreen,
            Show
        );
        let _ = disable_raw_mode();
    }

    fn reclaim(&mut self) {
        let _ = enable_raw_mode();
        let _ = execute!(
            stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste
        );
    }
}

/// Restore the terminal before the default panic handler prints, so a panic
/// doesn't leave the terminal in raw mode / the alternate screen.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture, DisableBracketedPaste);
        ratatui::restore();
        default_hook(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_folder_opens_without_git() {
        let folder = tempfile::tempdir().unwrap();
        let (root, has_git) = workspace(folder.path()).unwrap();
        assert_eq!(root, folder.path().canonicalize().unwrap());
        assert!(!has_git);
        assert!(!folder.path().join(".git").exists());
    }

    #[test]
    fn repository_subfolder_opens_the_worktree_root() {
        let folder = tempfile::tempdir().unwrap();
        let output = std::process::Command::new("git")
            .arg("init")
            .arg(folder.path())
            .output()
            .unwrap();
        assert!(output.status.success());
        let nested = folder.path().join("src");
        std::fs::create_dir(&nested).unwrap();
        let (root, has_git) = workspace(&nested).unwrap();
        assert_eq!(root, folder.path().canonicalize().unwrap());
        assert!(has_git);
    }

    #[test]
    fn missing_paths_and_files_are_rejected() {
        let folder = tempfile::tempdir().unwrap();
        assert!(workspace(&folder.path().join("missing")).is_err());
        let file = folder.path().join("file.txt");
        std::fs::write(&file, "hello").unwrap();
        assert!(workspace(&file)
            .unwrap_err()
            .to_string()
            .contains("not a directory"));
    }

    #[test]
    fn broken_git_metadata_is_not_treated_as_a_plain_folder() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join(".git"), "invalid gitfile\n").unwrap();
        assert!(workspace(folder.path()).is_err());
    }
}
