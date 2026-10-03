//! Git off the UI thread. `App` submits `GitJob`s to `GitJobs`; `run_job`
//! turns each into `GitResult`s with a `GitBackend`. `GitJobs` runs jobs
//! in submission order on a worker thread (`spawn_worker`) or right away on
//! the caller's thread (the default — tests stay deterministic). Either
//! way, results only come back through `poll`/`wait_idle`, which `App`
//! calls between dispatches — never in the middle of one.
//!
//! The worker drops a `Diff` job when another one is queued behind it:
//! only the latest selection matters while the user scrolls a list.

use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;

use crate::action::Action;
use crate::components::history::HISTORY_PAGE;
use crate::git::{DiffDoc, DiffSource, FileChange, GitBackend, Section};

/// Work for the git worker. Each produces `GitResult`s.
#[derive(Debug, Clone, PartialEq)]
pub enum GitJob {
    /// Status, branch and HEAD.
    Refresh,
    /// The diff for the main pane; `fresh` = newly selected (scroll resets)
    /// rather than reloaded.
    Diff {
        source: DiffSource,
        fresh: bool,
    },
    History {
        skip: usize,
    },
    CommitFiles(String),
    /// Stage, or unstage when already staged.
    ToggleStage(FileChange),
    StageAll,
    UnstageAll,
    Discard(FileChange),
    /// Commit (or amend) — only when it can't prompt on the terminal; a
    /// prompting commit runs on the UI thread (`App`).
    Commit {
        message: String,
        amend: bool,
    },
    LastCommitMessage,
}

/// What a job produced.
#[derive(Debug, Clone)]
pub enum GitResult {
    /// Broadcast as-is.
    Action(Action),
    /// HEAD after a refresh; `App` reloads history when it moved.
    Head(Option<String>),
    /// A diff for `source`; `App` drops it when the pane has moved on.
    Diff {
        source: DiffSource,
        doc: Arc<DiffDoc>,
        fresh: bool,
    },
}

/// A finished job and its results (empty when the worker skipped it).
#[derive(Debug)]
pub struct Done {
    pub job: GitJob,
    pub results: Vec<GitResult>,
}

fn error(msg: String) -> GitResult {
    GitResult::Action(Action::Error(msg))
}

/// Ok -> `Refresh` (the change shows up), Err -> an error message.
fn then_refresh(result: anyhow::Result<()>, what: &str) -> Vec<GitResult> {
    match result {
        Ok(()) => vec![GitResult::Action(Action::Refresh)],
        Err(e) => vec![error(format!("{what}: {e}"))],
    }
}

/// `Err("Nothing staged")` for a plain commit with an empty index — asked
/// of git at commit time, so a stage queued just before counts. An amend
/// needs nothing staged.
pub fn check_staged(git: &dyn GitBackend, amend: bool) -> Result<(), String> {
    if amend {
        return Ok(());
    }
    match git.status() {
        Ok(files) if files.iter().any(|f| f.section == Section::Staged) => Ok(()),
        Ok(_) => Err("Nothing staged".to_string()),
        Err(e) => Err(format!("status: {e}")),
    }
}

/// Results of `git commit`.
pub fn commit_results(result: anyhow::Result<()>, amend: bool) -> Vec<GitResult> {
    match result {
        Ok(()) => vec![
            GitResult::Action(Action::CommitDone { amended: amend }),
            GitResult::Action(Action::Refresh),
        ],
        Err(e) => vec![error(format!("commit: {e}"))],
    }
}

