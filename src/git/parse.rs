//! Pure parsing functions: `git status --porcelain=v1 -z` output and unified
//! diffs. No I/O happens here so everything is unit-testable.

use crate::git::{CellKind, DiffCell, DiffDoc, DiffRow, FileChange, RowKind, Section};

/// Parse `git status --porcelain=v1 -z` output.
///
/// Records are NUL-separated `XY <path>` fields. When X or Y is `R`/`C`
/// (rename/copy), the following NUL-separated field is the original path.
/// A file with both X and Y set produces an entry in `Staged` (code X) *and*
/// `Unstaged` (code Y); `??` produces an `Untracked` entry (code `U`).
/// Result is grouped by section (Staged, Unstaged, Untracked), each sorted by
/// path.
pub fn parse_status(data: &[u8]) -> Vec<FileChange> {
    let mut fields = data.split(|b| *b == 0);
    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut untracked = Vec::new();

    while let Some(field) = fields.next() {
        if field.len() < 4 {
            continue;
        }
        let x = field[0] as char;
        let y = field[1] as char;
        // field[2] is the separator space.
        let path = String::from_utf8_lossy(&field[3..]).into_owned();
        let mut orig_path = None;
        if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
            if let Some(orig) = fields.next() {
                orig_path = Some(String::from_utf8_lossy(orig).into_owned());
            }
        }
        let change = |section, code| FileChange {
            path: path.clone(),
            orig_path: orig_path.clone(),
            section,
            code,
        };
        if x == '?' || x == '!' {
            // `??` (untracked) / `!!` (ignored). Ignored entries are skipped.
            if x == '?' {
                untracked.push(change(Section::Untracked, 'U'));
            }
            continue;
        }
        if x != ' ' {
            staged.push(change(Section::Staged, x));
        }
        if y != ' ' {
            unstaged.push(change(Section::Unstaged, y));
        }
    }

    let by_path = |a: &FileChange, b: &FileChange| a.path.cmp(&b.path);
    staged.sort_by(by_path);
    unstaged.sort_by(by_path);
    untracked.sort_by(by_path);
    staged.extend(unstaged);
    staged.extend(untracked);
    staged
}

/// Parse a unified diff into side-by-side rows.
///
/// Within a hunk, a run of `-` lines followed by a run of `+` lines is zipped
/// row-by-row (the shorter side gets `None` fillers); context lines appear on
/// both sides. A `HunkHeader` row is emitted before every hunk except the
/// first. `\ No newline at end of file` markers are ignored and tabs are
/// expanded to 4 spaces.
pub fn parse_diff(path: &str, text: &str) -> DiffDoc {
    let mut doc = DiffDoc {
        path: path.to_string(),
        rows: Vec::new(),
        binary: false,
    };

    let mut old_no = 0usize;
    let mut new_no = 0usize;
    let mut pending_removed: Vec<DiffCell> = Vec::new();
    let mut pending_added: Vec<DiffCell> = Vec::new();
    let mut in_hunk = false;
    let mut seen_hunks = 0usize;

    for raw_line in text.lines() {
        if raw_line.starts_with("Binary files") {
            doc.binary = true;
        }
        if let Some((old_start, new_start)) = parse_hunk_header(raw_line) {
            flush_changed(&mut doc.rows, &mut pending_removed, &mut pending_added);
            if seen_hunks > 0 {
                doc.rows.push(DiffRow {
                    left: Some(DiffCell {
                        line_no: 0,
                        text: raw_line.to_string(),
                        kind: CellKind::Context,
                    }),
                    right: None,
                    kind: RowKind::HunkHeader,
                });
            }
            seen_hunks += 1;
            in_hunk = true;
            old_no = old_start;
            new_no = new_start;
            continue;
        }
        if !in_hunk {
            continue;
        }
        let (tag, content) = raw_line.split_at(raw_line.len().min(1));
        let content = expand_tabs(content);
        match tag {
            " " => {
                flush_changed(&mut doc.rows, &mut pending_removed, &mut pending_added);
                doc.rows.push(DiffRow {
                    left: Some(DiffCell {
                        line_no: old_no,
                        text: content.clone(),
                        kind: CellKind::Context,
                    }),
                    right: Some(DiffCell {
                        line_no: new_no,
                        text: content,
                        kind: CellKind::Context,
                    }),
                    kind: RowKind::Context,
                });
                old_no += 1;
                new_no += 1;
            }
            "-" => {
                pending_removed.push(DiffCell {
                    line_no: old_no,
                    text: content,
                    kind: CellKind::Removed,
                });
                old_no += 1;
            }
            "+" => {
                pending_added.push(DiffCell {
                    line_no: new_no,
                    text: content,
                    kind: CellKind::Added,
                });
                new_no += 1;
            }
            // `\ No newline at end of file` and anything unexpected.
            _ => continue,
        }
    }
    flush_changed(&mut doc.rows, &mut pending_removed, &mut pending_added);
    doc
}

