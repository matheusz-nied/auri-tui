# terminal-ide (`tide`)

A terminal IDE written in Rust with [ratatui]. Currently ships a VS Code–style
git panel (a changes list, a side-by-side diff viewer, a commit message input
and history) and a file explorer with a read-only file viewer.

## Architecture

Layered and decoupled — the golden rule is **components never call git**
(nor do any other I/O, filesystem included):

```
main.rs        terminal init/restore (+ panic hook), CLI arg = repo path
app.rs         App: owns components, focus, last-frame rects, the action queue.
               The ONLY place GitBackend/FsBackend are called; results are
               broadcast back as actions.
event.rs       crossterm polling (~250 ms) + a ~2 s Tick that drives Refresh
action.rs      Action enum — the single message type everything speaks
component.rs   Component trait — implement it to add a panel
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
highlight.rs   syntax highlighting (syntect + two-face = bat's grammars,
               embedded, OneHalfDark): pure, incremental `Highlighter`
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
`OpenFile`) are executed by `App` via `Box<dyn GitBackend>` /
`Box<dyn FsBackend>` → results (`StatusLoaded`, `DiffLoaded`,
`HistoryLoaded`, `CommitFilesLoaded`, `DirLoaded`, `FileLoaded`,
`FileReloaded`, `Error`) are broadcast to every component's `update`.

The diff pane's contents are described by `git::DiffSource`:
`Working(FileChange)` is reloaded on status ticks and cleared when the file
leaves the status; `Commit { hash, file }` is immutable — never reloaded or
cleared. `Changes`/`History` each carry an `active` flag so only the list
that owns the diff draws a strong selection; a status tick while a commit
diff is open never steals it back. `History` refetches only when `head()`
moves.

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
(`components/view_tabs.rs`, drawn by `App` with its own `tab_hits`
hitboxes → `Action::SetSidebarView`; labels shorten to Files | Git when
narrow). `Sidebar::on_mouse` consumes divider presses/drags (vertical =
sidebar width, horizontal split row = changes/history heights) before they
reach components — App calls it before the tab bar and panel routing.
`b` toggles the sidebar,
`[`/`]` resize it; `e` shows the Explorer view, `c`/`1`/`3` the Source
Control view while focusing CommitInput/Changes/History; `2` focuses the
main pane (diff or file).

### Preferences — `prefs/mod.rs`

`Preferences` persists to `$TIDE_CONFIG_DIR/preferences.toml` when
`TIDE_CONFIG_DIR` is set (test/override hook), else
`$XDG_CONFIG_HOME/tide/preferences.toml`, else
`~/.config/tide/preferences.toml` (that path on macOS too — no `dirs`
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
cargo run -- [repo]                              # run (defaults to cwd); binary is `tide`
cargo test                                       # unit + integration tests
cargo clippy --all-targets -- -D warnings        # lint
cargo fmt --check                                # format check (cargo fmt to fix)
```

[ratatui]: https://ratatui.rs
