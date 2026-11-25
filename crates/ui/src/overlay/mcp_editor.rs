//! MCP server kinds and auths as the editors name them, and stdio
//! command-line parsing.

/// The kinds of an MCP server, in the form's order.
pub const MCP_KINDS: &[&str] = &["http", "stdio"];

/// The auths of an HTTP MCP server, in the form's order.
pub const MCP_AUTHS: &[&str] = &["none", "bearer", "oauth"];

/// A stdio server as one line: the command, then its arguments, quoted only
/// when they hold spaces (so the line splits back into the same parts).
pub fn stdio_command_line(command: &str, args: &[String]) -> String {
    let mut parts = vec![quote_arg(command)];
    parts.extend(args.iter().map(|a| quote_arg(a)));
    parts.join(" ")
}

/// Splits a stdio command line on spaces, honoring single and double quotes.
pub fn split_command_line(line: &str) -> (String, Vec<String>) {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut has = false;
    for c in line.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                has = true;
            }
            None if c.is_whitespace() => {
                if has || !cur.is_empty() {
                    parts.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            None => cur.push(c),
        }
    }
    if has || !cur.is_empty() {
        parts.push(cur);
    }
    let mut it = parts.into_iter();
    let command = it.next().unwrap_or_default();
    (command, it.collect())
}

pub(super) fn quote_arg(s: &str) -> String {
    if s.is_empty() {
        "''".to_string()
    } else if s.chars().any(char::is_whitespace) {
        format!("\"{s}\"")
    } else {
        s.to_string()
    }
}
