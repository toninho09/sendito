//! The command palette and its command list.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteItem {
    Action(crate::keymap::Action),
    Open(String),
    Env(Option<String>),
    /// Applies and saves this theme.
    Theme(String),
    Command(&'static str, &'static str),
    /// Opens the settings dialog on this row.
    Setting(SettingId),
}

pub struct Palette {
    pub input: TextEditor,
    pub items: Vec<(PaletteItem, String, String)>,
    pub sel: usize,
    pub scroll: usize,
    /// A row is highlighted and Enter runs it. Always true for the app's
    /// palette (its untouched first row is harmless, see `visible`).
    pub armed: bool,
}

pub const COMMANDS: &[(&str, &str)] = &[
    (":send", "Send the current request"),
    (":cancel", "Cancel the running request"),
    (
        ":watch [seconds]",
        "Repeat the active HTTP request on an interval",
    ),
    (":end", "Finish sending on a client/bidi stream"),
    (
        ":env <name>",
        "Switch environment (names listed as you type)",
    ),
    (":envs", "Edit environments"),
    (":newenv", "Create an environment (asks for the name)"),
    (
        ":auth login [name]",
        "Sign in to an HTTP OAuth identity or an MCP server (lists them without a name)",
    ),
    (
        ":auth logout [name]",
        "Remove the OAuth tokens of an identity or an MCP server (lists them without a name)",
    ),
    (":w", "Save the request"),
    (":saveas <name>", "Save the request under a new name"),
    (":wq", "Save and quit"),
    (":q", "Quit (asks when there are unsaved changes)"),
    (":q!", "Quit without saving"),
    (":new", "New HTTP request (Ctrl+N)"),
    (":newgrpc", "New gRPC request (Ctrl+W g)"),
    (":newsse", "New SSE request (Ctrl+W E)"),
    (":newmqtt", "New MQTT request (Ctrl+W M)"),
    (":newws", "New WebSocket request (Ctrl+W y)"),
    (":newmcp", "New MCP request (Ctrl+W a)"),
    (":mcpservers", "Choose an MCP server"),
    (
        ":mcpservers edit",
        "Edit the MCP servers in the config file chosen in Settings (Ctrl+W S)",
    ),
    (
        ":mcpadd <name>",
        "Add an MCP server to the config file chosen in Settings (asks for the URL or command)",
    ),
    (
        ":mcpedit [name]",
        "Edit an MCP server: URL or command, auth, headers, env, TLS, OAuth (the list without a name)",
    ),
    (
        ":mcptools",
        "Discover and choose an MCP tool, resource or prompt",
    ),
    (
        ":mcp restart",
        "Close the kept MCP server sessions; the next call opens a new one",
    ),
    (":method", "Choose the HTTP method"),
    (":bodytype", "Choose the body type of the HTTP request"),
    (":authtype", "Choose the auth type of the request"),
    (":grpcmethod", "Choose the gRPC service and method"),
    (":rescan", "Read the collection from disk again"),
    (
        ":saveexample [name]",
        "Save the response on screen as an example of the request",
    ),
    (
        ":examples",
        "List the request's saved examples and show one (:examples delete <name> deletes)",
    ),
    (
        ":folder",
        "Edit the settings of the active request's folder (headers, auth, variables, scripts)",
    ),
    (
        ":import [path]",
        "Import Postman, Insomnia, OpenAPI/Swagger, a .http or HAR file or a Bruno directory, or paste curl",
    ),
    (
        ":curl",
        "Copy the request as curl (secret variables stay {{placeholders}})",
    ),
    (":curl!", "Copy the request as curl including secret values"),
    (":httpfile", "Copy the request as an .http file"),
    (":httpfile!", "Copy the .http file including secret values"),
    (
        ":code <language>",
        "Copy code that sends the request: python, fetch, go, httpie or wget",
    ),
    (":code! <language>", "Copy the code including secret values"),
    (
        ":cookies",
        "Manage the active environment's cookies: delete one or clear all (values masked)",
    ),
    (":cookies clear", "Clear the active environment's cookies"),
    (
        ":cookies delete <number>",
        "Delete one cookie from the active environment",
    ),
    (
        ":run [folder] [--data file]",
        "Run every request of a folder in a run tab (. is the collection root; the data file runs it once per row)",
    ),
    (":history", "Show history"),
    (":clearhistory", "Delete all history entries"),
    (":format", "Pretty-print the JSON body"),
    (":edit", "Edit the focused text in $VISUAL or $EDITOR"),
    (":pager", "Show the response text in $PAGER"),
    (
        ":jq [expr]",
        "Filter the response body with a jq expression",
    ),
    (
        ":template",
        "Fill the gRPC message with a template from the schema",
    ),
    (":reload", "Reload the gRPC schema"),
    (
        ":graphqlschema",
        "Reload the GraphQL schema for the HTTP request",
    ),
    (":settings", "Open settings"),
    (
        ":oauth",
        "Edit the HTTP OAuth identities (the file Settings saves to)",
    ),
    (
        ":keymap",
        "Edit your key bindings (the file Settings saves to)",
    ),
    (
        ":theme [name]",
        "Switch theme (names listed as you type; without a name, a list with a live preview); :theme reload reads your theme files again",
    ),
    (
        ":help [section]",
        "Open the help, or the section whose name has the text",
    ),
    (
        ":report",
        "Show the last report (import, curl export) again",
    ),
    (":savebody [path]", "Save the response body to a file"),
];

