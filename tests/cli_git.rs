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

/// A repo with a known 3-commit history: "first", "second", "third".
fn make_history_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    git(&dir, &["init"]);
    git(&dir, &["config", "user.name", "Tide Test"]);
    git(&dir, &["config", "user.email", "tide@example.com"]);
    for (name, content) in [
        ("first.txt", "one\n"),
        // Long enough that a one-line addition keeps >50% similarity,
        // so rename detection in a later commit still fires.
        ("second.txt", "two\ntwo\ntwo\ntwo\ntwo\n"),
        ("third.txt", "three\n"),
    ] {
        fs::write(dir.path().join(name), content).unwrap();
        git(&dir, &["add", name]);
        git(&dir, &["commit", "-m", name.strip_suffix(".txt").unwrap()]);
    }
    dir
}

#[test]
fn head_and_log_on_unborn_repo() {
    let dir = make_unborn_repo();
    let git = CliGit::new(dir.path());
    assert_eq!(git.head().unwrap(), None);
    assert!(git.log(0, 10).unwrap().is_empty());
}

#[test]
fn log_is_newest_first_and_pages() {
    let dir = make_history_repo();
    let git = CliGit::new(dir.path());

    assert!(git.head().unwrap().is_some());
    let log = git.log(0, 10).unwrap();
    assert_eq!(log.len(), 3);
    assert_eq!(log[0].subject, "third");
    assert_eq!(log[1].subject, "second");
    assert_eq!(log[2].subject, "first");
    assert_eq!(log[0].author, "Tide Test");
    assert!(log[0].time > 0);
    assert!(log[0].short.len() >= 7 && log[0].hash.starts_with(&log[0].short[..7]));

    // skip=1, limit=1 returns the middle commit.
    let page = git.log(1, 1).unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].hash, log[1].hash);
    // Past the end -> empty.
    assert!(git.log(3, 10).unwrap().is_empty());
}

#[test]
fn root_commit_files_are_all_added_and_diff_is_all_added() {
    let dir = make_history_repo();
    let git = CliGit::new(dir.path());
    let root = git.log(2, 1).unwrap().remove(0);

    let files = git.commit_files(&root.hash).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "first.txt");
    assert_eq!(files[0].code, 'A');

    let doc = git.commit_diff(&root.hash, &files[0]).unwrap();
    assert_eq!(doc.path, "first.txt");
    assert!(!doc.rows.is_empty());
    for row in &doc.rows {
        let right = row.right.as_ref().expect("added side present");
        assert_eq!(right.kind, terminal_ide::git::CellKind::Added);
    }
}

#[test]
fn commit_files_report_modify_and_rename() {
    let dir = make_history_repo();
    // Fourth commit: modify first.txt and rename second.txt -> renamed.txt.
    fs::write(dir.path().join("first.txt"), "one\nmore\n").unwrap();
    git(&dir, &["mv", "second.txt", "renamed.txt"]);
    fs::write(
        dir.path().join("renamed.txt"),
        "two\ntwo\ntwo\ntwo\ntwo\nrenamed\n",
    )
    .unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-m", "fourth"]);
    let git = CliGit::new(dir.path());
    let head = git.log(0, 1).unwrap().remove(0);

    let files = git.commit_files(&head.hash).unwrap();
    let modified = files
        .iter()
        .find(|f| f.path == "first.txt")
        .expect("M entry");
    assert_eq!(modified.code, 'M');
    let renamed = files
        .iter()
        .find(|f| f.path == "renamed.txt")
        .expect("R entry");
    assert_eq!(renamed.code, 'R');
    assert_eq!(renamed.orig_path.as_deref(), Some("second.txt"));

    // The rename diff shows the file content under the new path.
    let doc = git.commit_diff(&head.hash, renamed).unwrap();
    assert_eq!(doc.path, "renamed.txt");
    assert!(doc.rows.iter().any(|r| r.right.as_ref().is_some_and(
        |c| c.text.contains("renamed") && c.kind == terminal_ide::git::CellKind::Added
    )));
}

#[test]
fn merge_commit_files_diff_against_first_parent() {
    let dir = make_history_repo();
    // Branch off, add a file there, merge --no-ff back into the main branch.
    git(&dir, &["switch", "-c", "feature"]);
    fs::write(dir.path().join("feature.txt"), "feat\n").unwrap();
    git(&dir, &["add", "feature.txt"]);
    git(&dir, &["commit", "-m", "feature work"]);
    git(&dir, &["switch", "-"]);
    git(
        &dir,
        &["merge", "--no-ff", "-m", "merge feature", "feature"],
    );
    let git = CliGit::new(dir.path());
    let merge = git.log(0, 1).unwrap().remove(0);
    assert_eq!(merge.subject, "merge feature");

    // Vs first parent (the pre-merge main tip), the merge brings only the
    // branch's file — not the other commits' contents.
    let files = git.commit_files(&merge.hash).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "feature.txt");
    assert_eq!(files[0].code, 'A');
}
