# auri (`auri-tui`)

A terminal IDE written in Rust with [ratatui]. Currently ships a VS Code–style
git panel (a changes list, a side-by-side diff viewer, a commit message input
and history) and a file explorer with a file viewer that doubles as a simple
editor (explicit save, never automatic).

## Architecture

Layered and decoupled — the golden rule is **components never call git**
(nor do any other I/O, filesystem included):

```
main.rs        terminal init/restore (+ panic hook), CLI arg = repo path
               (or `update`)
update.rs      `auri update` (before the TUI starts): latest version from
               the `releases/latest` redirect, then that release's `dist`
               shell installer into this binary's dir — pure helpers
               (`parse_tag`, `is_newer`, `install_target`) unit-tested
app.rs         App: owns components, focus, last-frame rects, the action queue.
               The ONLY place git (via `GitJobs`) and FsBackend are used;
               results are broadcast back as actions.
git_worker.rs  git off the UI thread: `GitJob` -> `run_job` -> `GitResult`s;
               `GitJobs` runs jobs in order on a worker thread (or inline in
               tests) and drops a `Diff` job superseded by a newer one
event.rs       crossterm polling (100 ms idle, 16 ms while git/AI work or a
               watcher burst is in flight) + a Tick that drives Refresh
               (30 s with the fs watcher, 2 s without)
watch.rs       refresh on change: `RepoWatcher` trait (App polls it);
               `FsWatcher` = notify on a thread; pure `PathFilter`,
               `relevant_kind`, `Debouncer`
action.rs      Action enum — the single message type everything speaks
component.rs   Component trait — implement it to add a panel
keymap.rs      global keys as data (`GLOBAL`: key, `Command`, `Scope`, help,
               status chunk) — `lookup`, status-bar hints, help screen
git/           model types + GitBackend trait; `cli` shells out to git,
               `parse` has pure, unit-tested parsing functions
ai/            AI commit-message generation: pure prompt/cleanup helpers +
               `AiRunner` trait; `ProcessRunner` spawns the CLI (codex or
               opencode) on background threads — the app's one async side
               effect, polled by App once per loop
fs/            workspace reads: model types + FsBackend trait; `local` is
               the real impl (paths validated, root-relative; atomic
               `write_file`), `tree` the pure expand/collapse model the
               explorer flattens
buffer.rs      the editor's text model: lines, cursor/selection, undo/redo,
               `modified()` (version != saved) — pure, unit-tested
clipboard.rs   `Clipboard` trait; `Osc52` copies via the terminal
components/    commit_input.rs, changes.rs, history.rs, diff_view.rs,
               file_tree.rs, file_view.rs, hitbox.rs, confirm_dialog.rs
               (modal overlay example)
highlight.rs   syntax highlighting (syntect + two-face = bat's grammars,
               embedded; the token colors are the astro theme built in code):
               pure, incremental `Highlighter`
theme.rs       the "astro" palette shared with the landing page
               (`site/index.html`): warm black bg, ivory text, gold as the
               only accent. EVERY color the UI draws comes from here —
               components never spell a `Color::` literal (tests excepted).
               `app.rs` paints `theme::base()` over the whole frame first.
text.rs        display width in terminal columns (CJK/emoji = 2): `width`,
               `slice`, `truncate`, `ellipsize` — use these, never
               `chars().count()`, for anything drawn
icons.rs       explorer file-type icons: pure name -> (glyph, color);
               `text` badges (any font) or `nerd` glyphs (Nerd Font)
layout.rs      sidebar geometry: view (Explorer/Source Control), width/
               clamps/divider+split drag — pure, unit-tested
prefs/         persisted user preferences: `Preferences` (TOML document),
               `PrefsStore` trait, `FileStore` (atomic save) + `MemoryStore`
```

Data flow: a component returns an `Action` from `handle_key`/`handle_mouse`/
`update` → `App` enqueues it → side-effecting actions (`Refresh`,
`ToggleStage`, `Commit`, `SelectFile`, `Discard`, `UnstageAll`,
`LoadHistory`, `LoadCommitFiles`, `SelectCommitFile`, `LoadDirs`,
`OpenFile`, `WriteFile`, `CopyToClipboard`,
`LoadLastCommitMessage`) are executed by `App`
— git ones as `GitJob`s, fs ones directly via `Box<dyn FsBackend>` →
results (`StatusLoaded`, `DiffLoaded`, `HistoryLoaded`,
`CommitFilesLoaded`, `DirLoaded`, `FileLoaded`, `FileReloaded`,
`FileSaved`, `Error`)
are broadcast to every component's `update`.

