//! A tiny canonical TOML emitter.
//!
//! The output depends only on the value written, never on how a file looked
//! before, which makes re-saving stable byte for byte.

/// Writes TOML text in a fixed layout.
#[derive(Default)]
pub struct Writer {
    out: String,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn finish(self) -> String {
        self.out
    }

    fn blank_line(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with("\n\n") {
            self.out.push('\n');
        }
    }

    pub fn table(&mut self, name: &str) {
        self.blank_line();
        self.out.push('[');
        self.out.push_str(name);
        self.out.push_str("]\n");
    }

    pub fn array_table(&mut self, name: &str) {
        self.blank_line();
        self.out.push_str("[[");
        self.out.push_str(name);
        self.out.push_str("]]\n");
    }

    pub fn str(&mut self, key: &str, value: &str) {
        self.key(key);
        self.out.push_str(&quote(value));
        self.out.push('\n');
    }

    pub fn bool(&mut self, key: &str, value: bool) {
        self.key(key);
        self.out.push_str(if value { "true" } else { "false" });
        self.out.push('\n');
    }

    pub fn int(&mut self, key: &str, value: u64) {
        self.key(key);
        self.out.push_str(&value.to_string());
        self.out.push('\n');
    }

    pub fn str_array(&mut self, key: &str, values: &[String]) {
        self.key(key);
        self.out.push('[');
        for (i, v) in values.iter().enumerate() {
            if i > 0 {
                self.out.push_str(", ");
            }
            self.out.push_str(&quote_basic(v));
        }
        self.out.push_str("]\n");
    }

    /// `key = { a = "x", b = "y" }` with string values.
    pub fn inline_table(&mut self, key: &str, pairs: &[(&str, &str)]) {
        self.key(key);
        self.out.push_str("{ ");
        for (i, (k, v)) in pairs.iter().enumerate() {
            if i > 0 {
                self.out.push_str(", ");
            }
            self.out.push_str(&bare_or_quoted_key(k));
            self.out.push_str(" = ");
            self.out.push_str(&quote_basic(v));
        }
        self.out.push_str(" }\n");
    }

    pub fn raw_line(&mut self, line: &str) {
        self.out.push_str(line);
        self.out.push('\n');
    }

    fn key(&mut self, key: &str) {
        self.out.push_str(&bare_or_quoted_key(key));
        self.out.push_str(" = ");
    }
}

pub fn is_bare_key(k: &str) -> bool {
    !k.is_empty()
        && k.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub fn bare_or_quoted_key(k: &str) -> String {
    if is_bare_key(k) {
        k.to_string()
    } else {
        quote_basic(k)
    }
}

/// Quotes a string, choosing a multi-line form when it contains newlines.
pub fn quote(s: &str) -> String {
    if !s.contains('\n') {
        return quote_basic(s);
    }
    let literal_ok = !s.contains("'''")
        && !s.ends_with('\'')
        && s.chars()
            .all(|c| c == '\n' || c == '\t' || !(c.is_control()));
    if literal_ok {
        format!("'''\n{s}'''")
    } else {
        let mut out = String::from("\"\"\"\n");
        for c in s.chars() {
            match c {
                '\n' => out.push('\n'),
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                c => push_escaped(&mut out, c),
            }
        }
        out.push_str("\"\"\"");
        out
    }
}

/// A single-line basic string with escapes.
pub fn quote_basic(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => push_escaped(&mut out, c),
        }
    }
    out.push('"');
    out
}

fn push_escaped(out: &mut String, c: char) {
    match c {
        '\t' => out.push_str("\\t"),
        '\r' => out.push_str("\\r"),
        '\u{8}' => out.push_str("\\b"),
        '\u{c}' => out.push_str("\\f"),
        c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
        c => out.push(c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(s: &str) {
        let doc = format!("v = {}\n", quote(s));
        let t: toml::Table = toml::from_str(&doc).unwrap_or_else(|e| panic!("{doc}: {e}"));
        assert_eq!(t["v"].as_str(), Some(s), "doc: {doc}");
    }

    #[test]
    fn strings_roundtrip() {
        for s in [
            "",
            "plain",
            "with \"quotes\" and \\ back",
            "tab\there",
            "line1\nline2",
            "line1\nline2\n",
            "\nleading newline",
            "ends with quote'\n'",
            "has ''' inside\nx",
            "cr\r\nlf",
            "ctrl\u{1}\nx",
            "unicode é 🚀\nok",
            "\"\"\"\n\"",
            "back\\slash\nend\\",
            "\u{7f}del",
        ] {
            roundtrip(s);
        }
    }

    #[test]
    fn writer_layout() {
        let mut w = Writer::new();
        w.str("kind", "http");
        w.int("n", 5);
        w.bool("b", false);
        w.table("options");
        w.str_array("files", &["a".into(), "b\"c".into()]);
        w.array_table("params");
        w.str("name", "x");
        w.inline_table("weird key", &[("source", "local")]);
        w.raw_line("# end");
        let out = w.finish();
        assert_eq!(
            out,
            "kind = \"http\"\nn = 5\nb = false\n\n[options]\nfiles = [\"a\", \"b\\\"c\"]\n\n[[params]]\nname = \"x\"\n\"weird key\" = { source = \"local\" }\n# end\n"
        );
        assert!(toml::from_str::<toml::Table>(&out).is_ok());
    }
}
