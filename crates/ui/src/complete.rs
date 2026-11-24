//! Completion for the gRPC message and MCP arguments editors (object keys from
//! the schema) and for the gRPC method / MCP server and tool fields.

use sendito_core::complete::FieldNode;
use serde_json::Value;

use crate::editor::Pos;
use crate::overlay::fuzzy;
use crate::tab::Field;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompleteKind {
    /// Object keys of the message/arguments editor.
    Fields,
    GrpcMethod,
    McpServer,
    McpTool,
    /// Object keys in the response query bar's jq expression.
    JqKey,
}

/// One step into the input tree: an object key, or an element of a repeated or
/// map field (the element keeps the same node).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Key(String),
    Element,
}

/// Where the cursor sits inside the JSON text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonContext {
    /// Keys/elements from the root to the object being edited.
    pub path: Vec<Step>,
    /// Text typed for the key so far.
    pub prefix: String,
    /// Where the replacement starts (after the opening quote, or at the first
    /// typed character of an unquoted key).
    pub start: Pos,
    /// The user already typed the opening `"` of this key.
    pub quoted: bool,
    /// Keys already present in this object, so they are not offered twice.
    pub siblings: Vec<String>,
}

/// A suggestion: a field name plus the value snippet that completes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub name: String,
    pub detail: String,
    pub example: String,
    pub cursor_back: u16,
}

impl Candidate {
    /// The text that replaces `start..cursor`.
    pub fn insert(&self, quoted: bool) -> String {
        if quoted {
            format!("{}\": {}", self.name, self.example)
        } else {
            format!("\"{}\": {}", self.name, self.example)
        }
    }
}

/// The popup state. Not an overlay: it does not dim the screen and does not
/// swallow keys other than the ones it handles.
#[derive(Debug, Clone)]
pub struct Completion {
    pub kind: CompleteKind,
    /// Every candidate for the current key position; `items` is its filtered view.
    pub pool: Vec<Candidate>,
    pub items: Vec<Candidate>,
    pub query: String,
    pub sel: usize,
    pub scroll: usize,
    /// Which tab and field opened it; any change closes it.
    pub tab: u64,
    pub field: Option<Field>,
    /// Where the replacement starts in the focused editor.
    pub start: Pos,
    pub quoted: bool,
    /// A background load is running for it.
    pub loading: Option<&'static str>,
    /// The background load failed; it is retried only when asked.
    pub failed: Option<&'static str>,
}

impl Completion {
    /// Rows the popup shows at once; also the PageUp/PageDown step.
    pub const ROWS: usize = 8;

    pub fn new(kind: CompleteKind, tab: u64, field: Option<Field>) -> Self {
        Self {
            kind,
            pool: Vec::new(),
            items: Vec::new(),
            query: String::new(),
            sel: 0,
            scroll: 0,
            tab,
            field,
            start: Pos::default(),
            quoted: false,
            loading: None,
            failed: None,
        }
    }

    pub fn selected(&self) -> Option<&Candidate> {
        self.items.get(self.sel).or_else(|| self.items.first())
    }

    pub fn up(&mut self) {
        self.sel = crate::list::cycle(self.sel, -1, self.items.len());
        self.keep_visible();
    }

    pub fn down(&mut self) {
        self.sel = crate::list::cycle(self.sel, 1, self.items.len());
        self.keep_visible();
    }

    pub fn keep_visible(&mut self) {
        self.scroll = crate::list::keep_visible(self.sel, self.scroll, Self::ROWS);
    }

    pub fn page_up(&mut self) {
        self.sel = crate::list::page(self.sel, -1, Self::ROWS, self.items.len());
        self.keep_visible();
    }

    pub fn page_down(&mut self) {
        self.sel = crate::list::page(self.sel, 1, Self::ROWS, self.items.len());
        self.keep_visible();
    }

    /// Sets the whole candidate list (e.g. servers, tools, methods).
    pub fn set_pool(&mut self, pool: Vec<Candidate>, query: String) {
        self.pool = pool;
        self.loading = None;
        self.failed = None;
        self.filter(query);
    }

