//! The MCP server editor overlay and stdio command-line parsing.

use super::*;

/// The MCP servers from `[mcp.servers]`, edited as a table: a name, the kind
/// (stdio or http), the command or URL, and the auth of an HTTP server (none,
/// bearer token or OAuth). Extra settings (args, env, headers, OAuth fields)
/// are kept as they are in the file and are edited there.
pub struct McpEditor {
    pub table: Table,
    pub dirty: bool,
    pub error: Option<String>,
}

pub const MCP_KINDS: &[&str] = &["http", "stdio"];

pub const MCP_AUTHS: &[&str] = &["none", "bearer", "oauth"];

/// One row of the MCP servers editor.
pub struct McpRow {
    pub name: String,
    /// Index in `MCP_KINDS`.
    pub kind: usize,
    pub value: String,
    /// Index in `MCP_AUTHS`.
    pub auth: usize,
    pub token: String,
}

impl McpEditor {
    pub fn table() -> Table {
        Table::new(
            vec![
                Column {
                    title: "Name",
                    kind: ColKind::Text,
                    weight: 2,
                },
                Column {
                    title: "Kind",
                    kind: ColKind::Choice(MCP_KINDS),
                    weight: 1,
                },
                Column {
                    title: "URL or command",
                    kind: ColKind::Text,
                    weight: 5,
                },
                Column {
                    title: "Auth",
                    kind: ColKind::Choice(MCP_AUTHS),
                    weight: 1,
                },
                Column {
                    title: "Token",
                    kind: ColKind::Text,
                    weight: 2,
                },
            ],
            false,
        )
    }

    /// Loads `servers` into the table.
    pub fn load(
        &mut self,
        servers: &std::collections::BTreeMap<String, sendito_store::config::McpServer>,
    ) {
        use sendito_store::config::{McpServer, McpServerAuth};
        let mut t = Self::table();
        for (name, s) in servers {
            let (kind, value, auth, token) = match s {
                McpServer::Http(h) => {
                    let (auth, token) = match &h.auth {
                        None => (0, ""),
                        Some(McpServerAuth::Bearer { token }) => (1, token.as_str()),
                        Some(McpServerAuth::Oauth { .. }) => (2, ""),
                    };
                    (0, h.url.clone(), auth, token.to_string())
                }
                // stdio rows read as the command line they start: command and
                // arguments together (env stays in the file).
                McpServer::Stdio(s) => {
                    (1, stdio_command_line(&s.command, &s.args), 0, String::new())
                }
            };
            t.push(
                true,
                vec![
                    Cell::Text(TextEditor::single(name)),
                    Cell::Choice(kind),
                    Cell::Text(TextEditor::single(&value)),
                    Cell::Choice(auth),
                    Cell::Text(TextEditor::single(&token)),
                ],
            );
        }
        self.table = t;
        self.dirty = false;
        self.error = None;
    }

    /// The rows, skipping empty and duplicate names.
    pub fn rows(&self) -> Vec<McpRow> {
        let mut out: Vec<McpRow> = Vec::new();
        for r in self.table.values() {
            let name = r.cells[0].text().trim().to_string();
            if name.is_empty() || out.iter().any(|row| row.name == name) {
                continue;
            }
            out.push(McpRow {
                name,
                kind: r.cells[1].choice(),
                value: r.cells[2].text().trim().to_string(),
                auth: r.cells[3].choice(),
                token: r.cells[4].text().trim().to_string(),
            });
        }
        out
    }
}

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
