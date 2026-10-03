//! Refresh on change: a filesystem watcher (`notify`) tells `App` when the
//! repo changed, so it refreshes at once instead of polling git every few
//! seconds. Pure parts — which paths and event kinds matter (`PathFilter`,
//! `relevant_kind`) and how a burst becomes one signal (`Debouncer`) — are
//! unit-tested; `RepoWatcher` is what `App` polls (fake it in tests);
//! `FsWatcher` is the real one, a thread that owns the OS watcher.
//!
//! Watched: the worktree (minus `.git`, `target`, `node_modules`) and, in
//! the git dir, only `HEAD`, `index`, `packed-refs` and `refs/`. Read
//! events are dropped: `git status`, the explorer and the file viewer
//! read the worktree on every refresh, and a refresh must not cause the
//! next one.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use notify::event::{MetadataKind, ModifyKind};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher, WatcherKind};

/// A burst settles after this long without a new event.
pub const QUIET: Duration = Duration::from_millis(150);
/// …or this long after its first event, so a file written continuously
/// can't postpone the refresh forever.
pub const MAX_WAIT: Duration = Duration::from_secs(1);

/// Worktree directories never watched (any depth): build output and
/// dependencies churn constantly and `git status` ignores them anyway;
/// `.git` also covers nested repos and submodules.
const IGNORED_DIRS: [&str; 3] = [".git", "target", "node_modules"];

/// What a `RepoWatcher` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    /// Something in the repo changed; `last` is when the latest change of
    /// the settled burst was seen (always after the write itself).
    Changed { last: Instant },
    /// The watcher stopped (e.g. the OS watch limit was hit while adding a
    /// new directory) — fall back to polling.
    Failed(String),
}

/// Change notifications for the repo, polled by `App` once per loop.
pub trait RepoWatcher {
    /// The next settled event, if any — never blocks.
    fn poll(&mut self) -> Option<WatchEvent>;
    /// Changes were seen but have not settled yet — the loop polls faster
    /// meanwhile so the signal is picked up promptly.
    fn pending(&self) -> bool;
}

/// Which paths are worth a refresh. All paths canonical (FSEvents reports
/// `/private/var/...` for `/var/...`).
#[derive(Debug, Clone)]
pub struct PathFilter {
    root: PathBuf,
    git_dir: PathBuf,
    common_dir: PathBuf,
}

impl PathFilter {
    pub fn new(root: PathBuf, git_dir: PathBuf, common_dir: PathBuf) -> Self {
        Self {
            root,
            git_dir,
            common_dir,
        }
    }

    /// Inside the git dir: only `HEAD`, `index`, `packed-refs` and `refs/`
    /// (never a `*.lock`). In the worktree: anything outside the ignored
    /// directories — `Cargo.lock` and friends are tracked files.
    pub fn relevant(&self, path: &Path) -> bool {
        if let Some(rel) = self.in_git_dirs(path) {
            let is_lock = path.extension().is_some_and(|e| e == "lock");
            let mut parts = rel.components();
            let first = parts.next().and_then(|c| c.as_os_str().to_str());
            return !is_lock
                && match first {
                    Some("refs") => true,
                    Some("HEAD" | "index" | "packed-refs") => parts.next().is_none(),
                    _ => false,
                };
        }
        self.in_worktree(path)
    }

    /// `path` is a worktree path (not in the git dirs, not under an ignored
    /// directory) — for files, what to report; for directories, what to
    /// watch.
    pub fn in_worktree(&self, path: &Path) -> bool {
        if self.in_git_dirs(path).is_some() {
            return false;
        }
        let Ok(rel) = path.strip_prefix(&self.root) else {
            return false;
        };
        !rel.components().any(|c| match c {
            Component::Normal(name) => IGNORED_DIRS.iter().any(|d| name == *d),
            _ => false,
        })
    }

    /// `path` relative to the git dir or the common dir it is in.
    fn in_git_dirs<'a>(&self, path: &'a Path) -> Option<&'a Path> {
        path.strip_prefix(&self.git_dir)
            .or_else(|_| path.strip_prefix(&self.common_dir))
            .ok()
    }
}

/// Whether an event kind can change what git or the viewer shows. Opens,
/// reads and atime updates can't — and git itself causes them.
pub fn relevant_kind(kind: &EventKind) -> bool {
    !matches!(
        kind,
        EventKind::Access(_) | EventKind::Modify(ModifyKind::Metadata(MetadataKind::AccessTime))
    )
}

/// Turns a burst of events into one signal: due `QUIET` after the last
/// event, or `MAX_WAIT` after the first. Takes the clock as an argument.
#[derive(Debug, Default)]
pub struct Debouncer {
    first: Option<Instant>,
    last: Option<Instant>,
}

