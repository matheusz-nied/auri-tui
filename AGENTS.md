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
components/    commit_input.rs, changes.rs, diff_view.rs, hitbox.rs,
               confirm_dialog.rs (modal overlay example)
layout.rs      sidebar geometry: width/clamps/divider drag — pure, unit-tested
```

Data flow: a component returns an `Action` from `handle_key`/`handle_mouse`/
`update` → `App` enqueues it → side-effecting actions (`Refresh`,
`ToggleStage`, `Commit`, `SelectFile`, `Discard`, `UnstageAll`) are executed
by `App` via `Box<dyn GitBackend>` → results (`StatusLoaded`, `DiffLoaded`,
`Error`) are broadcast to every component's `update`.

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

The left column (commit input + changes) is a resizable, collapsible sidebar.
`Sidebar` owns width/visibility/drag state; `compute()` returns the panel
rects each frame. `Sidebar::on_mouse` consumes divider presses/drags before
they reach components — App calls it before panel routing. `b` toggles the
sidebar, `[`/`]` resize it; `c`/`1` re-show it while focusing their panel.

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