/// Run one job against `git`.
pub fn run_job(git: &dyn GitBackend, job: &GitJob) -> Vec<GitResult> {
    match job {
        GitJob::Refresh => {
            let mut out = Vec::new();
            out.push(match git.status() {
                Ok(files) => GitResult::Action(Action::StatusLoaded(files)),
                Err(e) => error(format!("status: {e}")),
            });
            out.push(match git.branch() {
                Ok(b) => GitResult::Action(Action::BranchLoaded(b)),
                Err(e) => error(format!("branch: {e}")),
            });
            out.push(match git.head() {
                Ok(head) => GitResult::Head(head),
                Err(e) => error(format!("head: {e}")),
            });
            out
        }
        GitJob::Diff { source, fresh } => {
            let doc = match source {
                DiffSource::Working(file) => git.diff(file),
                DiffSource::Commit { hash, file } => git.commit_diff(hash, file),
            };
            match doc {
                Ok(doc) => vec![GitResult::Diff {
                    source: source.clone(),
                    doc: Arc::new(doc),
                    fresh: *fresh,
                }],
                Err(e) => vec![error(format!("diff: {e}"))],
            }
        }
        GitJob::History { skip } => match git.log(*skip, HISTORY_PAGE) {
            Ok(commits) => vec![GitResult::Action(Action::HistoryLoaded {
                skip: *skip,
                commits,
            })],
            Err(e) => vec![error(format!("log: {e}"))],
        },
        GitJob::CommitFiles(hash) => match git.commit_files(hash) {
            Ok(files) => vec![GitResult::Action(Action::CommitFilesLoaded {
                hash: hash.clone(),
                files,
            })],
            Err(e) => vec![error(format!("commit files: {e}"))],
        },
        // `Changes` re-selects the file in its new section on the next
        // `StatusLoaded`, which emits a fresh `SelectFile`.
        GitJob::ToggleStage(file) if file.section == Section::Staged => {
            then_refresh(git.unstage(&file.path), "unstage")
        }
        GitJob::ToggleStage(file) => then_refresh(git.stage(&file.path), "stage"),
        GitJob::StageAll => then_refresh(git.stage_all(), "stage"),
        GitJob::UnstageAll => then_refresh(git.unstage_all(), "unstage"),
        GitJob::Discard(file) => then_refresh(git.discard(file), "discard"),
        GitJob::Commit { message, amend } => match check_staged(git, *amend) {
            Ok(()) => commit_results(git.commit(message, *amend), *amend),
            Err(e) => vec![error(e)],
        },
        GitJob::LastCommitMessage => match git.last_commit_message() {
            Ok(message) => vec![GitResult::Action(Action::LastCommitMessageLoaded(message))],
            Err(e) => vec![error(format!("amend: {e}"))],
        },
    }
}

/// Runs `GitJob`s and hands back their results in submission order. Also
/// owns the UI thread's own backend for the few calls that must stay
/// synchronous (`backend`) — call `wait_idle` first so they see every
/// queued change.
pub struct GitJobs {
    git: Box<dyn GitBackend>,
    /// Finished jobs waiting for `poll` (inline mode, or a dead worker).
    ready: VecDeque<Done>,
    /// `None`: jobs run inline on `submit`.
    worker: Option<Worker>,
}

struct Worker {
    jobs: Sender<GitJob>,
    done: Receiver<Done>,
    /// Submitted jobs not yet returned.
    in_flight: usize,
}

impl GitJobs {
    /// Jobs run on the caller's thread as they are submitted.
    pub fn inline(git: Box<dyn GitBackend>) -> Self {
        Self {
            git,
            ready: VecDeque::new(),
            worker: None,
        }
    }

    /// From now on, run jobs on a worker thread that owns `worker` (another
    /// backend for the same repo).
    pub fn spawn_worker(&mut self, worker: Box<dyn GitBackend + Send>) {
        let (jobs, jobs_rx) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        thread::Builder::new()
            .name("auri-git".to_string())
            .spawn(move || work(worker, jobs_rx, done_tx))
            .expect("failed to spawn the git worker thread");
        self.worker = Some(Worker {
            jobs,
            done,
            in_flight: 0,
        });
    }

    /// The UI thread's backend, for synchronous calls.
    pub fn backend(&self) -> &dyn GitBackend {
        &*self.git
    }

    pub fn submit(&mut self, job: GitJob) {
        let Some(worker) = &mut self.worker else {
            let results = run_job(&*self.git, &job);
            self.ready.push_back(Done { job, results });
            return;
        };
        if let Err(mpsc::SendError(job)) = worker.jobs.send(job) {
            self.worker_died();
            self.submit(job);
            return;
        }
        worker.in_flight += 1;
    }

