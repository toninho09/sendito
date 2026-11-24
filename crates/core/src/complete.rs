//! Shape of a message/tool input, for field-name completion in the editors.
//!
//! The gRPC schema builds it from proto descriptors and the MCP client builds it
//! from a tool's `inputSchema`; the UI walks it to suggest the keys of the object
//! under the cursor.

/// One field of an input message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldNode {
    /// The key as it must appear in the JSON (proto field name or JSON Schema property).
    pub name: String,
    /// Short type shown next to the name (`string`, `Inner`, `map<string, int64>`).
    pub detail: String,
    /// `true` for a map: the value shape is described by `children`, if any.
    pub map: bool,
    /// Text that completes `"name": ` (e.g. `""` for a string, `{}` for an object).
    pub example: String,
    /// Fields of a nested object; the value shape of a repeated/map field.
    pub children: Vec<FieldNode>,
    /// A proto3 `oneof` group: only one field of the group may be set.
    pub oneof: Option<String>,
}

impl FieldNode {
    pub fn new(
        name: impl Into<String>,
        detail: impl Into<String>,
        example: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            detail: detail.into(),
            map: false,
            example: example.into(),
            children: Vec::new(),
            oneof: None,
        }
    }

    /// The node a nested object starts from, so path lookups can descend into it.
    pub fn object(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            detail: String::new(),
            map: false,
            example: "{}".into(),
            children: Vec::new(),
            oneof: None,
        }
    }

    /// Where the cursor rests after the snippet is inserted: inside `""`, `{}`,
    /// `[]`, or at the end.
    pub fn cursor_back(&self) -> u16 {
        match self.example.as_str() {
            "\"\"" | "{}" | "[]" => 1,
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_back_marks_placeholders() {
        assert_eq!(FieldNode::new("a", "string", "\"\"").cursor_back(), 1);
        assert_eq!(FieldNode::new("b", "Inner", "{}").cursor_back(), 1);
        assert_eq!(FieldNode::new("c", "string", "[]").cursor_back(), 1);
        assert_eq!(FieldNode::new("d", "int32", "0").cursor_back(), 0);
        assert_eq!(FieldNode::new("e", "bool", "false").cursor_back(), 0);
    }
}