    /// Narrows the current pool by what was typed.
    pub fn filter(&mut self, query: String) {
        let q = query.to_ascii_lowercase();
        self.items = self
            .pool
            .iter()
            .filter(|c| {
                let name = c.name.to_ascii_lowercase();
                query.is_empty() || name.starts_with(&q) || fuzzy(&name, &q)
            })
            .cloned()
            .collect();
        self.items.sort_by(|a, b| {
            let pa = !a.name.to_ascii_lowercase().starts_with(&q);
            let pb = !b.name.to_ascii_lowercase().starts_with(&q);
            pa.cmp(&pb).then_with(|| a.name.cmp(&b.name))
        });
        if query != self.query {
            self.sel = 0;
            self.scroll = 0;
        }
        self.query = query;
        if self.sel >= self.items.len() {
            self.sel = self.items.len().saturating_sub(1);
            self.scroll = 0;
        }
    }
}

// ----- JSON scanning -----------------------------------------------------

#[derive(Debug)]
struct Frame {
    is_object: bool,
    /// The object expects a key (right after `{` or `,`).
    expect_key: bool,
    /// Keys already seen in this object.
    keys: Vec<String>,
    /// The key whose value is being read.
    pending: Option<String>,
    /// This container added a step to the path.
    step: bool,
}

/// The object-key context at `at`, or `None` when the cursor is not in a
/// position where a key can be typed. Tolerates half-typed JSON.
pub fn json_context(text: &str, at: Pos) -> Option<JsonContext> {
    let mut stack: Vec<Frame> = Vec::new();
    let mut path: Vec<Step> = Vec::new();
    let mut in_string = false;
    let mut escape = false;
    let mut string_is_key = false;
    let mut buf = String::new();
    let mut string_start = Pos::default();
    let mut bare_start: Option<Pos> = None;
    let mut row = 0usize;
    let mut col = 0usize;
    for ch in text.chars() {
        if row == at.row && col == at.col {
            break;
        }
        let here = Pos { row, col };
        if in_string {
            if escape {
                escape = false;
                if string_is_key {
                    buf.push(ch);
                }
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
                if string_is_key {
                    if let Some(f) = stack.last_mut() {
                        f.expect_key = false;
                        f.pending = Some(buf.clone());
                        f.keys.push(buf.clone());
                    }
                    string_is_key = false;
                }
            } else if string_is_key {
                buf.push(ch);
            }
        } else {
            match ch {
                '"' => {
                    in_string = true;
                    escape = false;
                    buf.clear();
                    string_is_key = stack.last().is_some_and(|f| f.is_object && f.expect_key);
                    string_start = Pos { row, col: col + 1 };
                }
                '{' | '[' => {
                    let step = match stack.last_mut() {
                        Some(f) if f.is_object => match f.pending.take() {
                            Some(k) => {
                                path.push(Step::Key(k));
                                true
                            }
                            None => false,
                        },
                        Some(_) => {
                            path.push(Step::Element);
                            true
                        }
                        None => false,
                    };
                    stack.push(Frame {
                        is_object: ch == '{',
                        expect_key: ch == '{',
                        keys: Vec::new(),
                        pending: None,
                        step,
                    });
                    bare_start = None;
                }
                '}' | ']' => {
                    if let Some(f) = stack.pop()
                        && f.step
                    {
                        path.pop();
                    }
                }
                ',' => {
                    if let Some(f) = stack.last_mut()
                        && f.is_object
                    {
                        f.expect_key = true;
                        f.pending = None;
                    }
                    bare_start = None;
                }
                c if c.is_whitespace() => {}
                _ => {
                    if let Some(f) = stack.last_mut()
                        && f.is_object
                        && f.expect_key
                        && bare_start.is_none()
                    {
                        bare_start = Some(here);
                    }
                }
            }
        }
        if ch == '\n' {
            row += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    let frame = stack.last()?;
    if !frame.is_object {
        return None;
    }
    let quoted = in_string && string_is_key;
    if !quoted && !frame.expect_key {
        return None;
    }
    let (prefix, start) = if quoted {
        (buf, string_start)
    } else {
        (
            slice(text, bare_start.unwrap_or(at), at),
            bare_start.unwrap_or(at),
        )
    };
    Some(JsonContext {
        path,
        prefix,
        start,
        quoted,
        siblings: frame.keys.clone(),
    })
}

/// Byte offset of a `(row, col)` position, clamped to the text.
fn byte_at(text: &str, p: Pos) -> usize {
    let mut off = 0usize;
    for _ in 0..p.row {
        match text[off..].find('\n') {
            Some(i) => off += i + 1,
            None => return text.len(),
        }
    }
    let line = &text[off..];
    let end = line.find('\n').unwrap_or(line.len());
    let take = line[..end]
        .char_indices()
        .nth(p.col)
        .map(|(b, _)| b)
        .unwrap_or(end);
    off + take
}

fn slice(text: &str, a: Pos, b: Pos) -> String {
    let (a, b) = (byte_at(text, a), byte_at(text, b));
    if a >= b {
        return String::new();
    }
    text.get(a..b).unwrap_or_default().to_string()
}

/// The node for a path, descending into object keys and through repeated/map
/// elements (which keep the node). `None` when the path left the tree.
fn node_at<'a>(root: &'a FieldNode, path: &[Step]) -> Option<&'a FieldNode> {
    let mut node = root;
    for st in path {
        if let Step::Key(k) = st {
            if node.map {
                continue;
            }
            node = node.children.iter().find(|c| &c.name == k)?;
        }
    }
    Some(node)
}

