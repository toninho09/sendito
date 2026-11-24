//! The environment, MCP server and config editor dialogs.

use super::*;

/// Where the environment editor's clicks go.
pub(super) struct EnvHits;

impl EditorHits for EnvHits {
    fn tab(&self, i: usize) -> OverlayHit {
        OverlayHit::EnvTab(i)
    }
    fn cell(&self, row: usize, col: usize) -> OverlayHit {
        OverlayHit::EnvCell { row, col }
    }
    fn delete(&self, row: usize) -> OverlayHit {
        OverlayHit::EnvDelete(row)
    }
    fn button(&self, i: usize) -> OverlayHit {
        OverlayHit::Button(i)
    }
}

/// Where the MCP servers editor's clicks go.
pub(super) struct McpHits;

impl EditorHits for McpHits {
    fn tab(&self, i: usize) -> OverlayHit {
        OverlayHit::Item(i)
    }
    fn cell(&self, row: usize, col: usize) -> OverlayHit {
        OverlayHit::McpCell { row, col }
    }
    fn delete(&self, row: usize) -> OverlayHit {
        OverlayHit::McpDelete(row)
    }
    fn button(&self, i: usize) -> OverlayHit {
        OverlayHit::McpButton(i)
    }
}

/// Where a config editor's clicks go.
pub(super) struct CfgHits;

impl EditorHits for CfgHits {
    fn tab(&self, i: usize) -> OverlayHit {
        OverlayHit::CfgTab(i)
    }
    fn cell(&self, row: usize, col: usize) -> OverlayHit {
        OverlayHit::CfgCell { row, col }
    }
    fn delete(&self, row: usize) -> OverlayHit {
        OverlayHit::CfgDelete(row)
    }
    fn button(&self, i: usize) -> OverlayHit {
        OverlayHit::CfgButton(i)
    }
}

pub(super) fn draw_env(
    app: &mut App,
    buf: &mut Buffer,
    frame: Frame<'_>,
    is_def: &impl Fn(&str) -> bool,
    e: &mut EnvEditor,
    cursor: &mut Option<(u16, u16)>,
) {
    let labels: Vec<String> = e
        .envs
        .iter()
        .map(|name| {
            if name == sendito_store::env::COLLECTION_ENV {
                "Collection variables".to_string()
            } else if app.active_env.as_deref() == Some(name.as_str()) {
                // The highlight shows the env being edited; this marks the one in use.
                format!("{name} (active)")
            } else {
                name.clone()
            }
        })
        .collect();
    let src = e
        .table
        .rows
        .get(e.table.sel_row)
        .and_then(|r| r.cells.get(1))
        .map(|c| c.choice());
    let explain = match src {
        Some(0) => "plain: written in this file, shared through git.",
        Some(1) => {
            "secrets: in your .sendito/secrets.toml, never committed. Type the value in Secret value."
        }
        Some(2) => "keychain: in your OS keychain. Type the value in Secret value.",
        Some(3) => "file: read from the file at the path.",
        _ => "Type a name in the last row to add a variable.",
    };
    let legend = "Source says where a value lives (Space changes it). For secrets \
            and keychain, select Secret value and type the value. ✕ deletes a row (Ctrl+D). \
            Ctrl+S saves; Esc asks before discarding unsaved changes.";
    *cursor = editor::render(
        app,
        buf,
        frame.body,
        frame.theme,
        EditorDialog {
            title: "Environments",
            width: 112,
            height: frame.area.height.saturating_sub(4).max(12),
            tabs: labels,
            tab_sel: e.current,
            reserved: 4,
            legend,
            explain,
            error: e.error.as_deref(),
            dirty: e.dirty,
            buttons: &["Save (Ctrl+S)", "New env (Ctrl+N)", "Close (Esc)"],
            is_def,
        },
        &mut e.table,
        &EnvHits,
    );
}

