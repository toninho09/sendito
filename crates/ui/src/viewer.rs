//! Read-only document viewer for response bodies and message logs: pretty
//! JSON with folding and highlighting, raw text, hex, wrapping and search.
//! Documents index lines by offset so 10 MB bodies stay cheap to render.

use std::collections::BTreeSet;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use unicode_width::UnicodeWidthChar;

use crate::editor::json_line_spans;
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Pretty,
    Raw,
    Hex,
}

impl ViewMode {
    pub fn label(self) -> &'static str {
        match self {
            ViewMode::Pretty => "Pretty",
            ViewMode::Raw => "Raw",
            ViewMode::Hex => "Hex",
        }
    }
}

/// Text with a line index.
#[derive(Debug, Clone, Default)]
pub struct Lines {
    pub text: String,
    /// Start byte of each line.
    starts: Vec<u32>,
}

impl Lines {
    pub fn new(text: String) -> Self {
        let mut starts = Vec::with_capacity(text.len() / 40 + 1);
        starts.push(0);
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i as u32 + 1);
            }
        }
        Self { text, starts }
    }

    pub fn len(&self) -> usize {
        self.starts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn line(&self, i: usize) -> &str {
        let Some(&s) = self.starts.get(i) else {
            return "";
        };
        let e = self
            .starts
            .get(i + 1)
            .map(|e| *e as usize - 1)
            .unwrap_or(self.text.len());
        let line = &self.text[s as usize..e.max(s as usize)];
        line.strip_suffix('\r').unwrap_or(line)
    }

    pub fn append(&mut self, s: &str) {
        let base = self.text.len();
        self.text.push_str(s);
        for (i, b) in s.bytes().enumerate() {
            if b == b'\n' {
                self.starts.push((base + i) as u32 + 1);
            }
        }
    }
}

/// A body prepared for viewing. Building it may take a while for big bodies,
/// so it is done off the UI thread.
#[derive(Debug, Clone, Default)]
pub struct Doc {
    /// Original bytes; empty when `raw` already holds them as valid UTF-8.
    bytes: Vec<u8>,
    pub raw: Lines,
    pub pretty: Option<Lines>,
    /// For each pretty line that opens a multi-line block, the line that closes it.
    fold_end: Vec<u32>,
    pub is_json: bool,
    pub valid_utf8: bool,
}

pub const NO_FOLD: u32 = u32::MAX;

impl Doc {
    pub fn from_bytes(bytes: Vec<u8>, content_type: Option<&str>) -> Self {
        let looks_json = content_type.is_some_and(|c| c.contains("json"))
            || bytes
                .iter()
                .find(|b| !b.is_ascii_whitespace())
                .is_some_and(|b| *b == b'{' || *b == b'[');
        let is_json = looks_json && serde_json::from_slice::<serde::de::IgnoredAny>(&bytes).is_ok();
        let (pretty, fold_end) = if is_json {
            let (p, f) = pretty_json(&bytes);
            (Some(Lines::new(p)), f)
        } else {
            (None, Vec::new())
        };
        let (bytes, raw, valid) = match String::from_utf8(bytes) {
            Ok(s) => (Vec::new(), Lines::new(s), true),
            Err(e) => {
                let b = e.into_bytes();
                let lossy = String::from_utf8_lossy(&b).into_owned();
                (b, Lines::new(lossy), false)
            }
        };
        Doc {
            bytes,
            raw,
            pretty,
            fold_end,
            is_json,
            valid_utf8: valid,
        }
    }

    pub fn text(s: &str) -> Self {
        Doc {
            raw: Lines::new(s.to_string()),
            valid_utf8: true,
            ..Default::default()
        }
    }

    pub fn bytes(&self) -> &[u8] {
        if self.valid_utf8 {
            self.raw.text.as_bytes()
        } else {
            &self.bytes
        }
    }

