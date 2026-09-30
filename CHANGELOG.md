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
- Commit message input; empty commits are disabled.
- Optional AI commit-message drafts via Codex CLI or opencode (`Ctrl-G`).
- Resizable, collapsible sidebar; layout and settings persisted in
  `~/.config/auri/preferences.toml`.
- Mouse support: clickable buttons, tabs and draggable dividers.
- `--help` and `--version`.

[Unreleased]: https://github.com/matheusz-nied/auri-tui/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/matheusz-nied/auri-tui/releases/tag/v0.1.0
