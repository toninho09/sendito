//! The editors for the config tables the settings dialog does not hold: the
//! details of one MCP server (env, headers, TLS check, OAuth fields), the
//! HTTP OAuth identities and the key bindings. They share one shape: tabs,
//! one table per tab, Ctrl+S saves every tab to one config file.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use sendito_store::config::{
    self, KeySpec, McpHttpServer, McpServer, McpServerAuth, McpStdioServer, OAuthIdentity,
};

use crate::app::{App, StatusKind};
use crate::form::{ColKind, Column, Table, TableOutcome};
use crate::keymap::{Action, KeySeq, Keymap};
use crate::overlay::{Confirm, ConfirmAction, Overlay};

/// What a config editor edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CfgKind {
    /// `[mcp.servers.<name>]` in the project file: what the servers table
    /// does not show.
    McpServer { name: String, http: bool },
    /// `[oauth.<name>]` identities for HTTP requests.
    OAuth,
    /// `[keymap.<mode>]` key bindings.
    Keymap,
}

pub struct CfgEditor {
    pub kind: CfgKind,
    /// The config file Ctrl+S writes.
    pub path: PathBuf,
    /// Tab labels; empty means one table and no tab row.
    pub tabs: Vec<String>,
    pub current: usize,
    /// One table per tab (at least one).
    pub tables: Vec<Table>,
    pub dirty: bool,
    pub error: Option<String>,
}

/// The fields of the Options tab of an HTTP MCP server, in table order.
pub const MCP_OPTIONS: &[&str] = &[
    "verify_tls",
    "client_id",
    "client_secret",
    "authorize_url",
    "token_url",
    "registration_url",
    "scopes",
];

/// The keymap tabs: the `[keymap.<mode>]` names.
pub const KEYMAP_MODES: &[&str] = &["all", "conventional", "vim"];

fn text_col(title: &'static str, weight: u16) -> Column {
    Column {
        title,
        kind: ColKind::Text,
        weight,
    }
}

