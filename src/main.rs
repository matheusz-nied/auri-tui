use std::io::stdout;
use std::path::PathBuf;

use anyhow::Result;
use auri_tui::app::{App, TerminalHandoff};
use auri_tui::fs::local::LocalFs;
use auri_tui::git::cli::{resolve_toplevel, CliGit};
use auri_tui::prefs::{FileStore, MemoryStore, PrefsStore};
use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
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
b = toggle sidebar, Tab = next panel, q = quit.";

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
    execute!(stdout(), EnableMouseCapture)?;

    // No home directory (nothing to derive a config path from) -> prefs are
    // in-memory only for this session.
    let store: Box<dyn PrefsStore> = match FileStore::default_path() {
        Some(path) => Box::new(FileStore::new(path)),
        None => Box::new(MemoryStore::new(None)),
    };
    let mut app = App::new(
        Box::new(CliGit::new(root.clone())),
        Box::new(LocalFs::new(root)),
        store,
    )
    .with_terminal_handoff(Box::new(Crossterm));
    let result = app.run(&mut terminal);

    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

/// Undoes/redoes what `ratatui::init` + `EnableMouseCapture` set up, so a
/// prompting `git commit` (GPG pinentry, hooks) gets a normal terminal.
struct Crossterm;

impl TerminalHandoff for Crossterm {
    fn release(&mut self) {
        let _ = execute!(stdout(), DisableMouseCapture, LeaveAlternateScreen, Show);
        let _ = disable_raw_mode();
    }

    fn reclaim(&mut self) {
        let _ = enable_raw_mode();
        let _ = execute!(stdout(), EnterAlternateScreen, EnableMouseCapture);
    }
}

/// Restore the terminal before the default panic handler prints, so a panic
/// doesn't leave the terminal in raw mode / the alternate screen.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture);
        ratatui::restore();
        default_hook(info);
    }));
}