/// `@@ -a[,b] +c[,d] @@` -> `(a, c)`. Returns `None` for non-header lines.
fn parse_hunk_header(line: &str) -> Option<(usize, usize)> {
    let rest = line.strip_prefix("@@ -")?;
    let (old_part, rest) = rest.split_once(' ')?;
    let new_part = rest.strip_prefix('+')?.split_whitespace().next()?;
    let parse = |part: &str| part.split(',').next()?.parse::<usize>().ok();
    Some((parse(old_part)?, parse(new_part)?))
}

/// Zip the buffered removed/added lines of a hunk into `Changed` rows.
fn flush_changed(rows: &mut Vec<DiffRow>, removed: &mut Vec<DiffCell>, added: &mut Vec<DiffCell>) {
    let n = removed.len().max(added.len());
    for i in 0..n {
        rows.push(DiffRow {
            left: removed.get(i).cloned(),
            right: added.get(i).cloned(),
            kind: RowKind::Changed,
        });
    }
    removed.clear();
    added.clear();
}

fn expand_tabs(s: &str) -> String {
    s.replace('\t', "    ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status_bytes(records: &[&str]) -> Vec<u8> {
        records.join("\0").into_bytes()
    }

    #[test]
    fn status_modified_added_deleted() {
        let data = status_bytes(&[" M src/lib.rs", "A  new.rs", " D gone.rs"]);
        let changes = parse_status(&data);
        assert_eq!(changes.len(), 3);
        assert_eq!(changes[0].section, Section::Staged);
        assert_eq!(changes[0].code, 'A');
        assert_eq!(changes[0].path, "new.rs");
        assert_eq!(changes[1].section, Section::Unstaged);
        assert_eq!(changes[1].code, 'D');
        assert_eq!(changes[1].path, "gone.rs");
        assert_eq!(changes[2].section, Section::Unstaged);
        assert_eq!(changes[2].code, 'M');
        assert_eq!(changes[2].path, "src/lib.rs");
    }

    #[test]
    fn status_untracked() {
        let data = status_bytes(&["?? fresh.txt"]);
        let changes = parse_status(&data);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].section, Section::Untracked);
        assert_eq!(changes[0].code, 'U');
    }

    #[test]
    fn status_both_staged_and_unstaged() {
        let data = status_bytes(&["MM both.txt"]);
        let changes = parse_status(&data);
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].section, Section::Staged);
        assert_eq!(changes[0].code, 'M');
        assert_eq!(changes[1].section, Section::Unstaged);
        assert_eq!(changes[1].code, 'M');
        assert_eq!(changes[0].path, "both.txt");
        assert_eq!(changes[1].path, "both.txt");
    }

    #[test]
    fn status_rename_consumes_next_field() {
        let data = status_bytes(&["R  dir/new.rs", "dir/old.rs", " M m.rs"]);
        let changes = parse_status(&data);
        let rename = changes
            .iter()
            .find(|c| c.code == 'R')
            .expect("rename entry");
        assert_eq!(rename.section, Section::Staged);
        assert_eq!(rename.path, "dir/new.rs");
        assert_eq!(rename.orig_path.as_deref(), Some("dir/old.rs"));
        // The field after the rename was consumed as orig path, not a record.
        let m = changes
            .iter()
            .find(|c| c.path == "m.rs")
            .expect("m.rs entry");
        assert_eq!(m.section, Section::Unstaged);
    }

    #[test]
    fn diff_modify_pairs_removed_with_added() {
        let text = "\
diff --git a/f.txt b/f.txt
--- a/f.txt
+++ b/f.txt
@@ -1,3 +1,4 @@
 ctx
-old one
-old two
+new one
+new two
+new three
 tail";
        let doc = parse_diff("f.txt", text);
        // ctx row, 3 changed rows, tail row
        assert_eq!(doc.rows.len(), 5);
        assert_eq!(doc.rows[0].kind, RowKind::Context);
        let changed: Vec<_> = doc
            .rows
            .iter()
            .filter(|r| r.kind == RowKind::Changed)
            .collect();
        assert_eq!(changed.len(), 3);
        assert_eq!(changed[0].left.as_ref().unwrap().text, "old one");
        assert_eq!(changed[0].right.as_ref().unwrap().text, "new one");
        assert_eq!(changed[1].left.as_ref().unwrap().text, "old two");
        assert_eq!(changed[1].right.as_ref().unwrap().text, "new two");
        // Extra added line has no left side.
        assert!(changed[2].left.is_none());
        assert_eq!(changed[2].right.as_ref().unwrap().text, "new three");
        assert_eq!(changed[2].right.as_ref().unwrap().kind, CellKind::Added);
    }

    #[test]
    fn diff_new_file_all_added() {
        let text = "\
diff --git a/new.txt b/new.txt
new file mode 100644
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,2 @@
+alpha
+beta";
        let doc = parse_diff("new.txt", text);
        assert_eq!(doc.rows.len(), 2);
        for row in &doc.rows {
            assert!(row.left.is_none());
            assert_eq!(row.right.as_ref().unwrap().kind, CellKind::Added);
        }
        assert_eq!(doc.rows[0].right.as_ref().unwrap().line_no, 1);
        assert_eq!(doc.rows[1].right.as_ref().unwrap().line_no, 2);
    }

    #[test]
    fn diff_deleted_file_all_removed() {
        let text = "\
diff --git a/old.txt b/old.txt
deleted file mode 100644
--- a/old.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-gone one
-gone two";
        let doc = parse_diff("old.txt", text);
        assert_eq!(doc.rows.len(), 2);
        for row in &doc.rows {
            assert!(row.right.is_none());
            assert_eq!(row.left.as_ref().unwrap().kind, CellKind::Removed);
        }
        assert_eq!(doc.rows[0].left.as_ref().unwrap().line_no, 1);
        assert_eq!(doc.rows[1].left.as_ref().unwrap().line_no, 2);
    }

    #[test]
    fn diff_multiple_hunks_emit_header_rows() {
        let text = "\
@@ -1,2 +1,2 @@
-a
+b
@@ -10,2 +10,2 @@
-c
+d";
        let doc = parse_diff("f.txt", text);
        let headers: Vec<_> = doc
            .rows
            .iter()
            .filter(|r| r.kind == RowKind::HunkHeader)
            .collect();
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].left.as_ref().unwrap().text, "@@ -10,2 +10,2 @@");
        // row order: changed, header, changed
        assert_eq!(doc.rows[0].kind, RowKind::Changed);
        assert_eq!(doc.rows[1].kind, RowKind::HunkHeader);
        assert_eq!(doc.rows[2].kind, RowKind::Changed);
    }

    #[test]
    fn diff_line_numbers_track_both_sides() {
        let text = "\
@@ -5,3 +7,3 @@
 keep
-drop
+add
 keep2";
        let doc = parse_diff("f.txt", text);
        assert_eq!(doc.rows.len(), 3);
        assert_eq!(doc.rows[0].left.as_ref().unwrap().line_no, 5);
        assert_eq!(doc.rows[0].right.as_ref().unwrap().line_no, 7);
        assert_eq!(doc.rows[1].left.as_ref().unwrap().line_no, 6);
        assert_eq!(doc.rows[1].right.as_ref().unwrap().line_no, 8);
        assert_eq!(doc.rows[2].left.as_ref().unwrap().line_no, 7);
        assert_eq!(doc.rows[2].right.as_ref().unwrap().line_no, 9);
    }

    #[test]
    fn diff_binary_detected() {
        let text = "Binary files a/pic.png and b/pic.png differ";
        let doc = parse_diff("pic.png", text);
        assert!(doc.binary);
        assert!(doc.rows.is_empty());
    }

    #[test]
    fn diff_no_newline_marker_ignored() {
        let text = "\
@@ -1,2 +1,2 @@
-last
\\ No newline at end of file
+last
\\ No newline at end of file";
        let doc = parse_diff("f.txt", text);
        assert_eq!(doc.rows.len(), 1);
        assert_eq!(doc.rows[0].kind, RowKind::Changed);
        assert_eq!(doc.rows[0].left.as_ref().unwrap().text, "last");
    }
}
