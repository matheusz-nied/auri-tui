# auri (`auri-tui`)

A terminal IDE written in Rust with [ratatui]. Currently ships a VS Code–style
git panel (a changes list, a side-by-side diff viewer, a commit message input
and history) and a file explorer with a read-only file viewer.

## Architecture

Layered and decoupled — the golden rule is **components never call git**
(nor do any other I/O, filesystem included):

```
main.rs        terminal init/restore (+ panic hook), CLI arg = repo path
app.rs         App: owns components, focus, last-frame rects, the action queue.
               The ONLY place git (via `GitJobs`) and FsBackend are used;
               results are broadcast back as actions.
git_worker.rs  git off the UI thread: `GitJob` -> `run_job` -> `GitResult`s;
               `GitJobs` runs jobs in order on a worker thread (or inline in
               tests) and drops a `Diff` job superseded by a newer one
event.rs       crossterm polling (~250 ms idle, 16 ms while git/AI work is in
               flight) + a ~2 s Tick that drives Refresh
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
               the real impl (paths validated, root-relative), `tree` the
               pure expand/collapse model the explorer flattens
components/    commit_input.rs, changes.rs, history.rs, diff_view.rs,
               file_tree.rs, file_view.rs, hitbox.rs, confirm_dialog.rs
               (modal overlay example)
editor.rs      `o` = edit in `$VISUAL`/`$EDITOR` (else `vi`): pure
               `command` (editor + per-editor line-jump args) +
               `EditorLauncher` trait; `ShellEditor` runs it via `sh -c`
highlight.rs   syntax highlighting (syntect + two-face = bat's grammars,
               embedded, OneHalfDark): pure, incremental `Highlighter`
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
`OpenFile`, `OpenInEditor`, `LoadLastCommitMessage`) are executed by `App`
— git ones as `GitJob`s, fs ones directly via `Box<dyn FsBackend>` →
results (`StatusLoaded`, `DiffLoaded`, `HistoryLoaded`,
`CommitFilesLoaded`, `DirLoaded`, `FileLoaded`, `FileReloaded`, `Error`)
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

`OpenInEditor { path, line }` (`o` in Changes, the tree, the file viewer
and the diff) always hands the terminal over: `App` checks the file still
exists (`fs.stamp`), runs `Box<dyn EditorLauncher>` (fake it via
`with_editor`; the default refuses) on the absolute path, reclaims, then
`Refresh`es so the edit shows up. Lines: the viewer's top line; the diff's
first changed new-side line in view (else its top line); commit diffs and
lists open without one. `+N` is passed only to editors whose syntax is
known (vi family, nano, emacs…; `path:N` for hx/subl/zed; `--goto` for
VS Code forks).

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

Syntax highlighting (`highlight.rs`) is pure, so components own it:
`FileView` keeps one `Highlighter` per doc, `DiffView` one per side (with
full-context diffs each side's cells in order *are* the old/new file).
The grammar is picked by file name, extension, then first line. It is
stateful, so `advance()` highlights from the top only as far as the view
has scrolled and caches it; a reload restarts it. Past `MAX_LINES` or
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
