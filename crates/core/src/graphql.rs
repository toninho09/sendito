//! Heuristic GraphQL cursor context. This module does not validate operations.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GqlStep {
    Field(String),
    OnType(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GqlPosition {
    Field,
    Argument,
    OnType,
}

#[derive(Debug, Clone)]
pub struct GraphqlContext {
    pub operation: String,
    pub path: Vec<GqlStep>,
    pub position: GqlPosition,
    pub field: Option<String>,
    pub siblings: Vec<String>,
    /// Byte offset of the partial name.
    pub start: usize,
    pub prefix: String,
}

fn name(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Returns the context before the cursor. Ignores comments and string contents.
pub fn graphql_context(before: &str) -> Option<GraphqlContext> {
    let bytes = before.as_bytes();
    let mut tokens: Vec<String> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' if bytes[i..].starts_with(b"{{") => {
                let end = before[i + 2..].find("}}")?;
                i += end + 4;
                tokens.push("VALUE".into());
            }
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                if i == bytes.len() {
                    return None;
                }
            }
            b'"' => {
                let block = bytes[i..].starts_with(b"\"\"\"");
                let delimiter = if block { 3 } else { 1 };
                i += delimiter;
                loop {
                    if i >= bytes.len() {
                        return None;
                    }
                    if bytes[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if (block && bytes[i..].starts_with(b"\"\"\"")) || (!block && bytes[i] == b'"')
                    {
                        i += delimiter;
                        break;
                    }
                    i += 1;
                }
                tokens.push("VALUE".into());
            }
            c if c.is_ascii_whitespace() => i += 1,
            c if name(c) => {
                let start = i;
                while i < bytes.len() && name(bytes[i]) {
                    i += 1;
                }
                tokens.push(before[start..i].into());
            }
            _ => {
                tokens.push((bytes[i] as char).to_string());
                i += 1;
            }
        }
    }
    let mut start = bytes.len();
    while start > 0 && name(bytes[start - 1]) {
        start -= 1;
    }
    let prefix = before[start..].to_string();
    if !prefix.is_empty() {
        tokens.pop();
    }
    if prefix
        .as_bytes()
        .first()
        .is_some_and(|b| b.is_ascii_digit())
    {
        return None;
    }
    let mut operation = "query".to_string();
    let mut stack: Vec<(Option<GqlStep>, Vec<String>)> = Vec::new();
    let mut pending: Option<String> = None;
    let mut fragment = false;
    let mut on_type = false;
    let mut directive = false;
    let mut parens = 0usize;
    let mut value_depth = 0usize;
    let mut argument_field = None;
    let mut arguments = Vec::new();
    let mut argument_name = None;
    let mut expect_argument = true;
    let mut variable = false;
    for (index, token) in tokens.iter().enumerate() {
        if parens > 0 {
            match token.as_str() {
                "(" => parens += 1,
                ")" => {
                    parens -= 1;
                    if parens == 0 {
                        directive = false;
                    }
                }
                "{" | "[" => value_depth += 1,
                "}" | "]" => {
                    value_depth = value_depth.saturating_sub(1);
                    if value_depth == 0 {
                        expect_argument = true;
                    }
                }
                ":" if value_depth == 0 => {
                    if let Some(name) = argument_name.take() {
                        arguments.push(name);
                    }
                    expect_argument = false;
                }
                "," if value_depth == 0 => expect_argument = true,
                "$" if value_depth == 0 => variable = true,
                _ if value_depth == 0 && parens == 1 => {
                    if variable {
                        variable = false;
                        expect_argument = true;
                    } else if expect_argument {
                        argument_name = Some(token.clone());
                    } else {
                        expect_argument = true;
                    }
                }
                _ => {}
            }
            continue;
        }
        match token.as_str() {
            "query" | "mutation" | "subscription" if stack.is_empty() => operation = token.clone(),
            "fragment" if stack.is_empty() => fragment = true,
            "." => {
                pending = None;
                fragment = true;
            }
            "on" if fragment => on_type = true,
            "@" => directive = true,
            "(" => {
                parens = 1;
                argument_field = if directive { None } else { pending.clone() };
                arguments.clear();
                argument_name = None;
                expect_argument = true;
            }
            "{" => {
                let step = if on_type {
                    pending.take().map(GqlStep::OnType)
                } else {
                    pending.take().map(GqlStep::Field)
                };
                stack.push((
                    if stack.is_empty() && !on_type {
                        None
                    } else {
                        step
                    },
                    Vec::new(),
                ));
                fragment = false;
                on_type = false;
                directive = false;
            }
            "}" => {
                stack.pop();
                pending = None;
            }
            ":" | "," => {}
            _ if directive => {
                directive = tokens.get(index + 1).is_some_and(|t| t == "(");
            }
            _ if fragment && !on_type && !stack.is_empty() => {
                fragment = false;
                pending = None;
            }
            _ => {
                pending = Some(token.clone());
                if !on_type
                    && !fragment
                    && let Some((_, siblings)) = stack.last_mut()
                {
                    siblings.push(token.clone());
                }
            }
        }
    }
    if stack.is_empty() {
        return None;
    }
    let position = if on_type {
        GqlPosition::OnType
    } else if parens > 0
        && argument_field.is_some()
        && value_depth == 0
        && expect_argument
        && !directive
    {
        GqlPosition::Argument
    } else if parens > 0 || directive || fragment {
        return None;
    } else {
        GqlPosition::Field
    };
    Some(GraphqlContext {
        operation,
        path: stack.iter().filter_map(|(step, _)| step.clone()).collect(),
        position,
        field: argument_field,
        siblings: if position == GqlPosition::Argument {
            arguments
        } else {
            stack.last()?.1.clone()
        },
        start,
        prefix,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn graphql_context_paths() {
        let c = graphql_context("query Q($id: ID!) { alias: hero(id: $id) { na").unwrap();
        assert_eq!(c.path, vec![GqlStep::Field("hero".into())]);
        assert_eq!(c.prefix, "na");
        assert_eq!(c.position, GqlPosition::Field);
        let c = graphql_context("{ hero { ... on Droid { pri").unwrap();
        assert_eq!(
            c.path,
            vec![
                GqlStep::Field("hero".into()),
                GqlStep::OnType("Droid".into())
            ]
        );
        assert_eq!(
            graphql_context("{ hero { ... on ").unwrap().position,
            GqlPosition::OnType
        );
        let c = graphql_context("mutation M { update(id: 1, na").unwrap();
        assert_eq!(c.operation, "mutation");
        assert_eq!(c.siblings, vec!["id"]);
        assert_eq!(c.field.as_deref(), Some("update"));
        assert_eq!(c.position, GqlPosition::Argument);
    }
    #[test]
    fn graphql_context_fragments_directives_and_templates() {
        for text in [
            "{ hero @include(if: true) { na",
            "{ hero @custom { na",
            "{ hero { name } hero { na",
        ] {
            let c = graphql_context(text).unwrap();
            assert_eq!(c.path, vec![GqlStep::Field("hero".into())], "{text}");
        }
        let c = graphql_context("fragment Details on Hero { na").unwrap();
        assert_eq!(c.path, vec![GqlStep::OnType("Hero".into())]);
        assert!(graphql_context("{ hero(id: {{missing").is_none());
        assert!(graphql_context("{ hero(id: {{value}}) { na").is_some());
        assert_eq!(
            graphql_context("{ hero { ...Details na").unwrap().prefix,
            "na"
        );
    }

    #[test]
    fn graphql_context_ignores_literals() {
        for text in [
            "{ hero(name: \"open",
            "{ # comment",
            "{ hero(value: { x: ",
            "{ hero @skip(if: ",
            "{ hero(name: \"\"\"open",
        ] {
            assert!(graphql_context(text).is_none(), "{text}");
        }
        let c = graphql_context("{ hero(name: \"{ nope }\") { id # ignored }\n na").unwrap();
        assert_eq!(c.path, vec![GqlStep::Field("hero".into())]);
        assert_eq!(c.siblings, vec!["id"]);
        assert!(graphql_context("{ hero { id } }").is_none());
    }
}
