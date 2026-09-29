//! User preferences: a small TOML file persisted across runs.
//!
//! `Preferences` is the root document — future sections become new fields
//! with `#[serde(default)]` so old files keep working. `PrefsStore` abstracts
//! where prefs live; `FileStore` writes atomically (tmp file + rename) and
//! `MemoryStore` is the non-persistent/test implementation.
//!
//! `App` is the only writer: components read their section from the
//! `Action::PreferencesChanged` broadcast, and changes are made via actions
//! that mutate `App::prefs` and go through `App::sync_prefs`.

use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// The root preferences document. Add a new section as a field with
/// `#[serde(default)]` — missing keys in existing files then deserialize
/// to defaults, and unknown keys are ignored.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub layout: LayoutPrefs,
}

/// Sidebar geometry persisted between runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutPrefs {
    /// Sidebar width in columns; `None` = 35% of the terminal.
    pub sidebar_width: Option<u16>,
    pub sidebar_visible: bool,
    /// History panel height in rows; `None` = 45% below the commit box.
    pub history_height: Option<u16>,
}

impl Default for LayoutPrefs {
    fn default() -> Self {
        Self {
            sidebar_width: None,
            sidebar_visible: true,
            history_height: None,
        }
    }
}

/// Where preferences are stored. `App` owns one `Box<dyn PrefsStore>`.
pub trait PrefsStore {
    /// `Ok(prefs)` — a missing file yields `Ok(Default)`. A parse/read error
    /// yields `Err`: the caller keeps defaults and must not save.
    fn load(&self) -> Result<Preferences>;
    fn save(&self, prefs: &Preferences) -> Result<()>;
}

/// File-backed store writing `<dir>/preferences.toml`.
pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The real preferences path: `$TIDE_CONFIG_DIR/preferences.toml`
    /// (test/override hook), else `$XDG_CONFIG_HOME/tide/preferences.toml`,
    /// else `$HOME/.config/tide/preferences.toml` (also on macOS).
    /// `None` when no home directory is known.
    pub fn default_path() -> Option<PathBuf> {
        resolve_path(
            std::env::var_os("TIDE_CONFIG_DIR"),
            std::env::var_os("XDG_CONFIG_HOME"),
            std::env::var_os("HOME"),
        )
    }
}

/// Pure path resolution — takes the env values so it's testable without
/// touching the process environment.
fn resolve_path(
    tide_dir: Option<std::ffi::OsString>,
    xdg: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    if let Some(dir) = tide_dir.filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir).join("preferences.toml"));
    }
    if let Some(dir) = xdg.filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir).join("tide").join("preferences.toml"));
    }
    home.filter(|d| !d.is_empty()).map(|h| {
        PathBuf::from(h)
            .join(".config")
            .join("tide")
            .join("preferences.toml")
    })
}

impl PrefsStore for FileStore {
    fn load(&self) -> Result<Preferences> {
        let text = match fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Preferences::default()),
            Err(e) => return Err(e).context("reading preferences"),
        };
        toml::from_str(&text).with_context(|| format!("parsing {}", self.path.display()))
    }

    fn save(&self, prefs: &Preferences) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).context("creating preferences directory")?;
        }
        let text = toml::to_string_pretty(prefs).context("serializing preferences")?;
        // Write-then-rename so a crash mid-write can't corrupt the file.
        let tmp = self.path.with_extension("toml.tmp");
        fs::write(&tmp, text).context("writing preferences")?;
        fs::rename(&tmp, &self.path).context("renaming preferences into place")?;
        Ok(())
    }
}

/// In-memory store for tests and for sessions with no config directory.
/// Cloning shares the same storage, so a test can keep a handle after the
/// store moved into `App`.
#[derive(Clone)]
pub struct MemoryStore {
    saved: std::rc::Rc<RefCell<Option<Preferences>>>,
    /// Test hook: when set, `load` fails with this message.
    load_error: Option<String>,
    /// How many times `save` ran.
    save_count: std::rc::Rc<RefCell<usize>>,
}

