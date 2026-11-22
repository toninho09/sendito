//! A text editor widget: single- or multi-line, selection, undo/redo,
//! clipboard, mouse placement, `{{var}}` and JSON highlighting, and a vim
//! subset (normal / insert / visual / visual-line).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use unicode_width::UnicodeWidthChar;

use crate::theme::Theme;

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

/// Shared clipboard: internal register plus pending OSC 52 writes.
#[derive(Debug, Default, Clone)]
pub struct Clipboard {
    pub text: String,
    pub linewise: bool,
    /// Text to send to the terminal clipboard (OSC 52) on the next flush.
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
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
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
        let text = if self.single_line {
            text.replace("\r\n", " ").replace(['\r', '\n'], " ")
        } else {
            text.replace("\r\n", "\n").replace('\r', "\n")
        };
        self.snapshot();
        self.delete_selection();
        let line = &self.lines[self.cursor.row];
        let b = char_to_byte(line, self.cursor.col);
        let tail = line[b..].to_string();
        let head = line[..b].to_string();
        let parts: Vec<&str> = text.split('\n').collect();
        if parts.len() == 1 {
            self.lines[self.cursor.row] = format!("{head}{text}{tail}");
            self.cursor.col += char_len(&text);
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

    fn handle_conventional(&mut self, key: &KeyEvent, vim: bool, cb: &mut Clipboard) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => {
                if vim {
                    self.vim = VimMode::Normal;
                    self.anchor = None;
                    self.typing = false;
                    if self.cursor.col > 0 {
                        self.cursor.col -= 1;
                    }
                    return Outcome::Handled;
                }
                if self.anchor.is_some() {
                    self.anchor = None;
                    return Outcome::Handled;
                }
                Outcome::Leave
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                if self.read_only {
                    return Outcome::Ignored;
                }
                self.type_char(c);
                Outcome::Changed
            }
            KeyCode::Char('z') if ctrl && !vim => {
                if self.undo() {
                    Outcome::Changed
                } else {
                    Outcome::Handled
                }
            }
            KeyCode::Char('y') if ctrl && !vim => {
                if self.redo() {
                    Outcome::Changed
                } else {
                    Outcome::Handled
                }
            }
            KeyCode::Char('a') if ctrl && !vim => {
                self.select_all();
                Outcome::Handled
            }
            KeyCode::Char('c') if ctrl && !vim => {
                self.copy(cb);
                Outcome::Handled
            }
            KeyCode::Char('x') if ctrl && !vim => {
                if self.read_only {
                    self.copy(cb);
                    return Outcome::Handled;
                }
                if self.selection().is_none() {
                    self.select_all();
                }
                self.copy(cb);
                self.snapshot();
                self.delete_selection();
                Outcome::Changed
            }
            KeyCode::Char('v') if ctrl && !vim => {
                if self.read_only || cb.text.is_empty() {
                    return Outcome::Handled;
                }
                let t = cb.text.clone();
                self.insert_str(&t);
                Outcome::Changed
            }
            KeyCode::Char('w') if ctrl && vim => Outcome::Ignored,
            KeyCode::Enter if !ctrl && !alt => {
                if self.single_line {
                    return Outcome::Submit;
                }
                if self.read_only {
                    return Outcome::Ignored;
                }
                self.newline();
                Outcome::Changed
            }
            KeyCode::Tab if !self.single_line && !shift && !ctrl => {
                if self.read_only {
                    return Outcome::Ignored;
                }
                self.insert_str("  ");
                Outcome::Changed
            }
            KeyCode::Backspace => {
                if self.backspace() {
                    Outcome::Changed
                } else {
                    Outcome::Handled
                }
            }
            KeyCode::Delete => {
                if self.delete_forward() {
                    Outcome::Changed
                } else {
                    Outcome::Handled
                }
            }
            KeyCode::Left => {
                if self.anchor.is_some() && !shift && self.selection().is_some() {
                    let (s, _) = self.selection().unwrap_or_default();
                    self.move_cursor(s, false);
                    return Outcome::Handled;
                }
                let to = if ctrl || alt {
                    self.word_backward(self.cursor)
                } else if self.cursor.col > 0 {
                    Pos {
                        col: self.cursor.col - 1,
                        ..self.cursor
                    }
                } else if self.cursor.row > 0 {
                    Pos {
                        row: self.cursor.row - 1,
                        col: char_len(&self.lines[self.cursor.row - 1]),
                    }
                } else {
                    self.cursor
                };
                self.move_cursor(to, shift);
                self.preferred_col = None;
                Outcome::Handled
            }
            KeyCode::Right => {
                if self.anchor.is_some() && !shift && self.selection().is_some() {
                    let (_, e) = self.selection().unwrap_or_default();
                    self.move_cursor(e, false);
                    return Outcome::Handled;
                }
                let len = char_len(&self.lines[self.cursor.row]);
                let to = if ctrl || alt {
                    self.word_forward(self.cursor, false)
                } else if self.cursor.col < len {
                    Pos {
                        col: self.cursor.col + 1,
                        ..self.cursor
                    }
                } else if self.cursor.row + 1 < self.lines.len() {
                    Pos {
                        row: self.cursor.row + 1,
                        col: 0,
                    }
                } else {
                    self.cursor
                };
                self.move_cursor(to, shift);
                self.preferred_col = None;
                Outcome::Handled
            }
            KeyCode::Up => {
                if self.single_line {
                    return Outcome::Ignored;
                }
                if self.vertical(-1, shift) {
                    Outcome::Handled
                } else {
                    Outcome::Ignored
                }
            }
            KeyCode::Down => {
                if self.single_line {
                    return Outcome::Ignored;
                }
                if self.vertical(1, shift) {
                    Outcome::Handled
                } else {
                    Outcome::Ignored
                }
            }
            KeyCode::PageUp if !self.single_line => {
                let h = self.last_size.1.max(2) as isize - 1;
                self.vertical(-h, shift);
                Outcome::Handled
            }
            KeyCode::PageDown if !self.single_line => {
                let h = self.last_size.1.max(2) as isize - 1;
                self.vertical(h, shift);
                Outcome::Handled
            }
            KeyCode::Home => {
                let to = if ctrl {
                    Pos::default()
                } else {
                    Pos {
                        col: 0,
                        ..self.cursor
                    }
                };
                self.move_cursor(to, shift);
                self.preferred_col = None;
                Outcome::Handled
            }
            KeyCode::End => {
                let to = if ctrl {
                    let row = self.lines.len() - 1;
                    Pos {
                        row,
                        col: char_len(&self.lines[row]),
                    }
                } else {
                    Pos {
                        col: char_len(&self.lines[self.cursor.row]),
                        ..self.cursor
                    }
                };
                self.move_cursor(to, shift);
                self.preferred_col = None;
                Outcome::Handled
            }
            _ => Outcome::Ignored,
        }
    }

    fn line_first_nonblank(&self, row: usize) -> usize {
        self.lines[row]
            .chars()
            .take_while(|c| c.is_whitespace())
            .count()
    }

    /// Resolves a vim motion key. Returns (target, linewise, inclusive).
    fn motion(&mut self, keys: &[char]) -> Option<(Pos, bool, bool)> {
        let c = self.cursor;
        let last_col = |r: usize| char_len(&self.lines[r]).saturating_sub(1);
        Some(match keys {
            ['h'] => (
                Pos {
                    col: c.col.saturating_sub(1),
                    ..c
                },
                false,
                false,
            ),
            ['l'] => (
                Pos {
                    col: (c.col + 1).min(char_len(&self.lines[c.row])),
                    ..c
                },
                false,
                false,
            ),
            ['j'] if !self.single_line => (
                Pos {
                    row: (c.row + 1).min(self.lines.len() - 1),
                    col: c.col,
                },
                true,
                true,
            ),
            ['k'] if !self.single_line => (
                Pos {
                    row: c.row.saturating_sub(1),
                    col: c.col,
                },
                true,
                true,
            ),
            ['w'] => (self.word_forward(c, false), false, false),
            ['e'] => (self.word_forward(c, true), false, true),
            ['b'] => (self.word_backward(c), false, false),
            ['0'] => (Pos { col: 0, ..c }, false, false),
            ['^'] => (
                Pos {
                    col: self.line_first_nonblank(c.row),
                    ..c
                },
                false,
                false,
            ),
            ['$'] => (
                Pos {
                    col: last_col(c.row),
                    ..c
                },
                false,
                true,
            ),
            ['g', 'g'] => (Pos { row: 0, col: 0 }, true, true),
            ['G'] => (
                Pos {
                    row: self.lines.len() - 1,
                    col: 0,
                },
                true,
                true,
            ),
            _ => return None,
        })
    }

    fn is_motion_prefix(keys: &[char]) -> bool {
        matches!(keys, ['g'])
    }

    fn normal_clamp(&mut self) {
        let len = char_len(&self.lines[self.cursor.row]);
        if self.vim == VimMode::Normal && len > 0 && self.cursor.col >= len {
            self.cursor.col = len - 1;
        }
    }

    fn enter_insert(&mut self) {
        self.vim = VimMode::Insert;
        self.anchor = None;
        self.snapshot();
        self.typing = true;
    }

    fn yank_lines(&self, s: usize, e: usize, cb: &mut Clipboard) {
        let mut t = self.lines[s..=e].join("\n");
        t.push('\n');
        cb.set(t, true);
    }

    fn delete_lines(&mut self, s: usize, e: usize) {
        self.lines.drain(s..=e);
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor.row = s.min(self.lines.len() - 1);
        self.cursor.col = self.line_first_nonblank(self.cursor.row);
    }

    fn apply_operator(
        &mut self,
        op: char,
        target: Pos,
        linewise: bool,
        inclusive: bool,
        cb: &mut Clipboard,
    ) -> Outcome {
        if linewise {
            let (s, e) = if target.row < self.cursor.row {
                (target.row, self.cursor.row)
            } else {
                (self.cursor.row, target.row)
            };
            self.yank_lines(s, e, cb);
            if op == 'y' {
                return Outcome::Handled;
            }
            if self.read_only {
                return Outcome::Handled;
            }
            self.snapshot();
            if op == 'c' {
                self.lines.drain(s + 1..=e);
                self.lines[s].clear();
                self.cursor = Pos { row: s, col: 0 };
                self.vim = VimMode::Insert;
                self.typing = true;
            } else {
                self.delete_lines(s, e);
            }
            return Outcome::Changed;
        }
        let (s, mut e) = if target < self.cursor {
            (target, self.cursor)
        } else {
            (self.cursor, target)
        };
        if inclusive {
            e.col = (e.col + 1).min(char_len(&self.lines[e.row]));
        }
        let text = self.range_text(s, e);
        cb.set(text, false);
        if op == 'y' {
            self.cursor = s;
            return Outcome::Handled;
        }
        if self.read_only {
            return Outcome::Handled;
        }
        self.snapshot();
        self.delete_range(s, e);
        if op == 'c' {
            self.vim = VimMode::Insert;
            self.typing = true;
        } else {
            self.normal_clamp();
        }
        Outcome::Changed
    }

    fn paste_vim(&mut self, before: bool, cb: &Clipboard) -> Outcome {
        if self.read_only || cb.text.is_empty() {
            return Outcome::Handled;
        }
        if cb.linewise && !self.single_line {
            self.snapshot();
            let text = cb.text.strip_suffix('\n').unwrap_or(&cb.text);
            let at = if before {
                self.cursor.row
            } else {
                self.cursor.row + 1
            };
            for (i, l) in text.split('\n').enumerate() {
                self.lines.insert(at + i, l.to_string());
            }
            self.cursor = Pos {
                row: at,
                col: self.line_first_nonblank(at),
            };
            return Outcome::Changed;
        }
        let text = cb.text.clone();
        if !before && !self.lines[self.cursor.row].is_empty() {
            self.cursor.col += 1;
        }
        self.insert_str(&text);
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
        }
        Outcome::Changed
    }

    fn handle_vim(&mut self, key: &KeyEvent, cb: &mut Clipboard) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Arrow keys and friends behave as motions.
        let code = match key.code {
            KeyCode::Left => KeyCode::Char('h'),
            KeyCode::Right => KeyCode::Char('l'),
            KeyCode::Up => KeyCode::Char('k'),
            KeyCode::Down => KeyCode::Char('j'),
            KeyCode::Home => KeyCode::Char('0'),
            KeyCode::End => KeyCode::Char('$'),
            other => other,
        };
        if matches!(key.code, KeyCode::Up | KeyCode::Down) && self.single_line {
            return Outcome::Ignored;
        }
        match code {
            KeyCode::Esc => {
                if !self.pending.is_empty() {
                    self.pending.clear();
                    return Outcome::Handled;
                }
                if matches!(self.vim, VimMode::Visual | VimMode::VisualLine) {
                    self.vim = VimMode::Normal;
                    self.anchor = None;
                    return Outcome::Handled;
                }
                Outcome::Leave
            }
            KeyCode::Enter => {
                if self.single_line {
                    Outcome::Submit
                } else {
                    self.vertical(1, false);
                    Outcome::Handled
                }
            }
            KeyCode::Char('r') if ctrl => {
                if self.redo() {
                    Outcome::Changed
                } else {
                    Outcome::Handled
                }
            }
            KeyCode::Char(_) if ctrl => Outcome::Ignored,
            KeyCode::Char(c) => {
                self.pending.push(c);
                let keys = self.pending.clone();
                let out = self.vim_keys(&keys, cb);
                match out {
                    None => Outcome::Handled,
                    Some(o) => {
                        self.pending.clear();
                        self.normal_clamp();
                        o
                    }
                }
            }
            KeyCode::Backspace => {
                let to = if self.cursor.col > 0 {
                    Pos {
                        col: self.cursor.col - 1,
                        ..self.cursor
                    }
                } else {
                    self.cursor
                };
                self.move_cursor(to, false);
                Outcome::Handled
            }
            _ => Outcome::Ignored,
        }
    }

    /// Returns `None` while a multi-key command is incomplete.
    fn vim_keys(&mut self, keys: &[char], cb: &mut Clipboard) -> Option<Outcome> {
        let visual = matches!(self.vim, VimMode::Visual | VimMode::VisualLine);
        if visual {
            match keys {
                ['d'] | ['x'] | ['c'] | ['y'] => {
                    let op = if keys[0] == 'x' { 'd' } else { keys[0] };
                    let linewise = self.vim == VimMode::VisualLine;
                    let sel = self.selection();
                    self.vim = VimMode::Normal;
                    self.anchor = None;
                    let Some((s, e)) = sel else {
                        return Some(Outcome::Handled);
                    };
                    if linewise {
                        self.cursor = s;
                        return Some(self.apply_operator(op, e, true, true, cb));
                    }
                    let text = self.range_text(s, e);
                    cb.set(text, false);
                    if op == 'y' || self.read_only {
                        self.cursor = s;
                        return Some(Outcome::Handled);
                    }
                    self.snapshot();
                    self.delete_range(s, e);
                    if op == 'c' {
                        self.vim = VimMode::Insert;
                        self.typing = true;
                    }
                    return Some(Outcome::Changed);
                }
                ['v'] => {
                    self.vim = if self.vim == VimMode::Visual {
                        VimMode::Normal
                    } else {
                        VimMode::Visual
                    };
                    if self.vim == VimMode::Normal {
                        self.anchor = None;
                    }
                    return Some(Outcome::Handled);
                }
                ['V'] => {
                    self.vim = if self.vim == VimMode::VisualLine {
                        VimMode::Normal
                    } else {
                        VimMode::VisualLine
                    };
                    if self.vim == VimMode::Normal {
                        self.anchor = None;
                    }
                    return Some(Outcome::Handled);
                }
                _ => {}
            }
            if Self::is_motion_prefix(keys) {
                return None;
            }
            if let Some((to, _, _)) = self.motion(keys) {
                self.cursor = to;
                self.clamp();
                return Some(Outcome::Handled);
            }
            return Some(Outcome::Handled);
        }
        let ro = self.read_only;
        match keys {
            ['i'] if !ro => {
                self.enter_insert();
                Some(Outcome::Handled)
            }
            ['a'] if !ro => {
                if !self.lines[self.cursor.row].is_empty() {
                    self.cursor.col += 1;
                }
                self.enter_insert();
                self.clamp();
                Some(Outcome::Handled)
            }
            ['I'] if !ro => {
                self.cursor.col = self.line_first_nonblank(self.cursor.row);
                self.enter_insert();
                Some(Outcome::Handled)
            }
            ['A'] if !ro => {
                self.cursor.col = char_len(&self.lines[self.cursor.row]);
                self.enter_insert();
                Some(Outcome::Handled)
            }
            ['o'] | ['O'] if !ro && !self.single_line => {
                self.snapshot();
                let indent: String = self.lines[self.cursor.row]
                    .chars()
                    .take_while(|c| *c == ' ')
                    .collect();
                let at = if keys[0] == 'o' {
                    self.cursor.row + 1
                } else {
                    self.cursor.row
                };
                self.lines.insert(at, indent.clone());
                self.cursor = Pos {
                    row: at,
                    col: char_len(&indent),
                };
                self.vim = VimMode::Insert;
                self.typing = true;
                Some(Outcome::Changed)
            }
            ['v'] => {
                self.vim = VimMode::Visual;
                self.anchor = Some(self.cursor);
                Some(Outcome::Handled)
            }
            ['V'] => {
                self.vim = VimMode::VisualLine;
                self.anchor = Some(self.cursor);
                Some(Outcome::Handled)
            }
            ['x'] if !ro => {
                if self.lines[self.cursor.row].is_empty() {
                    return Some(Outcome::Handled);
                }
                let c = self.cursor;
                Some(self.apply_operator('d', c, false, true, cb))
            }
            ['X'] if !ro => {
                if self.cursor.col == 0 {
                    return Some(Outcome::Handled);
                }
                let to = Pos {
                    col: self.cursor.col - 1,
                    ..self.cursor
                };
                Some(self.apply_operator('d', to, false, false, cb))
            }
            ['D'] | ['C'] if !ro => {
                let len = char_len(&self.lines[self.cursor.row]);
                let op = if keys[0] == 'D' { 'd' } else { 'c' };
                if len == 0 {
                    if op == 'c' {
                        self.enter_insert();
                    }
                    return Some(Outcome::Handled);
                }
                let to = Pos {
                    col: len - 1,
                    ..self.cursor
                };
                Some(self.apply_operator(op, to, false, true, cb))
            }
            ['p'] => Some(self.paste_vim(false, cb)),
            ['P'] => Some(self.paste_vim(true, cb)),
            ['u'] => Some(if self.undo() {
                Outcome::Changed
            } else {
                Outcome::Handled
            }),
            ['d'] | ['c'] | ['y'] | ['g'] | ['z'] => None,
            ['d', 'd'] | ['c', 'c'] | ['y', 'y'] => {
                let row = self.cursor.row;
                Some(self.apply_operator(keys[0], Pos { row, col: 0 }, true, true, cb))
            }
            [op @ ('d' | 'c' | 'y'), rest @ ..] => {
                if Self::is_motion_prefix(rest) {
                    return None;
                }
                // `cw` changes to the end of the word, like vim.
                let rest: Vec<char> = if *op == 'c' && rest == ['w'] {
                    vec!['e']
                } else {
                    rest.to_vec()
                };
                match self.motion(&rest) {
                    Some((to, lw, inc)) => Some(self.apply_operator(*op, to, lw, inc, cb)),
                    None => Some(Outcome::Handled),
                }
            }
            _ => {
                if let Some((to, _, _)) = self.motion(keys) {
                    let to = if matches!(keys, ['G'] | ['g', 'g']) {
                        Pos {
                            col: self.line_first_nonblank(to.row),
                            ..to
                        }
                    } else {
                        to
                    };
                    if matches!(keys, ['j'] | ['k']) {
                        let want = self.preferred_col.unwrap_or(self.cursor.col);
                        self.cursor = Pos {
                            row: to.row,
                            col: want.min(char_len(&self.lines[to.row])),
                        };
                        self.preferred_col = Some(want);
                    } else {
                        self.cursor = to;
                        self.preferred_col = None;
                    }
                    self.clamp();
                    return Some(Outcome::Handled);
                }
                if matches!(keys, ['j'] | ['k']) {
                    return Some(Outcome::Ignored);
                }
                Some(Outcome::Handled)
            }
        }
    }

    /// Places the cursor from a click at (x, y) relative to the text area.
    pub fn click(&mut self, x: u16, y: u16, extend: bool) {
        let x = x.saturating_sub(self.last_gutter);
        let row = (self.scroll_row + y as usize).min(self.lines.len() - 1);
        let line = &self.lines[row];
        let target = self.scroll_col + x as usize;
        let mut w = 0usize;
        let mut col = 0usize;
        for ch in line.chars() {
            let cw = ch.width().unwrap_or(0);
            if w + cw > target {
                break;
            }
            w += cw;
            col += 1;
        }
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else if !matches!(self.vim, VimMode::Visual | VimMode::VisualLine) {
            self.anchor = None;
        }
        self.cursor = Pos { row, col };
        self.typing = false;
        self.preferred_col = None;
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let max = self.lines.len().saturating_sub(1) as isize;
        self.scroll_row = (self.scroll_row as isize + delta).clamp(0, max) as usize;
    }

    fn display_col(line: &str, col: usize) -> usize {
        line.chars().take(col).map(|c| c.width().unwrap_or(0)).sum()
    }

    /// Draws the editor into `area` and returns the cursor's screen position.
    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        focused: bool,
        is_defined: &dyn Fn(&str) -> bool,
    ) -> Option<(u16, u16)> {
        if area.width == 0 || area.height == 0 {
            return None;
        }
        // Line numbers: right-aligned, one space on each side.
        let gw: u16 = if self.gutter && !self.single_line && area.width > 12 {
            let digits = self.lines.len().to_string().len().max(2) as u16;
            digits + 2
        } else {
            0
        };
        self.last_gutter = gw;
        let full = area;
        let area = Rect::new(area.x + gw, area.y, area.width - gw, area.height);
        self.last_size = (area.width, area.height);
        let h = area.height as usize;
        let w = area.width as usize;
        // Keep the cursor visible.
        if self.cursor.row < self.scroll_row {
            self.scroll_row = self.cursor.row;
        } else if self.cursor.row >= self.scroll_row + h {
            self.scroll_row = self.cursor.row + 1 - h;
        }
        let ccol = Self::display_col(&self.lines[self.cursor.row], self.cursor.col);
        if ccol < self.scroll_col {
            self.scroll_col = ccol;
        } else if ccol >= self.scroll_col + w {
            self.scroll_col = ccol + 1 - w;
        }
        let base = theme.base();
        let empty = self.lines.len() == 1 && self.lines[0].is_empty();
        if empty && !self.placeholder.is_empty() {
            // With the cursor in the field, the hint starts one cell to the
            // right so the cursor does not cover its first letter.
            let off: u16 = if focused && w > 4 { 2 } else { 0 };
            buf.set_stringn(
                area.x + off,
                area.y,
                &self.placeholder,
                w.saturating_sub(off as usize),
                theme.dim().add_modifier(Modifier::ITALIC),
            );
        }
        let sel = self.selection();
        let sel_style = theme.selected();
        // The current line stands out while the editor has focus.
        let line_bg = (focused && gw > 0 && !theme.mono).then_some(theme.surface_alt);
        for i in 0..h {
            let row = self.scroll_row + i;
            let y = area.y + i as u16;
            if gw > 0 {
                let is_cur = row == self.cursor.row;
                let num = if row < self.lines.len() {
                    format!("{:>w$} ", row + 1, w = (gw - 2) as usize)
                } else {
                    String::new()
                };
                let st = if is_cur && focused {
                    theme
                        .fg(theme.fg)
                        .patch(Style::default().bg(line_bg.unwrap_or(theme.surface)))
                } else {
                    theme.fg(theme.gutter)
                };
                buf.set_stringn(full.x, y, format!(" {num}"), gw as usize, st);
                if is_cur && let Some(bg) = line_bg {
                    for x in area.x..area.x + area.width {
                        if let Some(c) = buf.cell_mut((x, y)) {
                            c.set_bg(bg);
                        }
                    }
                }
            }
            if row >= self.lines.len() {
                continue;
            }
            let line = &self.lines[row];
            let styles = self.line_styles(line, theme, is_defined);
            let mut x = 0usize;
            for (ci, ch) in line.chars().enumerate() {
                let cw = ch.width().unwrap_or(0);
                if x + cw <= self.scroll_col {
                    x += cw;
                    continue;
                }
                let sx = x - self.scroll_col;
                if sx + cw > w {
                    break;
                }
                let mut st = styles.get(ci).copied().unwrap_or(base);
                if let Some((s, e)) = sel {
                    let p = Pos { row, col: ci };
                    if p >= s && p < e {
                        st = st.patch(sel_style);
                    }
                }
                let shown = if self.mask {
                    '•'
                } else if ch == '\t' {
                    ' '
                } else {
                    ch
                };
                if let Some(cell) = buf.cell_mut((area.x + sx as u16, y)) {
                    cell.set_char(shown).set_style(st);
                }
                x += cw;
            }
            // Show a selected line break as one highlighted cell.
            if let Some((s, e)) = sel {
                let p = Pos {
                    row,
                    col: char_len(line),
                };
                if p >= s && p < e && row < e.row {
                    let lw = Self::display_col(line, char_len(line));
                    if lw >= self.scroll_col
                        && lw - self.scroll_col < w
                        && let Some(cell) =
                            buf.cell_mut((area.x + (lw - self.scroll_col) as u16, y))
                    {
                        cell.set_style(sel_style);
                    }
                }
            }
        }
        if !focused {
            return None;
        }
        let cx = area.x + (ccol - self.scroll_col) as u16;
        let cy = area.y + (self.cursor.row - self.scroll_row) as u16;
        Some((cx, cy))
    }

    fn line_styles(
        &self,
        line: &str,
        theme: &Theme,
        is_defined: &dyn Fn(&str) -> bool,
    ) -> Vec<Style> {
        let base = theme.base();
        let n = char_len(line);
        let mut styles = vec![base; n];
        if self.highlight == Highlight::Url {
            for (s, e, st) in url_line_spans(line, theme) {
                for x in styles.iter_mut().take(e.min(n)).skip(s) {
                    *x = st;
                }
            }
        }
        if self.highlight == Highlight::Json {
            for (s, e, st) in json_line_spans(line, theme) {
                for x in styles.iter_mut().take(e.min(n)).skip(s) {
                    *x = st;
                }
            }
        }
        if line.contains("{{") {
            // Byte spans → char spans.
            for (span, name) in sendito_core::template::spans(line) {
                let s = line[..span.start].chars().count();
                let e = s + line[span.clone()].chars().count();
                let st = if is_defined(name) {
                    theme.fg(theme.var_ok).add_modifier(Modifier::BOLD)
                } else {
                    theme
                        .fg(theme.var_missing)
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                };
                for x in styles.iter_mut().take(e.min(n)).skip(s) {
                    *x = st;
                }
            }
            let bad = theme
                .fg(theme.var_missing)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
            for span in sendito_core::template::empty_placeholders(line) {
                let s = line[..span.start].chars().count();
                let e = s + line[span].chars().count();
                for x in styles.iter_mut().take(e.min(n)).skip(s) {
                    *x = bad;
                }
            }
        }
        styles
    }
}

