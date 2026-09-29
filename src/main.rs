use std::io::stdout;
use std::path::PathBuf;

use anyhow::Result;
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use terminal_ide::app::App;
use terminal_ide::fs::local::LocalFs;
use terminal_ide::git::cli::{resolve_toplevel, CliGit};
use terminal_ide::prefs::{FileStore, MemoryStore, PrefsStore};

fn main() -> Result<()> {
    let arg = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
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
    );
    let result = app.run(&mut terminal);

    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    result
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
