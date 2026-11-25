//! Read-only document viewer for response bodies and message logs: pretty
//! JSON, XML, HTML, YAML and CSV with folding and highlighting, raw text, hex,
//! wrapping and search.
//! Documents index lines by offset so 10 MB bodies stay cheap to render.

use std::collections::BTreeSet;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use unicode_width::UnicodeWidthChar;

use crate::editor::json_line_spans;
use crate::theme::Theme;

pub use sendito_pipeline::doc::*;

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
    /// Line numbers in a gutter.
    pub numbers: bool,
    pub syntax: Syntax,
    /// The cursor line and its JSON path, from the last `json_path` call.
    path_cache: Option<(usize, Option<String>)>,
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
            numbers: false,
            syntax: Syntax::Plain,
            path_cache: None,
        }
    }

    /// The jq path of the JSON value at the cursor (pretty view only).
    pub fn json_path(&mut self) -> Option<String> {
        if self.mode != ViewMode::Pretty {
            return None;
        }
        let line = self.cursor_line();
        if let Some((l, p)) = &self.path_cache
            && *l == line
        {
            return p.clone();
        }
        let p = self.doc.json_path(line);
        self.path_cache = Some((line, p.clone()));
        p
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
        // With no rows, the cursor is a line index: render maps it back.
        let line = self.cursor_line();
        self.wrap = !self.wrap;
        self.hscroll = 0;
        self.rows = None;
        self.rows_width = 0;
        self.cursor = line;
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

    /// Adds the wrapped rows of lines appended since the last build. The old
    /// last line is wrapped again, as an append can extend it.
    fn extend_rows(&mut self, width: u16) {
        let total = self.doc.line_count(self.mode);
        let from = self.rows_lines.saturating_sub(1);
        let w = width.max(1) as usize;
        let Some(mut rows) = self.rows.take() else {
            return;
        };
        while rows.last().is_some_and(|r| r.0 as usize >= from) {
            rows.pop();
        }
        for i in from..total {
            let line = self.doc.line_ref(self.mode, i);
            rows.push((i as u32, 0));
            wrap_breaks(&line, w, |idx| rows.push((i as u32, idx as u32)));
        }
        self.rows = Some(rows);
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
        let q: Vec<char> = query.chars().map(fold).collect();
        let mut matches = Vec::new();
        let mut capped = false;
        let mut lower: Vec<char> = Vec::new();
        let total = self.doc.line_count(self.mode);
        'lines: for i in 0..total {
            let line = self.doc.line_ref(self.mode, i);
            if line.len() < q.len() {
                continue;
            }
            // One folded char per char, so match offsets are char offsets.
            lower.clear();
            lower.extend(line.chars().map(fold));
            let mut j = 0;
            while j + q.len() <= lower.len() {
                if lower[j..j + q.len()] == q[..] {
                    if matches.len() == Search::MAX_MATCHES {
                        capped = true;
                        break 'lines;
                    }
                    matches.push((i as u32, j as u32, q.len() as u32));
                    j += q.len();
                } else {
                    j += 1;
                }
            }
        }
        let n = matches.len();
        self.search = Some(Search {
            query: query.to_string(),
            matches,
            current: 0,
            capped,
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
        s.current = crate::list::cycle(s.current, dir, s.matches.len());
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
        // Rebuild at the width last drawn, so the row index stays right.
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
                s.push_str(&format!(" · {}", se.position_label()));
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
        let folds: u16 = if self.mode == ViewMode::Pretty && self.doc.pretty.is_some() {
            2
        } else {
            0
        };
        // Line numbers (not in the hex view, which shows offsets).
        let num_w: u16 = if self.numbers && self.mode != ViewMode::Hex && area.width > 20 {
            self.doc.line_count(self.mode).to_string().len().max(2) as u16 + 1
        } else {
            0
        };
        let gutter = folds + num_w;
        let text_w = area.width.saturating_sub(gutter);
        self.height = area.height;
        let lines = self.doc.line_count(self.mode);
        if self.wrap
            && !self.folds_active()
            && self.mode != ViewMode::Hex
            && self.rows.is_some()
            && self.rows_width == text_w
            && lines > self.rows_lines
            && self.rows_lines > 0
        {
            // A log grew at its end: wrap only the new lines.
            self.extend_rows(text_w);
        } else if (self.wrap || self.folds_active())
            && (self.rows.is_none()
                || self.rows_width != text_w
                || self.rows_lines != self.doc.line_count(self.mode))
        {
            // The rows move when the width changes: keep the cursor on its line.
            let line = self.cursor_line();
            self.rebuild_rows(text_w);
            self.cursor = self.row_of_line(line);
        } else if !self.wrap && !self.folds_active() {
            self.rows = None;
        }
        let h = area.height as usize;
        let total = self.row_count();
        self.cursor = self.cursor.min(total.saturating_sub(1));
        self.scroll = crate::list::keep_visible(self.cursor, self.scroll, h);
        let base = theme.base();
        let pretty =
            (self.mode == ViewMode::Pretty && self.doc.pretty.is_some()).then_some(self.doc.format);
        let mut line_cache: Option<LineCache> = None;
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
            if num_w > 0 && start_char == 0 {
                let st = if is_cursor && focused {
                    theme.fg(theme.fg)
                } else {
                    theme.fg(theme.gutter)
                };
                buf.set_stringn(
                    area.x,
                    y,
                    format!("{:>w$} ", line_no + 1, w = (num_w - 1) as usize),
                    num_w as usize,
                    st.patch(Style::default().bg(row_style.bg.unwrap_or(theme.bg))),
                );
            }
            if folds > 0 && start_char == 0 {
                let marker = if self.folded.contains(&line_no) {
                    "▸"
                } else if self.doc.fold_end(line_no).is_some() {
                    "▾"
                } else {
                    " "
                };
                buf.set_string(
                    area.x + num_w,
                    y,
                    marker,
                    theme
                        .dim()
                        .patch(Style::default().bg(row_style.bg.unwrap_or(theme.bg))),
                );
            }
            // The rows of one wrapped line share its spans, matches and indent.
            if line_cache.as_ref().is_none_or(|c| c.0 != line_no) {
                let spans = match pretty {
                    Some(format) => pretty_spans(format, &line, line_no, theme),
                    None if self.mode != ViewMode::Hex => syntax_spans(self.syntax, &line, theme),
                    None => Vec::new(),
                };
                let matches: Vec<(u32, u32)> = self
                    .search
                    .as_ref()
                    .map(|s| {
                        // Matches are sorted by line: find this line's run.
                        let first = s.matches.partition_point(|m| (m.0 as usize) < line_no);
                        s.matches[first..]
                            .iter()
                            .take_while(|m| m.0 as usize == line_no)
                            .enumerate()
                            .map(|(k, m)| {
                                let cur = if first + k == s.current { 1 << 30 } else { 0 };
                                (m.1, m.1 + m.2 + cur)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let indent = if self.wrap && self.mode != ViewMode::Hex {
                    wrap_indent(&line, text_w as usize)
                } else {
                    0
                };
                line_cache = Some((line_no, spans, matches, indent));
            }
            let Some((_, spans, matches, indent)) = line_cache.as_ref() else {
                continue;
            };
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
            let mut sx = if start_char > 0 { *indent } else { 0 };
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
                for (ms, me) in matches {
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

/// Character spans for line `line_no` of a pretty body: sorted and not
/// overlapping.
pub fn pretty_spans(
    format: BodyFormat,
    line: &str,
    line_no: usize,
    theme: &Theme,
) -> Vec<(usize, usize, Style)> {
    match format {
        BodyFormat::Json => json_line_spans(line, theme),
        BodyFormat::Xml | BodyFormat::Html => markup_line_spans(line, theme),
        BodyFormat::Yaml => yaml_line_spans(line, theme),
        BodyFormat::Csv(_) => table_line_spans(line, line_no == 0, theme),
        BodyFormat::Text => Vec::new(),
    }
}

/// Tags: `<`, `/` and `>` dim, the name in the key color, attribute names in
/// the number color and quoted values in the string color. A comment,
/// doctype, CDATA or processing instruction line is all dim.
fn markup_line_spans(line: &str, theme: &Theme) -> Vec<(usize, usize, Style)> {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let t = line.trim_start();
    if t.starts_with("<!") || t.starts_with("<?") {
        out.push((
            n - t.chars().count(),
            n,
            theme.dim().add_modifier(Modifier::ITALIC),
        ));
        return out;
    }
    let mut push = |s: usize, e: usize, st: Style| {
        if e > s {
            out.push((s, e, st));
        }
    };
    let mut i = 0;
    while i < n {
        if chars[i] != '<'
            || !chars
                .get(i + 1)
                .is_some_and(|c| c.is_alphabetic() || *c == '/')
        {
            i += 1;
            continue;
        }
        let s = i;
        i += if chars[i + 1] == '/' { 2 } else { 1 };
        push(s, i, theme.dim());
        let s = i;
        while i < n && !chars[i].is_whitespace() && !matches!(chars[i], '>' | '/') {
            i += 1;
        }
        push(s, i, theme.fg(theme.json_key));
        while i < n && chars[i] != '>' {
            let s = i;
            match chars[i] {
                q @ ('"' | '\'') => {
                    i += 1;
                    while i < n && chars[i] != q {
                        i += 1;
                    }
                    i = (i + 1).min(n);
                    push(s, i, theme.fg(theme.json_str));
                }
                '/' => {
                    i += 1;
                    push(s, i, theme.dim());
                }
                c if c.is_whitespace() || c == '=' => i += 1,
                _ => {
                    while i < n && !chars[i].is_whitespace() && !matches!(chars[i], '=' | '>' | '/')
                    {
                        i += 1;
                    }
                    push(s, i, theme.fg(theme.json_num));
                }
            }
        }
        if i < n {
            push(i, i + 1, theme.dim());
            i += 1;
        }
    }
    out
}

/// Keys in the key color; quoted and plain values in the string color,
/// numbers and `true`/`false`/`null`/`~` in the number and literal colors;
/// comments, `- `, `---` and block indicators dim.
fn yaml_line_spans(line: &str, theme: &Theme) -> Vec<(usize, usize, Style)> {
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut i = chars.iter().take_while(|c| **c == ' ').count();
    let rest: String = chars[i..].iter().collect();
    if rest.starts_with('#') || matches!(rest.trim_end(), "---" | "...") {
        out.push((i, n, theme.dim()));
        return out;
    }
    if chars.get(i) == Some(&'-') && chars.get(i + 1).is_none_or(|c| *c == ' ') {
        out.push((i, i + 1, theme.dim()));
        i += 1;
        while i < n && chars[i] == ' ' {
            i += 1;
        }
    }
    // A comment starts at a `#` after a space, outside quotes.
    let mut end = n;
    let mut quote = None;
    for j in i..n {
        match (quote, chars[j]) {
            (None, q @ ('"' | '\'')) => quote = Some(q),
            (Some(q), c) if c == q => quote = None,
            (None, '#') if j == i || chars[j - 1] == ' ' => {
                end = j;
                break;
            }
            _ => {}
        }
    }
    // The key ends at the first `: ` (or a final `:`) outside quotes.
    let mut quote = None;
    let colon = (i..end).find(|&j| match (quote, chars[j]) {
        (None, q @ ('"' | '\'')) if j == i => {
            quote = Some(q);
            false
        }
        (Some(q), c) if c == q => {
            quote = None;
            false
        }
        (None, ':') => j + 1 == end || chars[j + 1] == ' ',
        _ => false,
    });
    if let Some(c) = colon {
        out.push((i, c, theme.fg(theme.json_key)));
        i = c + 1;
        while i < end && chars[i] == ' ' {
            i += 1;
        }
    }
    let value: String = chars[i..end].iter().collect();
    let v = value.trim_end();
    let v_end = i + v.chars().count();
    let style = if v.is_empty() {
        None
    } else if v.starts_with(['|', '>']) && v.len() <= 3 {
        Some(theme.dim())
    } else if matches!(v, "true" | "false" | "null" | "~") {
        Some(theme.fg(theme.json_lit))
    } else if v.parse::<f64>().is_ok() {
        Some(theme.fg(theme.json_num))
    } else {
        Some(theme.fg(theme.json_str))
    };
    if let Some(st) = style {
        out.push((i, v_end, st));
    }
    if end < n {
        out.push((end, n, theme.dim()));
    }
    out
}

/// The column separators dim; the first row (the header) bold in the key
/// color.
fn table_line_spans(line: &str, header: bool, theme: &Theme) -> Vec<(usize, usize, Style)> {
    let sep = sendito_pipeline::pretty::COLUMN_SEPARATOR;
    let sep_len = sep.chars().count();
    let bold = theme.fg(theme.json_key).add_modifier(Modifier::BOLD);
    let mut out = Vec::new();
    let mut start = 0;
    for (k, cell) in line.split(sep).enumerate() {
        if k > 0 {
            out.push((start, start + sep_len, theme.dim()));
            start += sep_len;
        }
        let len = cell.chars().count();
        if header && len > 0 {
            out.push((start, start + len, bold));
        }
        start += len;
    }
    out
}

/// Character spans for one line of a [`Syntax`] document: sorted and not
/// overlapping.
pub fn syntax_spans(syntax: Syntax, line: &str, theme: &Theme) -> Vec<(usize, usize, Style)> {
    if syntax == Syntax::Plain {
        return Vec::new();
    }
    let chars: Vec<char> = line.chars().collect();
    let n = chars.len();
    let heading = theme.fg(theme.title).add_modifier(Modifier::BOLD);
    let find = |from: usize, pat: &str| -> Option<usize> {
        let p: Vec<char> = pat.chars().collect();
        (from..=n.saturating_sub(p.len())).find(|&i| chars[i..].starts_with(&p))
    };
    let mut out: Vec<(usize, usize, Style)> = Vec::new();
    match syntax {
        Syntax::Plain => {}
        Syntax::Headers => {
            if line.starts_with("HTTP/") {
                // "HTTP/1.1 200 OK": the code and reason in the status color.
                let sp = find(0, " ").unwrap_or(n);
                out.push((0, sp, theme.dim()));
                let code: u16 = line
                    .get(sp + 1..sp + 4)
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(0);
                out.push((
                    sp,
                    n,
                    theme
                        .fg(theme.status_color(code))
                        .add_modifier(Modifier::BOLD),
                ));
            } else if matches!(line, "Request sent" | "Response headers" | "Trailers") {
                out.push((0, n, heading));
            } else if let Some(sp) = find(0, " ")
                && line.contains("://")
                && chars[..sp].iter().all(|c| c.is_ascii_uppercase())
            {
                // "GET http://…": the request line.
                let m: String = chars[..sp].iter().collect();
                out.push((
                    0,
                    sp,
                    theme
                        .fg(theme.method_color(&m))
                        .add_modifier(Modifier::BOLD),
                ));
            } else if let Some(c) = find(0, ": ")
                && c > 0
            {
                out.push((0, c, theme.fg(theme.json_key)));
                out.push((c, c + 1, theme.dim()));
                if line[line
                    .char_indices()
                    .nth(c + 2)
                    .map(|(b, _)| b)
                    .unwrap_or(line.len())..]
                    .contains(crate::MASK)
                {
                    out.push((c + 2, n, theme.dim()));
                }
            }
        }
        Syntax::Json => out.extend(crate::editor::json_line_spans(line, theme)),
        Syntax::Timing => {
            let phase = [
                ("DNS lookup", theme.phase_dns),
                ("TCP connect", theme.phase_connect),
                ("TLS handshake", theme.phase_tls),
                ("Waiting for first byte", theme.phase_wait),
                ("Download", theme.phase_download),
            ]
            .into_iter()
            .find(|(p, _)| line.starts_with(p))
            .map(|(_, c)| c);
            if line.starts_with("Phase") {
                out.push((0, n, theme.dim().add_modifier(Modifier::BOLD)));
            } else if line.starts_with("Total") {
                out.push((0, n, theme.fg(theme.fg).add_modifier(Modifier::BOLD)));
            } else if let Some(c) = phase {
                let mut i = 0;
                while i < n {
                    let ch = chars[i];
                    if is_bar(ch) {
                        let s0 = i;
                        while i < n && is_bar(chars[i]) {
                            i += 1;
                        }
                        out.push((s0, i, theme.fg(c)));
                    } else if ch == '·' {
                        let s0 = i;
                        while i < n && chars[i] == '·' {
                            i += 1;
                        }
                        out.push((s0, i, theme.fg(theme.border)));
                    } else if line[line.char_indices().nth(i).map(|(b, _)| b).unwrap_or(0)..]
                        .starts_with("skipped")
                    {
                        out.push((i, n, theme.dim()));
                        break;
                    } else {
                        i += 1;
                    }
                }
            } else if n > 16 && line.as_bytes().get(15) == Some(&b' ') && !line.starts_with(' ') {
                // "Protocol        HTTP/1.1"
                out.push((0, 16, theme.dim()));
            }
        }
        Syntax::Messages => {
            if line.ends_with(':') && !line.starts_with(' ') || line.starts_with("Assertions:") {
                out.push((0, n, heading));
            } else if let Some(i) = find(0, "✓ PASS") {
                out.push((i, i + 6, theme.fg(theme.ok).add_modifier(Modifier::BOLD)));
            } else if let Some(i) = find(0, "✗ FAIL") {
                out.push((i, i + 6, theme.fg(theme.err).add_modifier(Modifier::BOLD)));
                if let Some(a) = find(i, "(actual:") {
                    out.push((a, n, theme.dim()));
                }
            } else if let Some(arrow) = chars.iter().position(|c| matches!(c, '→' | '←' | '■'))
                && arrow >= 12
                && find(0, " +").is_some_and(|p| p < arrow)
            {
                // A gRPC log line: time, offset, direction, what, payload.
                out.push((0, arrow, theme.dim()));
                let what_end = (arrow + 2..n).find(|&j| chars[j] == ' ').unwrap_or(n);
                let color = match chars[arrow] {
                    '→' => theme.accent,
                    '←' => theme.ok,
                    _ if line.contains("OK (0)") => theme.ok,
                    _ => theme.err,
                };
                out.push((
                    arrow,
                    what_end,
                    theme.fg(color).add_modifier(Modifier::BOLD),
                ));
                let rest_start = (what_end..n).find(|&j| chars[j] != ' ').unwrap_or(n);
                let rest: String = chars[rest_start..].iter().collect();
                if rest.starts_with('{') || rest.starts_with('[') || rest.starts_with('(') {
                    for (s0, e0, st) in crate::editor::json_line_spans(&rest, theme) {
                        out.push((rest_start + s0, rest_start + e0, st));
                    }
                }
            } else if let Some(eq) = find(0, " = ")
                && line.starts_with("  ")
            {
                out.push((2, eq, theme.fg(theme.json_key)));
            } else if line.starts_with("unary:")
                || line.contains("stream:") && !line.starts_with(' ')
            {
                out.push((0, n, heading));
            }
        }
        Syntax::Help => {
            if line.starts_with("── ") {
                out.push((0, n, theme.accent().add_modifier(Modifier::BOLD)));
            } else if line.starts_with("  ") && n > 2 {
                let end = find(2, "  ").unwrap_or(n);
                out.push((2, end, theme.accent().add_modifier(Modifier::BOLD)));
            }
        }
    }
    out.sort_by_key(|s| s.0);
    out
}

fn is_bar(c: char) -> bool {
    matches!(c, '█' | '▉' | '▊' | '▋' | '▌' | '▍' | '▎' | '▏' | '▐' | '▕')
}

/// What the rows of one line share while a frame is drawn: the line, its
/// syntax spans, its search matches and its wrap indent.
type LineCache = (usize, Vec<(usize, usize, Style)>, Vec<(u32, u32)>, usize);

/// Case folding for search: the lowercase form when it is one char (most
/// scripts), else the char itself, so a line keeps its char count.
fn fold(c: char) -> char {
    let mut l = c.to_lowercase();
    match (l.next(), l.next()) {
        (Some(one), None) => one,
        _ => c,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text and style of each span.
    fn parts(line: &str, spans: &[(usize, usize, Style)]) -> Vec<(String, Style)> {
        let chars: Vec<char> = line.chars().collect();
        for w in spans.windows(2) {
            assert!(w[0].1 <= w[1].0, "{spans:?}");
        }
        spans
            .iter()
            .map(|(s, e, st)| (chars[*s..*e].iter().collect(), *st))
            .collect()
    }

    #[test]
    fn markup_colors() {
        let t = Theme::dusk();
        let line = "  <user id=\"1\" ok/>x < y</user>";
        let p = parts(line, &pretty_spans(BodyFormat::Xml, line, 3, &t));
        let texts: Vec<&str> = p.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(
            texts,
            [
                "<", "user", "id", "\"1\"", "ok", "/", ">", "</", "user", ">"
            ]
        );
        assert_eq!(p[1].1, t.fg(t.json_key));
        assert_eq!(p[2].1, t.fg(t.json_num));
        assert_eq!(p[3].1, t.fg(t.json_str));
        let c = pretty_spans(BodyFormat::Html, "  <!-- note -->", 0, &t);
        assert_eq!((c[0].0, c[0].1), (2, 15));
    }

    #[test]
    fn yaml_colors() {
        let t = Theme::dusk();
        let line = "  - name: \"a # b\" # c";
        let p = parts(line, &pretty_spans(BodyFormat::Yaml, line, 0, &t));
        assert_eq!(
            p,
            [
                ("-".into(), t.dim()),
                ("name".into(), t.fg(t.json_key)),
                ("\"a # b\"".into(), t.fg(t.json_str)),
                ("# c".into(), t.dim()),
            ]
        );
        let p = parts(
            "port: 443",
            &pretty_spans(BodyFormat::Yaml, "port: 443", 0, &t),
        );
        assert_eq!(p[1], ("443".into(), t.fg(t.json_num)));
        let p = parts(
            "on: true",
            &pretty_spans(BodyFormat::Yaml, "on: true", 0, &t),
        );
        assert_eq!(p[1], ("true".into(), t.fg(t.json_lit)));
        assert_eq!(pretty_spans(BodyFormat::Yaml, "# x", 0, &t)[0].2, t.dim());
    }

    #[test]
    fn table_colors() {
        let t = Theme::dusk();
        let line = "name │ São";
        let head = parts(line, &pretty_spans(BodyFormat::Csv(b','), line, 0, &t));
        let bold = t.fg(t.json_key).add_modifier(Modifier::BOLD);
        assert_eq!(
            head,
            [
                ("name".into(), bold),
                (" │ ".into(), t.dim()),
                ("São".into(), bold)
            ]
        );
        let body = parts(line, &pretty_spans(BodyFormat::Csv(b','), line, 1, &t));
        assert_eq!(body, [(" │ ".into(), t.dim())]);
    }

    #[test]
    fn syntax_colors() {
        let t = Theme::dusk();
        let s = syntax_spans(Syntax::Headers, "HTTP/1.1 404 Not Found", &t);
        assert_eq!(s[1].2.fg, Some(t.warn));
        let s = syntax_spans(Syntax::Headers, "content-type: application/json", &t);
        assert_eq!(s[0], (0, 12, t.fg(t.json_key)));
        let s = syntax_spans(Syntax::Headers, "POST http://x/y", &t);
        assert_eq!(s[0].2.fg, Some(t.post));
        let s = syntax_spans(Syntax::Timing, "TCP connect   0.1 ms  ··██▍···", &t);
        assert!(s.iter().any(|(_, _, st)| st.fg == Some(t.phase_connect)));
        let s = syntax_spans(
            Syntax::Messages,
            "  ✗ FAIL  status == 200   (actual: 404)",
            &t,
        );
        assert_eq!(s[0].2.fg, Some(t.err));
        let s = syntax_spans(
            Syntax::Messages,
            "12:00:00.000 +   0.001s  ← received  (2 B) {\"a\":1}",
            &t,
        );
        assert!(s.iter().any(|(_, _, st)| st.fg == Some(t.ok)));
        assert!(s.iter().any(|(_, _, st)| st.fg == Some(t.json_key)));
        let s = syntax_spans(Syntax::Help, "  Ctrl+W x         Cancel", &t);
        assert_eq!((s[0].0, s[0].1), (2, 10));
        // Spans never overlap.
        for w in s.windows(2) {
            assert!(w[0].1 <= w[1].0);
        }
    }

    #[test]
    fn a_growing_wrapped_log_gets_the_same_rows_as_a_rebuild() {
        let area = Rect::new(0, 0, 12, 5);
        let theme = Theme::dusk();
        let mut buf = Buffer::empty(area);
        let mut v = Viewer::new(Doc::text("short\n"));
        v.wrap = true;
        v.render(area, &mut buf, &theme, false);
        v.doc.append_text("a line that wraps more than once\nand ");
        v.render(area, &mut buf, &theme, false);
        v.doc.append_text("this one too, after the append\n");
        v.render(area, &mut buf, &theme, false);
        let grown = v.rows.clone();
        v.rows = None;
        v.render(area, &mut buf, &theme, false);
        assert_eq!(grown, v.rows);
    }

    #[test]
    fn search_folds_case_per_char_and_marks_a_capped_count() {
        // "İ" lowercases to two chars; the line must still match by char.
        let mut v = Viewer::new(Doc::text("İstanbul Éclair\ncafé"));
        assert_eq!(v.search("é"), 2);
        let s = v.search.as_ref().map(|s| s.matches.clone());
        assert_eq!(s, Some(vec![(0, 9, 1), (1, 3, 1)]));
        let many = "a".repeat(Search::MAX_MATCHES + 5);
        let mut v = Viewer::new(Doc::text(&many));
        assert_eq!(v.search("a"), Search::MAX_MATCHES);
        assert!(v.status().ends_with("+"), "{}", v.status());
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