impl Debouncer {
    pub fn event(&mut self, now: Instant) {
        self.first.get_or_insert(now);
        self.last = Some(now);
    }

    pub fn pending(&self) -> bool {
        self.last.is_some()
    }

    fn deadline(&self) -> Option<Instant> {
        Some((self.last? + QUIET).min(self.first? + MAX_WAIT))
    }

    /// How long to wait for more events before the burst is due; `None`
    /// when nothing is pending (wait indefinitely).
    pub fn timeout(&self, now: Instant) -> Option<Duration> {
        self.deadline().map(|d| d.saturating_duration_since(now))
    }

    /// The burst's last event time once it is due, resetting.
    pub fn take_due(&mut self, now: Instant) -> Option<Instant> {
        if now < self.deadline()? {
            return None;
        }
        self.first = None;
        self.last.take()
    }
}

/// The real watcher: `notify`'s recommended backend on a thread that
/// filters, debounces and forwards `WatchEvent`s.
pub struct FsWatcher {
    events: Receiver<WatchEvent>,
    pending: Arc<AtomicBool>,
}

impl FsWatcher {
    /// Watch the repo at `root`. All watches are set up before this
    /// returns, so an error (no backend, OS watch limit) means no watcher
    /// at all — the caller falls back to polling.
    pub fn start(root: &Path, git_dir: &Path, common_dir: &Path) -> Result<Self> {
        let canon = |p: &Path| {
            p.canonicalize()
                .with_context(|| format!("watch: {}", p.display()))
        };
        let filter = PathFilter::new(canon(root)?, canon(git_dir)?, canon(common_dir)?);
        let (raw_tx, raw_rx) = mpsc::channel();
        let mut watcher = RecommendedWatcher::new(raw_tx, notify::Config::default())?;
        // FSEvents and ReadDirectoryChangesW watch a whole tree natively, at
        // no per-directory cost: one recursive watch, the filter drops the
        // noise. Others (inotify, kqueue) spend a watch per directory, so
        // ignored trees (`target/`, `.git/objects`) are never added.
        let walk = !matches!(
            RecommendedWatcher::kind(),
            WatcherKind::Fsevent | WatcherKind::ReadDirectoryChangesWatcher
        );
        let mut watched = HashSet::new();
        {
            let mut paths = watcher.paths_mut();
            let root = &filter.root;
            if walk {
                add_tree(&filter, root, &mut watched, &mut |p| {
                    paths.add(p, RecursiveMode::NonRecursive)
                })?;
            } else {
                paths.add(root, RecursiveMode::Recursive)?;
            }
            // Under the root, the recursive watch already covers the git dir.
            if walk || !filter.git_dir.starts_with(root) {
                paths.add(&filter.git_dir, RecursiveMode::NonRecursive)?;
                if filter.common_dir != filter.git_dir {
                    paths.add(&filter.common_dir, RecursiveMode::NonRecursive)?;
                }
                let refs = filter.common_dir.join("refs");
                if refs.is_dir() {
                    paths.add(&refs, RecursiveMode::Recursive)?;
                }
            }
            paths.commit()?;
        }
        let (tx, events) = mpsc::channel();
        let pending = Arc::new(AtomicBool::new(false));
        let worker = Worker {
            watcher,
            filter,
            walk,
            watched,
            pending: Arc::clone(&pending),
        };
        thread::Builder::new()
            .name("auri-watch".to_string())
            .spawn(move || worker.run(raw_rx, tx))
            .context("watch: spawning thread")?;
        Ok(Self { events, pending })
    }
}

impl RepoWatcher for FsWatcher {
    fn poll(&mut self) -> Option<WatchEvent> {
        self.events.try_recv().ok()
    }

    fn pending(&self) -> bool {
        self.pending.load(Ordering::Relaxed)
    }
}

struct Worker {
    watcher: RecommendedWatcher,
    filter: PathFilter,
    /// Directories get their own watch (inotify/kqueue); `watched` holds
    /// them so new directories can be added.
    walk: bool,
    watched: HashSet<PathBuf>,
    pending: Arc<AtomicBool>,
}

