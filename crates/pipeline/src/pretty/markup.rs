//! XML and HTML re-indented: one tag per line, two spaces per level, a fold
//! per element. The bytes inside a tag, a comment, a CDATA section and the
//! text of HTML `pre`, `script`, `style` and `textarea` stay as received.

use crate::doc::NO_FOLD;

/// HTML elements that never have content or a closing tag.
const VOID: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// HTML elements whose content is text, shown as received.
const RAW_TEXT: [&str; 4] = ["pre", "script", "style", "textarea"];

#[derive(Debug, PartialEq)]
enum Token<'a> {
    /// Text between tags; its lines are trimmed and re-indented.
    Text(&'a str),
    /// The content of a `RAW_TEXT` element, kept as received.
    Raw(&'a str),
    /// An opening tag; `empty` when it has no content (`<a/>`, `<br>`).
    Open {
        name: String,
        raw: &'a str,
        empty: bool,
    },
    Close {
        name: String,
        raw: &'a str,
    },
    /// A comment, CDATA section, processing instruction or doctype.
    Other(&'a str),
}

/// The re-indented text and folds of an XML (`html` false) or HTML document.
/// `None` for XML whose closing tags do not match, or for a document with no
/// content.
pub fn pretty_markup(src: &str, html: bool) -> Option<(String, Vec<u32>)> {
    let tokens = tokenize(src, html);
    let mut w = Writer::default();
    // One entry per open element: its name and the line of its opening tag.
    let mut stack: Vec<(&str, u32)> = Vec::new();
    let mut k = 0;
    while k < tokens.len() {
        let depth = stack.len();
        match &tokens[k] {
            Token::Text(t) => {
                for l in t.lines().map(str::trim).filter(|l| !l.is_empty()) {
                    w.line(depth, l);
                }
            }
            Token::Raw(t) => w.raw(t),
            Token::Other(raw)
            | Token::Open {
                raw, empty: true, ..
            } => {
                w.block(depth, raw);
            }
            Token::Open { name, raw, .. } => {
                if let Some((line, used)) = inline(&tokens[k..], name, raw) {
                    w.line(depth, &line);
                    k += used;
                    continue;
                }
                let line = w.block(depth, raw);
                stack.push((name, line));
            }
            Token::Close { name, raw } => match stack.iter().rposition(|(n, _)| n == name) {
                Some(p) => {
                    if !html && p + 1 != stack.len() {
                        return None;
                    }
                    // HTML: elements left open (`<p>`, `<li>`) end before this tag.
                    let last = w.last_line();
                    for (_, open) in stack.drain(p + 1..) {
                        w.fold(open, last);
                    }
                    let (_, open) = stack.pop()?;
                    let line = w.block(stack.len(), raw);
                    w.fold(open, line);
                }
                None if html => {
                    w.block(depth, raw);
                }
                None => return None,
            },
        }
        k += 1;
    }
    if !html && !stack.is_empty() {
        return None;
    }
    let last = w.last_line();
    for (_, open) in stack {
        w.fold(open, last);
    }
    if w.fold_end.is_empty() {
        return None;
    }
    Some((w.out, w.fold_end))
}

/// An element with no content or one line of text, written on one line:
/// the line and the number of tokens it uses.
fn inline(tokens: &[Token], name: &str, raw: &str) -> Option<(String, usize)> {
    if raw.contains('\n') {
        return None;
    }
    match (tokens.get(1), tokens.get(2)) {
        (
            Some(Token::Close {
                name: c,
                raw: close,
            }),
            _,
        ) if c == name => Some((format!("{raw}{close}"), 2)),
        (
            Some(Token::Text(t)),
            Some(Token::Close {
                name: c,
                raw: close,
            }),
        ) if c == name && !t.trim().contains('\n') => {
            Some((format!("{raw}{}{close}", t.trim()), 3))
        }
        (
            Some(Token::Raw(t)),
            Some(Token::Close {
                name: c,
                raw: close,
            }),
        ) if c == name && !t.contains('\n') => Some((format!("{raw}{t}{close}"), 3)),
        _ => None,
    }
}

#[derive(Default)]
struct Writer {
    out: String,
    fold_end: Vec<u32>,
}

impl Writer {
    /// Adds one line at `depth`. Returns its index.
    fn line(&mut self, depth: usize, text: &str) -> u32 {
        if !self.fold_end.is_empty() {
            self.out.push('\n');
        }
        self.out.extend(std::iter::repeat_n(' ', depth * 2));
        self.out.push_str(text.strip_suffix('\r').unwrap_or(text));
        self.fold_end.push(NO_FOLD);
        self.last_line()
    }

    /// Adds `text` with its first line at `depth` and the next lines as
    /// received. Returns the index of the first line.
    fn block(&mut self, depth: usize, text: &str) -> u32 {
        let mut lines = text.split('\n');
        let first = self.line(depth, lines.next().unwrap_or_default());
        for l in lines {
            self.line(0, l);
        }
        first
    }

    /// Adds the content of a `RAW_TEXT` element as received, without the line
    /// break after the opening tag and the blank end before the closing tag.
    fn raw(&mut self, text: &str) {
        let t = text
            .strip_prefix("\r\n")
            .or_else(|| text.strip_prefix('\n'))
            .unwrap_or(text)
            .trim_end();
        if !t.is_empty() {
            self.block(0, t);
        }
    }

    fn fold(&mut self, open: u32, end: u32) {
        if end > open
            && let Some(slot) = self.fold_end.get_mut(open as usize)
        {
            *slot = end;
        }
    }

    fn last_line(&self) -> u32 {
        self.fold_end.len().saturating_sub(1) as u32
    }
}

fn tokenize(src: &str, html: bool) -> Vec<Token<'_>> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if !starts_markup(b, i) {
            let end = (i + 1..b.len())
                .find(|j| starts_markup(b, *j))
                .unwrap_or(b.len());
            out.push(Token::Text(&src[i..end]));
            i = end;
            continue;
        }
        let rest = &src[i..];
        let len = if rest.starts_with("<!--") {
            until(rest, 4, "-->")
        } else if rest.starts_with("<![CDATA[") {
            until(rest, 9, "]]>")
        } else if rest.starts_with("<?") {
            until(rest, 2, "?>")
        } else if rest.starts_with("<!") || rest.starts_with("</") {
            until(rest, 2, ">")
        } else {
            tag_len(rest)
        };
        let raw = &rest[..len];
        i += len;
        if raw.starts_with("<!") || raw.starts_with("<?") {
            out.push(Token::Other(raw));
        } else if let Some(r) = raw.strip_prefix("</") {
            out.push(Token::Close {
                name: tag_name(r, html),
                raw,
            });
        } else {
            let name = tag_name(&raw[1..], html);
            let empty = raw.ends_with("/>") || (html && VOID.contains(&name.as_str()));
            let raw_end = (html && !empty && RAW_TEXT.contains(&name.as_str()))
                .then(|| closing_tag(src, i, &name).unwrap_or(b.len()));
            out.push(Token::Open { name, raw, empty });
            if let Some(end) = raw_end {
                if end > i {
                    out.push(Token::Raw(&src[i..end]));
                }
                i = end;
            }
        }
    }
    out
}