Git runs on a worker thread (`main.rs`: `with_git_worker`, a second
`CliGit`); `App::dispatch` takes finished jobs (`GitJobs::poll`) between
actions and `accept` turns them into actions — so results never arrive
mid-dispatch. Tests use the default inline mode: jobs run on submit, so a
`dispatch()` still plays out the whole cascade. Rules:
- One `Refresh` job at a time (`refresh_in_flight`); more requests meanwhile
  collapse into one follow-up (`refresh_again`).
- A diff result is kept only if `App::diff` still equals its `DiffSource`
  (a newer selection made it stale), and a reload only if the doc changed
  (`last_diff`). Docs are `Arc<DiffDoc>` — shared, never copied. Until one
  arrives `DiffView` shows "Loading…".
- Synchronous git stays on the UI thread only where it must: a commit that
  may prompt (terminal handed over) and the AI prompt (staged patch). Both
  call `finish_git_jobs` first (`wait_idle`), so a stage queued just
  before is in. "Nothing staged" is asked of git at commit time
  (`git_worker::check_staged`), not read from the last status.

Refresh is driven by the filesystem watcher (`watch.rs`; `main.rs`
starts `FsWatcher` with `git::cli::git_dirs`, `App::with_watcher`):
worktree changes (minus `.git`, `target`, `node_modules` at any depth)
and, in the git dir, only `HEAD`, `index`, `packed-refs`, `refs/` (never
`*.lock`) settle into one `WatchEvent::Changed { last }` after `QUIET`
(150 ms) or `MAX_WAIT` (1 s). Read events (`Access`, atime) are dropped —
inotify reports opens, and `git status`/the explorer read on every
refresh, which would loop. FSEvents/Windows watch the root recursively;
inotify/kqueue walk it, one watch per non-ignored dir (new dirs added as
they appear). `App::poll_watcher` refreshes once input is quiet
(`should_refresh`), skipping a change that is already covered: seen
before the last refresh was submitted (`refresh_started`), or within
`SELF_WRITE_GRACE` after one of our own git writes (stage, discard,
commit — that job refreshes itself; FSEvents reports its writes a few ms
late). With a watcher the Tick is 30 s (`WATCH_TICK`, safety net); if it
fails to start, or reports `Failed` later (inotify watch limit), the Tick
is back to 2 s (`POLL_TICK`) with a status-bar warning. Fake it in tests
with `FakeWatcher` (`app.rs`).

Status sections: `Conflicted` (unmerged, code `!`, one entry, listed
first as "Merge Changes"; staging marks it resolved, no discard, its diff
is worktree vs HEAD), `Staged`, `Unstaged`, `Untracked` (code `U`).
`stage_all` never stages conflicted paths.

`git commit` runs with the terminal handed over (`TerminalHandoff`:
`release` leaves raw mode/alt screen, `reclaim` re-enters and forces a full
redraw) when `commit_may_prompt()` — `commit.gpgsign` or an executable
commit hook — so pinentry/interactive hooks get a usable tty. The full
redraw is `redraw_from_scratch` (`Terminal::resize` to the current size),
never `Terminal::clear`: that one queries the cursor position, and a
terminal that doesn't answer in time would end the app.

Commit input: the message is multi-line (Ctrl-J, or Alt/Shift-Enter when
reported, inserts a newline; Enter commits) and the box grows to
`MAX_LINES` text rows, then scrolls — `Component::preferred_height` feeds
`Sidebar::commit_height`, which `App` sets before `layout::compute`.
`Action::Commit { message, amend }`: amend (`git commit --amend`, Ctrl-A
or the `amend` toggle → `ToggleAmend`) needs nothing staged; a plain
commit errors "Nothing staged". Switching amend on with an empty box emits
`LoadLastCommitMessage` → `LastCommitMessageLoaded` prefills it; switching
off drops that prefill unless edited. `CommitDone` clears the box and
leaves amend mode.

Bracketed paste is on (`main.rs`, and around every terminal handoff):
a paste arrives as one `AppEvent::Paste`, which `App` sends to the focused
panel's `Component::handle_paste` (never to an overlay). `CommitInput`
inserts it with its newlines, so a pasted line break can't act as Enter.

Status-bar messages (`App::notify`) clear on the next key press or after
`MESSAGE_TTL` (errors: `ERROR_TTL`).

The diff pane's contents are described by `git::DiffSource`:
`Working(FileChange)` is reloaded on status ticks and cleared when the file
leaves the status; `Commit { hash, file }` is immutable — never reloaded or
cleared. `Changes`/`History` each carry an `active` flag so only the list
that owns the diff draws a strong selection; a status tick while a commit
diff is open never steals it back. `History` refetches only when `head()`
moves (`GitResult::Head` from each refresh).

### Main pane and explorer

