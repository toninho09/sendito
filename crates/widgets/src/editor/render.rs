//! Drawing the editor, mouse placement and line highlighting.

use super::*;

impl TextEditor {
    /// Places the cursor from a click at (x, y) relative to the text area.
    pub fn click(&mut self, x: u16, y: u16, extend: bool) {
        let x = x.saturating_sub(self.last_gutter);
        let row = (self.scroll_row + y as usize).min(self.lines.len() - 1);
        let line = &self.lines[row];
        let target = self.scroll_col + x as usize;
        let mut w = 0usize;
        let mut col = 0usize;
        for ch in line.chars() {
            let cw = cell_width(ch);
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

    pub(super) fn display_col(line: &str, col: usize) -> usize {
        line.chars().take(col).map(cell_width).sum()
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
                let cw = cell_width(ch);
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
                // Control characters never reach the terminal: an ESC in a
                // field could start an escape sequence there.
                let shown = if self.mask {
                    '•'
                } else if ch.is_control() {
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

    pub(super) fn line_styles(
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

/// Screen columns of one character. A control character (a tab too) takes
/// one cell and is drawn as a space.
pub(super) fn cell_width(ch: char) -> usize {
    if ch.is_control() {
        1
    } else {
        ch.width().unwrap_or(0)
    }
}
