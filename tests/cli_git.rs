use std::fs;
use std::process::Command;

use tempfile::TempDir;
use terminal_ide::git::cli::CliGit;
use terminal_ide::git::{GitBackend, Section};

fn git(dir: &TempDir, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(args)
        .status()
        .expect("failed to spawn git");
    assert!(status.success(), "git {args:?} failed");
}

/// Create a repo with one committed file, an unstaged modification and an
/// untracked file.
fn make_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    git(&dir, &["init"]);
    // Local-only identity so the test never touches global config.
    git(&dir, &["config", "user.name", "Tide Test"]);
    git(&dir, &["config", "user.email", "tide@example.com"]);
    fs::write(dir.path().join("tracked.txt"), "hello\n").unwrap();
    git(&dir, &["add", "tracked.txt"]);
    git(&dir, &["commit", "-m", "initial"]);
    fs::write(dir.path().join("tracked.txt"), "hello\nworld\n").unwrap();
    fs::write(dir.path().join("untracked.txt"), "brand new\n").unwrap();
    dir
}

#[test]
fn status_reports_sections() {
    let dir = make_repo();
    let git = CliGit::new(dir.path());
    let status = git.status().unwrap();

    let modified = status
        .iter()
        .find(|f| f.path == "tracked.txt")
        .expect("tracked.txt in status");
    assert_eq!(modified.section, Section::Unstaged);
    assert_eq!(modified.code, 'M');

    let untracked = status
        .iter()
        .find(|f| f.path == "untracked.txt")
        .expect("untracked.txt in status");
    assert_eq!(untracked.section, Section::Untracked);
    assert_eq!(untracked.code, 'U');
}

#[test]
fn stage_unstage_moves_file_between_sections() {
    let dir = make_repo();
    let git = CliGit::new(dir.path());

    git.stage("tracked.txt").unwrap();
    let status = git.status().unwrap();
    let staged = status
        .iter()
        .find(|f| f.path == "tracked.txt")
        .expect("tracked.txt in status");
    assert_eq!(staged.section, Section::Staged);

    git.unstage("tracked.txt").unwrap();
    let status = git.status().unwrap();
    let unstaged = status
        .iter()
        .find(|f| f.path == "tracked.txt")
        .expect("tracked.txt in status");
    assert_eq!(unstaged.section, Section::Unstaged);
}

#[test]
fn commit_clears_staged_file() {
    let dir = make_repo();
    let git = CliGit::new(dir.path());

    git.stage("tracked.txt").unwrap();
    git.commit("update tracked").unwrap();

    let status = git.status().unwrap();
    assert!(status.iter().all(|f| f.path != "tracked.txt"));
    // The untracked file is still reported.
    assert!(status.iter().any(|f| f.path == "untracked.txt"));
}

#[test]
fn diff_of_untracked_file_is_all_added() {
    let dir = make_repo();
    let git = CliGit::new(dir.path());
    let status = git.status().unwrap();
    let untracked = status
        .iter()
        .find(|f| f.path == "untracked.txt")
        .expect("untracked.txt in status")
        .clone();

    let doc = git.diff(&untracked).unwrap();
    assert_eq!(doc.path, "untracked.txt");
    assert!(!doc.binary);
    assert!(!doc.rows.is_empty());
    for row in &doc.rows {
        assert!(row.left.is_none(), "untracked diff has no old side");
        let right = row.right.as_ref().expect("new side present");
        assert_eq!(right.kind, terminal_ide::git::CellKind::Added);
    }
}

#[test]
fn branch_returns_current_name() {
    let dir = make_repo();
    let git = CliGit::new(dir.path());
    let branch = git.branch().unwrap();
    assert!(!branch.is_empty());
}
