# Releasing

Versions follow [SemVer](https://semver.org/). While on `0.x`, bump the
minor version for breaking changes or notable features and the patch
version for fixes.

## One-time setup

1. Create the GitHub repo `matheusz-nied/auri-tui` and push `main`.
2. Settings → Pages → Source: **GitHub Actions** (deploys `site/`).
3. `cargo login` with a crates.io token (for `cargo publish`).

## Every release

1. Make sure `main` is green (CI: fmt, clippy, tests on Linux + macOS).
2. Draft notes: `git cliff --unreleased --tag vX.Y.Z` and edit the result
   into `CHANGELOG.md` under `## [X.Y.Z] - YYYY-MM-DD` (dist uses this
   section as the GitHub release notes). Update the compare links at the
   bottom.
3. Bump `version` in `Cargo.toml`, run `cargo check` (updates
   `Cargo.lock`), and update the version badge in `site/index.html`.
4. `dist plan` — sanity-check the artifacts.
5. Commit `chore: release vX.Y.Z`, then tag and push:
   ```sh
   git tag vX.Y.Z && git push && git push --tags
   ```
   The `Release` workflow builds macOS/Linux binaries and creates the
   GitHub release with the shell installer.
6. `cargo publish` once the release is out.
7. Smoke test:
   ```sh
   curl -LsSf https://github.com/matheusz-nied/auri-tui/releases/latest/download/auri-tui-installer.sh | sh
   auri --version
   ```

## Demo GIF

`vhs demo/demo.tape` (needs [VHS](https://github.com/charmbracelet/vhs))
regenerates `demo/demo.gif`, used by the README and the landing page.
