//! Response documents: pretty JSON with folding, raw text, hex and search.
//! Documents index lines by offset so 10 MB bodies stay cheap to render.

use unicode_width::UnicodeWidthChar;

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

/// How a plain-text document is colored, line by line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Syntax {
    #[default]
    Plain,
    /// "Name: value" lines, an HTTP status line, "Request sent".
    Headers,
    /// The timing waterfall and its details.
    Timing,
    /// Assertions, variables and the gRPC message log.
    Messages,
    /// Raw JSON-RPC lines (the MCP "Request" tab).
    Json,
    /// The key list: "── Section ──" and "  keys  description".
    Help,
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

    /// A stream of JSON values, such as the output of a jq filter: the raw
    /// view has one value per line, the pretty view each value formatted and
    /// foldable.
    pub fn json_stream(values: &[Vec<u8>]) -> Self {
        let mut pretty = String::new();
        let mut fold_end: Vec<u32> = Vec::new();
        for (i, v) in values.iter().enumerate() {
            if i > 0 {
                pretty.push('\n');
            }
            let base = fold_end.len() as u32;
            let (p, f) = pretty_json(v);
            pretty.push_str(&p);
            fold_end.extend(
                f.into_iter()
                    .map(|e| if e == NO_FOLD { e } else { e + base }),
            );
        }
        let raw = String::from_utf8_lossy(&values.join(&b'\n')).into_owned();
        Doc {
            bytes: Vec::new(),
            raw: Lines::new(raw),
            pretty: Some(Lines::new(pretty)),
            fold_end,
            is_json: true,
            valid_utf8: true,
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

    /// The jq path of the JSON value on a pretty line, e.g. `.items[0].email`
    /// (`.` for a top-level value; a closing line gives its container).
    /// `None` when the document has no pretty form.
    pub fn json_path(&self, line: usize) -> Option<String> {
        let pretty = self.pretty.as_ref()?;
        if line >= pretty.len() {
            return None;
        }
        // One entry per open container: (is an array, next index, its segment).
        let mut stack: Vec<(bool, usize, String)> = Vec::new();
        let path = |stack: &[(bool, usize, String)], last: Option<&str>| {
            let mut p: String = stack.iter().map(|f| f.2.as_str()).collect();
            p.push_str(last.unwrap_or(""));
            if p.starts_with('.') {
                p
            } else {
                format!(".{p}")
            }
        };
        for i in 0..=line {
            let t = pretty.line(i).trim_start();
            if t.starts_with('}') || t.starts_with(']') {
                if i == line {
                    return Some(path(&stack, None));
                }
                stack.pop();
                continue;
            }
            let (key, value) = split_key(t);
            let seg = match stack.last_mut() {
                Some((true, next, _)) => {
                    *next += 1;
                    format!("[{}]", *next - 1)
                }
                Some(_) => key.map(key_segment).unwrap_or_default(),
                None => String::new(),
            };
            if i == line {
                return Some(path(&stack, Some(&seg)));
            }
            match value.trim_end_matches(',') {
                "{" => stack.push((false, 0, seg)),
                "[" => stack.push((true, 0, seg)),
                _ => {}
            }
        }
        None
    }
}

/// Splits a pretty JSON line into its object key (the quoted source text) and
/// the value after `": "`.
fn split_key(t: &str) -> (Option<&str>, &str) {
    if !t.starts_with('"') {
        return (None, t);
    }
    let b = t.as_bytes();
    let mut i = 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => break,
            _ => i += 1,
        }
    }
    match t.get(i + 1..).and_then(|r| r.strip_prefix(": ")) {
        Some(value) => (Some(&t[..=i]), value),
        None => (None, t),
    }
}

/// A jq path segment for a quoted key: `.name`, or `["a b"]` when the key
/// is not a plain identifier.
fn key_segment(quoted: &str) -> String {
    let inner = &quoted[1..quoted.len() - 1];
    let plain = inner
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if plain {
        format!(".{inner}")
    } else {
        format!("[{quoted}]")
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
pub fn wrap_indent(line: &str, w: usize) -> usize {
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
pub fn wrap_breaks(line: &str, w: usize, mut brk: impl FnMut(usize)) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_path_follows_keys_and_indices() {
        let d = Doc::from_bytes(
            br#"{"items":[{"id":1,"a b":{"x":[]}},{"id":2}],"n":"s\"t"}"#.to_vec(),
            None,
        );
        let at = |l| d.json_path(l).unwrap_or_default();
        let lines: Vec<String> = (0..d.line_count(ViewMode::Pretty))
            .map(|i| d.line(ViewMode::Pretty, i))
            .collect();
        let find = |s: &str| {
            lines
                .iter()
                .position(|l| l.contains(s))
                .unwrap_or(usize::MAX)
        };
        assert_eq!(at(0), ".");
        assert_eq!(at(find("\"items\"")), ".items");
        assert_eq!(at(find("\"id\": 1")), ".items[0].id");
        assert_eq!(at(find("\"x\"")), r#".items[0]["a b"].x"#);
        assert_eq!(at(find("\"id\": 2")), ".items[1].id");
        assert_eq!(at(find("\"n\"")), ".n");
        // A closing line belongs to its container.
        assert_eq!(at(lines.len() - 1), ".");
        assert_eq!(Doc::text("plain").json_path(0), None);
        let arr = Doc::from_bytes(br#"[{"a b":1}]"#.to_vec(), None);
        assert_eq!(arr.json_path(2).as_deref(), Some(r#".[0]["a b"]"#));
    }

    #[test]
    fn json_stream_formats_and_folds_each_value() {
        let d = Doc::json_stream(&[b"{\"a\":1}".to_vec(), b"\"x\"".to_vec(), b"[1,2]".to_vec()]);
        assert_eq!(d.raw.text, "{\"a\":1}\n\"x\"\n[1,2]");
        assert_eq!(d.line_count(ViewMode::Pretty), 8);
        assert_eq!(d.line(ViewMode::Pretty, 3), "\"x\"");
        // The folds of a later value are shifted to its own lines.
        assert_eq!(d.fold_end(0), Some(2));
        assert_eq!(d.fold_end(3), None);
        assert_eq!(d.fold_end(4), Some(7));
    }

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
}
