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

/// A repo with zero commits (unborn HEAD).
fn make_unborn_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    git(&dir, &["init"]);
    git(&dir, &["config", "user.name", "Tide Test"]);
    git(&dir, &["config", "user.email", "tide@example.com"]);
    fs::write(dir.path().join("new.txt"), "hello\n").unwrap();
    dir
}

#[test]
fn unstage_without_commits_falls_back_to_rm_cached() {
    let dir = make_unborn_repo();
    let git = CliGit::new(dir.path());

    git.stage("new.txt").unwrap();
    let status = git.status().unwrap();
    let staged = status
        .iter()
        .find(|f| f.path == "new.txt")
        .expect("new.txt in status");
    assert_eq!(staged.section, Section::Staged);

    // `git restore --staged` would fail here ("could not resolve 'HEAD'").
    git.unstage("new.txt").unwrap();
    let status = git.status().unwrap();
    let entry = status
        .iter()
        .find(|f| f.path == "new.txt")
        .expect("new.txt still in status");
    assert_eq!(entry.section, Section::Untracked);
}

#[test]
fn commit_works_on_unborn_repo() {
    let dir = make_unborn_repo();
    let git = CliGit::new(dir.path());

    git.stage("new.txt").unwrap();
    git.commit("first commit").unwrap();
    assert!(git.status().unwrap().is_empty());
    // After the first commit, `restore --staged` is usable again.
    fs::write(dir.path().join("new.txt"), "changed\n").unwrap();
    git.stage("new.txt").unwrap();
    git.unstage("new.txt").unwrap();
    let entry = git
        .status()
        .unwrap()
        .into_iter()
        .find(|f| f.path == "new.txt")
        .expect("new.txt in status");
    assert_eq!(entry.section, Section::Unstaged);
}

fn find(
    status: &[terminal_ide::git::FileChange],
    path: &str,
    section: Section,
) -> terminal_ide::git::FileChange {
    status
        .iter()
        .find(|f| f.path == path && f.section == section)
        .unwrap_or_else(|| panic!("{path} not in {section:?}"))
        .clone()
}

#[test]
fn discard_unstaged_modification_restores_content() {
    let dir = make_repo();
    let git = CliGit::new(dir.path());
    let file = find(&git.status().unwrap(), "tracked.txt", Section::Unstaged);

    git.discard(&file).unwrap();

    assert_eq!(
        fs::read_to_string(dir.path().join("tracked.txt")).unwrap(),
        "hello\n"
    );
    assert!(git
        .status()
        .unwrap()
        .iter()
        .all(|f| f.path != "tracked.txt"));
}

#[test]
fn discard_untracked_deletes_file() {
    let dir = make_repo();
    let git = CliGit::new(dir.path());
    let file = find(&git.status().unwrap(), "untracked.txt", Section::Untracked);

    git.discard(&file).unwrap();

    assert!(!dir.path().join("untracked.txt").exists());
}

#[test]
fn discard_deleted_in_worktree_restores_file() {
    let dir = make_repo();
    fs::remove_file(dir.path().join("tracked.txt")).unwrap();
    let git = CliGit::new(dir.path());
    let file = find(&git.status().unwrap(), "tracked.txt", Section::Unstaged);
    assert_eq!(file.code, 'D');

    git.discard(&file).unwrap();

    assert_eq!(
        fs::read_to_string(dir.path().join("tracked.txt")).unwrap(),
        "hello\n"
    );
}

#[test]
fn discard_staged_file_errors() {
    let dir = make_repo();
    let git = CliGit::new(dir.path());
    git.stage("tracked.txt").unwrap();
    let file = find(&git.status().unwrap(), "tracked.txt", Section::Staged);
    assert!(git.discard(&file).is_err());
}

#[test]
fn unstage_all_with_head() {
    let dir = make_repo();
    let git = CliGit::new(dir.path());
    git.stage_all().unwrap();
    assert!(git
        .status()
        .unwrap()
        .iter()
        .any(|f| f.section == Section::Staged));

    git.unstage_all().unwrap();

    assert!(git
        .status()
        .unwrap()
        .iter()
        .all(|f| f.section != Section::Staged));
}

#[test]
fn unstage_all_on_unborn_repo() {
    let dir = make_unborn_repo();
    let git = CliGit::new(dir.path());
    git.stage("new.txt").unwrap();

    git.unstage_all().unwrap();

    let status = git.status().unwrap();
    assert!(status.iter().all(|f| f.section != Section::Staged));
    assert_eq!(find(&status, "new.txt", Section::Untracked).path, "new.txt");
}

#[test]
fn branch_returns_current_name() {
    let dir = make_repo();
    let git = CliGit::new(dir.path());
    let branch = git.branch().unwrap();
    assert!(!branch.is_empty());
}