    /// The worker thread is gone (it panicked) with its in-flight jobs:
    /// run inline from now on and say so. The notice is filed as a
    /// `Refresh` so `App` stops waiting for one that may have been lost.
    fn worker_died(&mut self) {
        self.worker = None;
        self.ready.push_back(Done {
            job: GitJob::Refresh,
            results: vec![error(
                "git worker stopped — git now runs on the UI thread".to_string(),
            )],
        });
    }

    /// Whether submitted jobs haven't been collected yet.
    pub fn busy(&self) -> bool {
        !self.ready.is_empty() || self.worker.as_ref().is_some_and(|w| w.in_flight > 0)
    }

    /// Finished jobs, without blocking.
    pub fn poll(&mut self) -> Vec<Done> {
        let mut died = false;
        let mut out = Vec::new();
        if let Some(worker) = &mut self.worker {
            loop {
                match worker.done.try_recv() {
                    Ok(done) => {
                        worker.in_flight -= 1;
                        out.push(done);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        died = true;
                        break;
                    }
                }
            }
        }
        if died {
            self.worker_died();
        }
        self.ready.drain(..).chain(out).collect()
    }

    /// Block until every submitted job has finished; their results.
    pub fn wait_idle(&mut self) -> Vec<Done> {
        let mut died = false;
        let mut out = Vec::new();
        if let Some(worker) = &mut self.worker {
            while worker.in_flight > 0 {
                match worker.done.recv() {
                    Ok(done) => {
                        worker.in_flight -= 1;
                        out.push(done);
                    }
                    Err(_) => {
                        died = true;
                        break;
                    }
                }
            }
        }
        if died {
            self.worker_died();
        }
        self.ready.drain(..).chain(out).collect()
    }
}

/// The worker thread: run jobs in order until `App` drops its sender.
fn work(git: Box<dyn GitBackend + Send>, jobs: Receiver<GitJob>, done: Sender<Done>) {
    let mut queue: VecDeque<GitJob> = VecDeque::new();
    loop {
        if queue.is_empty() {
            match jobs.recv() {
                Ok(job) => queue.push_back(job),
                Err(_) => return,
            }
        }
        queue.extend(jobs.try_iter());
        let Some(job) = queue.pop_front() else {
            continue;
        };
        let results = if supersede(&job, &mut queue) {
            Vec::new()
        } else {
            run_job(&*git, &job)
        };
        if done.send(Done { job, results }).is_err() {
            return;
        }
    }
}

