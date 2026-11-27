//! The record editors: the named entries of one config table in a list on
//! the left, and the form of the selected entry on the right. The MCP
//! servers (`[mcp.servers]`) and the HTTP OAuth identities (`[oauth]`) use
//! it. The form shows only the fields that apply (an HTTP server has a URL,
//! a stdio server a command); Ctrl+S writes every entry to one config file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use sendito_store::config::{
    self, McpHttpServer, McpServer, McpServerAuth, McpStdioServer, OAuthGrant, OAuthIdentity,
};

use crate::app::{App, StatusKind};
use crate::editor::{Outcome, TextEditor};
use crate::form::{ColKind, Column, Table, TableOutcome};
use crate::keymap::Action;
use crate::overlay::{Confirm, ConfirmAction, MCP_AUTHS, MCP_KINDS, Overlay, split_command_line};
use crate::tab::plain_secret;

/// What a record editor edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordKind {
    /// `[mcp.servers.<name>]`.
    Mcp,
    /// `[oauth.<name>]` identities for HTTP requests.
    OAuth,
}

/// The grants of an OAuth identity as the form names them, in the order of
/// `OAuthGrant::ALL`.
pub const GRANTS: &[&str] = &[
    "Authorization code",
    "Client credentials",
    "Password",
    "Device code",
];

/// The value of one form field.
pub enum Input {
    Text(TextEditor),
    Choice {
        options: &'static [&'static str],
        sel: usize,
    },
    Toggle(bool),
    Table(Table),
    /// A section title between fields, not a value.
    Heading,
}

pub struct FormField {
    /// The key the editor reads the value with.
    pub id: &'static str,
    pub label: &'static str,
    /// What the field does, for the help line while it has the focus.
    pub help: &'static str,
    pub input: Input,
    /// A credential: a plain-text value gets a warning.
    pub secret: bool,
}

/// One entry of the table: its fields, all of them, shown or not.
pub struct Record {
    pub fields: Vec<FormField>,
}

/// Which side of the editor has the keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    List,
    Form,
}

pub struct RecordEditor {
    pub kind: RecordKind,
    /// The config file Ctrl+S writes.
    pub path: PathBuf,
    pub records: Vec<Record>,
    /// The selected entry; `records.len()` is the "new" row of the list.
    pub sel: usize,
    pub pane: Pane,
    /// The focused field, as an index into `visible()`.
    pub field: usize,
    pub dirty: bool,
    pub error: Option<String>,
    /// An information line (e.g. the result of a connection test).
    pub note: Option<String>,
}

fn text(id: &'static str, label: &'static str, help: &'static str, value: &str) -> FormField {
    FormField {
        id,
        label,
        help,
        input: Input::Text(TextEditor::single(value)),
        secret: false,
    }
}

fn secret(id: &'static str, label: &'static str, help: &'static str, value: &str) -> FormField {
    FormField {
        secret: true,
        ..text(id, label, help, value)
    }
}

fn choice(
    id: &'static str,
    label: &'static str,
    help: &'static str,
    options: &'static [&'static str],
    sel: usize,
) -> FormField {
    FormField {
        id,
        label,
        help,
        input: Input::Choice { options, sel },
        secret: false,
    }
}

fn heading(id: &'static str, label: &'static str) -> FormField {
    FormField {
        id,
        label,
        help: "",
        input: Input::Heading,
        secret: false,
    }
}

fn table(id: &'static str, help: &'static str, table: Table) -> FormField {
    FormField {
        id,
        label: "",
        help,
        input: Input::Table(table),
        secret: false,
    }
}