/// A `<` that starts a tag, comment, doctype or processing instruction.
fn starts_markup(b: &[u8], i: usize) -> bool {
    b[i] == b'<'
        && b.get(i + 1).is_some_and(|c| {
            c.is_ascii_alphabetic() || matches!(c, b'!' | b'?' | b'/' | b'_' | b':')
        })
}

/// The length of `rest` up to the end of `end`, searched after `skip` bytes;
/// all of `rest` when `end` is missing.
fn until(rest: &str, skip: usize, end: &str) -> usize {
    rest.get(skip..)
        .and_then(|r| r.find(end))
        .map(|p| skip + p + end.len())
        .unwrap_or(rest.len())
}

/// The length of the tag at the start of `rest`: a `>` inside a quoted
/// attribute value does not end it.
fn tag_len(rest: &str) -> usize {
    let mut quote = None;
    for (j, c) in rest.bytes().enumerate().skip(1) {
        match (quote, c) {
            (None, b'"' | b'\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, b'>') => return j + 1,
            _ => {}
        }
    }
    rest.len()
}

fn tag_name(s: &str, html: bool) -> String {
    let name: String = s
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '/' && *c != '>')
        .collect();
    if html {
        name.to_ascii_lowercase()
    } else {
        name
    }
}

/// The start of the `</name` tag at or after `from`, ignoring case.
fn closing_tag(src: &str, from: usize, name: &str) -> Option<usize> {
    let b = src.as_bytes();
    let mut i = from;
    while let Some(p) = src.get(i..).and_then(|r| r.find("</")) {
        let at = i + p;
        let n = b.get(at + 2..at + 2 + name.len())?;
        if n.eq_ignore_ascii_case(name.as_bytes()) {
            return Some(at);
        }
        i = at + 2;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(src: &str, html: bool) -> (Vec<String>, Vec<u32>) {
        let (p, f) = pretty_markup(src, html).unwrap();
        (p.lines().map(String::from).collect(), f)
    }

    #[test]
    fn minified_xml_is_indented_and_folded() {
        let (l, f) = lines(
            "<?xml version=\"1.0\"?><users><user id=\"1\"><name>Ana</name><tags/></user><user id=\"2\"></user></users>",
            false,
        );
        assert_eq!(
            l,
            [
                "<?xml version=\"1.0\"?>",
                "<users>",
                "  <user id=\"1\">",
                "    <name>Ana</name>",
                "    <tags/>",
                "  </user>",
                "  <user id=\"2\"></user>",
                "</users>",
            ]
        );
        assert_eq!(f[1], 7);
        assert_eq!(f[2], 5);
        assert_eq!(f[3], NO_FOLD);
        assert_eq!(f[6], NO_FOLD);
    }

    #[test]
    fn xml_sections_and_quoted_attributes_stay_as_received() {
        let (l, _) = lines(
            "<a x=\"1>2\"><!-- c\n  d --><![CDATA[<b> & \n x]]>text &amp; more</a>",
            false,
        );
        assert_eq!(
            l,
            [
                "<a x=\"1>2\">",
                "  <!-- c",
                "  d -->",
                "  <![CDATA[<b> & ",
                " x]]>",
                "  text &amp; more",
                "</a>",
            ]
        );
    }

    #[test]
    fn broken_xml_has_no_pretty_form() {
        assert!(pretty_markup("<a><b></a>", false).is_none());
        assert!(pretty_markup("<a>", false).is_none());
        assert!(pretty_markup("</a>", false).is_none());
        assert!(pretty_markup("   ", false).is_none());
    }

    #[test]
    fn html_void_raw_text_and_open_elements() {
        let (l, f) = lines(
            "<!DOCTYPE html><HTML><body><p>One<br>two<p>Three</p><pre>\n  a\n    b\n</pre><script>if (a < b) {}</script><ul><li>x<li>y</ul></body></html>",
            true,
        );
        assert_eq!(
            l,
            [
                "<!DOCTYPE html>",
                "<HTML>",
                "  <body>",
                "    <p>",
                "      One",
                "      <br>",
                "      two",
                "      <p>Three</p>",
                "      <pre>",
                "  a",
                "    b",
                "      </pre>",
                "      <script>if (a < b) {}</script>",
                "      <ul>",
                "        <li>",
                "          x",
                "          <li>",
                "            y",
                "      </ul>",
                "  </body>",
                "</html>",
            ]
        );
        assert_eq!((f[1], f[2], f[8], f[13]), (20, 19, 11, 18));
        // Elements left open end before the closing tag of their parent.
        assert_eq!((f[3], f[14], f[16]), (18, 17, 17));
    }

    #[test]
    fn a_stray_closing_tag_in_html_is_kept() {
        let (l, _) = lines("</div><p>a</p>", true);
        assert_eq!(l, ["</div>", "<p>a</p>"]);
    }
}
