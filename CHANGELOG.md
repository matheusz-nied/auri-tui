# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/). While on `0.x`, minor releases
may include breaking changes.

## [Unreleased]

## [0.3.0] - 2026-10-05

### Added

- `auri update`: installs the latest GitHub release in place of the
  running binary.

### Fixed

- A stray word in the README's install instructions.

## [0.2.0] - 2026-10-05

Edit files without leaving auri.

### Added

- Built-in editor in the file viewer: `i` (or `Enter`) starts editing,
  `Esc` stops. `i` in Changes or in a diff opens that file in the editor —
  from a diff, at the first change in view.
- Nothing is saved automatically: `Ctrl-S` saves, and the title shows `●`
  while there are unsaved changes. Saves are atomic and keep the file's
  line endings, final newline, permissions and symlinks.
- Unsaved changes are never lost silently: quitting or opening another
  file asks Save / Discard / Cancel (`Ctrl-C` twice quits anyway). If the
  file changes on disk meanwhile, your edits are kept and saving asks
  whether to overwrite it or reload it.
- Selection (Shift+arrows, mouse drag), undo/redo (`Ctrl-Z`/`Ctrl-Y`),
  cut/copy/paste (`Ctrl-X`/`Ctrl-C`/`Ctrl-V`, copies also reach the
  system clipboard through OSC 52), word moves, `Tab`/`Shift-Tab` indent
  and auto-indent on `Enter`.

### Removed

- `o` (open in `$VISUAL`/`$EDITOR`): editing now happens inside auri.

## [0.1.0] - 2026-10-03

First public release.

### Added

- File explorer with lazy directory listing, git status decorations and
  colored file-type icons (`text` or Nerd Font).
- Read-only file viewer with syntax highlighting.
- Source Control view: staged / unstaged changes, stage, unstage, stage all,
  discard (with confirmation).
- Side-by-side diff viewer with syntax highlighting and jump-to-change.
- Commit history panel with per-commit file diffs.
- Commit message input; empty commits are disabled. Multi-line messages
  (`Ctrl-J`) and amending the last commit (`Ctrl-A` or the `amend` toggle).
- `o` opens the selected file in `$VISUAL`/`$EDITOR`, at the line in view.
- Optional AI commit-message drafts via Codex CLI or opencode (`Ctrl-G`).
- Resizable, collapsible sidebar; layout and settings persisted in
  `~/.config/auri/preferences.toml`.
- Mouse support: clickable buttons, tabs and draggable dividers.
- Git runs on a background thread: the UI stays responsive while large
  diffs load or a slow repo refreshes.
- Changes show up as soon as they happen: a filesystem watcher triggers
  the refresh, so an idle auri runs no git at all. Without a watcher
  (e.g. the inotify limit is reached) it falls back to polling every 2 s
  and says so in the status bar.
- "Astro" color theme — warm black, ivory text and a gold accent, shared
  with the landing page.
- Bracketed paste: pasted text keeps its line breaks and never commits.
- `--help` and `--version`; `?` inside auri lists every key.

[Unreleased]: https://github.com/matheusz-nied/auri-tui/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/matheusz-nied/auri-tui/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/matheusz-nied/auri-tui/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/matheusz-nied/auri-tui/releases/tag/v0.1.0