/// The field names still available at `ctx`, filtered by what was typed.
pub fn field_candidates(root: &FieldNode, ctx: &JsonContext, query: &str) -> Vec<Candidate> {
    let Some(node) = node_at(root, &ctx.path) else {
        return Vec::new();
    };
    let used: Vec<&str> = node
        .children
        .iter()
        .filter(|c| c.oneof.is_some() && ctx.siblings.contains(&c.name))
        .filter_map(|c| c.oneof.as_deref())
        .collect();
    let q = query.to_ascii_lowercase();
    let mut out: Vec<Candidate> = node
        .children
        .iter()
        .filter(|c| !ctx.siblings.contains(&c.name))
        .filter(|c| c.oneof.as_deref().is_none_or(|g| !used.contains(&g)))
        .filter(|c| {
            query.is_empty()
                || fuzzy(&c.name.to_ascii_lowercase(), &q)
                || c.name.to_ascii_lowercase().starts_with(&q)
        })
        .map(|c| Candidate {
            name: c.name.clone(),
            detail: c.detail.clone(),
            example: c.example.clone(),
            cursor_back: c.cursor_back(),
        })
        .collect();
    out.sort_by(|a, b| {
        let pa = !a.name.to_ascii_lowercase().starts_with(&q);
        let pb = !b.name.to_ascii_lowercase().starts_with(&q);
        pa.cmp(&pb).then_with(|| a.name.cmp(&b.name))
    });
    out
}

/// Candidate list from a plain list of names (servers, tools, methods).
pub fn plain_candidates(
    items: impl IntoIterator<Item = (String, String)>,
    query: &str,
) -> Vec<Candidate> {
    let q = query.to_ascii_lowercase();
    let mut out: Vec<Candidate> = items
        .into_iter()
        .filter(|(name, _)| {
            let name = name.to_ascii_lowercase();
            query.is_empty() || name.starts_with(&q) || fuzzy(&name, &q)
        })
        .map(|(name, detail)| Candidate {
            name,
            detail,
            example: String::new(),
            cursor_back: 0,
        })
        .collect();
    out.sort_by(|a, b| {
        let pa = !a.name.to_ascii_lowercase().starts_with(&q);
        let pb = !b.name.to_ascii_lowercase().starts_with(&q);
        pa.cmp(&pb).then_with(|| a.name.cmp(&b.name))
    });
    out
}

// ----- MCP input schema --------------------------------------------------