    pub fn len(&self) -> usize {
        self.bytes().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lines(&self, mode: ViewMode) -> Option<&Lines> {
        match mode {
            ViewMode::Pretty => Some(self.pretty.as_ref().unwrap_or(&self.raw)),
            ViewMode::Raw => Some(&self.raw),
            ViewMode::Hex => None,
        }
    }

    pub fn line_count(&self, mode: ViewMode) -> usize {
        match self.lines(mode) {
            Some(l) => l.len(),
            None => self.len().div_ceil(16).max(1),
        }
    }

    pub fn line(&self, mode: ViewMode, i: usize) -> String {
        match self.lines(mode) {
            Some(l) => l.line(i).to_string(),
            None => hex_line(self.bytes(), i),
        }
    }

    pub fn line_ref(&self, mode: ViewMode, i: usize) -> std::borrow::Cow<'_, str> {
        match self.lines(mode) {
            Some(l) => std::borrow::Cow::Borrowed(l.line(i)),
            None => std::borrow::Cow::Owned(hex_line(self.bytes(), i)),
        }
    }

    pub fn fold_end(&self, line: usize) -> Option<usize> {
        self.fold_end
            .get(line)
            .copied()
            .filter(|e| *e != NO_FOLD)
            .map(|e| e as usize)
    }

    pub fn append_text(&mut self, s: &str) {
        self.raw.append(s);
    }
}

pub fn hex_line(bytes: &[u8], i: usize) -> String {
    let start = i * 16;
    let chunk = bytes
        .get(start..(start + 16).min(bytes.len()))
        .unwrap_or_default();
    let mut s = format!("{start:08x}  ");
    for j in 0..16 {
        if j == 8 {
            s.push(' ');
        }
        match chunk.get(j) {
            Some(b) => s.push_str(&format!("{b:02x} ")),
            None => s.push_str("   "),
        }
    }
    s.push(' ');
    s.push('│');
    for b in chunk {
        s.push(if b.is_ascii_graphic() || *b == b' ' {
            *b as char
        } else {
            '.'
        });
    }
    s.push('│');
    s
}

