use std::io::stdout;
use std::path::PathBuf;

use anyhow::Result;
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use terminal_ide::app::App;
use terminal_ide::git::cli::{resolve_toplevel, CliGit};

fn main() -> Result<()> {
    let arg = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());
    let root = resolve_toplevel(&PathBuf::from(arg))?;

    install_panic_hook();
    let mut terminal = ratatui::init();
    execute!(stdout(), EnableMouseCapture)?;

    let mut app = App::new(Box::new(CliGit::new(root)));
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