/// A palette command without its arguments (`":auth login [name]"` →
/// `"auth login"`) and whether it needs an argument before it can run.
pub fn command_base(c: &str) -> (&str, bool) {
    let c = c.trim_start_matches(':');
    let end = c.find(['<', '[']).unwrap_or(c.len());
    (c[..end].trim(), c.contains('<'))
}

impl Palette {
    pub fn visible(&self) -> Vec<usize> {
        let q = self.input.text().to_lowercase();
        let q = q.trim();
        if let Some(cmd) = q.strip_prefix(':') {
            // ":env" and ":env <prefix>" list the matching environments, real
            // ones first; "no environment" comes last.
            if cmd == "env" || cmd.starts_with("env ") {
                let arg = cmd[3..].trim();
                let mut v: Vec<usize> = self
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, (it, _, _))| {
                        matches!(it, PaletteItem::Env(Some(name)) if name.to_lowercase().starts_with(arg))
                    })
                    .map(|(i, _)| i)
                    .collect();
                if arg.is_empty() {
                    v.extend(
                        self.items
                            .iter()
                            .position(|(it, _, _)| *it == PaletteItem::Env(None)),
                    );
                }
                return v;
            }
            // ":theme <text>" lists the themes whose name holds the text.
            if let Some(arg) = cmd.strip_prefix("theme ").map(str::trim)
                && !arg.is_empty()
            {
                return self
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, (it, _, _))| {
                        matches!(it, PaletteItem::Theme(name) if name.to_lowercase().contains(arg))
                    })
                    .map(|(i, _)| i)
                    .collect();
            }
            let word = cmd.split_whitespace().next().unwrap_or("");
            return self
                .items
                .iter()
                .enumerate()
                .filter(|(_, (it, label, _))| {
                    matches!(it, PaletteItem::Command(..)) && label[1..].starts_with(word)
                })
                .map(|(i, _)| i)
                .collect();
        }
        let mut v: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, (_, label, detail))| {
                q.is_empty() || fuzzy(&label.to_lowercase(), q) || fuzzy(&detail.to_lowercase(), q)
            })
            .map(|(i, _)| i)
            .collect();
        // Rows that hold the typed text as written come before the scattered
        // matches: "rescan" finds :rescan before "Re-send … on an interval".
        v.sort_by_key(|i| {
            let (_, label, detail) = &self.items[*i];
            !(label.to_lowercase().contains(q) || detail.to_lowercase().contains(q))
        });
        // The untouched palette highlights its first row, so that row must be
        // harmless: the key list, never an action that sends or deletes.
        if q.is_empty()
            && let Some(pos) = v
                .iter()
                .position(|i| self.items[*i].0 == PaletteItem::Action(crate::keymap::Action::Help))
        {
            let help = v.remove(pos);
            v.insert(0, help);
        }
        v
    }
}