/// A table of text columns filled with `rows`.
fn text_table(cols: &[(&'static str, u16)], rows: &[Vec<String>]) -> Table {
    let mut t = Table::new(cols.iter().map(|(n, w)| text_col(n, *w)).collect(), false);
    for r in rows {
        let cells: Vec<&str> = r.iter().map(String::as_str).collect();
        t.push_texts(true, &cells);
    }
    t
}

/// A name/value table.
fn pairs_table(name: &'static str, value: &'static str, map: &BTreeMap<String, String>) -> Table {
    let rows: Vec<Vec<String>> = map
        .iter()
        .map(|(k, v)| vec![k.clone(), v.clone()])
        .collect();
    text_table(&[(name, 2), (value, 5)], &rows)
}

/// The non-empty rows of a table as trimmed texts.
fn rows_of(t: &Table) -> Vec<Vec<String>> {
    t.values()
        .iter()
        .map(|r| {
            r.cells
                .iter()
                .map(|c| c.text().trim().to_string())
                .collect()
        })
        .collect()
}

/// A name/value table back as a map; a name used twice is an error.
fn pairs_of(t: &Table, what: &str) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for r in rows_of(t) {
        let (k, v) = (r[0].clone(), r[1].clone());
        if k.is_empty() {
            return Err(format!("a {what} has a value but no name"));
        }
        if out.insert(k.clone(), v).is_some() {
            return Err(format!("{k}: two {what}s have this name"));
        }
    }
    Ok(out)
}

/// Scopes typed as one line, separated by spaces or commas.
fn split_list(s: &str) -> Vec<String> {
    s.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

/// A header whose name says it carries a credential.
fn secret_name(header: &str) -> bool {
    let h = header.to_ascii_lowercase();
    h == "authorization" || ["key", "token", "secret"].iter().any(|w| h.contains(w))
}

/// A secret value written as plain text instead of a `{{name}}` reference.
fn plain(v: &str) -> bool {
    !v.is_empty() && !v.contains("{{")
}

impl CfgEditor {
    pub fn table(&mut self) -> Option<&mut Table> {
        self.tables.get_mut(self.current)
    }

    fn mcp_server(name: &str, server: &McpServer, path: PathBuf) -> Self {
        let (http, tabs, tables) = match server {
            McpServer::Http(h) => {
                let oauth = match &h.auth {
                    Some(McpServerAuth::Oauth {
                        client_id,
                        client_secret,
                        authorize_url,
                        token_url,
                        registration_url,
                        scopes,
                    }) => vec![
                        client_id.clone().unwrap_or_default(),
                        client_secret.clone().unwrap_or_default(),
                        authorize_url.clone().unwrap_or_default(),
                        token_url.clone().unwrap_or_default(),
                        registration_url.clone().unwrap_or_default(),
                        scopes.join(" "),
                    ],
                    _ => vec![String::new(); MCP_OPTIONS.len() - 1],
                };
                let values = std::iter::once(h.verify_tls.to_string()).chain(oauth);
                let rows: Vec<Vec<String>> = MCP_OPTIONS
                    .iter()
                    .zip(values)
                    .map(|(f, v)| vec![f.to_string(), v])
                    .collect();
                (
                    true,
                    vec!["Options".to_string(), "Headers".to_string()],
                    vec![
                        text_table(&[("Field", 2), ("Value", 5)], &rows),
                        pairs_table("Header", "Value", &h.headers),
                    ],
                )
            }
            McpServer::Stdio(s) => (
                false,
                vec!["Environment".to_string()],
                vec![pairs_table("Variable", "Value", &s.env)],
            ),
        };
        Self {
            kind: CfgKind::McpServer {
                name: name.to_string(),
                http,
            },
            path,
            tabs,
            current: 0,
            tables,
            dirty: false,
            error: None,
        }
    }

    fn oauth(ids: &BTreeMap<String, OAuthIdentity>, path: PathBuf) -> Self {
        let rows: Vec<Vec<String>> = ids
            .iter()
            .map(|(name, i)| {
                vec![
                    name.clone(),
                    i.authorize_url.clone(),
                    i.token_url.clone(),
                    i.client_id.clone(),
                    i.client_secret.clone().unwrap_or_default(),
                    i.scopes.join(" "),
                ]
            })
            .collect();
        Self {
            kind: CfgKind::OAuth,
            path,
            tabs: Vec::new(),
            current: 0,
            tables: vec![text_table(
                &[
                    ("Name", 2),
                    ("Authorize URL", 4),
                    ("Token URL", 4),
                    ("Client ID", 3),
                    ("Client secret", 3),
                    ("Scopes", 3),
                ],
                &rows,
            )],
            dirty: false,
            error: None,
        }
    }

    fn keymap(map: &BTreeMap<String, BTreeMap<String, KeySpec>>, path: PathBuf) -> Self {
        let tables = KEYMAP_MODES
            .iter()
            .map(|mode| {
                // `default` is an old name of `all`.
                let rows: Vec<Vec<String>> = map
                    .iter()
                    .filter(|(m, _)| *m == mode || (*mode == "all" && *m == "default"))
                    .flat_map(|(_, actions)| actions.iter())
                    .map(|(a, k)| vec![a.clone(), k.keys().join(", ")])
                    .collect();
                text_table(&[("Action", 2), ("Keys (comma-separated)", 4)], &rows)
            })
            .collect();
        Self {
            kind: CfgKind::Keymap,
            path,
            tabs: KEYMAP_MODES.iter().map(|m| m.to_string()).collect(),
            current: 0,
            tables,
            dirty: false,
            error: None,
        }
    }

    /// The Options tab back as the server's TLS check and auth. OAuth fields
    /// need the auth set to oauth in the servers table.
    fn mcp_options(
        t: &Table,
        auth: Option<McpServerAuth>,
    ) -> Result<(bool, Option<McpServerAuth>), String> {
        let mut verify = true;
        let mut f: BTreeMap<String, String> = BTreeMap::new();
        for r in rows_of(t) {
            let (k, v) = (r[0].as_str(), r[1].clone());
            if !MCP_OPTIONS.contains(&k) {
                return Err(format!(
                    "unknown field {k:?} (use {})",
                    MCP_OPTIONS.join(", ")
                ));
            }
            if k == "verify_tls" {
                verify = match v.as_str() {
                    "" | "true" => true,
                    "false" => false,
                    _ => return Err("verify_tls is true or false".into()),
                };
            } else if !v.is_empty() {
                f.insert(k.to_string(), v);
            }
        }
        let opt = |k: &str| f.get(k).cloned();
        let auth = match auth {
            Some(McpServerAuth::Oauth { .. }) => Some(McpServerAuth::Oauth {
                client_id: opt("client_id"),
                client_secret: opt("client_secret"),
                authorize_url: opt("authorize_url"),
                token_url: opt("token_url"),
                registration_url: opt("registration_url"),
                scopes: opt("scopes").map(|s| split_list(&s)).unwrap_or_default(),
            }),
            _ if !f.is_empty() => {
                return Err(
                    "OAuth fields need the auth set to oauth: set it in the servers table first"
                        .into(),
                );
            }
            other => other,
        };
        Ok((verify, auth))
    }
}

impl App {
    /// Opens the details of the MCP server `name` of the project file.
    pub fn open_mcp_server_editor(&mut self, name: &str) -> Result<(), String> {
        let servers = self.project_mcp_servers()?;
        let server = servers.get(name).ok_or_else(|| {
            format!("{name} is not in the project config: save the table (Ctrl+S) first")
        })?;
        self.clear_transient_status();
        let ed = CfgEditor::mcp_server(name, server, self.project_config_path());
        self.overlay = Some(Overlay::Cfg(ed));
        Ok(())
    }

    /// The file the OAuth and keymap editors write: the settings scope.
    fn scope_file(&mut self) -> Option<PathBuf> {
        let p = self.scope_path(self.config_scope);
        if p.is_none() {
            self.set_status(
                "there is no global config file: set Save changes to project in Settings",
                StatusKind::Error,
            );
        }
        p
    }

    /// Reads one config file for an editor that rewrites one of its tables.
    /// A file that does not load cleanly is an error: a write would drop
    /// what it holds.
    fn load_one(&mut self, path: &std::path::Path) -> Option<config::Config> {
        let (cfg, warnings) = config::load(None, Some(path));
        if warnings.is_empty() {
            Some(cfg)
        } else {
            self.set_status(
                format!("fix the config first: {}", warnings.join("; ")),
                StatusKind::Error,
            );
            None
        }
    }

    /// `:oauth`: the HTTP OAuth identities of the settings scope's file.
    pub fn open_oauth_editor(&mut self) {
        self.clear_transient_status();
        let Some(path) = self.scope_file() else {
            return;
        };
        let Some(cfg) = self.load_one(&path) else {
            return;
        };
        self.overlay = Some(Overlay::Cfg(CfgEditor::oauth(&cfg.oauth, path)));
    }

    /// `:keymap`: the key bindings of the settings scope's file.
    pub fn open_keymap_editor(&mut self) {
        self.clear_transient_status();
        let Some(path) = self.scope_file() else {
            return;
        };
        let Some(cfg) = self.load_one(&path) else {
            return;
        };
        self.overlay = Some(Overlay::Cfg(CfgEditor::keymap(&cfg.keymap, path)));
    }

    /// Reads both config files again for the tables an editor wrote.
    fn merged_config(&self) -> config::Config {
        config::load(
            self.project.global_config.as_deref(),
            Some(&self.project_config_path()),
        )
        .0
    }

    /// Writes the editor's tables to its file. Returns false (with the
    /// reason in the editor) when a row is not valid or the write failed.
    pub fn save_cfg_editor(&mut self, ed: &mut CfgEditor) -> bool {
        match self.write_cfg_editor(ed) {
            Ok(plain) => {
                ed.dirty = false;
                ed.error = None;
                let file = crate::app::display_path(&ed.path, self.home.as_deref());
                if plain.is_empty() {
                    self.set_status(format!("saved to {file}"), StatusKind::Ok);
                } else {
                    self.set_status(
                        format!(
                            "saved to {file}; {} is plain text: put it in an environment \
                             secret and write {{{{name}}}}",
                            plain.join(", ")
                        ),
                        StatusKind::Warn,
                    );
                }
                true
            }
            Err(e) => {
                ed.error = Some(e);
                false
            }
        }
    }

    /// Validates and writes the editor; returns the secrets written as plain text.
    fn write_cfg_editor(&mut self, ed: &CfgEditor) -> Result<Vec<String>, String> {
        let mut plain_values = Vec::new();
        match &ed.kind {
            CfgKind::McpServer { name, http } => {
                let mut servers = self.project_mcp_servers()?;
                let entry = servers
                    .get_mut(name)
                    .ok_or_else(|| format!("{name} is no longer in the project config"))?;
                match (entry, http) {
                    (McpServer::Http(h), true) => {
                        let options = ed.tables.first().ok_or("no options table")?;
                        let headers = ed.tables.get(1).ok_or("no headers table")?;
                        let (verify, auth) = CfgEditor::mcp_options(options, h.auth.clone())?;
                        let headers = pairs_of(headers, "header")?;
                        if let Some(McpServerAuth::Oauth {
                            client_secret: Some(s),
                            ..
                        }) = &auth
                            && plain(s)
                        {
                            plain_values.push(format!("{name} client_secret"));
                        }
                        plain_values.extend(
                            headers
                                .iter()
                                .filter(|(k, v)| secret_name(k) && plain(v))
                                .map(|(k, _)| format!("the {k} header of {name}")),
                        );
                        *h = McpHttpServer {
                            url: h.url.clone(),
                            headers,
                            verify_tls: verify,
                            auth,
                        };
                    }
                    (McpServer::Stdio(s), false) => {
                        let env = ed.tables.first().ok_or("no environment table")?;
                        *s = McpStdioServer {
                            command: s.command.clone(),
                            args: s.args.clone(),
                            env: pairs_of(env, "variable")?,
                        };
                    }
                    _ => return Err(format!("{name} changed its kind: open it again")),
                }
                config::set_mcp_servers(&ed.path, &servers).map_err(|e| e.to_string())?;
                self.reload_mcp_servers();
            }
            CfgKind::OAuth => {
                let mut ids = BTreeMap::new();
                let table = ed.tables.first().ok_or("no table")?;
                for r in rows_of(table) {
                    let name = r[0].clone();
                    if name.is_empty() || name.contains(char::is_whitespace) {
                        return Err(
                            "each identity needs a name without spaces (the [oauth.<name>] key)"
                                .into(),
                        );
                    }
                    for (i, what) in [
                        (1, "an authorize URL"),
                        (2, "a token URL"),
                        (3, "a client ID"),
                    ] {
                        if r[i].is_empty() {
                            return Err(format!("{name}: set {what}"));
                        }
                    }
                    if plain(&r[4]) {
                        plain_values.push(format!("the client secret of {name}"));
                    }
                    let id = OAuthIdentity {
                        authorize_url: r[1].clone(),
                        token_url: r[2].clone(),
                        client_id: r[3].clone(),
                        client_secret: (!r[4].is_empty()).then(|| r[4].clone()),
                        scopes: split_list(&r[5]),
                    };
                    if ids.insert(name.clone(), id).is_some() {
                        return Err(format!("{name}: two identities have this name"));
                    }
                }
                config::set_table(&ed.path, &["oauth"], &ids).map_err(|e| e.to_string())?;
                self.project.config.oauth = self.merged_config().oauth;
            }
            CfgKind::Keymap => {
                let mut map: BTreeMap<String, BTreeMap<String, KeySpec>> = BTreeMap::new();
                for (mode, table) in KEYMAP_MODES.iter().zip(&ed.tables) {
                    let mut actions = BTreeMap::new();
                    for r in rows_of(table) {
                        let id = r[0].clone();
                        if Action::from_id(&id).is_none() {
                            return Err(format!("{mode}: unknown action {id:?}"));
                        }
                        let keys: Vec<String> = r[1]
                            .split(',')
                            .map(str::trim)
                            .filter(|k| !k.is_empty())
                            .map(str::to_string)
                            .collect();
                        for k in &keys {
                            KeySeq::parse(k).map_err(|e| format!("{mode}: {id}: {e}"))?;
                        }
                        let spec = match keys.as_slice() {
                            [one] => KeySpec::One(one.clone()),
                            _ => KeySpec::Many(keys),
                        };
                        if actions.insert(id.clone(), spec).is_some() {
                            return Err(format!("{mode}: {id} is listed twice"));
                        }
                    }
                    if !actions.is_empty() {
                        map.insert(mode.to_string(), actions);
                    }
                }
                config::set_table(&ed.path, &["keymap"], &map).map_err(|e| e.to_string())?;
                self.project.config.keymap = self.merged_config().keymap;
                let (keymap, warnings) =
                    Keymap::defaults().with_overrides(&self.project.config.keymap);
                self.keymap = keymap;
                if !warnings.is_empty() {
                    return Err(warnings.join("; "));
                }
            }
        }
        Ok(plain_values)
    }

    /// Where Esc (or a discard) goes: the details of a server return to the
    /// servers table, the other editors close.
    fn leave_cfg_editor(&mut self, kind: &CfgKind) {
        if matches!(kind, CfgKind::McpServer { .. }) {
            self.open_mcp_editor();
        }
    }

    /// Keys of a config editor; false closes it.
    pub(crate) fn cfg_key(&mut self, e: &mut CfgEditor, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Esc {
            if e.dirty {
                let ed = std::mem::replace(e, CfgEditor::oauth(&BTreeMap::new(), PathBuf::new()));
                self.parked_cfg = Some(ed);
                self.overlay = Some(Overlay::Confirm(Confirm {
                    title: "Unsaved changes".into(),
                    message: "Save the changes to the config file?".into(),
                    buttons: vec!["Save".into(), "Discard".into(), "Cancel".into()],
                    sel: 0,
                    action: ConfirmAction::CfgClose,
                }));
                return false;
            }
            let kind = e.kind.clone();
            self.leave_cfg_editor(&kind);
            return false;
        }
        if ctrl && key.code == KeyCode::Char('s') {
            self.save_cfg_editor(e);
            return true;
        }
        if ctrl && matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
            let d: isize = if key.code == KeyCode::PageUp { -1 } else { 1 };
            e.current = crate::list::cycle(e.current, d, e.tables.len().max(1));
            return true;
        }
        let Some(t) = e.table() else {
            return true;
        };
        let changed = if ctrl && key.code == KeyCode::Char('d') {
            t.delete_row()
        } else if ctrl && key.code == KeyCode::Char('n') {
            // The trailing row is the one typed into to add a row.
            t.sel_row = t.rows.len();
            t.sel_col = 0;
            t.ensure_row();
            false
        } else if matches!(key.code, KeyCode::Enter | KeyCode::Tab) {
            t.next_cell();
            false
        } else {
            match t.handle_key(&key, false, &mut self.clipboard) {
                TableOutcome::Changed => true,
                TableOutcome::Leave => return false,
                _ => false,
            }
        };
        if changed {
            e.dirty = true;
            e.error = None;
        }
        true
    }

    /// The answer to the "unsaved changes" question of a config editor.
    pub(crate) fn cfg_close(&mut self, sel: usize) {
        let Some(mut ed) = self.parked_cfg.take() else {
            return;
        };
        match sel {
            0 if self.save_cfg_editor(&mut ed) => self.leave_cfg_editor(&ed.kind),
            0 | 2.. => self.overlay = Some(Overlay::Cfg(ed)),
            _ => {
                self.leave_cfg_editor(&ed.kind);
                self.set_status("closed without saving", StatusKind::Info);
            }
        }
    }
}

/// What the selected row of a config editor means, for its explanation line.
pub fn cfg_explain(e: &CfgEditor) -> String {
    let Some(t) = e.tables.get(e.current) else {
        return String::new();
    };
    let cell = |c: usize| {
        t.rows
            .get(t.sel_row)
            .and_then(|r| r.cells.get(c))
            .map(|c| c.text().trim().to_string())
            .unwrap_or_default()
    };
    match &e.kind {
        CfgKind::McpServer { http: true, .. } if e.current == 0 => match cell(0).as_str() {
            "verify_tls" => {
                "verify_tls: true checks the server's certificate; false skips the check.".into()
            }
            "client_secret" => {
                "client_secret: write {{name}} and keep the value in an environment secret.".into()
            }
            "scopes" => "scopes: separated by spaces.".into(),
            "" => format!("Fields: {}.", MCP_OPTIONS.join(", ")),
            f => format!(
                "{f}: an OAuth override; the auth must be oauth. Empty = discovery finds it."
            ),
        },
        CfgKind::McpServer { http: true, .. } => {
            "A header sent with every POST; values may use {{variables}}.".into()
        }
        CfgKind::McpServer { .. } => {
            "A variable added to the child's environment; values may use {{variables}}.".into()
        }
        CfgKind::OAuth => match t.sel_col {
            0 => "Name: the [oauth.<name>] key a request's auth names.".into(),
            4 => {
                "Client secret: write {{name}} and keep the value in an environment secret.".into()
            }
            5 => "Scopes: separated by spaces.".into(),
            _ => "URLs and client ID may use {{variables}}; :auth login <name> signs in.".into(),
        },
        CfgKind::Keymap => {
            let id = cell(0);
            if let Some(a) = Action::from_id(&id) {
                format!("{id}: {}. Empty keys unbind it.", a.description())
            } else {
                let near: Vec<&str> = Action::ALL
                    .iter()
                    .map(|a| a.id())
                    .filter(|a| !id.is_empty() && a.starts_with(&id))
                    .take(8)
                    .collect();
                if near.is_empty() {
                    "Type an action name (F1 lists the actions) and its keys, e.g. ctrl+s, f5."
                        .into()
                } else {
                    format!("Actions: {}", near.join(", "))
                }
            }
        }
    }
}

/// An editor cell from a click (a text cell starts editing at `x`).
pub(crate) fn cfg_click(e: &mut CfgEditor, row: usize, col: usize, x: u16) {
    let Some(t) = e.table() else {
        return;
    };
    t.select(row, col);
    if t.editing_text() {
        t.ensure_row();
        if let Some(ed) = t.editor_mut() {
            ed.click(x, 0, false);
        }
    }
}

/// A row's ✕, same as Ctrl+D.
pub(crate) fn cfg_delete(e: &mut CfgEditor, row: usize) {
    let Some(t) = e.table() else {
        return;
    };
    t.sel_row = row.min(t.rows.len());
    if t.delete_row() {
        e.dirty = true;
    }
}
