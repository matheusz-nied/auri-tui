# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/). While on `0.x`, minor releases
may include breaking changes.

## [Unreleased]

## [0.1.0] - Unreleased

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
- Bracketed paste: pasted text keeps its line breaks and never commits.
- `--help` and `--version`; `?` inside auri lists every key.

[Unreleased]: https://github.com/matheusz-nied/auri-tui/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/matheusz-nied/auri-tui/releases/tag/v0.1.0