impl Worker {
    fn run(mut self, raw: Receiver<notify::Result<Event>>, tx: Sender<WatchEvent>) {
        let mut debouncer = Debouncer::default();
        loop {
            let msg = match debouncer.timeout(Instant::now()) {
                Some(t) => match raw.recv_timeout(t) {
                    Ok(msg) => Some(msg),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                },
                None => match raw.recv() {
                    Ok(msg) => Some(msg),
                    Err(_) => return,
                },
            };
            match msg {
                Some(Ok(event)) => {
                    if let Err(e) = self.track_dirs(&event) {
                        let _ = tx.send(WatchEvent::Failed(e.to_string()));
                        return;
                    }
                    if relevant_kind(&event.kind)
                        && event.paths.iter().any(|p| self.filter.relevant(p))
                    {
                        debouncer.event(Instant::now());
                        self.pending.store(true, Ordering::Relaxed);
                    }
                }
                Some(Err(e)) if matches!(e.kind, notify::ErrorKind::MaxFilesWatch) => {
                    let _ = tx.send(WatchEvent::Failed(e.to_string()));
                    return;
                }
                Some(Err(_)) | None => {}
            }
            if let Some(last) = debouncer.take_due(Instant::now()) {
                if tx.send(WatchEvent::Changed { last }).is_err() {
                    return; // App is gone.
                }
                self.pending.store(false, Ordering::Relaxed);
            }
        }
    }

    /// Per-directory watching: watch directories created (or moved in)
    /// under the worktree, forget removed ones.
    fn track_dirs(&mut self, event: &Event) -> notify::Result<()> {
        if !self.walk {
            return Ok(());
        }
        match event.kind {
            EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(_)) => {
                for path in &event.paths {
                    let is_dir = path.symlink_metadata().is_ok_and(|m| m.is_dir());
                    if is_dir && !self.watched.contains(path) && self.filter.in_worktree(path) {
                        let watcher = &mut self.watcher;
                        add_tree(&self.filter, path, &mut self.watched, &mut |p| {
                            watcher.watch(p, RecursiveMode::NonRecursive)
                        })?;
                    }
                }
            }
            _ => {}
        }
        if matches!(
            event.kind,
            EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_))
        ) {
            for path in &event.paths {
                if !path.exists() {
                    self.watched.retain(|w| !w.starts_with(path));
                }
            }
        }
        Ok(())
    }
}

