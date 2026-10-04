//! The text model behind the file viewer's edit mode: lines, a cursor with
//! an optional selection, undo/redo and the "modified since saved" state.
//!
//! Pure (no I/O) — `FileView` owns one per open file and `App` writes
//! `text()` to disk on save. The document round-trips exactly: line ending
//! (LF, or CRLF when *every* line uses it), a missing final newline, tabs
//! and odd characters (a lone `\r` stays in its line) all come back out of
//! `text()` unchanged. Positions are byte offsets on grapheme boundaries;
//! the `display_*` helpers map them to screen columns (tabs expanded to
//! `TAB_WIDTH` stops, wide chars two columns, control chars one `�`).

use unicode_segmentation::UnicodeSegmentation;

use crate::fs::TAB_WIDTH;
use crate::text;

/// Undo steps kept; older ones are dropped.
const MAX_UNDO: usize = 1000;

/// A place in the document: line index and byte offset into that line
/// (always on a grapheme boundary).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

impl Pos {
    pub fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

/// What Tab inserts, guessed from the file's existing indentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indent {
    Tabs,
    Spaces(usize),
}

/// How an edit was made — consecutive edits of the same kind merge into
/// one undo step (a typed word, a run of Backspace).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Typing,
    Backspace,
    Delete,
    Other,
}

/// One undo step: `removed` was replaced by `inserted` at `start`.
#[derive(Debug, Clone)]
struct Edit {
    start: Pos,
    removed: String,
    inserted: String,
    cursor_before: Pos,
    anchor_before: Option<Pos>,
    cursor_after: Pos,
    version_before: u64,
    version_after: u64,
    kind: Kind,
}

#[derive(Debug, Clone)]
pub struct Buffer {
    /// Never empty: an empty document is one empty line.
    lines: Vec<String>,
    crlf: bool,
    final_newline: bool,
    indent: Indent,
    cursor: Pos,
    /// The other end of the selection, when there is one.
    anchor: Option<Pos>,
    /// Display column kept across Up/Down through shorter lines.
    goal: Option<usize>,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    /// The last undo step may still absorb the next edit; any cursor move
    /// closes it.
    merge: bool,
    /// Identifies the current contents: every edit gets a fresh number,
    /// undo/redo restore the step's. `modified()` = differs from `saved`.
    version: u64,
    next_version: u64,
    saved: u64,
    /// First line changed since `take_changed_from` (highlighting restarts
    /// there).
    changed_from: Option<usize>,
}

impl Buffer {
    /// A buffer holding `text`, cursor at the top, nothing modified.
    pub fn new(text: &str) -> Self {
        let crlf =
            text.contains("\r\n") && text.matches('\n').count() == text.matches("\r\n").count();
        let mut lines: Vec<String> = text
            .split('\n')
            .map(|l| {
                if crlf {
                    l.strip_suffix('\r').unwrap_or(l)
                } else {
                    l
                }
                .to_string()
            })
            .collect();
        // A trailing newline ends the last line rather than starting one.
        let final_newline = lines.len() > 1 && lines.last().is_some_and(String::is_empty);
        if final_newline {
            lines.pop();
        }
        let indent = detect_indent(&lines);
        Self {
            lines,
            crlf,
            final_newline,
            indent,
            cursor: Pos::default(),
            anchor: None,
            goal: None,
            undo: Vec::new(),
            redo: Vec::new(),
            merge: false,
            version: 0,
            next_version: 1,
            saved: 0,
            changed_from: None,
        }
    }