/// Character spans for a URL: scheme and host dimmed, query keys and values
/// in the JSON key and string colors (`{{variables}}` are colored on top).
pub fn url_line_spans(line: &str, theme: &Theme) -> Vec<(usize, usize, Style)> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let n = chars.len();
    let find = |from: usize, pat: &[char]| {
        (from..n.saturating_sub(pat.len() - 1)).find(|&i| chars[i..i + pat.len()] == *pat)
    };
    // "scheme://host" (or a leading {{base}} variable, left to the variable colors).
    let mut path_start = 0;
    if let Some(i) = find(0, &[':', '/', '/']) {
        let host_end = (i + 3..n)
            .find(|&j| matches!(chars[j], '/' | '?' | '#'))
            .unwrap_or(n);
        out.push((0, i + 3, theme.dim()));
        out.push((i + 3, host_end, theme.fg(theme.fg)));
        path_start = host_end;
    }
    let Some(q) = (path_start..n).find(|&j| chars[j] == '?') else {
        return out;
    };
    out.push((q, q + 1, theme.dim()));
    let mut i = q + 1;
    while i < n && chars[i] != '#' {
        let end = (i..n).find(|&j| matches!(chars[j], '&' | '#')).unwrap_or(n);
        match (i..end).find(|&j| chars[j] == '=') {
            Some(eq) => {
                out.push((i, eq, theme.fg(theme.json_key)));
                out.push((eq, eq + 1, theme.dim()));
                out.push((eq + 1, end, theme.fg(theme.json_str)));
            }
            None => out.push((i, end, theme.fg(theme.json_key))),
        }
        if end < n && chars[end] == '&' {
            out.push((end, end + 1, theme.dim()));
        }
        i = end + 1;
    }
    if let Some(h) = (q..n).find(|&j| chars[j] == '#') {
        out.push((h, n, theme.dim()));
    }
    out
}

/// Character spans (start, end, style) for JSON-ish syntax on one line.
pub fn json_line_spans(line: &str, theme: &Theme) -> Vec<(usize, usize, Style)> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            let start = i;
            i += 1;
            while i < chars.len() {
                if chars[i] == '\\' {
                    i += 2;
                    continue;
                }
                if chars[i] == '"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            let end = i.min(chars.len());
            let mut j = end;
            while j < chars.len() && chars[j] == ' ' {
                j += 1;
            }
            let is_key = j < chars.len() && chars[j] == ':';
            out.push((
                start,
                end,
                theme.fg(if is_key {
                    theme.json_key
                } else {
                    theme.json_str
                }),
            ));
            continue;
        }
        if c == '-' || c.is_ascii_digit() {
            let start = i;
            while i < chars.len()
                && (chars[i].is_ascii_digit() || matches!(chars[i], '-' | '+' | '.' | 'e' | 'E'))
            {
                i += 1;
            }
            out.push((start, i, theme.fg(theme.json_num)));
            continue;
        }
        if c.is_ascii_alphabetic() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_alphabetic() {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            if matches!(word.as_str(), "true" | "false" | "null") {
                out.push((start, i, theme.fg(theme.json_lit)));
            }
            continue;
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
