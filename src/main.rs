use std::io::stdout;
use std::path::PathBuf;

use anyhow::Result;
use auri_tui::app::{App, TerminalHandoff};
use auri_tui::clipboard::Osc52;
use auri_tui::fs::local::LocalFs;
use auri_tui::git::cli::{git_dirs, resolve_toplevel, CliGit};
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

Usage: auri [REPO]

Arguments:
  [REPO]  Path inside a git repository (default: current directory)

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
        _ => {}
    }
    let root = resolve_toplevel(&PathBuf::from(arg))?;

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
    let watcher = git_dirs(&root)
        .and_then(|(git_dir, common_dir)| FsWatcher::start(&root, &git_dir, &common_dir))
        .map(|w| Box::new(w) as Box<dyn RepoWatcher>);
    // Two stateless git backends: one for the worker thread (status,
    // diffs, staging…), one for the few calls the UI thread makes itself.
    let mut app = App::new(
        Box::new(CliGit::new(root.clone())),
        Box::new(LocalFs::new(root.clone())),
        store,
    )
    .with_git_worker(Box::new(CliGit::new(root)))
    .with_terminal_handoff(Box::new(Crossterm))
    .with_clipboard(Box::new(Osc52))
    .with_watcher(watcher);
    let result = app.run(&mut terminal);

    let _ = execute!(stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    result
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