    /// The document as it goes to disk, original line endings included.
    pub fn text(&self) -> String {
        let eol = if self.crlf { "\r\n" } else { "\n" };
        let mut out = self.lines.join(eol);
        if self.final_newline {
            out.push_str(eol);
        }
        out
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn cursor(&self) -> Pos {
        self.cursor
    }

    pub fn indent(&self) -> Indent {
        self.indent
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// Unsaved changes: the contents differ from the last saved (or
    /// loaded) version. Undoing back to it clears this.
    pub fn modified(&self) -> bool {
        self.version != self.saved
    }

    /// `version` was written to disk.
    pub fn mark_saved(&mut self, version: u64) {
        self.saved = version;
        // Typing on after a save starts a new undo step, so undo can stop
        // exactly at the saved state.
        self.merge = false;
    }

    /// First line changed since the last call, if any.
    pub fn take_changed_from(&mut self) -> Option<usize> {
        self.changed_from.take()
    }

    /// The selection, start first; `None` when empty.
    pub fn selection(&self) -> Option<(Pos, Pos)> {
        let anchor = self.anchor.filter(|a| *a != self.cursor)?;
        Some(if anchor < self.cursor {
            (anchor, self.cursor)
        } else {
            (self.cursor, anchor)
        })
    }

    pub fn selected_text(&self) -> Option<String> {
        self.selection().map(|(s, e)| self.slice(s, e))
    }

    // --- editing ------------------------------------------------------

    /// Type one character over the selection.
    pub fn type_char(&mut self, c: char) {
        let (start, end) = self.target();
        let kind = if self.selection().is_some() {
            Kind::Other
        } else {
            Kind::Typing
        };
        self.apply(start, end, c.encode_utf8(&mut [0; 4]), kind);
    }

    /// Insert `text` (a paste) over the selection; any line ending style
    /// becomes the document's.
    pub fn insert(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let (start, end) = self.target();
        self.apply(start, end, &text, Kind::Other);
    }

    /// Enter: a new line indented like the current one (as far as the
    /// cursor, so Enter inside the indentation doesn't double it).
    pub fn newline(&mut self) {
        let (start, end) = self.target();
        let line = &self.lines[start.line][..start.col];
        let indent: String = line
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        self.apply(start, end, &format!("\n{indent}"), Kind::Other);
    }

    pub fn backspace(&mut self) {
        if let Some((s, e)) = self.selection() {
            return self.apply(s, e, "", Kind::Other);
        }
        let c = self.cursor;
        let from = if c.col > 0 {
            Pos::new(c.line, prev_boundary(&self.lines[c.line], c.col))
        } else if c.line > 0 {
            Pos::new(c.line - 1, self.lines[c.line - 1].len())
        } else {
            return;
        };
        self.apply(from, c, "", Kind::Backspace);
    }

    pub fn delete(&mut self) {
        if let Some((s, e)) = self.selection() {
            return self.apply(s, e, "", Kind::Other);
        }
        let c = self.cursor;
        let line = &self.lines[c.line];
        let to = if c.col < line.len() {
            Pos::new(c.line, next_boundary(line, c.col))
        } else if c.line + 1 < self.lines.len() {
            Pos::new(c.line + 1, 0)
        } else {
            return;
        };
        self.apply(c, to, "", Kind::Delete);
    }

    /// Tab: indent every selected line when the selection spans lines,
    /// otherwise insert one indent unit (spaces up to the next stop).
    pub fn tab(&mut self) {
        if let Some((s, e)) = self.selection() {
            if s.line != e.line {
                return self.indent_lines(true);
            }
        }
        let unit = match self.indent {
            Indent::Tabs => "\t".to_string(),
            Indent::Spaces(w) => {
                let (start, _) = self.target();
                let col = display_col(&self.lines[start.line], start.col);
                " ".repeat(w - col % w)
            }
        };
        let (start, end) = self.target();
        self.apply(start, end, &unit, Kind::Other);
    }

    /// Shift-Tab: remove one indent level from the selected lines (or the
    /// cursor's).
    pub fn dedent(&mut self) {
        self.indent_lines(false);
    }

    /// Delete the selection and return it; with none, the whole line
    /// (returned with its newline).
    pub fn cut(&mut self) -> Option<String> {
        if let Some((s, e)) = self.selection() {
            let text = self.slice(s, e);
            self.apply(s, e, "", Kind::Other);
            return Some(text);
        }
        let line = self.cursor.line;
        let len = self.lines[line].len();
        let text = format!("{}\n", self.lines[line]);
        let (from, to) = if line + 1 < self.lines.len() {
            (Pos::new(line, 0), Pos::new(line + 1, 0))
        } else if line > 0 {
            (
                Pos::new(line - 1, self.lines[line - 1].len()),
                Pos::new(line, len),
            )
        } else {
            (Pos::new(0, 0), Pos::new(0, len))
        };
        self.apply(from, to, "", Kind::Other);
        Some(text)
    }

    pub fn undo(&mut self) -> bool {
        let Some(e) = self.undo.pop() else {
            return false;
        };
        let end = end_of(e.start, &e.inserted);
        self.replace(e.start, end, &e.removed);
        self.cursor = e.cursor_before;
        self.anchor = e.anchor_before;
        self.version = e.version_before;
        self.redo.push(e);
        self.after_move();
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(e) = self.redo.pop() else {
            return false;
        };
        let end = end_of(e.start, &e.removed);
        self.replace(e.start, end, &e.inserted);
        self.cursor = e.cursor_after;
        self.anchor = None;
        self.version = e.version_after;
        self.undo.push(e);
        self.after_move();
        true
    }

    // --- movement -----------------------------------------------------
    //
    // `extend` = Shift held: the selection grows from where the cursor was.
    // Without it, Left/Right first collapse a selection to its edge.

    pub fn left(&mut self, extend: bool) {
        if let (false, Some((s, _))) = (extend, self.selection()) {
            return self.place(s, false);
        }
        let c = self.cursor;
        let to = if c.col > 0 {
            Pos::new(c.line, prev_boundary(&self.lines[c.line], c.col))
        } else if c.line > 0 {
            Pos::new(c.line - 1, self.lines[c.line - 1].len())
        } else {
            c
        };
        self.place(to, extend);
    }

    pub fn right(&mut self, extend: bool) {
        if let (false, Some((_, e))) = (extend, self.selection()) {
            return self.place(e, false);
        }
        let c = self.cursor;
        let line = &self.lines[c.line];
        let to = if c.col < line.len() {
            Pos::new(c.line, next_boundary(line, c.col))
        } else if c.line + 1 < self.lines.len() {
            Pos::new(c.line + 1, 0)
        } else {
            c
        };
        self.place(to, extend);
    }

    /// Up (`delta < 0`) or down by `|delta|` lines, keeping the display
    /// column; past the first/last line it goes to that line's edge.
    pub fn vertical(&mut self, delta: isize, extend: bool) {
        let c = self.cursor;
        let goal = self
            .goal
            .unwrap_or_else(|| display_col(&self.lines[c.line], c.col));
        let last = self.lines.len() - 1;
        let target = c.line as isize + delta;
        let to = if target < 0 {
            Pos::new(0, 0)
        } else if target as usize > last {
            Pos::new(last, self.lines[last].len())
        } else {
            let line = target as usize;
            Pos::new(line, byte_at_col(&self.lines[line], goal))
        };
        self.place(to, extend);
        self.goal = Some(goal);
    }

    /// Home: the first non-blank character, or column 0 when already there.
    pub fn home(&mut self, extend: bool) {
        let c = self.cursor;
        let line = &self.lines[c.line];
        let first = line.len() - line.trim_start_matches([' ', '\t']).len();
        let col = if c.col == first { 0 } else { first };
        self.place(Pos::new(c.line, col), extend);
    }

    pub fn end(&mut self, extend: bool) {
        let line = self.cursor.line;
        self.place(Pos::new(line, self.lines[line].len()), extend);
    }

    pub fn doc_start(&mut self, extend: bool) {
        self.place(Pos::default(), extend);
    }

    pub fn doc_end(&mut self, extend: bool) {
        let last = self.lines.len() - 1;
        self.place(Pos::new(last, self.lines[last].len()), extend);
    }

    /// To the start of the previous word (Ctrl/Alt-Left).
    pub fn word_left(&mut self, extend: bool) {
        let c = self.cursor;
        let to = if c.col == 0 {
            return self.left(extend);
        } else {
            let line = &self.lines[c.line];
            let before: Vec<(usize, char)> = line[..c.col].char_indices().collect();
            let mut i = before.len();
            while i > 0 && before[i - 1].1.is_whitespace() {
                i -= 1;
            }
            let class = before.get(i.wrapping_sub(1)).map(|(_, ch)| is_word(*ch));
            while i > 0
                && Some(is_word(before[i - 1].1)) == class
                && !before[i - 1].1.is_whitespace()
            {
                i -= 1;
            }
            Pos::new(c.line, before.get(i).map_or(c.col, |(b, _)| *b))
        };
        self.place(to, extend);
    }

    /// To the end of the next word (Ctrl/Alt-Right).
    pub fn word_right(&mut self, extend: bool) {
        let c = self.cursor;
        let line = &self.lines[c.line];
        if c.col == line.len() {
            return self.right(extend);
        }
        let mut chars = line[c.col..].char_indices().peekable();
        while chars.next_if(|(_, ch)| ch.is_whitespace()).is_some() {}
        let class = chars.peek().map(|(_, ch)| is_word(*ch));
        while chars
            .next_if(|(_, ch)| Some(is_word(*ch)) == class && !ch.is_whitespace())
            .is_some()
        {}
        let col = chars.peek().map_or(line.len(), |(i, _)| c.col + i);
        self.place(Pos::new(c.line, col), extend);
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(Pos::default());
        let last = self.lines.len() - 1;
        self.cursor = Pos::new(last, self.lines[last].len());
        self.after_move();
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    /// Put the cursor at a screen position (a click): `line`, display
    /// column `col`, both clamped into the document.
    pub fn click(&mut self, line: usize, col: usize, extend: bool) {
        let line = line.min(self.lines.len() - 1);
        let to = Pos::new(line, byte_at_col(&self.lines[line], col));
        self.place(to, extend);
    }

    // --- internals ----------------------------------------------------

    fn place(&mut self, to: Pos, extend: bool) {
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = to;
        self.after_move();
    }

    fn after_move(&mut self) {
        self.goal = None;
        self.merge = false;
    }

    /// The range an edit replaces: the selection, or the empty range at
    /// the cursor.
    fn target(&self) -> (Pos, Pos) {
        self.selection().unwrap_or((self.cursor, self.cursor))
    }

    fn slice(&self, start: Pos, end: Pos) -> String {
        if start.line == end.line {
            return self.lines[start.line][start.col..end.col].to_string();
        }
        let mut out = self.lines[start.line][start.col..].to_string();
        for line in &self.lines[start.line + 1..end.line] {
            out.push('\n');
            out.push_str(line);
        }
        out.push('\n');
        out.push_str(&self.lines[end.line][..end.col]);
        out
    }

    /// Replace `[start, end)` with `text` (`\n`-separated); returns the end
    /// of the inserted text. No undo bookkeeping.
    fn replace(&mut self, start: Pos, end: Pos, text: &str) -> Pos {
        let head = &self.lines[start.line][..start.col];
        let tail = &self.lines[end.line][end.col..];
        let mut new: Vec<String> = text.split('\n').map(str::to_string).collect();
        let last = new.len() - 1;
        let after = Pos::new(
            start.line + last,
            if last == 0 {
                start.col + new[0].len()
            } else {
                new[last].len()
            },
        );
        new[0].insert_str(0, head);
        new[last].push_str(tail);
        self.lines.splice(start.line..=end.line, new);
        self.changed_from = Some(self.changed_from.map_or(start.line, |l| l.min(start.line)));
        after
    }

    /// An edit with undo: merged into the previous step when it continues
    /// it (same kind, adjacent), otherwise a new step. Leaves the cursor
    /// after the inserted text, with no selection.
    fn apply(&mut self, start: Pos, end: Pos, text: &str, kind: Kind) {
        let removed = self.slice(start, end);
        let (cursor_before, anchor_before) = (self.cursor, self.anchor);
        let version_before = self.version;
        let after = self.replace(start, end, text);
        self.redo.clear();
        let version = self.next_version;
        self.next_version += 1;
        self.version = version;

        let merged = self.merge
            && self.undo.last_mut().is_some_and(|last| {
                let ok = last.kind == kind
                    && match kind {
                        // A word is one step: a space after a non-space
                        // starts the next one.
                        Kind::Typing => {
                            removed.is_empty()
                                && last.cursor_after == start
                                && !(text.starts_with(char::is_whitespace)
                                    && !last.inserted.ends_with(char::is_whitespace))
                        }
                        Kind::Backspace => last.inserted.is_empty() && end == last.start,
                        Kind::Delete => last.inserted.is_empty() && start == last.start,
                        Kind::Other => false,
                    };
                if ok {
                    match kind {
                        Kind::Typing => last.inserted.push_str(text),
                        Kind::Backspace => {
                            last.start = start;
                            last.removed.insert_str(0, &removed);
                        }
                        _ => last.removed.push_str(&removed),
                    }
                    last.cursor_after = after;
                    last.version_after = version;
                }
                ok
            });
        if !merged {
            if self.undo.len() == MAX_UNDO {
                self.undo.remove(0);
            }
            self.undo.push(Edit {
                start,
                removed,
                inserted: text.to_string(),
                cursor_before,
                anchor_before,
                cursor_after: after,
                version_before,
                version_after: version,
                kind,
            });
        }
        self.cursor = after;
        self.anchor = None;
        self.goal = None;
        self.merge = true;
    }

    /// Indent (or dedent) every line the selection touches — the cursor's
    /// line without one — as one undo step, keeping the selection.
    fn indent_lines(&mut self, indent: bool) {
        let (s, e) = self.target();
        // A selection ending at column 0 doesn't include that line.
        let last = if e.line > s.line && e.col == 0 {
            e.line - 1
        } else {
            e.line
        };
        let unit = match self.indent {
            Indent::Tabs => "\t".to_string(),
            Indent::Spaces(w) => " ".repeat(w),
        };
        // Bytes added (+) or removed (-) at the start of each line.
        let mut shifts = Vec::new();
        let new: Vec<String> = (s.line..=last)
            .map(|i| {
                let line = &self.lines[i];
                if indent {
                    if line.is_empty() {
                        shifts.push(0);
                        return String::new();
                    }
                    shifts.push(unit.len() as isize);
                    format!("{unit}{line}")
                } else {
                    let n = if line.starts_with('\t') {
                        1
                    } else {
                        let w = match self.indent {
                            Indent::Spaces(w) => w,
                            Indent::Tabs => TAB_WIDTH,
                        };
                        (line.len() - line.trim_start_matches(' ').len()).min(w)
                    };
                    shifts.push(-(n as isize));
                    line[n..].to_string()
                }
            })
            .collect();
        if shifts.iter().all(|d| *d == 0) {
            return;
        }
        let shift = |p: Pos| -> Pos {
            match shifts.get(p.line.wrapping_sub(s.line)) {
                // Column 0 stays put: a selection from a line's start
                // keeps covering the new indentation.
                Some(&d) if p.line >= s.line && p.line <= last && !(indent && p.col == 0) => {
                    Pos::new(p.line, (p.col as isize + d).max(0) as usize)
                }
                _ => p,
            }
        };
        let (cursor, anchor) = (shift(self.cursor), self.anchor.map(shift));
        let end = Pos::new(last, self.lines[last].len());
        self.apply(Pos::new(s.line, 0), end, &new.join("\n"), Kind::Other);
        self.cursor = cursor;
        self.anchor = anchor;
        if let Some(step) = self.undo.last_mut() {
            step.cursor_after = cursor;
        }
        self.merge = false;
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Where `text` ends when inserted at `start`.
fn end_of(start: Pos, text: &str) -> Pos {
    match text.rfind('\n') {
        None => Pos::new(start.line, start.col + text.len()),
        Some(i) => Pos::new(start.line + text.matches('\n').count(), text.len() - i - 1),
    }
}

fn prev_boundary(line: &str, col: usize) -> usize {
    line[..col]
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(i, _)| i)
}

fn next_boundary(line: &str, col: usize) -> usize {
    line[col..]
        .graphemes(true)
        .next()
        .map_or(col, |g| col + g.len())
}

/// Tabs if more lines are indented with them, else spaces in the smallest
/// indentation step seen (2..=8; 4 when nothing is indented).
fn detect_indent(lines: &[String]) -> Indent {
    let mut tabs = 0;
    let mut spaces = 0;
    let mut step: Option<usize> = None;
    for line in lines {
        if line.starts_with('\t') {
            tabs += 1;
        } else if line.starts_with(' ') {
            let n = line.len() - line.trim_start_matches(' ').len();
            if line.len() > n {
                spaces += 1;
                if n >= 2 {
                    step = Some(step.map_or(n, |s| s.min(n)));
                }
            }
        }
    }
    if tabs > spaces {
        Indent::Tabs
    } else {
        Indent::Spaces(step.unwrap_or(4).clamp(2, 8))
    }
}

/// Columns one grapheme takes when it starts at display column `col`.
fn grapheme_cols(g: &str, col: usize) -> usize {
    if g == "\t" {
        TAB_WIDTH - col % TAB_WIDTH
    } else if g.contains(char::is_control) {
        1
    } else {
        text::width(g)
    }
}

/// `line` as drawn: tabs expanded, control characters as `�`.
pub fn display_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut col = 0;
    for g in line.graphemes(true) {
        let w = grapheme_cols(g, col);
        if g == "\t" {
            out.extend(std::iter::repeat_n(' ', w));
        } else if g.contains(char::is_control) {
            out.push('\u{FFFD}');
        } else {
            out.push_str(g);
        }
        col += w;
    }
    out
}

/// Display column of byte offset `byte` in `line`.
pub fn display_col(line: &str, byte: usize) -> usize {
    let mut col = 0;
    for g in line[..byte].graphemes(true) {
        col += grapheme_cols(g, col);
    }
    col
}

/// The byte offset whose display column is `col` — or, inside a wide
/// character or tab, the one where it starts; the line's end past it.
pub fn byte_at_col(line: &str, col: usize) -> usize {
    let mut at = 0;
    for (i, g) in line.grapheme_indices(true) {
        let w = grapheme_cols(g, at);
        if at + w > col {
            return i;
        }
        at += w;
    }
    line.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(b: &Buffer) -> (usize, usize) {
        (b.cursor.line, b.cursor.col)
    }

    #[test]
    fn round_trips_line_endings_and_final_newline() {
        for text in [
            "",
            "\n",
            "a",
            "a\n",
            "a\nb",
            "a\r\nb\r\n",
            "a\r\nb\nc\n",
            "lone\rcr\n",
            "\tx\n\n\n",
        ] {
            assert_eq!(Buffer::new(text).text(), text, "{text:?}");
        }
        let b = Buffer::new("a\r\nb\r\n");
        assert_eq!(b.lines(), ["a", "b"]);
        // Mixed endings stay LF-separated with the `\r` kept in its line.
        assert_eq!(Buffer::new("a\r\nb\n").lines(), ["a\r", "b"]);
        assert_eq!(Buffer::new("").lines(), [""]);
    }

    #[test]
    fn typing_uses_the_document_line_ending() {
        let mut b = Buffer::new("a\r\nb\r\n");
        b.end(false);
        b.newline();
        b.type_char('x');
        assert_eq!(b.text(), "a\r\nx\r\nb\r\n");
        b.insert("1\n2\r\n3");
        assert_eq!(b.text(), "a\r\nx1\r\n2\r\n3\r\nb\r\n");
    }

    #[test]
    fn edits_and_modified_state_follow_undo_and_save() {
        let mut b = Buffer::new("hello\n");
        assert!(!b.modified());
        b.end(false);
        for c in " world".chars() {
            b.type_char(c);
        }
        assert_eq!(b.text(), "hello world\n");
        assert!(b.modified());
        // " world" is one step (a space after a word starts a new one).
        assert!(b.undo());
        assert_eq!(b.text(), "hello\n");
        assert!(!b.modified());
        assert!(!b.undo());
        assert!(b.redo());
        assert_eq!(b.text(), "hello world\n");
        assert_eq!(at(&b), (0, 11));

        b.mark_saved(b.version());
        assert!(!b.modified());
        b.type_char('!');
        assert!(b.modified());
        b.undo();
        // Back at exactly the saved text.
        assert!(!b.modified());
        assert_eq!(b.text(), "hello world\n");
        b.undo();
        assert!(b.modified());
    }

    #[test]
    fn words_are_separate_undo_steps() {
        let mut b = Buffer::new("");
        for c in "ab cd".chars() {
            b.type_char(c);
        }
        b.undo();
        assert_eq!(b.text(), "ab");
        b.undo();
        assert_eq!(b.text(), "");
    }

    #[test]
    fn a_cursor_move_ends_the_undo_step() {
        let mut b = Buffer::new("");
        b.type_char('a');
        b.left(false);
        b.type_char('b');
        assert_eq!(b.text(), "ba");
        b.undo();
        assert_eq!(b.text(), "a");
    }

    #[test]
    fn backspace_and_delete_merge_and_join_lines() {
        let mut b = Buffer::new("ab\ncd");
        b.vertical(1, false);
        b.end(false);
        b.backspace();
        b.backspace();
        b.backspace();
        assert_eq!(b.text(), "ab");
        b.undo();
        assert_eq!(b.text(), "ab\ncd");
        assert_eq!(at(&b), (1, 2));

        let mut b = Buffer::new("ab\ncd");
        b.delete();
        b.delete();
        b.delete();
        assert_eq!(b.text(), "cd");
        b.undo();
        assert_eq!(b.text(), "ab\ncd");
        // At the very start/end there is nothing to remove.
        b.doc_start(false);
        b.backspace();
        b.doc_end(false);
        b.delete();
        assert_eq!(b.text(), "ab\ncd");
        assert!(!b.modified());
    }

    #[test]
    fn graphemes_move_and_delete_as_one() {
        let mut b = Buffer::new("e\u{301}日x");
        b.right(false);
        assert_eq!(b.cursor.col, 3);
        b.right(false);
        assert_eq!(b.cursor.col, 6);
        b.backspace();
        assert_eq!(b.text(), "e\u{301}x");
        b.backspace();
        assert_eq!(b.text(), "x");
    }

    #[test]
    fn newline_keeps_indentation() {
        let mut b = Buffer::new("    fn x() {");
        b.end(false);
        b.newline();
        b.type_char('y');
        assert_eq!(b.text(), "    fn x() {\n    y");
        // Inside the indentation only what's before the cursor is copied.
        let mut b = Buffer::new("\t\tz");
        b.right(false);
        b.newline();
        assert_eq!(b.text(), "\t\n\t\tz");
    }

    #[test]
    fn selection_is_replaced_cut_and_collapsed() {
        let mut b = Buffer::new("one two\nthree");
        b.word_right(true);
        assert_eq!(b.selected_text().as_deref(), Some("one"));
        b.type_char('1');
        assert_eq!(b.text(), "1 two\nthree");
        b.undo();
        assert_eq!(b.selected_text().as_deref(), Some("one"));

        b.doc_end(true);
        assert_eq!(b.cut().as_deref(), Some("one two\nthree"));
        assert_eq!(b.text(), "");
        b.undo();

        let mut b = Buffer::new("abc");
        b.right(false);
        b.right(true);
        b.left(false);
        assert_eq!((at(&b), b.selection()), ((0, 1), None));
        b.right(true);
        b.right(false);
        assert_eq!(at(&b), (0, 2));
    }

    #[test]
    fn cut_without_selection_takes_the_line() {
        let mut b = Buffer::new("a\nb\nc\n");
        b.vertical(1, false);
        assert_eq!(b.cut().as_deref(), Some("b\n"));
        assert_eq!(b.text(), "a\nc\n");
        b.doc_end(false);
        assert_eq!(b.cut().as_deref(), Some("c\n"));
        assert_eq!(b.text(), "a\n");
        assert_eq!(b.cut().as_deref(), Some("a\n"));
        assert_eq!(b.text(), "\n");
    }

    #[test]
    fn select_all_and_insert_replaces_everything() {
        let mut b = Buffer::new("x\ny\n");
        b.select_all();
        b.insert("z");
        assert_eq!(b.text(), "z\n");
    }

    #[test]
    fn tab_inserts_detected_unit_and_indents_selections() {
        let mut b = Buffer::new("fn a() {\n  x\n}\n");
        assert_eq!(b.indent(), Indent::Spaces(2));
        b.right(false);
        b.tab();
        assert_eq!(b.lines()[0], "f n a() {");

        let mut b = Buffer::new("a\n\tb\nc");
        assert_eq!(b.indent(), Indent::Tabs);
        b.vertical(1, true);
        b.end(true);
        b.tab();
        assert_eq!(b.text(), "\ta\n\t\tb\nc");
        // The selection follows its text.
        assert_eq!(b.selected_text().as_deref(), Some("\ta\n\t\tb"));
        b.dedent();
        b.dedent();
        assert_eq!(b.text(), "a\nb\nc");
        b.undo();
        b.undo();
        assert_eq!(b.text(), "\ta\n\t\tb\nc");
        b.undo();
        assert_eq!(b.text(), "a\n\tb\nc");
        assert!(!b.modified());
    }

    #[test]
    fn dedent_removes_at_most_one_level() {
        let mut b = Buffer::new("      x\n  y\nz");
        assert_eq!(b.indent(), Indent::Spaces(2));
        b.select_all();
        b.dedent();
        assert_eq!(b.text(), "    x\ny\nz");
        let mut b = Buffer::new("z");
        b.dedent();
        assert!(!b.modified());
    }

    #[test]
    fn vertical_keeps_the_goal_column() {
        let mut b = Buffer::new("abcdef\nab\nabcdef\n\tx");
        b.end(false);
        b.vertical(1, false);
        assert_eq!(at(&b), (1, 2));
        b.vertical(1, false);
        assert_eq!(at(&b), (2, 6));
        // Column 6 is inside the tab's 4 columns... past them: after `x`.
        b.vertical(1, false);
        assert_eq!(at(&b), (3, 2));
        b.vertical(10, false);
        assert_eq!(at(&b), (3, 2));
        b.vertical(-10, false);
        assert_eq!(at(&b), (0, 0));
    }

    #[test]
    fn home_toggles_between_indent_and_column_zero() {
        let mut b = Buffer::new("    x");
        b.end(false);
        b.home(false);
        assert_eq!(at(&b), (0, 4));
        b.home(false);
        assert_eq!(at(&b), (0, 0));
    }

    #[test]
    fn word_moves() {
        let mut b = Buffer::new("foo.bar  baz");
        b.word_right(false);
        assert_eq!(at(&b), (0, 3));
        b.word_right(false);
        assert_eq!(at(&b), (0, 4));
        b.word_right(false);
        assert_eq!(at(&b), (0, 7));
        b.word_right(false);
        assert_eq!(at(&b), (0, 12));
        b.word_left(false);
        assert_eq!(at(&b), (0, 9));
        b.word_left(false);
        assert_eq!(at(&b), (0, 4));
        b.word_left(false);
        assert_eq!(at(&b), (0, 3));
    }

    #[test]
    fn display_columns_expand_tabs_and_wide_chars() {
        assert_eq!(display_line("\tx\u{7}日"), "    x\u{FFFD}日");
        assert_eq!(display_line("ab\tc"), "ab  c");
        let line = "a\t日b";
        assert_eq!(display_col(line, 1), 1);
        assert_eq!(display_col(line, 2), 4);
        assert_eq!(display_col(line, 5), 6);
        assert_eq!(byte_at_col(line, 2), 1); // inside the tab
        assert_eq!(byte_at_col(line, 5), 2); // inside 日
        assert_eq!(byte_at_col(line, 6), 5);
        assert_eq!(byte_at_col(line, 99), line.len());
    }

    #[test]
    fn click_clamps_and_extends() {
        let mut b = Buffer::new("abc\nde");
        b.click(5, 9, false);
        assert_eq!(at(&b), (1, 2));
        b.click(0, 1, true);
        assert_eq!(b.selected_text().as_deref(), Some("bc\nde"));
    }

    #[test]
    fn changed_from_reports_the_first_touched_line() {
        let mut b = Buffer::new("a\nb\nc");
        assert_eq!(b.take_changed_from(), None);
        b.vertical(2, false);
        b.type_char('x');
        b.doc_start(false);
        b.vertical(1, false);
        b.type_char('y');
        assert_eq!(b.take_changed_from(), Some(1));
        assert_eq!(b.take_changed_from(), None);
    }
}
