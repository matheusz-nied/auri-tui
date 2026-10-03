# auri

**Browse files and git diffs side by side, right in your terminal.**

<!-- Regenerate with `vhs demo/demo.tape` -->
![auri demo](demo/demo.gif)

auri is a small terminal UI for looking at your repository: a file explorer
with syntax-highlighted previews, and a VS Code–style source control view
with side-by-side diffs. Mouse and keyboard both work.

- **Explorer** — lazy file tree with git status decorations and a read-only,
  syntax-highlighted file viewer.
- **Source Control** — staged / unstaged changes, side-by-side diffs
  (`n`/`N` jump between changes), stage, unstage and discard.
- **History** — browse past commits and the diff of every file they touched.
- **Commit** — write a message and commit; optionally let an AI CLI draft
  the message from your staged changes.

## Install

```sh
# Shell installer (macOS / Linux)
curl -LsSf https://github.com/matheusz-nied/auri-tui/releases/latest/download/auri-tui-installer.sh | sh

# From source (Rust toolchain)
cargo install auri-tui
```

Prebuilt binaries are attached to every
[GitHub release](https://github.com/matheusz-nied/auri-tui/releases).

**Requirements:** `git` on your `PATH`. A terminal with true color gives
the best highlighting.

## Usage

```sh
auri            # current repository
auri path/to/repo
```

The left sidebar has two views — **Explorer** and **Source Control** —
switch with the tab bar, `e` or `c`. The right side shows the open file or
diff. Drag the dividers to resize.

### Keys

| Key | Action |
|---|---|
| `e` | Explorer view |
| `c` | Source Control view (focus the commit box) |
| `1` / `3` | Focus changes / history |
| `2` | Focus the main pane (file or diff) |
| `Tab` / `Shift-Tab` | Next / previous panel |
| `b` | Toggle sidebar |
| `[` / `]` | Shrink / grow sidebar |
| `r` | Refresh |
| `q` / `Ctrl-C` | Quit |

**Lists** — `j`/`k` or arrows to move, `g`/`G` top/bottom, `Enter` to open.
In the tree, `l`/`h` expand/collapse.

**Changes** — `Space`/`s` stage or unstage, `a` stage all, `d` discard.

**Edit** — `o` opens the selected file (changes, tree, file viewer or diff)
in `$VISUAL`/`$EDITOR` (default `vi`), at the line in view when the editor
supports it; auri resumes when the editor exits.

**Diff / file viewer** — `j`/`k` scroll, `PgUp`/`PgDn`, `h`/`l` scroll
sideways, `n`/`N` next/previous change (diff).

**Commit box** — type, `Enter` to commit, `Ctrl-J` for a new line (the box
grows; pasted text keeps its line breaks and never commits), `Esc` to
leave. `Ctrl-A` or the `amend` toggle switches to amending
the last commit: an empty box gets its message, and nothing needs to be
staged.

### AI commit messages (optional)

`Ctrl-G` (or the ✦ button in the commit box) asks an AI CLI to write a
one-line message from the staged diff. It only fills the input — it never
commits. `Esc` cancels, `Ctrl-T` switches provider.

Supported providers, which must be installed and logged in:
[Codex CLI](https://github.com/openai/codex) (`codex`) and
[opencode](https://opencode.ai) (`opencode`).

## Configuration

Preferences are saved automatically to
`~/.config/auri/preferences.toml` (or `$XDG_CONFIG_HOME/auri/`, or
`$AURI_CONFIG_DIR` if set). Everything is optional:

```toml
[layout]
sidebar_view = "explorer"   # or "source_control"
sidebar_visible = true
# sidebar_width = 40        # columns; default 35% of the terminal

[explorer]
icons = "text"              # or "nerd" (needs a Nerd Font)

[ai]
provider = "codex"          # or "opencode"
codex_model = "gpt-6-luna"
codex_reasoning_effort = "low"
opencode_model = "deepseek/deepseek-flash"
timeout_secs = 60
max_diff_chars = 20000
```

## Why "auri"?

Latin for *of gold*. Albireo, the famous double star in Cygnus, shows a
gold and a blue star side by side — two views of the same spot in the sky,
like a diff.

## Development

```sh
cargo run -- [repo]
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

See [AGENTS.md](AGENTS.md) for the architecture and
[CHANGELOG.md](CHANGELOG.md) for release notes.

## License

[MIT](LICENSE)