The main pane follows the sidebar view (`main_panel()`): Source Control
shows `DiffView` (`App::diff: Option<DiffSource>`, set by `SelectFile`/
`SelectCommitFile`), Explorer shows `FileView` (`App::file: Option<OpenFile>`,
set by a successful `OpenFile`). Both slots are kept, so switching views
flips between the last diff and the last file; only the visible panel gets
a rect and focus (`panel_visible()`), and focus on the main pane stays on
the main pane across a switch (`set_view`). `FileTree` tracks the `open`
path (re-clicking it is a no-op). Listings are lazy — expanding a dir
emits `LoadDirs([dir])`, and every `Refresh` makes the tree emit `LoadDirs`
for all visible dirs (root included, which is how the first listing
arrives). On `Refresh` `App` re-reads the open file only when
`fs.stamp()` changed; a deleted file keeps its last contents. Files over
`fs::MAX_FILE_BYTES` are truncated; binaries (NUL in the first 8 KB) are
not shown; tabs are expanded and control chars replaced at load. Tree rows
get git decorations from `StatusLoaded` (`file_tree::decorations`) and a
2-column colored icon from `icons.rs` (the name takes the git color, the
icon keeps its type color); `[explorer] icons = "text" | "nerd"` picks the
glyph set (`text` default — `nerd` shows boxes without a Nerd Font). Tests
fake the filesystem with `FakeFs` in `app.rs`.

### Editing and saving — `buffer.rs`, `components/file_view.rs`