/// Re-indents valid JSON without parsing values, so numbers and string escapes
/// are shown exactly as received. Returns the text and fold pairs.
pub fn pretty_json(src: &[u8]) -> (String, Vec<u32>) {
    let mut out: Vec<u8> = Vec::with_capacity(src.len() + src.len() / 2);
    let mut fold_end: Vec<u32> = Vec::new();
    let mut open_stack: Vec<u32> = Vec::new();
    let mut line: u32 = 0;
    let mut indent = 0usize;
    let mut i = 0;
    let n = src.len();
    let newline = |out: &mut Vec<u8>, line: &mut u32, indent: usize, fold_end: &mut Vec<u32>| {
        out.push(b'\n');
        *line += 1;
        fold_end.push(NO_FOLD);
        out.extend(std::iter::repeat_n(b' ', indent * 2));
    };
    fold_end.push(NO_FOLD);
    while i < n {
        let c = src[i];
        match c {
            b'"' => {
                let start = i;
                i += 1;
                while i < n {
                    match src[i] {
                        b'\\' => i += 2,
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
                out.extend_from_slice(&src[start..i.min(n)]);
                continue;
            }
            b'{' | b'[' => {
                let close = if c == b'{' { b'}' } else { b']' };
                let mut j = i + 1;
                while j < n && src[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < n && src[j] == close {
                    out.push(c);
                    out.push(close);
                    i = j + 1;
                    continue;
                }
                out.push(c);
                open_stack.push(line);
                indent += 1;
                newline(&mut out, &mut line, indent, &mut fold_end);
            }
            b'}' | b']' => {
                indent = indent.saturating_sub(1);
                newline(&mut out, &mut line, indent, &mut fold_end);
                out.push(c);
                if let Some(open) = open_stack.pop()
                    && let Some(slot) = fold_end.get_mut(open as usize)
                {
                    *slot = line;
                }
            }
            b',' => {
                out.push(b',');
                newline(&mut out, &mut line, indent, &mut fold_end);
            }
            b':' => out.extend_from_slice(b": "),
            c if c.is_ascii_whitespace() => {}
            c => out.push(c),
        }
        i += 1;
    }
    (String::from_utf8(out).unwrap_or_default(), fold_end)
}

#[derive(Debug, Clone, Default)]
pub struct Search {
    pub query: String,
    /// (line, start char, char length)
    pub matches: Vec<(u32, u32, u32)>,
    pub current: usize,
}

/// Columns that continuation rows of a wrapped line are indented by: the
/// line's own indentation plus two, at most half the width.
fn wrap_indent(line: &str, w: usize) -> usize {
    let chars: Vec<char> = line.chars().collect();
    let lead = chars.iter().take_while(|c| **c == ' ').count();
    // A Stream log event ("12:00:00.000 +   0.001s  → sent      {...}")
    // continues under its message column, or under its label when narrow.
    if let Some(a) = chars
        .windows(4)
        .position(|w| w[0] == 's' && w[1] == ' ' && w[2] == ' ' && matches!(w[3], '→' | '←' | '■'))
    {
        let arrow: usize = chars[..a + 3].iter().map(|c| c.width().unwrap_or(0)).sum();
        // Leave room for a typical metadata pair or JSON fragment.
        for col in [arrow + 12, arrow + 2] {
            if col + 24 <= w {
                return col;
            }
        }
    }
    // A two-column line ("key    description") continues under its second
    // column.
    let mut i = lead;
    while i + 2 < chars.len() {
        if chars[i] == ' ' && chars[i + 1] == ' ' && i > lead {
            let mut j = i;
            while j < chars.len() && chars[j] == ' ' {
                j += 1;
            }
            if j < chars.len() {
                let col: usize = chars[..j].iter().map(|c| c.width().unwrap_or(0)).sum();
                if col <= w / 2 {
                    return col;
                }
            }
            break;
        }
        i += 1;
    }
    (lead + 2).min(w / 2)
}

/// Calls `brk` with the char index where each continuation row of `line`
/// starts when wrapped to `w` columns: at a word boundary when the row has
/// one, else mid-word.
fn wrap_breaks(line: &str, w: usize, mut brk: impl FnMut(usize)) {
    let chars: Vec<char> = line.chars().collect();
    let width = |c: char| c.width().unwrap_or(0).max(usize::from(c == '\t'));
    if chars.iter().map(|c| width(*c)).sum::<usize>() <= w {
        return;
    }
    let indent = wrap_indent(line, w);
    let mut avail = w;
    let (mut start, mut col) = (0usize, 0usize);
    let mut last_space: Option<usize> = None;
    let mut idx = 0;
    while idx < chars.len() {
        let cw = width(chars[idx]);
        if col + cw > avail && idx > start {
            let next = w.saturating_sub(indent).max(1);
            // Break after the last space, unless the carried-over word would
            // not fit on the next row either.
            let at = match last_space {
                Some(b)
                    if b > start
                        && b <= idx
                        && chars[b..idx].iter().map(|c| width(*c)).sum::<usize>() + cw <= next =>
                {
                    b
                }
                _ => idx,
            };
            brk(at);
            start = at;
            avail = next;
            last_space = None;
            col = chars[at..idx].iter().map(|c| width(*c)).sum();
            continue;
        }
        col += cw;
        if chars[idx] == ' ' {
            last_space = Some(idx + 1);
        }
        idx += 1;
    }
}

/// Viewer state for one document.
#[derive(Debug, Clone)]
pub struct Viewer {
    pub doc: Doc,
    pub mode: ViewMode,
    /// Cursor as an index into visible rows.
    pub cursor: usize,
    pub scroll: usize,
    pub hscroll: usize,
    pub wrap: bool,
    pub folded: BTreeSet<usize>,
    pub search: Option<Search>,
    /// Visible rows: (doc line, start char within line). `None` = identity.
    rows: Option<Vec<(u32, u32)>>,
    rows_width: u16,
    /// Doc line count when `rows` was built (logs grow while shown).
    rows_lines: usize,
    height: u16,
}

impl Viewer {
    pub fn new(doc: Doc) -> Self {
        let mode = if doc.pretty.is_some() {
            ViewMode::Pretty
        } else {
            ViewMode::Raw
        };
        Self {
            doc,
            mode,
            cursor: 0,
            scroll: 0,
            hscroll: 0,
            wrap: false,
            folded: BTreeSet::new(),
            search: None,
            rows: None,
            rows_width: 0,
            rows_lines: 0,
            height: 0,
        }
    }

    pub fn empty() -> Self {
        Self::new(Doc::default())
    }

    pub fn set_mode(&mut self, m: ViewMode) {
        if self.mode != m {
            self.mode = m;
            self.cursor = 0;
            self.scroll = 0;
            self.hscroll = 0;
            self.folded.clear();
            self.rows = None;
            self.rows_width = 0;
            if let Some(s) = self.search.take() {
                self.search(&s.query);
            }
        }
    }

    pub fn toggle_wrap(&mut self) {
        self.wrap = !self.wrap;
        self.hscroll = 0;
        self.rows = None;
        self.rows_width = 0;
    }

    fn folds_active(&self) -> bool {
        self.mode == ViewMode::Pretty && !self.folded.is_empty()
    }

    fn rebuild_rows(&mut self, width: u16) {
        if !self.wrap && !self.folds_active() {
            self.rows = None;
            self.rows_width = width;
            return;
        }
        let total = self.doc.line_count(self.mode);
        let mut rows = Vec::new();
        let mut i = 0;
        let w = width.max(1) as usize;
        while i < total {
            if self.wrap && self.mode != ViewMode::Hex {
                let line = self.doc.line_ref(self.mode, i);
                rows.push((i as u32, 0));
                wrap_breaks(&line, w, |idx| rows.push((i as u32, idx as u32)));
            } else {
                rows.push((i as u32, 0));
            }
            if self.mode == ViewMode::Pretty
                && self.folded.contains(&i)
                && let Some(end) = self.doc.fold_end(i)
            {
                i = end + 1;
                continue;
            }
            i += 1;
        }
        self.rows = Some(rows);
        self.rows_width = width;
        self.rows_lines = total;
    }

    pub fn row_count(&self) -> usize {
        match &self.rows {
            Some(r) => r.len(),
            None => self.doc.line_count(self.mode),
        }
    }

    fn row(&self, v: usize) -> (usize, usize) {
        match &self.rows {
            Some(r) => r
                .get(v)
                .map(|(l, c)| (*l as usize, *c as usize))
                .unwrap_or((0, 0)),
            None => (v, 0),
        }
    }

    /// Doc line under the cursor.
    pub fn cursor_line(&self) -> usize {
        self.row(self.cursor).0
    }

    fn row_of_line(&self, line: usize) -> usize {
        match &self.rows {
            Some(r) => r
                .iter()
                .position(|(l, _)| *l as usize >= line)
                .unwrap_or(r.len().saturating_sub(1)),
            None => line,
        }
    }

    pub fn move_cursor(&mut self, delta: isize) {
        let max = self.row_count().saturating_sub(1) as isize;
        self.cursor = (self.cursor as isize + delta).clamp(0, max.max(0)) as usize;
    }

    pub fn page(&mut self, dir: isize) {
        let h = self.height.max(2) as isize - 1;
        self.move_cursor(dir * h);
    }

    pub fn top(&mut self) {
        self.cursor = 0;
    }

    pub fn bottom(&mut self) {
        self.cursor = self.row_count().saturating_sub(1);
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let max = self.row_count().saturating_sub(1) as isize;
        self.scroll = (self.scroll as isize + delta).clamp(0, max.max(0)) as usize;
        let h = self.height.max(1) as usize;
        if self.cursor < self.scroll {
            self.cursor = self.scroll;
        } else if self.cursor >= self.scroll + h {
            self.cursor = self.scroll + h - 1;
        }
    }

    pub fn hscroll_by(&mut self, delta: isize) {
        if self.wrap {
            return;
        }
        self.hscroll = (self.hscroll as isize + delta).max(0) as usize;
    }

    /// Folds or unfolds the block starting at (or containing) the cursor line.
    pub fn toggle_fold(&mut self) -> bool {
        if self.mode != ViewMode::Pretty || self.doc.pretty.is_none() {
            return false;
        }
        let line = self.cursor_line();
        let target = if self.doc.fold_end(line).is_some() {
            Some(line)
        } else {
            // Innermost block containing the line.
            (0..line)
                .rev()
                .find(|s| self.doc.fold_end(*s).is_some_and(|e| e >= line))
        };
        let Some(t) = target else { return false };
        if !self.folded.remove(&t) {
            self.folded.insert(t);
        }
        let w = self.last_width();
        self.rebuild_rows(w);
        self.cursor = self.row_of_line(t);
        true
    }

    pub fn fold_all(&mut self) {
        if self.mode != ViewMode::Pretty {
            return;
        }
        let total = self.doc.line_count(ViewMode::Pretty);
        // Keep the root open so the top-level keys stay visible.
        for i in 1..total {
            if self.doc.fold_end(i).is_some() {
                self.folded.insert(i);
            }
        }
        self.cursor = 0;
        self.scroll = 0;
        self.rows_width = 0;
    }

    pub fn unfold_all(&mut self) {
        let line = self.cursor_line();
        self.folded.clear();
        self.rows = None;
        self.rows_width = 0;
        self.cursor = line;
    }

    /// Case-insensitive search in the current view. Returns the match count.
    pub fn search(&mut self, query: &str) -> usize {
        if query.is_empty() {
            self.search = None;
            return 0;
        }
        let q: Vec<char> = query.to_lowercase().chars().collect();
        let mut matches = Vec::new();
        let total = self.doc.line_count(self.mode);
        for i in 0..total {
            let line = self.doc.line_ref(self.mode, i);
            if line.len() < q.len() {
                continue;
            }
            let lower: Vec<char> = line.chars().flat_map(|c| c.to_lowercase()).collect();
            if lower.len() != line.chars().count() {
                // Lowercasing changed the length; fall back to ASCII matching.
                let l = line.to_ascii_lowercase();
                let ql = query.to_ascii_lowercase();
                let mut from = 0;
                while let Some(p) = l[from..].find(&ql) {
                    let start = l[..from + p].chars().count();
                    matches.push((i as u32, start as u32, ql.chars().count() as u32));
                    from += p + ql.len().max(1);
                }
                continue;
            }
            let mut j = 0;
            while j + q.len() <= lower.len() {
                if lower[j..j + q.len()] == q[..] {
                    matches.push((i as u32, j as u32, q.len() as u32));
                    j += q.len();
                } else {
                    j += 1;
                }
            }
            if matches.len() > 100_000 {
                break;
            }
        }
        let n = matches.len();
        self.search = Some(Search {
            query: query.to_string(),
            matches,
            current: 0,
        });
        if n > 0 {
            // Start at the first match at or after the cursor.
            let cl = self.cursor_line() as u32;
            let idx = self
                .search
                .as_ref()
                .and_then(|s| s.matches.iter().position(|m| m.0 >= cl))
                .unwrap_or(0);
            if let Some(s) = &mut self.search {
                s.current = idx;
            }
            self.jump_to_match();
        }
        n
    }

    pub fn next_match(&mut self, dir: isize) -> bool {
        let Some(s) = &mut self.search else {
            return false;
        };
        if s.matches.is_empty() {
            return false;
        }
        let n = s.matches.len() as isize;
        s.current = ((s.current as isize + dir).rem_euclid(n)) as usize;
        self.jump_to_match();
        true
    }

    fn jump_to_match(&mut self) {
        let Some((line, col, len)) = self
            .search
            .as_ref()
            .and_then(|s| s.matches.get(s.current).copied())
        else {
            return;
        };
        let line = line as usize;
        // Unfold any block hiding the match.
        let hidden: Vec<usize> = self
            .folded
            .iter()
            .copied()
            .filter(|s| *s < line && self.doc.fold_end(*s).is_some_and(|e| e >= line))
            .collect();
        for h in hidden {
            self.folded.remove(&h);
        }
        self.rows_width = 0;
        let w = self.last_width();
        self.rebuild_rows(w);
        self.cursor = self.row_of_line(line);
        if let Some(rows) = &self.rows
            && self.wrap
        {
            while self.cursor + 1 < rows.len()
                && rows[self.cursor + 1].0 as usize == line
                && rows[self.cursor + 1].1 <= col
            {
                self.cursor += 1;
            }
        }
        if !self.wrap {
            let w = w.max(10) as usize;
            let (c, l) = (col as usize, len as usize);
            if c < self.hscroll || c + l > self.hscroll + w {
                self.hscroll = c.saturating_sub(w / 3);
            }
        }
        let h = self.height.max(1) as usize;
        if self.cursor < self.scroll || self.cursor >= self.scroll + h {
            self.scroll = self.cursor.saturating_sub(h / 3);
        }
    }

    fn last_width(&self) -> u16 {
        if self.rows_width == 0 {
            80
        } else {
            self.rows_width
        }
    }

    pub fn status(&self) -> String {
        // Lines of the document, whether or not they are wrapped.
        let total = self.doc.line_count(self.mode);
        let mut s = format!("line {}/{}", (self.cursor_line() + 1).min(total), total);
        if let Some(se) = &self.search {
            if se.matches.is_empty() {
                s.push_str(&format!(" · no match for \"{}\"", se.query));
            } else {
                s.push_str(&format!(" · match {}/{}", se.current + 1, se.matches.len()));
            }
        }
        s
    }

    /// Row index for a click at relative `y`.
    pub fn click(&mut self, y: u16) {
        let max = self.row_count().saturating_sub(1);
        self.cursor = (self.scroll + y as usize).min(max);
    }

    pub fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme, focused: bool) {
        if area.width < 4 || area.height == 0 {
            return;
        }
        let gutter: u16 = if self.mode == ViewMode::Pretty && self.doc.pretty.is_some() {
            2
        } else {
            0
        };
        let text_w = area.width.saturating_sub(gutter);
        self.height = area.height;
        if (self.wrap || self.folds_active())
            && (self.rows.is_none()
                || self.rows_width != text_w
                || self.rows_lines != self.doc.line_count(self.mode))
        {
            self.rebuild_rows(text_w);
        } else if !self.wrap && !self.folds_active() {
            self.rows = None;
        }
        let h = area.height as usize;
        let total = self.row_count();
        self.cursor = self.cursor.min(total.saturating_sub(1));
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + h {
            self.scroll = self.cursor + 1 - h;
        }
        let base = theme.base();
        let pretty_json = self.mode == ViewMode::Pretty && self.doc.pretty.is_some();
        for i in 0..h {
            let v = self.scroll + i;
            if v >= total {
                break;
            }
            let y = area.y + i as u16;
            let (line_no, start_char) = self.row(v);
            let line = self.doc.line_ref(self.mode, line_no);
            let is_cursor = v == self.cursor;
            let row_style = if is_cursor && focused {
                if theme.mono {
                    base.add_modifier(Modifier::BOLD)
                } else {
                    base.bg(theme.bar_bg)
                }
            } else {
                base
            };
            for x in 0..area.width {
                if let Some(c) = buf.cell_mut((area.x + x, y)) {
                    c.set_char(' ').set_style(row_style);
                }
            }
            if gutter > 0 && start_char == 0 {
                let marker = if self.folded.contains(&line_no) {
                    "▸"
                } else if self.doc.fold_end(line_no).is_some() {
                    "▾"
                } else {
                    " "
                };
                buf.set_string(
                    area.x,
                    y,
                    marker,
                    theme
                        .dim()
                        .patch(Style::default().bg(row_style.bg.unwrap_or(theme.bg))),
                );
            }
            let spans = if pretty_json {
                json_line_spans(&line, theme)
            } else {
                Vec::new()
            };
            let matches: Vec<(u32, u32)> = self
                .search
                .as_ref()
                .map(|s| {
                    s.matches
                        .iter()
                        .enumerate()
                        .filter(|(_, m)| m.0 as usize == line_no)
                        .map(|(idx, m)| {
                            (m.1, m.1 + m.2 + if idx == s.current { 1 << 30 } else { 0 })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let skip = if self.wrap { 0 } else { self.hscroll };
            // A wrapped row ends where the next row of the same line starts.
            let end_char = if self.wrap {
                match self.rows.as_ref().and_then(|r| r.get(v + 1)) {
                    Some((l, c)) if *l as usize == line_no => *c as usize,
                    _ => usize::MAX,
                }
            } else {
                usize::MAX
            };
            let mut x = 0usize;
            let mut sx = if self.wrap && start_char > 0 && self.mode != ViewMode::Hex {
                wrap_indent(&line, text_w as usize)
            } else {
                0
            };
            let mut span_idx = 0;
            let mut clipped = false;
            for (ci, ch) in line.chars().enumerate().skip(start_char) {
                if ci >= end_char {
                    break;
                }
                let cw = ch.width().unwrap_or(0).max(if ch == '\t' { 1 } else { 0 });
                if x < skip {
                    x += cw;
                    continue;
                }
                if sx + cw > text_w as usize {
                    clipped = true;
                    break;
                }
                while span_idx < spans.len() && spans[span_idx].1 <= ci {
                    span_idx += 1;
                }
                let mut st = row_style;
                if span_idx < spans.len() && spans[span_idx].0 <= ci && ci < spans[span_idx].1 {
                    st = st.patch(spans[span_idx].2);
                }
                for (ms, me) in &matches {
                    let current = *me >= (1 << 30);
                    let me = me & ((1 << 30) - 1);
                    if (ci as u32) >= *ms && (ci as u32) < me {
                        st = if theme.mono {
                            st.add_modifier(if current {
                                Modifier::REVERSED
                            } else {
                                Modifier::UNDERLINED
                            })
                        } else if current {
                            st.bg(theme.accent).fg(theme.bg)
                        } else {
                            st.bg(theme.search_bg)
                        };
                    }
                }
                let shown = if ch == '\t' || ch.is_control() {
                    ' '
                } else {
                    ch
                };
                if let Some(c) = buf.cell_mut((area.x + gutter + sx as u16, y)) {
                    c.set_char(shown).set_style(st);
                }
                x += cw;
                sx += cw;
            }
            // A line that goes on past the right edge ends in "…" (w wraps it).
            if clipped
                && text_w > 1
                && let Some(c) = buf.cell_mut((area.x + gutter + text_w - 1, y))
            {
                c.set_char('…').set_style(row_style.patch(theme.dim()));
            }
            if self.folded.contains(&line_no)
                && start_char == 0
                && let Some(end) = self.doc.fold_end(line_no)
            {
                let note = format!(" … {} lines ", end - line_no);
                let nx = area.x + gutter + (sx as u16).min(text_w);
                let room = (area.x + area.width).saturating_sub(nx) as usize;
                buf.set_stringn(
                    nx,
                    y,
                    &note,
                    room,
                    theme.dim().add_modifier(Modifier::ITALIC),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_log_events_continue_under_their_message() {
        let line =
            "12:00:00.000 +   0.001s  ← received  (12 B) {\"text\":\"a long message that wraps\"}";
        let arrow = line.chars().position(|c| c == '←').unwrap_or(0);
        assert_eq!(wrap_indent(line, 80), arrow + 12);
        assert_eq!(wrap_indent(line, 52), arrow + 2);
    }

    #[test]
    fn wrapped_rows_never_exceed_the_width() {
        let line = "Content-Type: multipart/form-data; boundary=0123456789abcdef0123456789abcdef0123456789";
        for w in [10usize, 20, 33, 40] {
            let mut starts = vec![0];
            wrap_breaks(line, w, |i| starts.push(i));
            starts.push(line.chars().count());
            let indent = wrap_indent(line, w);
            for (k, pair) in starts.windows(2).enumerate() {
                let len = pair[1] - pair[0];
                let room = if k == 0 { w } else { w - indent };
                assert!(len <= room, "w={w} row {k}: {len} > {room}");
            }
        }
    }

    #[test]
    fn pretty_preserves_tokens() {
        let src = br#"{"a":1.50,"b":[1,2,{"c":"x\"y"}],"e":{},"f":[],"g":12345678901234567890}"#;
        let (p, folds) = pretty_json(src);
        assert!(p.contains("\"a\": 1.50"));
        assert!(p.contains("\"g\": 12345678901234567890"));
        assert!(p.contains("\"c\": \"x\\\"y\""));
        assert!(p.contains("\"e\": {}"));
        assert_eq!(p.lines().next(), Some("{"));
        assert_eq!(folds[0] as usize, p.lines().count() - 1);
        let v1: serde_json::Value = serde_json::from_slice(src).unwrap();
        let v2: serde_json::Value = serde_json::from_str(&p).unwrap();
        assert_eq!(v1, v2);
    }

    #[test]
    fn doc_modes() {
        let d = Doc::from_bytes(b"{\"a\": [1, 2]}".to_vec(), Some("application/json"));
        assert!(d.is_json);
        assert_eq!(d.line_count(ViewMode::Pretty), 6);
        assert_eq!(d.line_count(ViewMode::Raw), 1);
        assert_eq!(d.line_count(ViewMode::Hex), 1);
        assert!(d.line(ViewMode::Hex, 0).starts_with("00000000  7b 22"));
        assert_eq!(d.fold_end(0), Some(5));
        let bin = Doc::from_bytes(vec![0xff, 0xfe, b'a'], None);
        assert!(!bin.valid_utf8);
        assert_eq!(bin.bytes(), &[0xff, 0xfe, b'a']);
        assert!(!bin.is_json);
        let t = Doc::from_bytes(b"line1\r\nline2".to_vec(), Some("text/plain"));
        assert_eq!(t.line(ViewMode::Raw, 0), "line1");
        assert_eq!(t.len(), 12);
        let notjson = Doc::from_bytes(b"{not json".to_vec(), Some("application/json"));
        assert!(!notjson.is_json);
        let mut log = Doc::text("a\n");
        log.append_text("b\nc");
        assert_eq!(log.line_count(ViewMode::Raw), 3);
        assert!(Doc::default().is_empty());
    }

    #[test]
    fn folding_and_search() {
        let body = br#"{"items":[{"id":1,"name":"Alpha"},{"id":2,"name":"beta"}],"count":2}"#;
        let mut v = Viewer::new(Doc::from_bytes(body.to_vec(), None));
        let area = Rect::new(0, 0, 40, 10);
        let theme = Theme::dusk();
        let mut buf = Buffer::empty(area);
        v.render(area, &mut buf, &theme, true);
        let total = v.row_count();
        v.cursor = 1; // "items": [
        assert!(v.toggle_fold());
        v.render(area, &mut buf, &theme, true);
        assert!(v.row_count() < total);
        assert!(v.toggle_fold());
        v.render(area, &mut buf, &theme, true);
        assert_eq!(v.row_count(), total);
        v.fold_all();
        v.render(area, &mut buf, &theme, true);
        assert!(v.row_count() < total);
        assert_eq!(v.search("BETA"), 1);
        v.render(area, &mut buf, &theme, true);
        assert!(v.status().contains("match 1/1"));
        assert!(v.next_match(1));
        v.unfold_all();
        assert_eq!(v.search("name"), 2);
        v.next_match(-1);
        assert_eq!(v.search("zzz"), 0);
        assert!(v.status().contains("no match"));
        assert_eq!(v.search(""), 0);
        v.set_mode(ViewMode::Raw);
        v.toggle_wrap();
        v.render(
            Rect::new(0, 0, 10, 5),
            &mut Buffer::empty(Rect::new(0, 0, 10, 5)),
            &theme,
            true,
        );
        assert!(v.row_count() > 1);
        v.bottom();
        v.page(-1);
        v.top();
        v.move_cursor(3);
        v.scroll_by(2);
        v.hscroll_by(5);
        v.click(1);
        v.set_mode(ViewMode::Hex);
        v.render(area, &mut buf, &theme, false);
        assert!(!v.toggle_fold());
    }
}
