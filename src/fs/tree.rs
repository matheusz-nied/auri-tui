//! Pure explorer tree model: loaded directory listings + the set of expanded
//! directories, flattened into display rows. No I/O — `FileTree` asks `App`
//! for listings (`LoadDirs`) and feeds the answers back via `set_children`.

use std::collections::{HashMap, HashSet};

use super::{DirEntry, EntryKind};

/// One visible row of the flattened tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub path: String,
    pub name: String,
    /// 0 for top-level entries.
    pub depth: usize,
    pub kind: EntryKind,
    /// Only meaningful for directories.
    pub expanded: bool,
}

/// The root (`""`) is always expanded. Expansion of nested dirs is
/// remembered when an ancestor collapses, like VS Code.
#[derive(Debug, Default)]
pub struct Tree {
    /// Directory path -> sorted children, for every directory listed so far.
    children: HashMap<String, Vec<DirEntry>>,
    expanded: HashSet<String>,
}

impl Tree {
    pub fn is_loaded(&self, dir: &str) -> bool {
        self.children.contains_key(dir)
    }

    pub fn is_expanded(&self, dir: &str) -> bool {
        dir.is_empty() || self.expanded.contains(dir)
    }

    /// Store a listing. Returns `false` when it's identical to the current
    /// one (nothing to redraw). Subdirectories that vanished are forgotten
    /// together with their descendants, so a re-created dir starts
    /// collapsed.
    pub fn set_children(&mut self, dir: &str, entries: Vec<DirEntry>) -> bool {
        if self.children.get(dir) == Some(&entries) {
            return false;
        }
        if let Some(old) = self.children.get(dir) {
            let gone: Vec<String> = old
                .iter()
                .filter(|o| o.kind == EntryKind::Dir && !entries.contains(o))
                .map(|o| o.path.clone())
                .collect();
            for g in gone {
                self.forget(&g);
            }
        }
        self.children.insert(dir.to_string(), entries);
        true
    }

    /// Drop `dir` and everything below it.
    fn forget(&mut self, dir: &str) {
        let prefix = format!("{dir}/");
        let inside = |p: &String| p == dir || p.starts_with(&prefix);
        self.children.retain(|p, _| !inside(p));
        self.expanded.retain(|p| !inside(p));
    }

    /// Expand `dir`. Returns `true` when its listing still has to be loaded.
    pub fn expand(&mut self, dir: &str) -> bool {
        if !dir.is_empty() {
            self.expanded.insert(dir.to_string());
        }
        !self.is_loaded(dir)
    }

    pub fn collapse(&mut self, dir: &str) {
        self.expanded.remove(dir);
    }

    pub fn collapse_all(&mut self) {
        self.expanded.clear();
    }

    /// Depth-first visible rows: top-level entries plus the children of
    /// every expanded directory whose ancestors are expanded too. An
    /// expanded dir whose listing hasn't arrived yet shows no children.
    pub fn rows(&self) -> Vec<TreeRow> {
        let mut out = Vec::new();
        self.push_rows("", 0, &mut out);
        out
    }

    fn push_rows(&self, dir: &str, depth: usize, out: &mut Vec<TreeRow>) {
        let Some(entries) = self.children.get(dir) else {
            return;
        };
        for e in entries {
            let expanded = e.kind == EntryKind::Dir && self.expanded.contains(&e.path);
            out.push(TreeRow {
                path: e.path.clone(),
                name: e.name.clone(),
                depth,
                kind: e.kind,
                expanded,
            });
            if expanded {
                self.push_rows(&e.path, depth + 1, out);
            }
        }
    }

    /// Directories whose listing is on screen — the root plus every visible
    /// expanded dir. These are re-read on each refresh.
    pub fn visible_dirs(&self) -> Vec<String> {
        let mut dirs = vec![String::new()];
        dirs.extend(
            self.rows()
                .into_iter()
                .filter(|r| r.kind == EntryKind::Dir && r.expanded)
                .map(|r| r.path),
        );
        dirs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(path: &str) -> DirEntry {
        e(path, EntryKind::Dir)
    }

    fn f(path: &str) -> DirEntry {
        e(path, EntryKind::File)
    }

    fn e(path: &str, kind: EntryKind) -> DirEntry {
        DirEntry {
            name: path.rsplit('/').next().unwrap().to_string(),
            path: path.to_string(),
            kind,
        }
    }

    fn paths(t: &Tree) -> Vec<String> {
        t.rows()
            .iter()
            .map(|r| format!("{}{}", "  ".repeat(r.depth), r.name))
            .collect()
    }

    fn sample() -> Tree {
        let mut t = Tree::default();
        t.set_children("", vec![d("src"), f("README.md")]);
        t.set_children("src", vec![d("src/fs"), f("src/main.rs")]);
        t.set_children("src/fs", vec![f("src/fs/mod.rs")]);
        t
    }

    #[test]
    fn rows_follow_expansion() {
        let mut t = sample();
        assert_eq!(paths(&t), ["src", "README.md"]);
        assert!(!t.expand("src"), "already loaded");
        t.expand("src/fs");
        assert_eq!(
            paths(&t),
            ["src", "  fs", "    mod.rs", "  main.rs", "README.md"]
        );
        assert_eq!(t.visible_dirs(), ["", "src", "src/fs"]);
        // Collapsing a parent hides but remembers the nested expansion.
        t.collapse("src");
        assert_eq!(paths(&t), ["src", "README.md"]);
        assert_eq!(t.visible_dirs(), [""]);
        t.expand("src");
        assert_eq!(paths(&t).len(), 5);
        t.collapse_all();
        assert_eq!(paths(&t), ["src", "README.md"]);
    }

    #[test]
    fn expand_unloaded_dir_requests_load() {
        let mut t = Tree::default();
        t.set_children("", vec![d("a")]);
        assert!(t.expand("a"));
        assert!(t.is_expanded("a"));
        // No children yet: just the dir row.
        assert_eq!(paths(&t), ["a"]);
        t.set_children("a", vec![f("a/x")]);
        assert_eq!(paths(&t), ["a", "  x"]);
    }

    #[test]
    fn identical_listing_reports_unchanged() {
        let mut t = sample();
        assert!(!t.set_children("", vec![d("src"), f("README.md")]));
        assert!(t.set_children("", vec![d("src")]));
    }

    #[test]
    fn vanished_dir_is_forgotten_with_descendants() {
        let mut t = sample();
        t.expand("src");
        t.expand("src/fs");
        t.set_children("", vec![f("README.md")]);
        assert!(!t.is_loaded("src") && !t.is_loaded("src/fs"));
        assert!(!t.is_expanded("src/fs"));
        // Re-created: starts collapsed and unloaded.
        t.set_children("", vec![d("src"), f("README.md")]);
        assert_eq!(paths(&t), ["src", "README.md"]);
        assert!(t.expand("src"));
    }
}