/// Converts a tool's `inputSchema` into the field tree.
pub fn schema_node(schema: &Value, name: &str) -> FieldNode {
    FieldNode {
        name: name.to_string(),
        detail: String::new(),
        map: false,
        example: "{}".into(),
        children: schema_fields(schema, schema, 0),
        oneof: None,
    }
}

fn deref<'a>(root: &'a Value, v: &'a Value) -> &'a Value {
    let mut cur = v;
    for _ in 0..8 {
        let Some(r) = cur.get("$ref").and_then(Value::as_str) else {
            return cur;
        };
        let key = r.rsplit('/').next().unwrap_or(r);
        let target = ["$defs", "definitions"]
            .iter()
            .find_map(|k| root.get(*k).and_then(|d| d.get(key)));
        match target {
            Some(t) => cur = t,
            None => return cur,
        }
    }
    cur
}

/// A schema that only says "one of these": use the first object alternative so
/// its keys can be offered, like the arguments template does.
fn variant(v: &Value) -> &Value {
    for key in ["anyOf", "oneOf"] {
        if let Some(list) = v.get(key).and_then(Value::as_array)
            && let Some(first) = list
                .iter()
                .find(|s| {
                    s.get("properties").is_some()
                        || s.get("type").and_then(Value::as_str) == Some("object")
                })
                .or_else(|| list.first())
        {
            return first;
        }
    }
    v
}

fn schema_fields(root: &Value, node: &Value, depth: usize) -> Vec<FieldNode> {
    if depth > 8 {
        return Vec::new();
    }
    let node = deref(root, variant(node));
    let Some(props) = node.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    props
        .iter()
        .map(|(name, v)| {
            let v = deref(root, variant(v));
            let detail = type_detail(v);
            let example = value_example(v);
            FieldNode {
                name: name.clone(),
                detail,
                map: false,
                example,
                children: schema_fields(root, v, depth + 1),
                oneof: None,
            }
        })
        .collect()
}

fn type_detail(v: &Value) -> String {
    let t = v.get("type").and_then(Value::as_str);
    match t {
        Some("array") => {
            let inner = v.get("items").map(type_detail).unwrap_or_default();
            if inner.is_empty() {
                "array".into()
            } else {
                format!("array of {inner}")
            }
        }
        Some("object") => "object".into(),
        Some("integer") => "integer".into(),
        Some("number") => "number".into(),
        Some("boolean") => "boolean".into(),
        Some("null") => "null".into(),
        Some(other) => other.to_string(),
        None if v.get("properties").is_some() => "object".into(),
        None => "any".into(),
    }
}

fn value_example(v: &Value) -> String {
    if let Some(d) = v.get("default")
        && !d.is_null()
    {
        return serde_json::to_string(d).unwrap_or_else(|_| "null".into());
    }
    if let Some(first) = v
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|e| e.first())
    {
        return serde_json::to_string(first).unwrap_or_else(|_| "null".into());
    }
    match v.get("type").and_then(Value::as_str) {
        Some("string") => "\"\"".into(),
        Some("integer") | Some("number") => "0".into(),
        Some("boolean") => "false".into(),
        Some("array") => "[]".into(),
        Some("object") => "{}".into(),
        Some("null") => "null".into(),
        None if v.get("properties").is_some() => "{}".into(),
        _ => "null".into(),
    }
}

// ----- jq keys -------------------------------------------------------------

/// Where an object key can be completed in a jq expression with the cursor
/// at char `col`: the expression whose output holds the keys (`""` for the
/// input), the key typed so far, and the char column where that key starts.
/// `None` when the cursor is not after a path step's `.`.
pub fn jq_context(text: &str, col: usize) -> Option<(String, String, usize)> {
    let before: Vec<char> = text.chars().take(col).collect();
    let mut start = before.len();
    while start > 0 && (before[start - 1].is_ascii_alphanumeric() || before[start - 1] == '_') {
        start -= 1;
    }
    if start == 0 || before[start - 1] != '.' {
        return None;
    }
    let prefix: String = before[start..].iter().collect();
    let base: String = before[..start - 1].iter().collect();
    let base = base.trim_end();
    // After a pipe, the step reads the output of what comes before it.
    let base = base.strip_suffix('|').map_or(base, str::trim_end);
    // Inside a call or after an operator the step has another input.
    let ok = base
        .chars()
        .last()
        .is_none_or(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ']' | ')' | '"' | '?'));
    ok.then(|| (base.to_string(), prefix, start))
}

