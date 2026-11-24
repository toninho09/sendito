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
        if (self.wrap || self.folds_active())
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
            let spans = if pretty_json {
                json_line_spans(&line, theme)
            } else if self.mode != ViewMode::Hex {
                syntax_spans(self.syntax, &line, theme)
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

/// Character spans for one line of a [`Syntax`] document: sorted and not
/// overlapping.
pub fn syntax_spans(syntax: Syntax, line: &str, theme: &Theme) -> Vec<(usize, usize, Style)> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