impl MemoryStore {
    pub fn new(prefs: Option<Preferences>) -> Self {
        Self {
            saved: std::rc::Rc::new(RefCell::new(prefs)),
            load_error: None,
            save_count: std::rc::Rc::new(RefCell::new(0)),
        }
    }

    /// A store whose `load` always fails (App must not save into it).
    pub fn failing_load(msg: &str) -> Self {
        Self {
            saved: std::rc::Rc::new(RefCell::new(None)),
            load_error: Some(msg.to_string()),
            save_count: std::rc::Rc::new(RefCell::new(0)),
        }
    }

    pub fn saved(&self) -> Option<Preferences> {
        self.saved.borrow().clone()
    }

    pub fn save_count(&self) -> usize {
        *self.save_count.borrow()
    }
}

impl PrefsStore for MemoryStore {
    fn load(&self) -> Result<Preferences> {
        if let Some(e) = &self.load_error {
            anyhow::bail!("{e}");
        }
        Ok(self.saved.borrow().clone().unwrap_or_default())
    }

    fn save(&self, prefs: &Preferences) -> Result<()> {
        *self.save_count.borrow_mut() += 1;
        *self.saved.borrow_mut() = Some(prefs.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn missing_file_loads_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::new(dir.path().join("preferences.toml"));
        assert_eq!(store.load().unwrap(), Preferences::default());
    }

    #[test]
    fn round_trip_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::new(dir.path().join("nested").join("preferences.toml"));
        let prefs = Preferences {
            layout: LayoutPrefs {
                sidebar_width: Some(50),
                sidebar_visible: false,
                history_height: Some(9),
            },
        };
        store.save(&prefs).unwrap();
        // Parent dirs created, no temp file left behind.
        assert!(dir.path().join("nested").join("preferences.toml").exists());
        assert!(!dir
            .path()
            .join("nested")
            .join("preferences.toml.tmp")
            .exists());
        assert!(!dir
            .path()
            .join("nested")
            .join("preferences.toml.toml.tmp")
            .exists());
        assert_eq!(store.load().unwrap(), prefs);
    }

    #[test]
    fn partial_file_fills_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.toml");
        fs::write(&path, "[layout]\nsidebar_width = 50\n").unwrap();
        let store = FileStore::new(path);
        let prefs = store.load().unwrap();
        assert_eq!(prefs.layout.sidebar_width, Some(50));
        assert!(prefs.layout.sidebar_visible);
        assert_eq!(prefs.layout.history_height, None);
    }

    #[test]
    fn unknown_keys_and_sections_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.toml");
        fs::write(
            &path,
            "[layout]\nsidebar_width = 42\nbogus = true\n[future_section]\nx = 1\n",
        )
        .unwrap();
        let prefs = FileStore::new(path).load().unwrap();
        assert_eq!(prefs.layout.sidebar_width, Some(42));
    }

    #[test]
    fn invalid_toml_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.toml");
        fs::write(&path, "[[[not toml").unwrap();
        assert!(FileStore::new(path).load().is_err());
    }

    #[test]
    fn resolve_path_prefers_tide_dir_then_xdg_then_home() {
        let t = Some(OsString::from("/t"));
        let x = Some(OsString::from("/x"));
        let h = Some(OsString::from("/h"));
        assert_eq!(
            resolve_path(t.clone(), x.clone(), h.clone()),
            Some(PathBuf::from("/t/preferences.toml"))
        );
        assert_eq!(
            resolve_path(None, x.clone(), h.clone()),
            Some(PathBuf::from("/x/tide/preferences.toml"))
        );
        assert_eq!(
            resolve_path(None, None, h.clone()),
            Some(PathBuf::from("/h/.config/tide/preferences.toml"))
        );
        assert_eq!(resolve_path(None, None, None), None);
        // Empty strings count as unset.
        assert_eq!(
            resolve_path(Some(OsString::new()), None, h),
            Some(PathBuf::from("/h/.config/tide/preferences.toml"))
        );
    }
}