/// Whether `job` can be skipped because `queue` holds a newer job of the
/// same kind. A skipped fresh diff passes its `fresh` on, so the diff that
/// does load still resets the scroll.
fn supersede(job: &GitJob, queue: &mut VecDeque<GitJob>) -> bool {
    let GitJob::Diff { source, fresh } = job else {
        return false;
    };
    let Some(GitJob::Diff {
        source: next,
        fresh: next_fresh,
    }) = queue.iter_mut().find(|j| matches!(j, GitJob::Diff { .. }))
    else {
        return false;
    };
    if *fresh && next == source {
        *next_fresh = true;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{Commit, CommitFile};
    use anyhow::Result;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// A `Send` backend that logs calls. While `gate` is set, `stage`
    /// blocks until the test sends on it — jobs queue up behind it.
    #[derive(Default)]
    struct Fake {
        calls: Arc<Mutex<Vec<String>>>,
        staged: bool,
        gate: Option<Mutex<Receiver<()>>>,
        /// `head` panics (kills the worker thread).
        panic_on_head: bool,
    }

    impl Fake {
        fn log(&self, call: impl Into<String>) {
            self.calls.lock().unwrap().push(call.into());
        }
    }

    fn change(path: &str, section: Section) -> FileChange {
        FileChange {
            path: path.to_string(),
            orig_path: None,
            section,
            code: 'M',
        }
    }

    impl GitBackend for Fake {
        fn root(&self) -> PathBuf {
            PathBuf::from("/repo")
        }
        fn status(&self) -> Result<Vec<FileChange>> {
            self.log("status");
            let section = if self.staged {
                Section::Staged
            } else {
                Section::Unstaged
            };
            Ok(vec![change("a.rs", section)])
        }
        fn diff(&self, file: &FileChange) -> Result<DiffDoc> {
            self.log(format!("diff {}", file.path));
            Ok(DiffDoc {
                path: file.path.clone(),
                rows: vec![],
                binary: false,
            })
        }
        fn stage(&self, path: &str) -> Result<()> {
            if let Some(gate) = &self.gate {
                gate.lock().unwrap().recv().unwrap();
            }
            self.log(format!("stage {path}"));
            Ok(())
        }
        fn unstage(&self, path: &str) -> Result<()> {
            self.log(format!("unstage {path}"));
            Ok(())
        }
        fn discard(&self, _file: &FileChange) -> Result<()> {
            Ok(())
        }
        fn stage_all(&self) -> Result<()> {
            Ok(())
        }
        fn unstage_all(&self) -> Result<()> {
            Ok(())
        }
        fn commit(&self, message: &str, amend: bool) -> Result<()> {
            self.log(format!("commit {message} amend={amend}"));
            Ok(())
        }
        fn last_commit_message(&self) -> Result<String> {
            Ok("last".to_string())
        }
        fn branch(&self) -> Result<String> {
            Ok("main".to_string())
        }
        fn head(&self) -> Result<Option<String>> {
            assert!(!self.panic_on_head, "worker crash");
            Ok(Some("abc".to_string()))
        }
        fn log(&self, _skip: usize, _limit: usize) -> Result<Vec<Commit>> {
            Ok(vec![])
        }
        fn commit_files(&self, _hash: &str) -> Result<Vec<CommitFile>> {
            Ok(vec![])
        }
        fn commit_diff(&self, _hash: &str, _file: &CommitFile) -> Result<DiffDoc> {
            anyhow::bail!("unused")
        }
        fn staged_patch(&self) -> Result<(String, String)> {
            Ok((String::new(), String::new()))
        }
    }

    fn actions(results: &[GitResult]) -> Vec<String> {
        results
            .iter()
            .map(|r| match r {
                GitResult::Action(a) => format!("{a:?}"),
                GitResult::Head(h) => format!("Head({h:?})"),
                GitResult::Diff { source, fresh, .. } => format!("Diff({source:?}, {fresh})"),
            })
            .collect()
    }

    fn diff_job(path: &str, fresh: bool) -> GitJob {
        GitJob::Diff {
            source: DiffSource::Working(change(path, Section::Unstaged)),
            fresh,
        }
    }

    #[test]
    fn refresh_reports_status_branch_and_head() {
        let out = actions(&run_job(&Fake::default(), &GitJob::Refresh));
        assert_eq!(out.len(), 3);
        assert!(out[0].starts_with("StatusLoaded"), "{out:?}");
        assert_eq!(out[1], "BranchLoaded(\"main\")");
        assert_eq!(out[2], "Head(Some(\"abc\"))");
    }

    #[test]
    fn mutations_refresh_and_toggle_follows_the_section() {
        let git = Fake::default();
        let staged = GitJob::ToggleStage(change("a.rs", Section::Staged));
        assert_eq!(actions(&run_job(&git, &staged)), ["Refresh"]);
        run_job(
            &git,
            &GitJob::ToggleStage(change("b.rs", Section::Unstaged)),
        );
        assert_eq!(*git.calls.lock().unwrap(), ["unstage a.rs", "stage b.rs"]);
    }

    #[test]
    fn commit_asks_git_whether_anything_is_staged() {
        let commit = |amend| GitJob::Commit {
            message: "m".to_string(),
            amend,
        };
        let git = Fake::default();
        assert_eq!(
            actions(&run_job(&git, &commit(false))),
            ["Error(\"Nothing staged\")"]
        );
        // An amend needs nothing staged.
        assert_eq!(
            actions(&run_job(&git, &commit(true))),
            ["CommitDone { amended: true }", "Refresh"]
        );
        let git = Fake {
            staged: true,
            ..Fake::default()
        };
        assert_eq!(
            actions(&run_job(&git, &commit(false))),
            ["CommitDone { amended: false }", "Refresh"]
        );
    }

    #[test]
    fn inline_jobs_finish_on_submit() {
        let mut jobs = GitJobs::inline(Box::new(Fake::default()));
        jobs.submit(GitJob::LastCommitMessage);
        assert!(jobs.busy());
        let done = jobs.poll();
        assert_eq!(done.len(), 1);
        assert_eq!(
            actions(&done[0].results),
            ["LastCommitMessageLoaded(\"last\")"]
        );
        assert!(!jobs.busy());
    }

    /// A worker whose first job (a stage) blocks until `release` is sent.
    fn gated_worker() -> (GitJobs, Sender<()>, Arc<Mutex<Vec<String>>>) {
        let (release, gate) = mpsc::channel();
        let worker = Fake {
            gate: Some(Mutex::new(gate)),
            ..Fake::default()
        };
        let calls = Arc::clone(&worker.calls);
        let mut jobs = GitJobs::inline(Box::new(Fake::default()));
        jobs.spawn_worker(Box::new(worker));
        (jobs, release, calls)
    }

    #[test]
    fn worker_runs_in_order_and_skips_superseded_diffs() {
        let (mut jobs, release, calls) = gated_worker();
        jobs.submit(GitJob::ToggleStage(change("a.rs", Section::Unstaged)));
        jobs.submit(diff_job("x.rs", true));
        jobs.submit(diff_job("y.rs", true));
        jobs.submit(diff_job("y.rs", false));
        jobs.submit(GitJob::LastCommitMessage);
        assert!(jobs.busy());
        assert!(
            jobs.poll().is_empty(),
            "nothing done while the stage blocks"
        );
        release.send(()).unwrap();
        let done = jobs.wait_idle();
        assert!(!jobs.busy());

        let order: Vec<_> = done.iter().map(|d| d.job.clone()).collect();
        assert_eq!(
            order[0],
            GitJob::ToggleStage(change("a.rs", Section::Unstaged))
        );
        assert_eq!(order[4], GitJob::LastCommitMessage);
        // Only the last diff ran — and it is fresh: the skipped fresh
        // request for the same file passed that on.
        assert!(done[1].results.is_empty() && done[2].results.is_empty());
        assert_eq!(
            actions(&done[3].results),
            [format!(
                "Diff({:?}, true)",
                DiffSource::Working(change("y.rs", Section::Unstaged))
            )]
        );
        assert_eq!(*calls.lock().unwrap(), ["stage a.rs", "diff y.rs"]);
    }

    #[test]
    fn poll_collects_without_blocking() {
        let (mut jobs, release, _) = gated_worker();
        jobs.submit(GitJob::ToggleStage(change("a.rs", Section::Unstaged)));
        assert!(jobs.poll().is_empty());
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut done = Vec::new();
        while done.is_empty() && Instant::now() < deadline {
            done = jobs.poll();
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(actions(&done[0].results), ["Refresh"]);
        assert!(!jobs.busy());
    }

    #[test]
    fn a_dead_worker_is_reported_and_jobs_run_inline_after() {
        let mut jobs = GitJobs::inline(Box::new(Fake::default()));
        jobs.spawn_worker(Box::new(Fake {
            panic_on_head: true,
            ..Fake::default()
        }));
        jobs.submit(GitJob::Refresh);
        let done = jobs.wait_idle();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].job, GitJob::Refresh, "unblocks App's refresh");
        assert!(actions(&done[0].results)[0].contains("git worker stopped"));
        // From now on jobs run on the UI thread's backend.
        jobs.submit(GitJob::Refresh);
        assert_eq!(actions(&jobs.poll()[0].results)[2], "Head(Some(\"abc\"))");
    }
}