/// Add `dir` and every worktree directory under it (symlinks not
/// followed, ignored directories skipped) via `add`. A directory that
/// vanished meanwhile is skipped; other errors (the OS watch limit) stop.
fn add_tree(
    filter: &PathFilter,
    dir: &Path,
    watched: &mut HashSet<PathBuf>,
    add: &mut dyn FnMut(&Path) -> notify::Result<()>,
) -> notify::Result<()> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        match add(&dir) {
            Ok(()) => {}
            Err(e) if matches!(e.kind, notify::ErrorKind::PathNotFound) => continue,
            Err(e) => return Err(e),
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) && filter.in_worktree(&path) {
                stack.push(path);
            }
        }
        watched.insert(dir);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, AccessMode, CreateKind, DataChange};

    fn filter() -> PathFilter {
        PathFilter::new(
            PathBuf::from("/repo"),
            PathBuf::from("/repo/.git"),
            PathBuf::from("/repo/.git"),
        )
    }

    #[test]
    fn git_dir_only_head_index_and_refs() {
        let f = filter();
        for yes in [
            "/repo/.git/HEAD",
            "/repo/.git/index",
            "/repo/.git/packed-refs",
            "/repo/.git/refs/heads/main",
            "/repo/.git/refs/heads/feature/x",
        ] {
            assert!(f.relevant(Path::new(yes)), "{yes}");
        }
        for no in [
            "/repo/.git",
            "/repo/.git/index.lock",
            "/repo/.git/HEAD.lock",
            "/repo/.git/refs/heads/main.lock",
            "/repo/.git/objects/ab/cdef",
            "/repo/.git/logs/HEAD",
            "/repo/.git/FETCH_HEAD",
            "/repo/.git/ORIG_HEAD",
            "/repo/.git/HEAD/x",
        ] {
            assert!(!f.relevant(Path::new(no)), "{no}");
        }
    }

    #[test]
    fn worktree_skips_ignored_dirs_but_not_lockfiles() {
        let f = filter();
        for yes in [
            "/repo/src/main.rs",
            "/repo/Cargo.lock",
            "/repo/new",
            "/repo",
        ] {
            assert!(f.relevant(Path::new(yes)), "{yes}");
        }
        for no in [
            "/repo/target/debug/auri",
            "/repo/web/node_modules/x/index.js",
            "/repo/crates/a/target/x",
            "/repo/vendor/sub/.git/index",
            "/elsewhere/file",
        ] {
            assert!(!f.relevant(Path::new(no)), "{no}");
        }
    }

    #[test]
    fn linked_worktree_git_dirs() {
        let f = PathFilter::new(
            PathBuf::from("/wt"),
            PathBuf::from("/repo/.git/worktrees/wt"),
            PathBuf::from("/repo/.git"),
        );
        assert!(f.relevant(Path::new("/repo/.git/worktrees/wt/HEAD")));
        assert!(f.relevant(Path::new("/repo/.git/worktrees/wt/index")));
        assert!(f.relevant(Path::new("/repo/.git/refs/heads/main")));
        assert!(!f.relevant(Path::new("/repo/.git/objects/ab/cd")));
        assert!(f.relevant(Path::new("/wt/src/lib.rs")));
        assert!(!f.relevant(Path::new("/repo/src/lib.rs")));
    }

    #[test]
    fn reads_are_not_changes() {
        assert!(!relevant_kind(&EventKind::Access(AccessKind::Open(
            AccessMode::Any
        ))));
        assert!(!relevant_kind(&EventKind::Access(AccessKind::Close(
            AccessMode::Read
        ))));
        assert!(!relevant_kind(&EventKind::Modify(ModifyKind::Metadata(
            MetadataKind::AccessTime
        ))));
        assert!(relevant_kind(&EventKind::Modify(ModifyKind::Data(
            DataChange::Any
        ))));
        assert!(relevant_kind(&EventKind::Create(CreateKind::File)));
        assert!(relevant_kind(&EventKind::Remove(
            notify::event::RemoveKind::Any
        )));
        assert!(relevant_kind(&EventKind::Any));
    }

    #[test]
    fn debouncer_settles_after_quiet() {
        let t0 = Instant::now();
        let mut d = Debouncer::default();
        assert_eq!(d.timeout(t0), None);
        assert_eq!(d.take_due(t0), None);
        d.event(t0);
        let t1 = t0 + Duration::from_millis(100);
        d.event(t1);
        assert!(d.pending());
        assert_eq!(d.timeout(t1), Some(QUIET));
        assert_eq!(d.take_due(t1 + QUIET - Duration::from_millis(1)), None);
        // Reports the last event's time, then resets.
        assert_eq!(d.take_due(t1 + QUIET), Some(t1));
        assert!(!d.pending());
        assert_eq!(d.timeout(t1 + QUIET), None);
    }

    #[test]
    fn debouncer_caps_a_continuous_stream() {
        let t0 = Instant::now();
        let mut d = Debouncer::default();
        let mut now = t0;
        while now < t0 + MAX_WAIT {
            d.event(now);
            assert_eq!(d.take_due(now), None);
            now += Duration::from_millis(50);
        }
        // Events never paused for QUIET, yet the burst is due at MAX_WAIT.
        d.event(now);
        assert_eq!(d.timeout(now), Some(Duration::ZERO));
        assert_eq!(d.take_due(now), Some(now));
    }

    /// The real watcher on a temp dir: worktree writes are reported, git
    /// internals and reads are not.
    #[test]
    fn fs_watcher_reports_worktree_changes_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".git/objects/ab")).unwrap();
        std::fs::create_dir_all(root.join(".git/refs/heads")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("a.txt"), "a").unwrap();
        let git = root.join(".git");
        let mut w = FsWatcher::start(root, &git, &git).unwrap();
        // Let setup noise (if any) settle, then forget it.
        thread::sleep(Duration::from_millis(500));
        while w.poll().is_some() {}

        std::fs::write(root.join(".git/objects/ab/cd"), "x").unwrap();
        std::fs::write(root.join(".git/index.lock"), "x").unwrap();
        std::fs::write(root.join("target/out"), "x").unwrap();
        let _ = std::fs::read_to_string(root.join("a.txt")).unwrap();
        let _ = std::fs::read_dir(root).unwrap().count();
        thread::sleep(Duration::from_millis(600));
        assert_eq!(w.poll(), None);

        let before = Instant::now();
        std::fs::write(root.join("a.txt"), "changed").unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match w.poll() {
                Some(WatchEvent::Changed { last }) => {
                    assert!(last >= before);
                    break;
                }
                Some(other) => panic!("unexpected {other:?}"),
                None if Instant::now() > deadline => panic!("no change reported"),
                None => thread::sleep(Duration::from_millis(20)),
            }
        }

        // A directory created later is watched too.
        std::fs::create_dir(root.join("sub")).unwrap();
        thread::sleep(Duration::from_millis(400));
        while w.poll().is_some() {}
        std::fs::write(root.join("sub/new.txt"), "n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while w.poll().is_none() {
            assert!(Instant::now() < deadline, "no change in new dir");
            thread::sleep(Duration::from_millis(20));
        }
    }
}
