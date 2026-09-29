# terminal-ide (`tide`)

A terminal IDE written in Rust with [ratatui]. Currently ships a VS Code–style
git panel: a changes list (staged/unstaged), a side-by-side diff viewer, and a
commit message input.

## Architecture

Layered and decoupled — the golden rule is **components never call git**:

```
main.rs        terminal init/restore (+ panic hook), CLI arg = repo path
app.rs         App: owns components, focus, last-frame rects, the action queue.
               The ONLY place GitBackend is called; results are broadcast back
               as actions.
event.rs       crossterm polling (~250 ms) + a ~2 s Tick that drives Refresh
action.rs      Action enum — the single message type everything speaks
component.rs   Component trait — implement it to add a panel
git/           model types + GitBackend trait; `cli` shells out to git,
               `parse` has pure, unit-tested parsing functions
ai/            AI commit-message generation: pure prompt/cleanup helpers +
               `AiRunner` trait; `ProcessRunner` spawns the CLI (codex or
               opencode) on background threads — the app's one async side
               effect, polled by App once per loop
components/    commit_input.rs, changes.rs, history.rs, diff_view.rs,
               hitbox.rs, confirm_dialog.rs (modal overlay example)
layout.rs      sidebar geometry: width/clamps/divider+split drag — pure,
               unit-tested
prefs/         persisted user preferences: `Preferences` (TOML document),
               `PrefsStore` trait, `FileStore` (atomic save) + `MemoryStore`
```

Data flow: a component returns an `Action` from `handle_key`/`handle_mouse`/
`update` → `App` enqueues it → side-effecting actions (`Refresh`,
`ToggleStage`, `Commit`, `SelectFile`, `Discard`, `UnstageAll`,
`LoadHistory`, `LoadCommitFiles`, `SelectCommitFile`) are executed
by `App` via `Box<dyn GitBackend>` → results (`StatusLoaded`, `DiffLoaded`,
`HistoryLoaded`, `CommitFilesLoaded`, `Error`) are broadcast to every
component's `update`.

The diff pane's contents are described by `git::DiffSource`:
`Working(FileChange)` is reloaded on status ticks and cleared when the file
leaves the status; `Commit { hash, file }` is immutable — never reloaded or
cleared. `Changes`/`History` each carry an `active` flag so only the list
that owns the diff draws a strong selection; a status tick while a commit
diff is open never steals it back. `History` refetches only when `head()`
moves.

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

The left column (commit input + changes + commit history) is a resizable,
collapsible sidebar. `Sidebar` owns width/visibility/drag state plus the
changes/history split; `compute()` returns the panel rects each frame.
`Sidebar::on_mouse` consumes divider presses/drags (vertical = sidebar width,
horizontal split row = changes/history heights) before they reach
components — App calls it before panel routing. `b` toggles the sidebar,
`[`/`]` resize it; `c`/`1`/`3` re-show it while focusing CommitInput/
Changes/History; `2` focuses the diff.

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
