//! A text editor widget: single- or multi-line, selection, undo/redo,
//! clipboard, mouse placement, `{{var}}` and JSON highlighting, and a vim
//! subset (normal / insert / visual / visual-line).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use unicode_width::UnicodeWidthChar;

use crate::theme::Theme;

mod conventional;
mod render;
mod vim;
pub use render::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimMode {
    Normal,
    Insert,
    Visual,
    VisualLine,
}

impl VimMode {
    pub fn label(self) -> &'static str {
        match self {
            VimMode::Normal => "NORMAL",
            VimMode::Insert => "INSERT",
            VimMode::Visual => "VISUAL",
            VimMode::VisualLine => "V-LINE",
        }
    }
}

/// Shared clipboard: internal register plus the text the UI sends to the
/// system clipboard (OSC 52 and the local clipboard program).
#[derive(Debug, Default, Clone)]
pub struct Clipboard {
    pub text: String,
    pub linewise: bool,
    /// Text to send to the system clipboard on the next flush.
    pub pending: Option<String>,
}

impl Clipboard {
    pub fn set(&mut self, text: String, linewise: bool) {
        self.pending = Some(text.clone());
        self.text = text;
        self.linewise = linewise;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The key was consumed; text unchanged.
    Handled,
    /// The key changed the text.
    Changed,
    /// The editor does not use this key.
    Ignored,
    /// Enter in a single-line editor.
    Submit,
    /// Esc pressed where it means "leave the editor".
    Leave,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Pos {
    pub row: usize,
    pub col: usize,
}

#[derive(Debug, Clone)]
struct Snapshot {
    lines: Vec<String>,
    cursor: Pos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Highlight {
    None,
    Json,
    /// Scheme and host dimmed, query keys and values colored.
    Url,
}

/// Undo steps a text keeps.
const UNDO_MAX_STEPS: usize = 200;
/// Bytes the undo steps of a text may hold together.
const UNDO_MAX_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct TextEditor {
    lines: Vec<String>,
    pub cursor: Pos,
    /// Selection anchor; the selection spans anchor..cursor.
    pub anchor: Option<Pos>,
    pub single_line: bool,
    pub read_only: bool,
    pub highlight: Highlight,
    pub placeholder: String,
    pub mask: bool,
    pub scroll_row: usize,
    pub scroll_col: usize,
    pub vim: VimMode,
    pending: Vec<char>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// Coalesces consecutive typing into one undo step.
    typing: bool,
    /// Whether the current coalesced step is a run of deletions.
    deleting: bool,
    preferred_col: Option<usize>,
    last_size: (u16, u16),
    /// Line numbers to the left of a multi-line editor.
    pub gutter: bool,
    /// Width of the gutter in the last frame (clicks are shifted by it).
    last_gutter: u16,
}

fn char_to_byte(s: &str, col: usize) -> usize {
    s.char_indices().nth(col).map(|(b, _)| b).unwrap_or(s.len())
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum CharClass {
    Space,
    Word,
    Punct,
}

fn class(c: char) -> CharClass {
    if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

impl TextEditor {
    pub fn new(text: &str, single_line: bool) -> Self {
        let mut e = Self {
            lines: vec![String::new()],
            cursor: Pos::default(),
            anchor: None,
            single_line,
            read_only: false,
            highlight: Highlight::None,
            placeholder: String::new(),
            mask: false,
            scroll_row: 0,
            scroll_col: 0,
            vim: VimMode::Normal,
            pending: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            typing: false,
            deleting: false,
            preferred_col: None,
            last_size: (0, 0),
            gutter: false,
            last_gutter: 0,
        };
        e.set_text(text);
        e
    }

    pub fn single(text: &str) -> Self {
        Self::new(text, true)
    }

    pub fn multi(text: &str) -> Self {
        Self::new(text, false)
    }

    pub fn with_placeholder(mut self, p: &str) -> Self {
        self.placeholder = p.to_string();
        self
    }

    pub fn with_highlight(mut self, h: Highlight) -> Self {
        self.highlight = h;
        self
    }

    /// The length of [`TextEditor::text`] in bytes, without joining the lines.
    pub fn byte_len(&self) -> usize {
        self.lines.iter().map(String::len).sum::<usize>() + self.lines.len().saturating_sub(1)
    }

    /// The text without its leading and trailing whitespace is `s`. Joins
    /// the lines only when the text holds no more than `s` outside whitespace.
    pub fn trimmed_is(&self, s: &str) -> bool {
        let content: usize = self.lines.iter().map(|l| l.trim().len()).sum();
        content <= s.len() && self.text().trim() == s
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Replaces the content, keeping the cursor in range. Clears history.
    pub fn set_text(&mut self, text: &str) {
        let t = if self.single_line {
            text.replace(['\r', '\n'], "")
        } else {
            text.replace("\r\n", "\n")
        };
        self.lines = t.split('\n').map(str::to_string).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.anchor = None;
        self.clamp();
        self.undo.clear();
        self.redo.clear();
        self.typing = false;
    }

    /// Replaces the content as an undoable edit.
    pub fn replace_all(&mut self, text: &str) {
        self.snapshot();
        let t = if self.single_line {
            text.replace(['\r', '\n'], "")
        } else {
            text.replace("\r\n", "\n")
        };
        self.lines = t.split('\n').map(str::to_string).collect();
        self.anchor = None;
        self.clamp();
    }

    fn clamp(&mut self) {
        if self.cursor.row >= self.lines.len() {
            self.cursor.row = self.lines.len() - 1;
        }
        let len = char_len(&self.lines[self.cursor.row]);
        if self.cursor.col > len {
            self.cursor.col = len;
        }
    }

    /// Moves the cursor to `pos` (kept inside the text) and drops the selection.
    pub fn move_to(&mut self, pos: Pos) {
        self.cursor = pos;
        self.anchor = None;
        self.clamp();
    }

    pub fn move_to_end(&mut self) {
        self.cursor.row = self.lines.len() - 1;
        self.cursor.col = char_len(&self.lines[self.cursor.row]);
        self.anchor = None;
    }

    fn snapshot(&mut self) {
        self.undo.push(Snapshot {
            lines: self.lines.clone(),
            cursor: self.cursor,
        });
        // Each step copies the whole text: cap the steps and the bytes they
        // hold (the newest step always stays).
        let size = |s: &Snapshot| s.lines.iter().map(String::len).sum::<usize>();
        let mut total: usize = self.undo.iter().map(size).sum();
        let drop = self
            .undo
            .iter()
            .take_while(|s| {
                let over = total > UNDO_MAX_BYTES;
                total -= size(s);
                over
            })
            .count()
            .max(self.undo.len().saturating_sub(UNDO_MAX_STEPS))
            .min(self.undo.len() - 1);
        self.undo.drain(..drop);
        self.redo.clear();
        self.typing = false;
    }

    pub fn undo(&mut self) -> bool {
        if let Some(s) = self.undo.pop() {
            self.redo.push(Snapshot {
                lines: std::mem::take(&mut self.lines),
                cursor: self.cursor,
            });
            self.lines = s.lines;
            self.cursor = s.cursor;
            self.anchor = None;
            self.typing = false;
            self.clamp();
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self) -> bool {
        if let Some(s) = self.redo.pop() {
            self.undo.push(Snapshot {
                lines: std::mem::take(&mut self.lines),
                cursor: self.cursor,
            });
            self.lines = s.lines;
            self.cursor = s.cursor;
            self.anchor = None;
            self.typing = false;
            self.clamp();
            true
        } else {
            false
        }
    }

    /// Ordered selection bounds, if any non-empty selection exists.
    pub fn selection(&self) -> Option<(Pos, Pos)> {
        let a = self.anchor?;
        if self.vim == VimMode::VisualLine {
            let (s, e) = if a <= self.cursor {
                (a, self.cursor)
            } else {
                (self.cursor, a)
            };
            return Some((
                Pos { row: s.row, col: 0 },
                Pos {
                    row: e.row,
                    col: char_len(&self.lines[e.row]),
                },
            ));
        }
        if a == self.cursor && self.vim != VimMode::Visual {
            return None;
        }
        let (s, mut e) = if a <= self.cursor {
            (a, self.cursor)
        } else {
            (self.cursor, a)
        };
        if self.vim == VimMode::Visual {
            // Visual mode includes the character under the cursor.
            e.col = (e.col + 1).min(char_len(&self.lines[e.row]));
        }
        Some((s, e))
    }

    pub fn selected_text(&self) -> Option<String> {
        let (s, e) = self.selection()?;
        Some(self.range_text(s, e))
    }

    fn range_text(&self, s: Pos, e: Pos) -> String {
        if s.row == e.row {
            let l = &self.lines[s.row];
            return l[char_to_byte(l, s.col)..char_to_byte(l, e.col)].to_string();
        }
        let mut out = String::new();
        let first = &self.lines[s.row];
        out.push_str(&first[char_to_byte(first, s.col)..]);
        for line in &self.lines[s.row + 1..e.row] {
            out.push('\n');
            out.push_str(line);
        }
        out.push('\n');
        let last = &self.lines[e.row];
        out.push_str(&last[..char_to_byte(last, e.col)]);
        out
    }

    fn delete_range(&mut self, s: Pos, e: Pos) {
        let tail = {
            let last = &self.lines[e.row];
            last[char_to_byte(last, e.col)..].to_string()
        };
        let head = {
            let first = &self.lines[s.row];
            first[..char_to_byte(first, s.col)].to_string()
        };
        self.lines.drain(s.row + 1..=e.row);
        self.lines[s.row] = head + &tail;
        self.cursor = s;
        self.anchor = None;
    }

    fn delete_selection(&mut self) -> bool {
        if let Some((s, e)) = self.selection() {
            self.delete_range(s, e);
            true
        } else {
            false
        }
    }

    /// Inserts text at the cursor (replacing the selection).
    pub fn insert_str(&mut self, text: &str) {
        if self.read_only {
            return;
        }
        let text = self.normalize(text);
        self.snapshot();
        self.delete_selection();
        self.put_at_cursor(&text);
    }

    /// Folds newlines to spaces in single-line editors (and CRLF to LF otherwise).
    pub fn normalize(&self, text: &str) -> String {
        if self.single_line {
            text.replace("\r\n", " ").replace(['\r', '\n'], " ")
        } else {
            text.replace("\r\n", "\n").replace('\r', "\n")
        }
    }

    fn put_at_cursor(&mut self, text: &str) {
        let line = &self.lines[self.cursor.row];
        let b = char_to_byte(line, self.cursor.col);
        let tail = line[b..].to_string();
        let head = line[..b].to_string();
        let parts: Vec<&str> = text.split('\n').collect();
        if parts.len() == 1 {
            self.lines[self.cursor.row] = format!("{head}{text}{tail}");
            self.cursor.col += char_len(text);
        } else {
            self.lines[self.cursor.row] = format!("{head}{}", parts[0]);
            let mut row = self.cursor.row;
            for p in &parts[1..parts.len() - 1] {
                row += 1;
                self.lines.insert(row, p.to_string());
            }
            row += 1;
            let last = parts[parts.len() - 1];
            self.lines.insert(row, format!("{last}{tail}"));
            self.cursor = Pos {
                row,
                col: char_len(last),
            };
        }
        self.preferred_col = None;
    }

    /// Replaces the text between two positions, as one undo step.
    pub fn replace_span(&mut self, start: Pos, end: Pos, text: &str) {
        if self.read_only || start > end {
            return;
        }
        let end = Pos {
            row: end.row.min(self.lines.len().saturating_sub(1)),
            col: end.col,
        };
        let start = Pos {
            row: start.row.min(end.row),
            col: start.col,
        };
        let text = self.normalize(text);
        self.snapshot();
        self.delete_range(start, end);
        self.put_at_cursor(&text);
    }

    fn type_char(&mut self, c: char) {
        if self.read_only {
            return;
        }
        if !self.typing || self.deleting || self.anchor.is_some() {
            self.snapshot();
            self.typing = true;
            self.deleting = false;
        }
        self.delete_selection();
        let line = &mut self.lines[self.cursor.row];
        // A closing bracket typed on an indentation-only line dedents it one level.
        if !self.single_line && matches!(c, '}' | ']') && self.cursor.col >= 2 {
            let before: String = line.chars().take(self.cursor.col).collect();
            if before.chars().all(|ch| ch == ' ') {
                let b = char_to_byte(line, self.cursor.col);
                line.replace_range(b - 2..b, "");
                self.cursor.col -= 2;
            }
        }
        let b = char_to_byte(line, self.cursor.col);
        line.insert(b, c);
        self.cursor.col += 1;
        self.preferred_col = None;
    }

    fn backspace(&mut self) -> bool {
        if self.read_only {
            return false;
        }
        if self.anchor.is_some() && self.selection().is_some() {
            self.snapshot();
            return self.delete_selection();
        }
        self.anchor = None;
        if self.cursor.col > 0 {
            if !self.typing || !self.deleting {
                self.snapshot();
                self.typing = true;
                self.deleting = true;
            }
            let line = &mut self.lines[self.cursor.row];
            let b = char_to_byte(line, self.cursor.col - 1);
            line.remove(b);
            self.cursor.col -= 1;
            true
        } else if self.cursor.row > 0 {
            self.snapshot();
            let line = self.lines.remove(self.cursor.row);
            self.cursor.row -= 1;
            self.cursor.col = char_len(&self.lines[self.cursor.row]);
            self.lines[self.cursor.row].push_str(&line);
            true
        } else {
            false
        }
    }

    fn delete_forward(&mut self) -> bool {
        if self.read_only {
            return false;
        }
        if self.anchor.is_some() && self.selection().is_some() {
            self.snapshot();
            return self.delete_selection();
        }
        self.anchor = None;
        let len = char_len(&self.lines[self.cursor.row]);
        if self.cursor.col < len {
            self.snapshot();
            let line = &mut self.lines[self.cursor.row];
            let b = char_to_byte(line, self.cursor.col);
            line.remove(b);
            true
        } else if self.cursor.row + 1 < self.lines.len() {
            self.snapshot();
            let next = self.lines.remove(self.cursor.row + 1);
            self.lines[self.cursor.row].push_str(&next);
            true
        } else {
            false
        }
    }

    fn newline(&mut self) {
        if self.read_only {
            return;
        }
        // Keep the indentation of the current line.
        let indent: String = self.lines[self.cursor.row]
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        let indent = indent.chars().take(self.cursor.col).collect::<String>();
        self.insert_str(&format!("\n{indent}"));
    }

    fn char_at(&self, p: Pos) -> Option<char> {
        self.lines.get(p.row)?.chars().nth(p.col)
    }

    /// The `{{placeholder}}` around `p` as a char range on its line: word
    /// motions treat a placeholder as one word, so `dw` removes it whole.
    fn var_span_at(&self, p: Pos) -> Option<(usize, usize)> {
        let line = self.lines.get(p.row)?;
        if !line.contains("{{") {
            return None;
        }
        sendito_core::template::spans(line)
            .into_iter()
            .map(|(r, _)| {
                let s = line[..r.start].chars().count();
                (s, s + line[r].chars().count())
            })
            .find(|(s, e)| p.col >= *s && p.col < *e)
    }

    fn word_forward(&self, p: Pos, end: bool) -> Pos {
        if !end && let Some((_, e)) = self.var_span_at(p) {
            // Past the placeholder, then past the spaces after it.
            let mut q = Pos { row: p.row, col: e };
            while self.char_at(q).is_some_and(char::is_whitespace) {
                q.col += 1;
            }
            return q;
        }
        let found = self.word_forward_plain(p, end);
        if end && let Some((_, e)) = self.var_span_at(found) {
            return Pos {
                row: found.row,
                col: e - 1,
            };
        }
        found
    }

    fn word_forward_plain(&self, p: Pos, end: bool) -> Pos {
        let mut p = p;
        let total = self.lines.len();
        let line_len = |r: usize| char_len(&self.lines[r]);
        let advance = |p: &mut Pos| -> bool {
            if p.col < line_len(p.row) {
                p.col += 1;
                true
            } else if p.row + 1 < total {
                p.row += 1;
                p.col = 0;
                true
            } else {
                false
            }
        };
        if end {
            if !advance(&mut p) {
                return p;
            }
            while self.char_at(p).is_none_or(|c| c.is_whitespace()) {
                if !advance(&mut p) {
                    return p;
                }
            }
            let cls = self.char_at(p).map(class);
            loop {
                let mut n = p;
                if !advance(&mut n) || n.row != p.row || self.char_at(n).map(class) != cls {
                    return p;
                }
                p = n;
            }
        }
        let start_cls = self.char_at(p).map(class);
        let start_row = p.row;
        while p.row == start_row && self.char_at(p).map(class) == start_cls && start_cls.is_some() {
            if !advance(&mut p) {
                return p;
            }
        }
        while self.char_at(p).is_none_or(|c| c.is_whitespace()) {
            if p.row != start_row && p.col == 0 && line_len(p.row) == 0 {
                return p;
            }
            if !advance(&mut p) {
                return p;
            }
        }
        p
    }

    fn word_backward(&self, p: Pos) -> Pos {
        let found = self.word_backward_plain(p);
        match self.var_span_at(found) {
            Some((s, _)) => Pos {
                row: found.row,
                col: s,
            },
            None => found,
        }
    }

    fn word_backward_plain(&self, p: Pos) -> Pos {
        let mut p = p;
        let retreat = |p: &mut Pos| -> bool {
            if p.col > 0 {
                p.col -= 1;
                true
            } else if p.row > 0 {
                p.row -= 1;
                p.col = char_len(&self.lines[p.row]);
                true
            } else {
                false
            }
        };
        if !retreat(&mut p) {
            return p;
        }
        while self.char_at(p).is_none_or(|c| c.is_whitespace()) {
            if !retreat(&mut p) {
                return p;
            }
        }
        let cls = self.char_at(p).map(class);
        loop {
            if p.col == 0 {
                return p;
            }
            let n = Pos {
                row: p.row,
                col: p.col - 1,
            };
            if self.char_at(n).map(class) != cls {
                return p;
            }
            p = n;
        }
    }

    fn move_cursor(&mut self, to: Pos, extend: bool) {
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else if !matches!(self.vim, VimMode::Visual | VimMode::VisualLine) {
            self.anchor = None;
        }
        self.cursor = to;
        self.clamp();
        self.typing = false;
    }

    fn vertical(&mut self, delta: isize, extend: bool) -> bool {
        let target = self.cursor.row as isize + delta;
        if target < 0 || target >= self.lines.len() as isize {
            if !extend
                && self.anchor.is_some()
                && !matches!(self.vim, VimMode::Visual | VimMode::VisualLine)
            {
                self.anchor = None;
            }
            let to = if target < 0 { 0 } else { self.lines.len() - 1 };
            if to == self.cursor.row {
                return false;
            }
        }
        let row = target.clamp(0, self.lines.len() as isize - 1) as usize;
        let want = self.preferred_col.unwrap_or(self.cursor.col);
        let col = want.min(char_len(&self.lines[row]));
        self.move_cursor(Pos { row, col }, extend);
        self.preferred_col = Some(want);
        true
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(Pos::default());
        self.move_to_end_keep_anchor();
    }

    fn move_to_end_keep_anchor(&mut self) {
        self.cursor.row = self.lines.len() - 1;
        self.cursor.col = char_len(&self.lines[self.cursor.row]);
    }

    pub fn copy(&self, cb: &mut Clipboard) {
        let text = self.selected_text().unwrap_or_else(|| self.text());
        cb.set(text, false);
    }

    /// Handles a key. `vim` selects modal editing.
    pub fn handle_key(&mut self, key: &KeyEvent, vim: bool, cb: &mut Clipboard) -> Outcome {
        if vim && self.vim != VimMode::Insert {
            return self.handle_vim(key, cb);
        }
        self.handle_conventional(key, vim, cb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_steps_of_a_large_text_stay_under_the_byte_cap() {
        let big = "x".repeat(UNDO_MAX_BYTES / 4);
        let mut e = TextEditor::multi(&big);
        for _ in 0..10 {
            e.snapshot();
        }
        assert!(e.undo.len() <= 4, "{}", e.undo.len());
        assert!(e.undo());
        let mut small = TextEditor::multi("a");
        for _ in 0..(UNDO_MAX_STEPS + 20) {
            small.snapshot();
        }
        assert_eq!(small.undo.len(), UNDO_MAX_STEPS);
    }

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn kc(c: char) -> KeyEvent {
        k(KeyCode::Char(c))
    }
    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }
    fn shift(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }
    fn typ(e: &mut TextEditor, s: &str, vim: bool, cb: &mut Clipboard) {
        for c in s.chars() {
            if c == '\n' {
                e.handle_key(&k(KeyCode::Enter), vim, cb);
            } else if c == '\x1b' {
                e.handle_key(&k(KeyCode::Esc), vim, cb);
            } else {
                e.handle_key(&kc(c), vim, cb);
            }
        }
    }

    #[test]
    fn conventional_editing() {
        let mut cb = Clipboard::default();
        let mut e = TextEditor::multi("");
        typ(&mut e, "{\n  \"a\": 1\n}", false, &mut cb);
        assert_eq!(e.text(), "{\n  \"a\": 1\n}");
        e.handle_key(&k(KeyCode::Backspace), false, &mut cb);
        assert_eq!(e.text(), "{\n  \"a\": 1\n");
        assert_eq!(e.handle_key(&ctrl('z'), false, &mut cb), Outcome::Changed);
        assert_eq!(e.text(), "{\n  \"a\": 1\n}");
        e.handle_key(&ctrl('y'), false, &mut cb);
        assert_eq!(e.text(), "{\n  \"a\": 1\n");
        e.handle_key(&ctrl('a'), false, &mut cb);
        e.handle_key(&ctrl('c'), false, &mut cb);
        assert_eq!(cb.text, "{\n  \"a\": 1\n");
        assert!(cb.pending.is_some());
        e.handle_key(&ctrl('x'), false, &mut cb);
        assert_eq!(e.text(), "");
        e.handle_key(&ctrl('v'), false, &mut cb);
        assert_eq!(e.text(), "{\n  \"a\": 1\n");
        e.handle_key(&ctrl('z'), false, &mut cb);
        e.handle_key(&ctrl('z'), false, &mut cb);
        assert_eq!(e.text(), "{\n  \"a\": 1\n");
        let mut e = TextEditor::single("hello world");
        e.handle_key(&k(KeyCode::Home), false, &mut cb);
        e.handle_key(&shift(KeyCode::End), false, &mut cb);
        assert_eq!(e.selected_text().as_deref(), Some("hello world"));
        e.handle_key(&kc('X'), false, &mut cb);
        assert_eq!(e.text(), "X");
        assert_eq!(
            e.handle_key(&k(KeyCode::Enter), false, &mut cb),
            Outcome::Submit
        );
        assert_eq!(
            e.handle_key(&k(KeyCode::Up), false, &mut cb),
            Outcome::Ignored
        );
        assert_eq!(
            e.handle_key(&k(KeyCode::Esc), false, &mut cb),
            Outcome::Leave
        );
        e.insert_str("a\nb");
        assert_eq!(e.text(), "Xa b");
        e.handle_key(
            &KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL),
            false,
            &mut cb,
        );
        assert_eq!(e.cursor.col, 3);
        e.handle_key(&k(KeyCode::Delete), false, &mut cb);
        assert_eq!(e.text(), "Xa ");
    }

    #[test]
    fn multiline_navigation() {
        let mut cb = Clipboard::default();
        let mut e = TextEditor::multi("abc\nd\nefgh");
        e.handle_key(&k(KeyCode::End), false, &mut cb);
        e.handle_key(&k(KeyCode::Down), false, &mut cb);
        assert_eq!(e.cursor, Pos { row: 1, col: 1 });
        e.handle_key(&k(KeyCode::Down), false, &mut cb);
        assert_eq!(e.cursor, Pos { row: 2, col: 3 });
        assert_eq!(
            e.handle_key(&k(KeyCode::Down), false, &mut cb),
            Outcome::Ignored
        );
        e.handle_key(
            &KeyEvent::new(KeyCode::Home, KeyModifiers::CONTROL),
            false,
            &mut cb,
        );
        assert_eq!(e.cursor, Pos::default());
        e.handle_key(&k(KeyCode::Right), false, &mut cb);
        e.handle_key(&k(KeyCode::Right), false, &mut cb);
        e.handle_key(&k(KeyCode::Right), false, &mut cb);
        e.handle_key(&k(KeyCode::Right), false, &mut cb);
        assert_eq!(e.cursor, Pos { row: 1, col: 0 });
        e.handle_key(&k(KeyCode::Backspace), false, &mut cb);
        assert_eq!(e.text(), "abcd\nefgh");
        e.handle_key(&k(KeyCode::Tab), false, &mut cb);
        assert_eq!(e.text(), "abc  d\nefgh");
        e.handle_key(&k(KeyCode::End), false, &mut cb);
        e.handle_key(&k(KeyCode::Delete), false, &mut cb);
        assert_eq!(e.text(), "abc  defgh");
        e.click(1, 0, false);
        assert_eq!(e.cursor.col, 1);
        e.click(3, 0, true);
        assert_eq!(e.selected_text().as_deref(), Some("bc"));
        e.handle_key(&k(KeyCode::Left), false, &mut cb);
        assert_eq!(e.cursor.col, 1);
        e.scroll_by(5);
        assert_eq!(e.scroll_row, 0);
    }

    #[test]
    fn vim_word_motions_keep_placeholders_whole() {
        let mut cb = Clipboard::default();
        let mut e = TextEditor::single("{{base_url}}/users");
        e.vim = VimMode::Normal;
        typ(&mut e, "0dw", true, &mut cb);
        assert_eq!(e.lines()[0], "/users");
        let mut e = TextEditor::single("x {{base_url}}/y");
        e.vim = VimMode::Normal;
        typ(&mut e, "$b", true, &mut cb);
        assert_eq!(e.cursor.col, 2);
        typ(&mut e, "e", true, &mut cb);
        assert_eq!(e.cursor.col, 13);
    }

    #[test]
    fn vim_basics() {
        let mut cb = Clipboard::default();
        let mut e = TextEditor::multi("one two three\nsecond line\nthird");
        e.vim = VimMode::Normal;
        typ(&mut e, "w", true, &mut cb);
        assert_eq!(e.cursor.col, 4);
        typ(&mut e, "dw", true, &mut cb);
        assert_eq!(e.lines()[0], "one three");
        typ(&mut e, "u", true, &mut cb);
        assert_eq!(e.lines()[0], "one two three");
        typ(&mut e, "$", true, &mut cb);
        assert_eq!(e.cursor.col, 12);
        typ(&mut e, "0x", true, &mut cb);
        assert_eq!(e.lines()[0], "ne two three");
        typ(&mut e, "jdd", true, &mut cb);
        assert_eq!(
            e.lines(),
            &["ne two three".to_string(), "third".to_string()]
        );
        assert!(cb.linewise);
        typ(&mut e, "P", true, &mut cb);
        assert_eq!(e.lines()[1], "second line");
        typ(&mut e, "ggyyGp", true, &mut cb);
        assert_eq!(e.lines()[3], "ne two three");
        typ(&mut e, "ggA!\x1b", true, &mut cb);
        assert_eq!(e.lines()[0], "ne two three!");
        assert_eq!(e.vim, VimMode::Normal);
        typ(&mut e, "Iab\x1b", true, &mut cb);
        assert_eq!(e.lines()[0], "abne two three!");
        typ(&mut e, "ociao\x1b", true, &mut cb);
        assert_eq!(e.lines()[1], "ciao");
        typ(&mut e, "Ofirst\x1b", true, &mut cb);
        assert_eq!(e.lines()[1], "first");
        typ(&mut e, "ccnew\x1b", true, &mut cb);
        assert_eq!(e.lines()[1], "new");
        typ(&mut e, "0vly", true, &mut cb);
        assert_eq!(cb.text, "ne");
        typ(&mut e, "Vd", true, &mut cb);
        assert_eq!(e.lines()[1], "ciao");
        typ(&mut e, "0cwhey\x1b", true, &mut cb);
        assert_eq!(e.lines()[1], "hey");
        typ(&mut e, "0D", true, &mut cb);
        assert_eq!(e.lines()[1], "");
        e.handle_key(&ctrl('r'), true, &mut cb);
        assert_eq!(
            e.handle_key(&k(KeyCode::Esc), true, &mut cb),
            Outcome::Leave
        );
        typ(&mut e, "ggwbe", true, &mut cb);
        assert_eq!(e.cursor.col, 3);
        typ(&mut e, "a", true, &mut cb);
        assert_eq!(e.vim, VimMode::Insert);
    }

    #[test]
    fn vim_single_line() {
        let mut cb = Clipboard::default();
        let mut e = TextEditor::single("http://x");
        e.vim = VimMode::Normal;
        assert_eq!(e.handle_key(&kc('j'), true, &mut cb), Outcome::Ignored);
        assert_eq!(
            e.handle_key(&k(KeyCode::Enter), true, &mut cb),
            Outcome::Submit
        );
        typ(&mut e, "$ay\x1b", true, &mut cb);
        assert_eq!(e.text(), "http://xy");
        typ(&mut e, "0dw", true, &mut cb);
        assert_eq!(e.text(), "://xy");
        typ(&mut e, "Xp", true, &mut cb);
        e.read_only = true;
        typ(&mut e, "dd", true, &mut cb);
        assert!(!e.text().is_empty());
    }

    #[test]
    fn render_and_highlight() {
        let theme = Theme::dusk();
        let mut e = TextEditor::multi("{\"a\": \"{{x}}\", \"n\": 1, \"t\": true}\nline2")
            .with_highlight(Highlight::Json);
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        let pos = e.render(area, &mut buf, &theme, true, &|n| n == "x");
        assert_eq!(pos, Some((0, 0)));
        e.move_to_end();
        let pos = e.render(area, &mut buf, &theme, true, &|_| false);
        assert_eq!(pos, Some((5, 0)));
        assert_eq!(e.scroll_row, 1);
        let spans = json_line_spans("{\"k\": \"v\", \"n\": -1.5e3, \"b\": null}", &theme);
        assert_eq!(spans.len(), 6);
        let mut p = TextEditor::single("").with_placeholder("Enter URL");
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 1));
        p.render(Rect::new(0, 0, 20, 1), &mut buf, &theme, false, &|_| true);
        assert_eq!(buf[(0, 0)].symbol(), "E");
        let mut m = TextEditor::single("secret");
        m.mask = true;
        m.render(Rect::new(0, 0, 20, 1), &mut buf, &theme, false, &|_| true);
        assert_eq!(buf[(0, 0)].symbol(), "•");
    }

    #[test]
    fn url_spans_and_gutter() {
        let theme = Theme::dusk();
        let spans = url_line_spans("https://api.test/v1/users?page=2&q#top", &theme);
        let at = |c: usize| {
            spans
                .iter()
                .rev()
                .find(|(s, e, _)| *s <= c && c < *e)
                .map(|x| x.2)
        };
        assert_eq!(at(0), Some(theme.dim()));
        assert_eq!(at(26), Some(theme.fg(theme.json_key)));
        assert_eq!(at(31), Some(theme.fg(theme.json_str)));
        assert_eq!(at(33), Some(theme.fg(theme.json_key)));
        assert_eq!(at(35), Some(theme.dim()));
        assert!(url_line_spans("{{base}}/users", &theme).is_empty());
        // Line numbers shift the text; a click lands on the right column.
        let mut e = TextEditor::multi("abc\ndef");
        e.gutter = true;
        let area = Rect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        let pos = e.render(area, &mut buf, &theme, true, &|_| true);
        assert_eq!(pos, Some((4, 0)));
        assert_eq!(buf[(2, 1)].symbol(), "2");
        assert_eq!(buf[(4, 1)].symbol(), "d");
        e.click(6, 1, false);
        assert_eq!(e.cursor, Pos { row: 1, col: 2 });
    }
}
