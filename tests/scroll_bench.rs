//! Timing probes for the trackpad-scroll-overshoot bug (run manually):
//!   cargo test --test scroll_bench -- --ignored --nocapture
//! 1. How long one DiffView frame takes at a large terminal size — this is the
//!    per-scroll-event cost when `App::run` draws after every single event.
//! 2. How long the synchronous git calls in a Tick-refresh take.

use std::time::Instant;

use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

use terminal_ide::action::Action;
use terminal_ide::component::Component;
use terminal_ide::components::diff_view::DiffView;
use terminal_ide::git::cli::CliGit;
use terminal_ide::git::{
    CellKind, DiffCell, DiffDoc, DiffRow, FileChange, GitBackend, RowKind, Section,
};

fn big_doc(rows: usize) -> DiffDoc {
    let row = |i| DiffRow {
        kind: if i % 40 < 2 {
            RowKind::Changed
        } else {
            RowKind::Context
        },
        left: Some(DiffCell {
            line_no: i,
            text: format!("let value_{i} = some_function_call(argument_{i});"),
            kind: CellKind::Removed,
        }),
        right: Some(DiffCell {
            line_no: i,
            text: format!("let value_{i} = some_function_call(argument_{i});"),
            kind: CellKind::Added,
        }),
    };
    DiffDoc {
        path: "src/app.rs".to_string(),
        rows: (0..rows).map(row).collect(),
        binary: false,
    }
}

#[test]
#[ignore]
fn bench_diffview_render() {
    let mut v = DiffView::default();
    v.update(&Action::DiffLoaded(big_doc(3000)));
    let mut term = Terminal::new(TestBackend::new(250, 90)).unwrap();
    let area = Rect::new(0, 0, 250, 90);
    term.draw(|f| v.render(f, area, true)).unwrap();
    let n = 20;
    let start = Instant::now();
    for _ in 0..n {
        term.draw(|f| v.render(f, area, true)).unwrap();
    }
    let avg = start.elapsed() / n;
    println!("DiffView render 3000-row doc @250x90: avg {:?}", avg);
}

#[test]
#[ignore]
fn bench_git_calls() {
    // This repo itself: realistic working tree size.
    let git = CliGit::new(env!("CARGO_MANIFEST_DIR"));
    let start = Instant::now();
    let files = git.status().unwrap();
    println!("status(): {:?} ({} files)", start.elapsed(), files.len());
    let f = FileChange {
        path: "src/app.rs".to_string(),
        orig_path: None,
        section: Section::Unstaged,
        code: 'M',
    };
    let start = Instant::now();
    let doc = git.diff(&f).unwrap();
    println!(
        "diff(src/app.rs): {:?} ({} rows)",
        start.elapsed(),
        doc.rows.len()
    );
}