pub(super) fn draw_mcp(
    app: &mut App,
    buf: &mut Buffer,
    frame: Frame<'_>,
    is_def: &impl Fn(&str) -> bool,
    m: &mut McpEditor,
    cursor: &mut Option<(u16, u16)>,
) {
    let explain = match m.table.rows.get(m.table.sel_row) {
        Some(r) => match (r.cells.get(1).map(|c| c.choice()), m.table.sel_col) {
            (Some(1), _) => "stdio: started as a child process; the value is the command.",
            (_, 3) => {
                "Auth: none, bearer (the Token column) or oauth (sign in when the server asks)."
            }
            (_, 4) => {
                "Token: the bearer token; write {{name}} and keep the value in an environment secret."
            }
            _ => "http: a URL the tool calls go to (variables allowed).",
        },
        None => "Type a name in the last row to add a server.",
    };
    let legend = "Each row is a [mcp.servers] entry: a name, the kind and the auth (Space \
            changes them), the URL or command, and a bearer token. Typing in the last row adds \
            one; ✕ deletes a row (Ctrl+D). Ctrl+O opens the selected server's env (stdio) or \
            headers, TLS check and OAuth fields (http). Ctrl+S saves to .sendito/config.toml; \
            Esc asks before discarding unsaved changes.";
    *cursor = editor::render(
        app,
        buf,
        frame.body,
        frame.theme,
        EditorDialog {
            title: "MCP servers",
            width: 108,
            // The frame grows with the list: a few servers get a short
            // dialog, a long list scrolls inside the screen height.
            height: (m.table.rows.len() as u16 + 13)
                .min(frame.area.height.saturating_sub(4).max(12)),
            tabs: Vec::new(),
            tab_sel: 0,
            reserved: 3,
            legend,
            explain,
            error: m.error.as_deref(),
            dirty: m.dirty,
            buttons: &[
                "Save (Ctrl+S)",
                "Add row (Ctrl+N)",
                "Details (Ctrl+O)",
                "Close (Esc)",
            ],
            is_def,
        },
        &mut m.table,
        &McpHits,
    );
}

pub(super) fn draw_cfg(
    app: &mut App,
    buf: &mut Buffer,
    frame: Frame<'_>,
    is_def: &impl Fn(&str) -> bool,
    e: &mut CfgEditor,
    cursor: &mut Option<(u16, u16)>,
) {
    use crate::config_editor::CfgKind;
    let file = crate::app::display_path(&e.path, app.home.as_deref());
    let (title, legend) = match &e.kind {
        CfgKind::McpServer { name, http: true } => (
            format!("MCP server {name}"),
            format!(
                "Options: the TLS check and, for auth = oauth, the OAuth overrides (empty = \
                 discovery). Headers: sent with every POST. Ctrl+PgUp/PgDn switches tabs. \
                 Ctrl+S saves to {file}; Esc returns to the servers."
            ),
        ),
        CfgKind::McpServer { name, .. } => (
            format!("MCP server {name}"),
            format!(
                "Variables added to the child process's environment. Typing in the last row \
                 adds one; ✕ deletes a row (Ctrl+D). Ctrl+S saves to {file}; Esc returns to \
                 the servers."
            ),
        ),
        CfgKind::OAuth => (
            "OAuth identities".to_string(),
            format!(
                "Each row is an [oauth.<name>] identity (authorization code + PKCE) that a \
                 request's auth names. Typing in the last row adds one; ✕ deletes a row \
                 (Ctrl+D). Ctrl+S saves to {file} (Settings chooses the file); Esc asks \
                 before discarding unsaved changes."
            ),
        ),
        CfgKind::Keymap => (
            "Key bindings".to_string(),
            format!(
                "Each row replaces the keys of one action: all applies in both modes, \
                 conventional and vim in one. Keys look like ctrl+s, f5 or \"ctrl+w x\" (a \
                 sequence). Ctrl+PgUp/PgDn switches tabs. Ctrl+S saves to {file} (Settings \
                 chooses the file)."
            ),
        ),
    };
    let explain = crate::config_editor::cfg_explain(e);
    let rows = e.tables.get(e.current).map_or(0, |t| t.rows.len()) as u16;
    let current = e.current;
    let tabs = e.tabs.clone();
    let Some(table) = e.tables.get_mut(current) else {
        return;
    };
    *cursor = editor::render(
        app,
        buf,
        frame.body,
        frame.theme,
        EditorDialog {
            title: &title,
            width: 112,
            height: (rows + 14).min(frame.area.height.saturating_sub(4).max(12)),
            tabs,
            tab_sel: current,
            reserved: 3,
            legend: &legend,
            explain: &explain,
            error: e.error.as_deref(),
            dirty: e.dirty,
            buttons: &["Save (Ctrl+S)", "Add row (Ctrl+N)", "Close (Esc)"],
            is_def,
        },
        table,
        &CfgHits,
    );
}