`FileView` is modal: view mode (`j`/`k`…) and edit mode (`i`/Enter in,
Esc out), where keys are text. It always renders from a `Buffer`; edits
stay in it until the user saves — **nothing is ever written
automatically**. Editing needs `FileDoc::source` (the exact contents; `None`
for binary, truncated or non-UTF-8 files, which stay read-only so a save
can't corrupt them). `Buffer::text()` round-trips the file byte for byte
(LF/CRLF, missing final newline, tabs, stray `\r`).

`App` learns the editor's state through `Component::edit_state()`
(`inserting`, `modified`, `selection`) — it feeds `keymap::Ctx` (`editing`:
only Ctrl globals are live, Tab/Esc/letters go to the editor; `modified`:
`Ctrl-S` → `Command::Save` is bound; `selection`: Ctrl-C copies instead of
quitting) and the guards below.

`i` in Changes and the diff emits `EditFile { path, line }`: `App`
(`edit_file`) opens the file unless it is already open (its edits kept),
switches to the Explorer view, focuses `FileView` and broadcasts
`StartEditing { path, line }` — the viewer enters edit mode with the
cursor on `line` (the diff's first changed new-side line in view, else its
top line; none from Changes or a commit diff).

Save flow: `Ctrl-S` → `SaveFile { then }` (broadcast) → `FileView` answers
`WriteFile { path, contents, version, force, then }` → `App::write_file`
calls `FsBackend::write_file(path, contents, expected)` with the stamp
from open/last save. `Written(stamp)`: the stamp is updated (so the
refresh doesn't reload what we wrote), `SELF_WRITE_GRACE` is set, then
`FileSaved { path, version }` (the buffer marks that version saved),
`Refresh`, and `then`. `Conflict` (changed or deleted on disk) asks
Overwrite (`WriteFile { force: true }`) / Reload (`DiscardEdits` +
`Refresh`) / Cancel.

Unsaved edits are never lost silently: `App::dispatch` checks every action
with `unsaved_prompt` *before* broadcasting it — `Quit`, `OpenFile` and
`EditFile` of another file become a Save / Discard / Cancel
`Confirm` (the `alt` button) whose buttons run `SaveFile { then }` or
`DiscardEdits { then }` with the original action as `then`. Ctrl-C in that
prompt (for a quit) quits anyway (`quit_prompted`). A refresh that finds
the file changed on disk while modified does not reload it — one warning
per on-disk stamp (`OpenFile::warned`); saving then hits the conflict
prompt. Overlays render in reverse so the input owner is on top.

Syntax highlighting (`highlight.rs`) is pure, so components own it:
`FileView` keeps one `Highlighter` per doc, `DiffView` one per side (with
full-context diffs each side's cells in order *are* the old/new file).
The grammar is picked by file name, extension, then first line. It is
stateful, so `advance()` highlights from the top only as far as the view
has scrolled and caches it; a reload restarts it. The parser state is
checkpointed every `CHECKPOINT` lines so an edit (`invalidate_from`)
re-highlights from the checkpoint before it, not from the top. Past `MAX_LINES` or
after a line over `MAX_LINE_BYTES` (minified code) lines render plain.
Only fg + bold/italic are applied, so diff backgrounds show through.
`[profile.dev.package."*"] opt-level = 3` keeps it fast in debug builds.

AI commit messages: Ctrl-G (any panel, when no overlay captures input) or
the ✦ button in the commit box runs `GenerateCommitMessage` — `App` gathers
the staged patch (`git.staged_patch()` + `log(0, 10)` subjects), builds the
`ai::build_prompt` stdin and starts `ai::command()` on `Box<dyn AiRunner>`
(fake it via `with_ai_runner` in tests). `poll_ai` maps each `AiOutcome` to
`CommitMessageGenerated`/`CommitMessageFailed`; the cleaned one-line result
only fills the input — it never commits. Esc while running sends
`CancelCommitMessage` (kills the child). Ctrl-T toggles
`prefs.ai.provider` codex<->opencode (persisted via `save_prefs`). `[ai]`
prefs: `provider`, `codex_model`, `codex_reasoning_effort`,
`opencode_model`, `timeout_secs`, `max_diff_chars`.

### Clickable buttons — `components/hitbox.rs`

`Hitboxes` is a `Vec<(Rect, Action)>` a component clears and fills during
`render` (it knows exact positions); on `Down(Left)` it checks
`hitboxes.hit(col, row)` **before** any other click handling. Buttons are
3-column `" {glyph} "` spans (`button_span`). `ConfirmDialog` in
`overlays` shows the pattern for modal UI: `captures_input() == true` makes
`App` route *all* key/mouse events to the overlay alone (Ctrl-C still quits).
`mouse_leave()` is called when the cursor leaves a panel — clear hover state
there.

### Sidebar layout — `layout.rs`

The left column is a resizable, collapsible sidebar with two views
(`SidebarView`, persisted as `layout.sidebar_view`): Explorer (the file
tree) or Source Control (commit input + changes + commit history).
`Sidebar` owns width/visibility/view/drag state plus the changes/history
split; `compute()` returns the panel rects each frame.
The sidebar's top row is a clickable Explorer | Source Control tab bar
(followed by a blank spacing row)
(`components/view_tabs.rs`, drawn by `App` with its own `tab_hits`
hitboxes → `Action::SetSidebarView`; labels shorten to Files | Git when
narrow). `Sidebar::on_mouse` consumes divider presses/drags (vertical =
sidebar width, horizontal split row = changes/history heights) before they
reach components — App calls it before the tab bar and panel routing.
`b` toggles the sidebar,
`[`/`]` resize it; `e` shows the Explorer view, `c`/`1`/`3` the Source
Control view while focusing CommitInput/Changes/History; `2` focuses the
main pane (diff or file).

### Keys — `keymap.rs`

Global keys are a table, not code: `App::on_key` asks
`keymap::lookup(key, ctx)` first (`Ctx`: overlay open, typing in the commit
box, AI running) and runs the `Command` (`App::run_command`); otherwise the
key goes to the open overlay, else the focused panel. `Scope` decides where
a binding is live: `Anywhere` (Ctrl-C, even in a dialog), `Global` (also
while typing), `Navigation` (not while typing — letters are text there),
`Typing`. The status bar's global hints (`status_hints`, from each
binding's `status` chunk) and the `?` help overlay (`components/help.rs`:
`GLOBAL`, `MOUSE`, then every panel's `hints()` split by `hint_rows`) are
generated from the same data — to add a global key, add a `Binding` (and a
`Command` arm); a panel key goes in its `handle_key` *and* its `hints()`.
Panel hints must not repeat global keys.

### Preferences — `prefs/mod.rs`

`Preferences` persists to `$AURI_CONFIG_DIR/preferences.toml` when
`AURI_CONFIG_DIR` is set (test/override hook), else
`$XDG_CONFIG_HOME/auri/preferences.toml`, else
`~/.config/auri/preferences.toml` (that path on macOS too — no `dirs`
crate). Saves are atomic (tmp file + rename). A missing file loads defaults;
an *unparseable* file disables saving for the session and shows an error —
it is never overwritten.

**App is the only writer.** To add a preference: add a field or a new
`#[serde(default)]` section to `Preferences`; read it in your component's
`update(Action::PreferencesChanged)`; to change it, add an `Action` handled
in `App` that mutates `self.prefs` then calls the save path (`sync_prefs`).
Unknown keys in the file are ignored, so forward/backward compatibility is
free. Tests must never touch the real config dir — use `MemoryStore` or a
`FileStore` pointed at a tempdir.

### Adding a new panel

1. Add a `PanelId` variant in `action.rs`.
2. Implement `Component` for the panel in `src/components/`.
3. Register it in `App::new` and give it a rect in `App::render`.
4. If it needs new data or triggers new side effects, add `Action` variants
   and handle them in `App::execute`.

## Commands

```sh
cargo run -- [repo]                              # run (defaults to cwd); binary is `auri`
cargo test                                       # unit + integration tests
cargo clippy --all-targets -- -D warnings        # lint
cargo fmt --check                                # format check (cargo fmt to fix)
```

[ratatui]: https://ratatui.rs