/// A table of text columns filled with `rows`.
fn text_table(cols: &[(&'static str, u16)], rows: &[Vec<String>]) -> Table {
    let cols = cols
        .iter()
        .map(|(title, weight)| Column {
            title,
            kind: ColKind::Text,
            weight: *weight,
        })
        .collect();
    let mut t = Table::new(cols, false);
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
fn secret_header(header: &str) -> bool {
    let h = header.to_ascii_lowercase();
    h == "authorization" || ["key", "token", "secret"].iter().any(|w| h.contains(w))
}

/// One OAuth override of an MCP server, as text.
fn oauth_field(a: &McpServerAuth, k: &str) -> Option<String> {
    let McpServerAuth::Oauth {
        client_id,
        client_secret,
        authorize_url,
        token_url,
        registration_url,
        scopes,
    } = a
    else {
        return None;
    };
    match k {
        "client_id" => client_id.clone(),
        "client_secret" => client_secret.clone(),
        "authorize_url" => authorize_url.clone(),
        "token_url" => token_url.clone(),
        "registration_url" => registration_url.clone(),
        _ => Some(scopes.join(" ")),
    }
}

const NAME_HELP_MCP: &str =
    "The [mcp.servers.<name>] key a request names in its Server field; no spaces.";
const SECRET_HELP: &str = "Keep it in an environment secret and write {{name}} here.";
const DISCOVERY_HELP: &str = "Optional: empty lets discovery find it.";

impl Record {
    fn get(&self, id: &str) -> Option<&FormField> {
        self.fields.iter().find(|f| f.id == id)
    }

    /// The trimmed text of a text field.
    pub fn text(&self, id: &str) -> String {
        match self.get(id).map(|f| &f.input) {
            Some(Input::Text(e)) => e.text().trim().to_string(),
            _ => String::new(),
        }
    }

    fn choice(&self, id: &str) -> usize {
        match self.get(id).map(|f| &f.input) {
            Some(Input::Choice { sel, .. }) => *sel,
            _ => 0,
        }
    }

    fn toggle(&self, id: &str) -> bool {
        matches!(self.get(id).map(|f| &f.input), Some(Input::Toggle(true)))
    }

    fn table(&self, id: &str) -> Option<&Table> {
        match self.get(id).map(|f| &f.input) {
            Some(Input::Table(t)) => Some(t),
            _ => None,
        }
    }

    /// The entry's name, as the list shows it.
    pub fn name(&self) -> String {
        self.text("name")
    }

    /// A server as the form shows it; `None` is a new HTTP server.
    fn mcp(name: &str, server: Option<&McpServer>) -> Self {
        let empty = BTreeMap::new();
        let (kind, url, auth, token, verify, headers, oauth, command, args, env) = match server {
            Some(McpServer::Stdio(s)) => (
                1,
                "",
                0,
                "",
                true,
                &empty,
                None,
                s.command.as_str(),
                s.args.clone(),
                &s.env,
            ),
            Some(McpServer::Http(h)) => {
                let (auth, token, oauth) = match &h.auth {
                    None => (0, "", None),
                    Some(McpServerAuth::Bearer { token }) => (1, token.as_str(), None),
                    Some(o @ McpServerAuth::Oauth { .. }) => (2, "", Some(o)),
                };
                (
                    0,
                    h.url.as_str(),
                    auth,
                    token,
                    h.verify_tls,
                    &h.headers,
                    oauth,
                    "",
                    Vec::new(),
                    &empty,
                )
            }
            None => (0, "", 0, "", true, &empty, None, "", Vec::new(), &empty),
        };
        let o = |k: &str| oauth.and_then(|a| oauth_field(a, k)).unwrap_or_default();
        let args: Vec<Vec<String>> = args.into_iter().map(|a| vec![a]).collect();
        Record {
            fields: vec![
                text("name", "Name", NAME_HELP_MCP, name),
                choice(
                    "kind",
                    "Kind",
                    "http: a URL the tool calls go to. stdio: a program Sendito starts.",
                    MCP_KINDS,
                    kind,
                ),
                text(
                    "url",
                    "URL",
                    "The Streamable HTTP endpoint, e.g. https://host/mcp; {{variables}} allowed.",
                    url,
                ),
                choice(
                    "auth",
                    "Auth",
                    "none, a bearer token, or OAuth (Sendito signs in when the server asks).",
                    MCP_AUTHS,
                    auth,
                ),
                secret(
                    "token",
                    "Token",
                    "Sent as Authorization: Bearer <token>. Keep it in an environment secret and write {{name}} here.",
                    token,
                ),
                FormField {
                    id: "verify_tls",
                    label: "Verify TLS",
                    help: "Off accepts any certificate. Use it only for self-signed test servers.",
                    input: Input::Toggle(verify),
                    secret: false,
                },
                heading("headers_title", "Headers"),
                table(
                    "headers",
                    "Headers sent with every request to the server; values may use {{variables}}.",
                    pairs_table("Header", "Value", headers),
                ),
                heading("oauth_title", "OAuth (optional: discovery finds these)"),
                text(
                    "client_id",
                    "Client ID",
                    "Optional: empty registers a client when the server allows it.",
                    &o("client_id"),
                ),
                secret(
                    "client_secret",
                    "Client secret",
                    SECRET_HELP,
                    &o("client_secret"),
                ),
                text(
                    "authorize_url",
                    "Authorize URL",
                    DISCOVERY_HELP,
                    &o("authorize_url"),
                ),
                text("token_url", "Token URL", DISCOVERY_HELP, &o("token_url")),
                text(
                    "registration_url",
                    "Register URL",
                    DISCOVERY_HELP,
                    &o("registration_url"),
                ),
                text(
                    "scopes",
                    "Scopes",
                    "Separated by spaces. Empty asks for the scopes the server offers.",
                    &o("scopes"),
                ),
                text(
                    "command",
                    "Command",
                    "The program to start, e.g. npx. Put its arguments in the table below, one per row.",
                    command,
                ),
                heading("args_title", "Arguments"),
                table(
                    "args",
                    "One argument per row, as typed: spaces and quotes stay part of it.",
                    text_table(&[("Argument", 1)], &args),
                ),
                heading("env_title", "Environment"),
                table(
                    "env",
                    "Variables added to the program's environment; values may use {{variables}}.",
                    pairs_table("Variable", "Value", env),
                ),
            ],
        }
    }

    /// An identity as the form shows it; `None` is a new one.
    fn oauth(name: &str, id: Option<&OAuthIdentity>) -> Self {
        let d = OAuthIdentity::default();
        let i = id.unwrap_or(&d);
        let grant = OAuthGrant::ALL
            .iter()
            .position(|g| *g == i.grant)
            .unwrap_or(0);
        let opt = |v: &Option<String>| v.clone().unwrap_or_default();
        Record {
            fields: vec![
                text(
                    "name",
                    "Name",
                    "The [oauth.<name>] key a request's OAuth auth names; no spaces.",
                    name,
                ),
                choice(
                    "grant",
                    "Grant",
                    "How Sendito gets a token. Authorization code (a browser sign-in) and device code need a sign-in.",
                    GRANTS,
                    grant,
                ),
                text(
                    "authorize_url",
                    "Authorize URL",
                    "The page where you sign in; {{variables}} allowed.",
                    &i.authorize_url,
                ),
                text(
                    "device_url",
                    "Device URL",
                    "The device authorization endpoint; {{variables}} allowed.",
                    &i.device_authorization_url,
                ),
                text(
                    "token_url",
                    "Token URL",
                    "The endpoint that issues tokens; {{variables}} allowed.",
                    &i.token_url,
                ),
                text(
                    "client_id",
                    "Client ID",
                    "The client ID the provider gave you.",
                    &i.client_id,
                ),
                secret(
                    "client_secret",
                    "Client secret",
                    "Optional for public clients. Keep it in an environment secret and write {{name}} here.",
                    &opt(&i.client_secret),
                ),
                text(
                    "username",
                    "Username",
                    "The resource owner's user name; {{variables}} allowed.",
                    &opt(&i.username),
                ),
                secret("password", "Password", SECRET_HELP, &opt(&i.password)),
                text(
                    "scopes",
                    "Scopes",
                    "Separated by spaces, e.g. read write.",
                    &i.scopes.join(" "),
                ),
            ],
        }
    }

    /// Whether field `id` applies to this entry.
    fn shown(&self, kind: RecordKind, id: &str) -> bool {
        match kind {
            RecordKind::Mcp => {
                let http = self.choice("kind") == 0;
                let auth = self.choice("auth");
                match id {
                    "name" | "kind" => true,
                    "url" | "auth" | "verify_tls" | "headers_title" | "headers" => http,
                    "token" => http && auth == 1,
                    "command" | "args_title" | "args" | "env_title" | "env" => !http,
                    _ => http && auth == 2,
                }
            }
            RecordKind::OAuth => {
                let grant = OAuthGrant::ALL
                    .get(self.choice("grant"))
                    .copied()
                    .unwrap_or_default();
                match id {
                    "authorize_url" => grant == OAuthGrant::AuthorizationCode,
                    "device_url" => grant == OAuthGrant::DeviceCode,
                    "username" | "password" => grant == OAuthGrant::Password,
                    _ => true,
                }
            }
        }
    }

    /// The server back as config, with the credentials written as plain text.
    fn to_mcp(&self, name: &str) -> Result<(McpServer, Vec<String>), String> {
        let mut plain = Vec::new();
        if self.choice("kind") == 1 {
            let mut command = self.text("command");
            if command.is_empty() {
                return Err(format!("{name}: set the command"));
            }
            // One argument per row, as typed (a space can be part of it).
            let mut args: Vec<String> = self
                .table("args")
                .map(|t| {
                    t.values()
                        .into_iter()
                        .map(|r| r.cells[0].text().to_string())
                        .collect()
                })
                .unwrap_or_default();
            // A whole command line typed in the Command field splits on spaces.
            if args.is_empty() && command.contains(char::is_whitespace) {
                (command, args) = split_command_line(&command);
            }
            let env = match self.table("env") {
                Some(t) => pairs_of(t, "variable").map_err(|e| format!("{name}: {e}"))?,
                None => BTreeMap::new(),
            };
            return Ok((
                McpServer::Stdio(McpStdioServer { command, args, env }),
                plain,
            ));
        }
        let url = self.text("url");
        if url.is_empty() {
            return Err(format!("{name}: set the URL"));
        }
        let opt = |id: &str| Some(self.text(id)).filter(|v| !v.is_empty());
        let auth = match self.choice("auth") {
            1 => {
                let token = self.text("token");
                if token.is_empty() {
                    return Err(format!("{name}: set the token, or set the auth to none"));
                }
                if plain_secret(&token) {
                    plain.push(format!("the token of {name}"));
                }
                Some(McpServerAuth::Bearer { token })
            }
            2 => {
                if opt("client_secret").is_some_and(|s| plain_secret(&s)) {
                    plain.push(format!("the client secret of {name}"));
                }
                Some(McpServerAuth::Oauth {
                    client_id: opt("client_id"),
                    client_secret: opt("client_secret"),
                    authorize_url: opt("authorize_url"),
                    token_url: opt("token_url"),
                    registration_url: opt("registration_url"),
                    scopes: split_list(&self.text("scopes")),
                })
            }
            _ => None,
        };
        let headers = match self.table("headers") {
            Some(t) => pairs_of(t, "header").map_err(|e| format!("{name}: {e}"))?,
            None => BTreeMap::new(),
        };
        plain.extend(
            headers
                .iter()
                .filter(|(k, v)| secret_header(k) && plain_secret(v))
                .map(|(k, _)| format!("the {k} header of {name}")),
        );
        Ok((
            McpServer::Http(McpHttpServer {
                url,
                headers,
                verify_tls: self.toggle("verify_tls"),
                auth,
            }),
            plain,
        ))
    }

    /// The identity back as config, with the credentials written as plain text.
    fn to_oauth(&self, name: &str) -> Result<(OAuthIdentity, Vec<String>), String> {
        let grant = OAuthGrant::ALL
            .get(self.choice("grant"))
            .copied()
            .unwrap_or_default();
        let need = |id: &str, what: &str| {
            if self.text(id).is_empty() {
                Err(format!("{name}: set {what}"))
            } else {
                Ok(())
            }
        };
        match grant {
            OAuthGrant::AuthorizationCode => need("authorize_url", "the authorize URL")?,
            OAuthGrant::DeviceCode => need("device_url", "the device URL")?,
            OAuthGrant::Password => need("username", "the username")?,
            OAuthGrant::ClientCredentials => {}
        }
        need("token_url", "the token URL")?;
        need("client_id", "the client ID")?;
        let given = |id: &str| Some(self.text(id)).filter(|v| !v.is_empty());
        let password = (grant == OAuthGrant::Password)
            .then(|| given("password"))
            .flatten();
        let mut plain = Vec::new();
        for (v, what) in [
            (given("client_secret"), "client secret"),
            (password.clone(), "password"),
        ] {
            if v.is_some_and(|v| plain_secret(&v)) {
                plain.push(format!("the {what} of {name}"));
            }
        }
        let id = OAuthIdentity {
            grant,
            authorize_url: if grant == OAuthGrant::AuthorizationCode {
                self.text("authorize_url")
            } else {
                String::new()
            },
            device_authorization_url: if grant == OAuthGrant::DeviceCode {
                self.text("device_url")
            } else {
                String::new()
            },
            token_url: self.text("token_url"),
            client_id: self.text("client_id"),
            client_secret: given("client_secret"),
            username: (grant == OAuthGrant::Password)
                .then(|| given("username"))
                .flatten(),
            password,
            scopes: split_list(&self.text("scopes")),
        };
        Ok((id, plain))
    }
}

impl RecordEditor {
    fn new(kind: RecordKind, path: PathBuf, records: Vec<Record>) -> Self {
        Self {
            kind,
            path,
            records,
            sel: 0,
            pane: Pane::List,
            field: 0,
            dirty: false,
            error: None,
            note: None,
        }
    }

    pub fn mcp(path: PathBuf, servers: &BTreeMap<String, McpServer>) -> Self {
        let records = servers
            .iter()
            .map(|(n, s)| Record::mcp(n, Some(s)))
            .collect();
        Self::new(RecordKind::Mcp, path, records)
    }

    pub fn oauth(path: PathBuf, ids: &BTreeMap<String, OAuthIdentity>) -> Self {
        let records = ids.iter().map(|(n, i)| Record::oauth(n, Some(i))).collect();
        Self::new(RecordKind::OAuth, path, records)
    }

    /// An editor with nothing in it, left behind while the real one is parked.
    pub fn placeholder() -> Self {
        Self::new(RecordKind::Mcp, PathBuf::new(), Vec::new())
    }

    pub fn record(&self) -> Option<&Record> {
        self.records.get(self.sel)
    }

    /// The fields of the selected entry that apply, as indices into its
    /// `fields`.
    pub fn visible(&self) -> Vec<usize> {
        let Some(r) = self.record() else {
            return Vec::new();
        };
        (0..r.fields.len())
            .filter(|i| r.shown(self.kind, r.fields[*i].id))
            .collect()
    }

    /// The focused field.
    pub fn focused(&mut self) -> Option<&mut FormField> {
        let i = *self.visible().get(self.field)?;
        self.records.get_mut(self.sel)?.fields.get_mut(i)
    }

    /// The focused field, to read.
    pub fn focused_field(&self) -> Option<&FormField> {
        let i = *self.visible().get(self.field)?;
        self.records.get(self.sel)?.fields.get(i)
    }

    /// Selects the entry named `name`.
    pub fn select(&mut self, name: &str) {
        if let Some(i) = self.records.iter().position(|r| r.name() == name) {
            self.sel = i;
        }
    }

    /// Adds an empty entry, selected, with the focus on its name.
    pub fn add(&mut self) {
        self.records.push(match self.kind {
            RecordKind::Mcp => Record::mcp("", None),
            RecordKind::OAuth => Record::oauth("", None),
        });
        self.sel = self.records.len() - 1;
        self.pane = Pane::Form;
        self.field = 0;
        self.dirty = true;
        self.error = None;
    }

    /// Moves the field focus by `d`, past the section titles.
    fn step_field(&mut self, d: isize) {
        let Some(r) = self.record() else {
            return;
        };
        let vis = self.visible();
        let mut i = self.field as isize;
        loop {
            i += d;
            if i < 0 || i >= vis.len() as isize {
                return;
            }
            if !matches!(r.fields[vis[i as usize]].input, Input::Heading) {
                break;
            }
        }
        self.field = i as usize;
        // A table taken from below starts on its last row.
        let up = d < 0;
        if let Some(Input::Table(t)) = self.focused().map(|f| &mut f.input) {
            t.sel_row = if up { t.rows.len() } else { 0 };
            t.sel_col = 0;
        }
    }

    /// Puts the focus on the first field (the name) of the form.
    fn enter_form(&mut self) {
        if self.record().is_some() {
            self.pane = Pane::Form;
            self.field = 0;
        }
    }

    /// Keeps `field` on a field that is still shown (a choice can hide the
    /// fields after it).
    fn clamp_field(&mut self) {
        let n = self.visible().len();
        self.field = self.field.min(n.saturating_sub(1));
    }

    /// Writes the entries as config: `(names, plain-text credentials)` or
    /// the first problem.
    fn check_names(&self) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        for r in &self.records {
            let name = r.name();
            if name.is_empty() || name.contains(char::is_whitespace) {
                return Err(format!(
                    "each entry needs a name without spaces (got {name:?})"
                ));
            }
            if !seen.insert(name.clone()) {
                return Err(format!("{name}: two entries have this name"));
            }
        }
        Ok(())
    }

    /// Selects the first entry with a problem and puts the focus on it.
    fn show_problem(&mut self, name: &str) {
        if let Some(i) = self.records.iter().position(|r| r.name() == name) {
            self.sel = i;
            self.pane = Pane::Form;
            self.clamp_field();
        }
    }
}

impl RecordEditor {
    /// A paste into the focused text field or table cell.
    pub fn paste(&mut self, text: &str) {
        if self.pane != Pane::Form {
            return;
        }
        let pasted = match self.focused().map(|f| &mut f.input) {
            Some(Input::Text(ed)) => {
                ed.insert_str(text);
                true
            }
            Some(Input::Table(t)) => {
                t.paste(text);
                true
            }
            _ => false,
        };
        if pasted {
            self.dirty = true;
        }
    }

    /// The mouse wheel: the list selection, or the field focus.
    pub fn wheel(&mut self, d: isize) {
        match self.pane {
            Pane::List => {
                self.sel = crate::list::step(self.sel, d, self.records.len() + 1);
            }
            Pane::Form => self.step_field(d),
        }
    }

    /// A click on the list (`RecList`), a field or a table cell; `x` is the
    /// column inside the clicked area.
    pub fn click(&mut self, hit: crate::app::OverlayHit, x: u16) {
        use crate::app::OverlayHit as H;
        match hit {
            H::RecList(i) if i >= self.records.len() => self.add(),
            H::RecList(i) => {
                self.sel = i;
                self.pane = Pane::List;
                self.field = 0;
            }
            H::RecField(i) => {
                self.pane = Pane::Form;
                self.field = i;
                let changed = match self.focused().map(|f| &mut f.input) {
                    Some(Input::Choice { options, sel }) => {
                        *sel = crate::list::cycle(*sel, 1, options.len());
                        true
                    }
                    Some(Input::Toggle(on)) => {
                        *on = !*on;
                        true
                    }
                    Some(Input::Text(ed)) => {
                        ed.click(x, 0, false);
                        false
                    }
                    _ => false,
                };
                if changed {
                    self.dirty = true;
                    self.clamp_field();
                }
            }
            H::RecCell { field, row, col } => {
                self.pane = Pane::Form;
                self.field = field;
                let mut changed = false;
                if let Some(Input::Table(t)) = self.focused().map(|f| &mut f.input) {
                    t.select(row, col);
                    if t.editing_text() {
                        t.ensure_row();
                        if let Some(ed) = t.editor_mut() {
                            ed.click(x, 0, false);
                        }
                    } else {
                        changed = t.activate(1);
                    }
                }
                self.dirty |= changed;
            }
            H::RecDelete { field, row } => {
                self.pane = Pane::Form;
                self.field = field;
                let mut changed = false;
                if let Some(Input::Table(t)) = self.focused().map(|f| &mut f.input) {
                    t.sel_row = row.min(t.rows.len());
                    changed = t.delete_row();
                }
                self.dirty |= changed;
            }
            _ => {}
        }
    }
}

/// What the form's buttons are, in order, with the key of each from `key`.
pub fn buttons(kind: RecordKind, key: impl Fn(Action) -> String) -> Vec<String> {
    let (third, action) = match kind {
        RecordKind::Mcp => ("Test", Action::DialogTest),
        RecordKind::OAuth => ("Sign in", Action::DialogSignIn),
    };
    vec![
        format!("Save ({})", key(Action::DialogConfirm)),
        format!("New ({})", key(Action::DialogNew)),
        format!("{third} ({})", key(action)),
        "Close (Esc)".into(),
    ]
}

impl App {
    /// The file the OAuth and keymap editors write: the settings scope.
    pub(crate) fn scope_file(&mut self) -> Option<PathBuf> {
        match self.mcp_config_path() {
            Ok(p) => Some(p),
            Err(e) => {
                self.set_status(e, StatusKind::Error);
                None
            }
        }
    }

    /// Reads one config file for an editor that rewrites one of its tables.
    /// A file that does not load cleanly is an error: a write would drop
    /// what it holds.
    pub(crate) fn load_one(&mut self, path: &Path) -> Option<config::Config> {
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

    /// Reads both config files again for the tables an editor wrote.
    pub(crate) fn merged_config(&self) -> config::Config {
        config::load(
            self.project.global_config.as_deref(),
            Some(&self.project_config_path()),
        )
        .0
    }

    /// `Ctrl+W S`: the MCP servers of the settings scope's file.
    pub fn open_mcp_editor(&mut self) {
        self.clear_transient_status();
        match self
            .mcp_config_path()
            .and_then(|path| Self::mcp_servers_in(&path).map(|s| (path, s)))
        {
            Ok((path, servers)) => {
                self.overlay = Some(Overlay::Records(RecordEditor::mcp(path, &servers)));
            }
            Err(e) => self.set_status(e, StatusKind::Error),
        }
    }

    /// `:mcpedit <name>`: the servers of the file that holds `name` (the
    /// project file first, then the global file), with its form open.
    pub fn open_mcp_server_editor(&mut self, name: &str) -> Result<(), String> {
        let (path, _) = self
            .mcp_server_file(name)
            .ok_or_else(|| format!("{name} is in no config file: :mcpadd {name} adds it"))?;
        let servers = Self::mcp_servers_in(&path)?;
        self.clear_transient_status();
        let mut ed = RecordEditor::mcp(path, &servers);
        ed.select(name);
        ed.enter_form();
        self.overlay = Some(Overlay::Records(ed));
        Ok(())
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
        self.overlay = Some(Overlay::Records(RecordEditor::oauth(path, &cfg.oauth)));
    }

    /// Writes every entry to the editor's file. Returns false (with the
    /// reason in the editor) when an entry is not valid or the write failed.
    pub fn save_records(&mut self, ed: &mut RecordEditor) -> bool {
        match self.write_records(ed) {
            Ok(plain) => {
                let sel = ed.record().map(Record::name);
                // Read the file back: the form shows what it holds now.
                self.reload_records(ed);
                if let Some(name) = sel {
                    ed.select(&name);
                }
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
            Err((name, e)) => {
                if let Some(name) = name {
                    ed.show_problem(&name);
                }
                ed.error = Some(e);
                false
            }
        }
    }

    /// The editor's entries from its file again (after a save).
    fn reload_records(&mut self, ed: &mut RecordEditor) {
        let fresh = match ed.kind {
            RecordKind::Mcp => Self::mcp_servers_in(&ed.path)
                .ok()
                .map(|s| RecordEditor::mcp(ed.path.clone(), &s)),
            RecordKind::OAuth => {
                let (cfg, _) = config::load(None, Some(&ed.path));
                Some(RecordEditor::oauth(ed.path.clone(), &cfg.oauth))
            }
        };
        if let Some(fresh) = fresh {
            ed.records = fresh.records;
            ed.sel = ed.sel.min(ed.records.len());
            ed.clamp_field();
        }
    }

    /// Validates and writes the entries; returns the credentials written as
    /// plain text, or the entry with a problem and the problem.
    fn write_records(
        &mut self,
        ed: &RecordEditor,
    ) -> Result<Vec<String>, (Option<String>, String)> {
        ed.check_names().map_err(|e| (None, e))?;
        let mut plain = Vec::new();
        match ed.kind {
            RecordKind::Mcp => {
                let mut servers = BTreeMap::new();
                for r in &ed.records {
                    let name = r.name();
                    let (server, p) = r.to_mcp(&name).map_err(|e| (Some(name.clone()), e))?;
                    plain.extend(p);
                    servers.insert(name, server);
                }
                config::set_mcp_servers(&ed.path, &servers).map_err(|e| (None, e.to_string()))?;
                self.reload_mcp_servers();
            }
            RecordKind::OAuth => {
                let mut ids = BTreeMap::new();
                for r in &ed.records {
                    let name = r.name();
                    let (id, p) = r.to_oauth(&name).map_err(|e| (Some(name.clone()), e))?;
                    plain.extend(p);
                    ids.insert(name, id);
                }
                config::set_table(&ed.path, &["oauth"], &ids).map_err(|e| (None, e.to_string()))?;
                self.project.config.oauth = self.merged_config().oauth;
            }
        }
        Ok(plain)
    }

    /// Keys of a record editor; false closes it.
    pub(crate) fn records_key(&mut self, e: &mut RecordEditor, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let act = self.dialog_action(&key);
        if key.code == KeyCode::Esc {
            if e.pane == Pane::Form {
                e.pane = Pane::List;
                return true;
            }
            if e.dirty {
                let ed = std::mem::replace(e, RecordEditor::placeholder());
                self.parked_records = Some(ed);
                self.overlay = Some(Overlay::Confirm(Confirm {
                    title: "Unsaved changes".into(),
                    message: "Save the changes to the config file?".into(),
                    buttons: vec!["Save".into(), "Discard".into(), "Cancel".into()],
                    sel: 0,
                    action: ConfirmAction::RecordsClose,
                }));
            }
            return false;
        }
        match act {
            Some(Action::DialogConfirm) => {
                self.save_records(e);
                return true;
            }
            Some(Action::DialogNew) => {
                e.add();
                return true;
            }
            Some(Action::DialogTest) if e.kind == RecordKind::Mcp => {
                self.test_mcp_server(e);
                return true;
            }
            Some(Action::DialogSignIn) if e.kind == RecordKind::OAuth => {
                return !self.sign_in_identity(e);
            }
            _ => {}
        }
        let delete = act == Some(Action::DialogDelete);
        match e.pane {
            Pane::List => self.records_list_key(e, key, ctrl, delete),
            Pane::Form => self.records_form_key(e, key, delete),
        }
        true
    }

    fn records_list_key(&mut self, e: &mut RecordEditor, key: KeyEvent, ctrl: bool, delete: bool) {
        match key.code {
            _ if delete => self.delete_record(e),
            KeyCode::Up | KeyCode::Char('k') if !ctrl => e.sel = e.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') if !ctrl => e.sel = (e.sel + 1).min(e.records.len()),
            KeyCode::Enter | KeyCode::Right | KeyCode::Tab if e.sel >= e.records.len() => e.add(),
            KeyCode::Enter | KeyCode::Right | KeyCode::Tab => e.enter_form(),
            KeyCode::Delete => self.delete_record(e),
            _ => {}
        }
        e.error = None;
    }

    /// Removes the selected entry (the file changes on Ctrl+S).
    fn delete_record(&mut self, e: &mut RecordEditor) {
        if e.sel < e.records.len() {
            let name = e.records.remove(e.sel).name();
            e.sel = e.sel.min(e.records.len().saturating_sub(1));
            e.dirty = true;
            e.note = Some(format!(
                "deleted {}; {} writes the file",
                if name.is_empty() {
                    "the new entry"
                } else {
                    &name
                },
                self.dialog_key(Action::DialogConfirm)
            ));
        }
    }

    fn records_form_key(&mut self, e: &mut RecordEditor, key: KeyEvent, delete: bool) {
        let shift_tab = key.code == KeyCode::BackTab
            || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT));
        if shift_tab {
            e.step_field(-1);
            return;
        }
        if key.code == KeyCode::Tab {
            e.step_field(1);
            return;
        }
        let Some(f) = e.focused() else {
            e.pane = Pane::List;
            return;
        };
        let mut changed = false;
        let mut step = 0;
        match &mut f.input {
            Input::Text(ed) => match key.code {
                KeyCode::Up => step = -1,
                KeyCode::Down => step = 1,
                _ => match ed.handle_key(&key, false, &mut self.clipboard) {
                    Outcome::Changed => changed = true,
                    Outcome::Submit => step = 1,
                    _ => {}
                },
            },
            Input::Choice { options, sel } => match key.code {
                KeyCode::Up => step = -1,
                KeyCode::Down | KeyCode::Enter => step = 1,
                KeyCode::Left => {
                    *sel = crate::list::cycle(*sel, -1, options.len());
                    changed = true;
                }
                KeyCode::Right | KeyCode::Char(' ') => {
                    *sel = crate::list::cycle(*sel, 1, options.len());
                    changed = true;
                }
                _ => {}
            },
            Input::Toggle(on) => match key.code {
                KeyCode::Up => step = -1,
                KeyCode::Down | KeyCode::Enter => step = 1,
                KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right => {
                    *on = !*on;
                    changed = true;
                }
                _ => {}
            },
            Input::Table(t) => {
                if delete {
                    changed = t.delete_row();
                } else if key.code == KeyCode::Enter {
                    t.next_cell();
                } else {
                    match t.handle_key(&key, false, &mut self.clipboard) {
                        TableOutcome::Changed => changed = true,
                        TableOutcome::ExitUp => step = -1,
                        TableOutcome::ExitDown => step = 1,
                        _ => {}
                    }
                }
            }
            Input::Heading => step = 1,
        }
        if changed {
            e.dirty = true;
            e.error = None;
            e.note = None;
            e.clamp_field();
        }
        if step != 0 {
            e.step_field(step);
        }
    }

    /// The answer to the "unsaved changes" question of a record editor.
    pub(crate) fn records_close(&mut self, sel: usize) {
        let Some(mut ed) = self.parked_records.take() else {
            return;
        };
        match sel {
            0 if self.save_records(&mut ed) => {}
            0 | 2.. => self.overlay = Some(Overlay::Records(ed)),
            _ => self.set_status("closed without saving", StatusKind::Info),
        }
    }

    /// `Ctrl+T`: lists the tools of the selected server, as saved.
    fn test_mcp_server(&mut self, e: &mut RecordEditor) {
        let name = e.record().map(Record::name).unwrap_or_default();
        if name.is_empty() {
            e.error = Some("select a server first".into());
            return;
        }
        if e.dirty {
            e.error = Some(format!(
                "save ({}) first: the test uses the saved server",
                self.dialog_key(Action::DialogConfirm)
            ));
            return;
        }
        e.error = None;
        e.note = Some(format!("testing {name}…"));
        self.start_mcp_test(&name);
    }

    /// `Ctrl+L`: signs in with the selected identity. Returns true when the
    /// editor closes for the sign-in.
    fn sign_in_identity(&mut self, e: &mut RecordEditor) -> bool {
        let Some(r) = e.record() else {
            e.error = Some("select an identity first".into());
            return false;
        };
        let grant = OAuthGrant::ALL
            .get(r.choice("grant"))
            .copied()
            .unwrap_or_default();
        if !grant.interactive() {
            e.note = Some(
                "this grant needs no sign-in: Sendito gets a token when a request needs one".into(),
            );
            return false;
        }
        if e.dirty {
            e.error = Some(format!(
                "save ({}) first: the sign-in uses the saved identity",
                self.dialog_key(Action::DialogConfirm)
            ));
            return false;
        }
        let name = r.name();
        self.run_command(&format!("auth login {name}"));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The help line of a field names no key: keys change in the keymap,
    /// and the buttons and the hint row show the keys that are set.
    #[test]
    fn field_help_names_no_key() {
        for r in [Record::mcp("a", None), Record::oauth("a", None)] {
            for f in &r.fields {
                for key in ["Ctrl+", "Alt+", "F2", "F5", "Enter", "Esc"] {
                    assert!(!f.help.contains(key), "{}: {}", f.id, f.help);
                }
            }
        }
    }

    #[test]
    fn mcp_fields_follow_the_kind_and_the_auth() {
        let http = McpServer::Http(McpHttpServer {
            url: "https://a.test/mcp".into(),
            headers: BTreeMap::from([("x-key".into(), "{{k}}".into())]),
            verify_tls: false,
            auth: Some(McpServerAuth::Bearer {
                token: "{{t}}".into(),
            }),
        });
        let ed = RecordEditor::mcp(
            PathBuf::new(),
            &BTreeMap::from([("a".into(), http.clone())]),
        );
        let r = ed.record().unwrap();
        let ids: Vec<&str> = ed.visible().iter().map(|i| r.fields[*i].id).collect();
        assert!(ids.contains(&"token") && ids.contains(&"headers"));
        assert!(!ids.contains(&"command") && !ids.contains(&"client_id"));
        assert_eq!(r.to_mcp("a").unwrap().0, http);

        let stdio = McpServer::Stdio(McpStdioServer {
            command: "npx".into(),
            args: vec!["-y".into(), "a b".into()],
            env: BTreeMap::from([("K".into(), "{{v}}".into())]),
        });
        let ed = RecordEditor::mcp(
            PathBuf::new(),
            &BTreeMap::from([("s".into(), stdio.clone())]),
        );
        let r = ed.record().unwrap();
        let ids: Vec<&str> = ed.visible().iter().map(|i| r.fields[*i].id).collect();
        assert!(ids.contains(&"command") && ids.contains(&"args"));
        assert!(!ids.contains(&"url"));
        assert_eq!(r.to_mcp("s").unwrap().0, stdio);
    }

    #[test]
    fn a_command_line_in_the_command_field_splits() {
        let mut r = Record::mcp("s", None);
        for f in &mut r.fields {
            match (f.id, &mut f.input) {
                ("kind", Input::Choice { sel, .. }) => *sel = 1,
                ("command", Input::Text(e)) => e.set_text("npx -y \"a b\""),
                _ => {}
            }
        }
        let McpServer::Stdio(s) = r.to_mcp("s").unwrap().0 else {
            panic!("not stdio")
        };
        assert_eq!(s.command, "npx");
        assert_eq!(s.args, vec!["-y", "a b"]);
    }

    #[test]
    fn oauth_fields_follow_the_grant() {
        let id = OAuthIdentity {
            grant: OAuthGrant::Password,
            token_url: "https://t".into(),
            client_id: "c".into(),
            username: Some("u".into()),
            password: Some("{{p}}".into()),
            scopes: vec!["read".into()],
            ..OAuthIdentity::default()
        };
        let ed = RecordEditor::oauth(PathBuf::new(), &BTreeMap::from([("x".into(), id.clone())]));
        let r = ed.record().unwrap();
        let ids: Vec<&str> = ed.visible().iter().map(|i| r.fields[*i].id).collect();
        assert!(ids.contains(&"username") && ids.contains(&"password"));
        assert!(!ids.contains(&"authorize_url") && !ids.contains(&"device_url"));
        assert_eq!(r.to_oauth("x").unwrap().0, id);
        // The password grant needs a username.
        let mut r = Record::oauth("y", Some(&id));
        for f in &mut r.fields {
            if let ("username", Input::Text(e)) = (f.id, &mut f.input) {
                e.set_text("");
            }
        }
        assert!(r.to_oauth("y").unwrap_err().contains("username"));
    }
}
