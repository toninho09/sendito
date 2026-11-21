//! `{{var}}` template parsing.
//!
//! A placeholder is `{{` + optional spaces + a name + optional spaces + `}}`.
//! Names may contain ASCII letters, digits, `_`, `-`, `.` and a leading `$`
//! (dynamic variables such as `{{$uuid}}`). Anything else between braces is
//! literal text.

use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment<'a> {
    Text(&'a str),
    Var { name: &'a str, span: Range<usize> },
}

fn is_name_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.')
}

/// Splits `s` into literal text and variable placeholders.
pub fn parse(s: &str) -> Vec<Segment<'_>> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut text_start = 0;
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'{'
            && bytes[i + 1] == b'{'
            && let Some((name_range, end)) = scan_placeholder(bytes, i)
        {
            if text_start < i {
                out.push(Segment::Text(&s[text_start..i]));
            }
            out.push(Segment::Var {
                name: &s[name_range],
                span: i..end,
            });
            i = end;
            text_start = end;
            continue;
        }
        i += 1;
    }
    if text_start < s.len() {
        out.push(Segment::Text(&s[text_start..]));
    }
    out
}

/// Tries to read a placeholder starting at `start` (which points at `{{`).
/// Returns the name byte range and the index after the closing `}}`.
fn scan_placeholder(b: &[u8], start: usize) -> Option<(Range<usize>, usize)> {
    let mut j = start + 2;
    while j < b.len() && b[j] == b' ' {
        j += 1;
    }
    let name_start = j;
    if j < b.len() && b[j] == b'$' {
        j += 1;
    }
    while j < b.len() && is_name_char(b[j]) {
        j += 1;
    }
    let name_end = j;
    if name_end == name_start || (name_end == name_start + 1 && b[name_start] == b'$') {
        return None;
    }
    while j < b.len() && b[j] == b' ' {
        j += 1;
    }
    if j + 1 < b.len() && b[j] == b'}' && b[j + 1] == b'}' {
        Some((name_start..name_end, j + 2))
    } else {
        None
    }
}

/// Byte spans of `{{}}` / `{{ }}`: braces with no name, which are sent as
/// literal text (editors flag them, as they are usually a mistake).
pub fn empty_placeholders(s: &str) -> Vec<Range<usize>> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] == b'{' && b[i + 1] == b'{' {
            let mut j = i + 2;
            while j < b.len() && b[j] == b' ' {
                j += 1;
            }
            if j + 1 < b.len() && b[j] == b'}' && b[j + 1] == b'}' {
                out.push(i..j + 2);
                i = j + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Names of all variables referenced in `s`, in order of appearance (may repeat).
pub fn var_names(s: &str) -> impl Iterator<Item = &str> {
    parse(s).into_iter().filter_map(|seg| match seg {
        Segment::Var { name, .. } => Some(name),
        Segment::Text(_) => None,
    })
}

/// Byte spans of every placeholder in `s` together with the variable name.
pub fn spans(s: &str) -> Vec<(Range<usize>, &str)> {
    parse(s)
        .into_iter()
        .filter_map(|seg| match seg {
            Segment::Var { name, span } => Some((span, name)),
            Segment::Text(_) => None,
        })
        .collect()
}

pub fn has_vars(s: &str) -> bool {
    s.contains("{{") && var_names(s).next().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_placeholders_are_found() {
        assert_eq!(empty_placeholders("a{{}}b{{ }}c{{x}}"), vec![1..5, 6..11]);
        assert!(empty_placeholders("{{x}} {").is_empty());
        assert!(var_names("{{}}").next().is_none());
    }

    #[test]
    fn parses_basic() {
        let segs = parse("http://{{host}}:{{ port }}/x");
        assert_eq!(
            segs,
            vec![
                Segment::Text("http://"),
                Segment::Var {
                    name: "host",
                    span: 7..15
                },
                Segment::Text(":"),
                Segment::Var {
                    name: "port",
                    span: 16..26
                },
                Segment::Text("/x"),
            ]
        );
    }

    #[test]
    fn ignores_non_placeholders() {
        assert_eq!(parse("{{}}"), vec![Segment::Text("{{}}")]);
        assert_eq!(parse("{{ a b }}"), vec![Segment::Text("{{ a b }}")]);
        assert_eq!(parse("{{$}}"), vec![Segment::Text("{{$}}")]);
        assert_eq!(
            parse("{\"a\":{\"b\":1}}"),
            vec![Segment::Text("{\"a\":{\"b\":1}}")]
        );
        assert_eq!(parse("{{a"), vec![Segment::Text("{{a")]);
        assert_eq!(parse(""), vec![]);
    }

    #[test]
    fn dynamic_and_nested() {
        let names: Vec<_> = var_names("{{$uuid}}-{{{x}}}-{{a.b-c_d}}").collect();
        assert_eq!(names, vec!["$uuid", "x", "a.b-c_d"]);
        assert!(has_vars("a{{b}}"));
        assert!(!has_vars("a{{ }}"));
        assert_eq!(spans("x{{y}}"), vec![(1..6, "y")]);
    }

    #[test]
    fn unicode_text() {
        let names: Vec<_> = var_names("café {{ñ}} {{ok}}").collect();
        assert_eq!(names, vec!["ok"]);
    }
}