/// The keys of the objects that `base` gives over the body, with the type
/// of their first value.
pub fn jq_keys(base: &str, src: &[u8]) -> Vec<Candidate> {
    let expr = if base.trim().is_empty() { "." } else { base };
    let Ok(outs) = crate::filter::apply(expr, src) else {
        return Vec::new();
    };
    let mut keys: std::collections::BTreeMap<String, &'static str> = Default::default();
    for o in outs.iter().take(200) {
        if let Ok(Value::Object(m)) = serde_json::from_slice::<Value>(o) {
            for (k, v) in m {
                let kind = match v {
                    Value::Object(_) => "object",
                    Value::Array(_) => "array",
                    Value::String(_) => "string",
                    Value::Number(_) => "number",
                    Value::Bool(_) => "boolean",
                    Value::Null => "null",
                };
                keys.entry(k).or_insert(kind);
            }
        }
    }
    keys.into_iter()
        .map(|(name, detail)| Candidate {
            name,
            detail: detail.to_string(),
            example: String::new(),
            cursor_back: 0,
        })
        .collect()
}

/// The text a key takes after `.` in a jq path: the name, or the name in
/// quotes when it is not a plain identifier.
pub fn jq_key_text(name: &str) -> String {
    let plain = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if plain {
        name.to_string()
    } else {
        serde_json::to_string(name).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn jq_context_finds_the_step_input() {
        use super::jq_context;
        let at = |t: &str| jq_context(t, t.chars().count());
        assert_eq!(at("."), Some((String::new(), String::new(), 1)));
        assert_eq!(at(".items[].na"), Some((".items[]".into(), "na".into(), 9)));
        assert_eq!(
            at(".items[] | .na"),
            Some((".items[]".into(), "na".into(), 12))
        );
        assert_eq!(at("map(.na"), None);
        assert_eq!(at("na"), None);
    }

    #[test]
    fn jq_keys_lists_object_keys() {
        use super::{jq_key_text, jq_keys};
        let body = br#"{"items":[{"id":1,"a b":true},{"id":2,"name":"x"}]}"#;
        let names: Vec<String> = jq_keys(".items[]", body)
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, ["a b", "id", "name"]);
        assert_eq!(jq_keys("", body)[0].detail, "array");
        assert!(jq_keys(".items[", body).is_empty());
        // A quoted key is a valid path step.
        let step = format!(".items[0].{}", jq_key_text("a b"));
        assert_eq!(
            crate::filter::apply(&step, body),
            Ok(vec![b"true".to_vec()])
        );
        assert_eq!(jq_key_text("id"), "id");
    }

    use super::*;

    fn ctx(text: &str) -> Option<JsonContext> {
        let at = Pos {
            row: 0,
            col: text.chars().count(),
        };
        json_context(text, at)
    }

    /// Context at the end of a multi-line text.
    fn ctx_end(text: &str) -> Option<JsonContext> {
        let lines: Vec<&str> = text.split('\n').collect();
        let at = Pos {
            row: lines.len() - 1,
            col: lines.last().map(|l| l.chars().count()).unwrap_or(0),
        };
        json_context(text, at)
    }

    #[test]
    fn empty_object_asks_for_a_key() {
        let c = ctx("{").expect("key position");
        assert!(c.path.is_empty());
        assert_eq!(c.prefix, "");
        assert!(!c.quoted);
    }

    #[test]
    fn after_a_comma_asks_for_a_key() {
        let c = ctx("{\"text\": \"x\", ").expect("key position");
        assert_eq!(c.siblings, vec!["text".to_string()]);
        assert_eq!(c.prefix, "");
    }

    #[test]
    fn partial_quoted_key() {
        let c = ctx("{\"te").expect("key position");
        assert!(c.quoted);
        assert_eq!(c.prefix, "te");
        assert_eq!(c.start, Pos { row: 0, col: 2 });
    }

    #[test]
    fn partial_bare_key() {
        let c = ctx_end("{\n  te").expect("key position");
        assert!(!c.quoted);
        assert_eq!(c.prefix, "te");
        assert_eq!(c.start, Pos { row: 1, col: 2 });
    }

    #[test]
    fn not_a_key_position_inside_a_value() {
        assert!(ctx("{\"text\": \"ab").is_none());
        assert!(ctx("{\"text\": ").is_none());
    }

    #[test]
    fn complete_key_is_not_edited() {
        assert!(ctx("{\"text\"").is_none());
        assert!(ctx("{\"text\": 1").is_none());
    }

    #[test]
    fn nested_path_and_array_elements() {
        let c = ctx("{\"inner\": {\"x\": 1, ").expect("key position");
        assert_eq!(c.path, vec![Step::Key("inner".into())]);
        assert_eq!(c.siblings, vec!["x".to_string()]);

        let c = ctx("[{\"x\": 1}, {\"y\": 2, ").expect("key position inside an array");
        assert_eq!(c.path, vec![Step::Element]);
        assert_eq!(c.siblings, vec!["y".to_string()]);
    }

    #[test]
    fn scan_survives_broken_text() {
        assert!(json_context("", Pos::default()).is_none());
        assert!(json_context("{\"a\": [1, 2", Pos { row: 0, col: 12 }).is_none());
        let at = Pos { row: 2, col: 99 };
        assert!(json_context("{\n", at).is_some());
    }

    fn tree() -> FieldNode {
        let mut root = FieldNode::new("Root", "", "{}");
        root.children.push(FieldNode::new("text", "string", "\"\""));
        let mut inner = FieldNode::new("inner", "Inner", "{}");
        inner.children.push(FieldNode::new("count", "int32", "0"));
        root.children.push(inner);
        root.children
            .push(FieldNode::new("tags", "repeated string", "[]"));
        root
    }

    #[test]
    fn candidates_hide_present_keys_and_filter() {
        let c = ctx("{\"text\": \"x\", te").expect("key position");
        let got = field_candidates(&tree(), &c, "");
        let names: Vec<&str> = got.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["inner", "tags"]);

        let got = field_candidates(&tree(), &c, "in");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "inner");
        assert_eq!(got[0].insert(false), "\"inner\": {}");
        assert_eq!(got[0].insert(true), "inner\": {}");
    }

    #[test]
    fn candidates_descend_into_nested_objects() {
        let c = ctx("{\"inner\": {\"co").expect("key position");
        let got = field_candidates(&tree(), &c, "co");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "count");
        assert_eq!(got[0].cursor_back, 0);
    }

    #[test]
    fn oneof_group_is_offered_once() {
        let mut root = FieldNode::new("Root", "", "{}");
        let mut a = FieldNode::new("name", "string", "\"\"");
        a.oneof = Some("choice".into());
        let mut b = FieldNode::new("number", "int32", "0");
        b.oneof = Some("choice".into());
        root.children.extend([a, b]);
        let c = ctx("{\"name\": \"x\", ").expect("key position");
        assert!(field_candidates(&root, &c, "").is_empty());
    }

    #[test]
    fn mcp_schema_becomes_fields() {
        let schema: Value = serde_json::json!({
            "type": "object",
            "properties": {
                "a": {"type": "integer", "default": 3},
                "b": {"type": "string", "enum": ["x", "y"]},
                "c": {"type": "object", "properties": {"d": {"type": "boolean"}}},
                "e": {"$ref": "#/$defs/item"}
            },
            "$defs": {"item": {"type": "object", "properties": {"f": {"type": "string"}}}}
        });
        let root = schema_node(&schema, "tool");
        let names: Vec<&str> = root.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c", "e"]);
        assert_eq!(root.children[0].example, "3");
        assert_eq!(root.children[1].example, "\"x\"");
        assert_eq!(root.children[2].detail, "object");
        assert_eq!(root.children[2].children[0].name, "d");
        assert_eq!(root.children[3].children[0].name, "f");
    }
}
