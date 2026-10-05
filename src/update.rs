//! `auri update`: replace this binary with the latest GitHub release.
//!
//! Runs before the TUI starts (plain stdout). The newest version comes from
//! where `releases/latest` redirects (`…/releases/tag/vX.Y.Z`); if it is
//! newer, that release's shell installer (built by `dist`, see
//! `RELEASING.md`) installs it into the directory this binary runs from —
//! it swaps the file with `mv`, so replacing the running binary is safe.
//! `curl` does the downloads (the installer needs it anyway). The pure parts
//! (`parse_tag`, `is_newer`, `install_target`) are unit-tested.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

const REPO: &str = "matheusz-nied/auri-tui";
/// The env var `dist`'s installer reads for a `cargo-home` layout prefix
/// (it installs into `<prefix>/bin`).
const INSTALL_DIR_VAR: &str = "AURI_TUI_INSTALL_DIR";
/// Install into exactly this directory, without touching shell profiles.
const UNMANAGED_VAR: &str = "AURI_TUI_UNMANAGED_INSTALL";

/// Where the installer should put the new binary.
#[derive(Debug, PartialEq, Eq)]
pub enum InstallTarget {
    /// `<prefix>/bin/auri` (`~/.cargo/bin`, where the installer and
    /// `cargo install` put it): `AURI_TUI_INSTALL_DIR=<prefix>`.
    Prefix(PathBuf),
    /// Any other directory: installed there as is.
    Dir(PathBuf),
}

pub fn run() -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .context("can't find the running auri binary")?;
    let target = install_target(&exe)?;

    println!("Checking for updates…");
    let latest = latest_version()?;
    if !is_newer(&latest, current)? {
        println!("auri {current} is up to date.");
        return Ok(());
    }
    println!("Updating auri {current} → {latest}…");

    let installer = std::env::temp_dir().join(format!("auri-installer-{}.sh", std::process::id()));
    let url =
        format!("https://github.com/{REPO}/releases/download/v{latest}/auri-tui-installer.sh");
    let fetched = curl()
        .args(["--proto", "=https", "--tlsv1.2", "-fsSL", "-o"])
        .arg(&installer)
        .arg(&url)
        .status()
        .context("running curl")?;
    if !fetched.success() {
        bail!("couldn't download the installer from {url}");
    }
    let mut sh = Command::new("sh");
    sh.arg(&installer);
    match &target {
        InstallTarget::Prefix(prefix) => sh.env(INSTALL_DIR_VAR, prefix),
        InstallTarget::Dir(dir) => sh.env(UNMANAGED_VAR, dir),
    };
    let installed = sh.status().context("running the installer");
    let _ = std::fs::remove_file(&installer);
    if !installed?.success() {
        bail!("the installer failed — nothing was changed");
    }
    println!("Updated to auri {latest}.");
    Ok(())
}

fn curl() -> Command {
    Command::new("curl")
}

/// The newest release's version, from where `releases/latest` redirects.
fn latest_version() -> Result<String> {
    let out = curl()
        .args(["--proto", "=https", "--tlsv1.2", "-fsSL", "-o", "/dev/null"])
        .args(["-w", "%{url_effective}"])
        .arg(format!("https://github.com/{REPO}/releases/latest"))
        .output()
        .context("running curl (is it installed?)")?;
    if !out.status.success() {
        bail!(
            "couldn't reach GitHub: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let url = String::from_utf8_lossy(&out.stdout);
    parse_tag(&url).with_context(|| format!("no release found (GitHub answered {url})"))
}

/// `…/releases/tag/v1.2.3` -> `1.2.3`.
pub fn parse_tag(url: &str) -> Option<String> {
    let (_, tag) = url.trim().rsplit_once("/releases/tag/")?;
    let version = tag.strip_prefix('v').unwrap_or(tag);
    (!version.is_empty() && !version.contains('/')).then(|| version.to_string())
}

/// Whether `latest` is a higher `MAJOR.MINOR.PATCH` than `current`
/// (pre-release suffixes are ignored).
pub fn is_newer(latest: &str, current: &str) -> Result<bool> {
    fn parse(v: &str) -> Result<(u64, u64, u64)> {
        let core = v.split(['-', '+']).next().unwrap_or(v);
        let parts: Vec<u64> = core
            .split('.')
            .map(str::parse)
            .collect::<Result<_, _>>()
            .with_context(|| format!("not a version: {v}"))?;
        match parts[..] {
            [a, b, c] => Ok((a, b, c)),
            _ => bail!("not a version: {v}"),
        }
    }
    Ok(parse(latest)? > parse(current)?)
}

/// Where to install the update for a binary running at `exe`. A binary
/// inside a Cargo `target/` directory is a development build: updating it
/// would make no sense (`git pull` and rebuild instead).
pub fn install_target(exe: &Path) -> Result<InstallTarget> {
    let dir = exe.parent().context("the binary has no parent directory")?;
    if dir.ancestors().any(|a| {
        a.file_name().is_some_and(|n| n == "target")
            && a.parent().is_some_and(|p| p.join("Cargo.toml").exists())
    }) {
        bail!(
            "this auri is a development build ({}) — update it with `git pull` and `cargo build`",
            exe.display()
        );
    }
    Ok(match (dir.file_name(), dir.parent()) {
        (Some(name), Some(prefix)) if name == "bin" => InstallTarget::Prefix(prefix.to_path_buf()),
        _ => InstallTarget::Dir(dir.to_path_buf()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_comes_from_the_redirect_url() {
        let url = "https://github.com/matheusz-nied/auri-tui/releases/tag/v0.2.0\n";
        assert_eq!(parse_tag(url).as_deref(), Some("0.2.0"));
        assert_eq!(
            parse_tag("https://x/releases/tag/1.0.0").as_deref(),
            Some("1.0.0")
        );
        // No release yet: GitHub stays on the releases page.
        assert_eq!(parse_tag("https://github.com/o/r/releases"), None);
        assert_eq!(parse_tag("https://x/releases/tag/"), None);
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("0.10.0", "0.9.3").unwrap());
        assert!(is_newer("1.0.0", "0.99.99").unwrap());
        assert!(!is_newer("0.2.0", "0.2.0").unwrap());
        assert!(!is_newer("0.2.0", "0.3.0").unwrap());
        assert!(!is_newer("0.3.0-beta.1", "0.3.0").unwrap());
        assert!(is_newer("0.2", "0.1.0").is_err());
        assert!(is_newer("latest", "0.1.0").is_err());
    }

    #[test]
    fn installs_where_the_binary_is() {
        assert_eq!(
            install_target(Path::new("/home/u/.cargo/bin/auri")).unwrap(),
            InstallTarget::Prefix(PathBuf::from("/home/u/.cargo"))
        );
        assert_eq!(
            install_target(Path::new("/opt/tools/auri")).unwrap(),
            InstallTarget::Dir(PathBuf::from("/opt/tools"))
        );
    }

    #[test]
    fn development_builds_are_refused() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::write(repo.path().join("Cargo.toml"), "").unwrap();
        let exe = repo.path().join("target/debug/auri");
        assert!(install_target(&exe).is_err());
        // A `target` dir that isn't a Cargo build dir is fine.
        let other = tempfile::tempdir().unwrap();
        let exe = other.path().join("target/auri");
        assert_eq!(
            install_target(&exe).unwrap(),
            InstallTarget::Dir(other.path().join("target"))
        );
    }
}
