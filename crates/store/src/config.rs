//! Layered configuration: built-in defaults < global file < project file.
//!
//! * global: `$XDG_CONFIG_HOME/sendito/config.toml` (default `~/.config/sendito/config.toml`)
//! * project: `<collection>/.sendito/config.toml`

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::collection::{META_DIR, write_atomic};
use crate::{Result, StoreError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[derive(Default)]
pub struct Config {
    pub ui: UiConfig,
    pub http: HttpConfig,
    pub grpc: GrpcConfig,
    pub websocket: WsConfig,
    pub mcp: McpConfig,
    /// Named OAuth identities for ordinary HTTP requests.
    pub oauth: BTreeMap<String, OAuthIdentity>,
    pub history: HistoryConfig,
    pub collection: CollectionConfig,
    /// `[keymap.<mode>] <action> = "key"` or `["key1", "key2"]`.
    pub keymap: BTreeMap<String, BTreeMap<String, KeySpec>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum KeySpec {
    One(String),
    Many(Vec<String>),
}

impl KeySpec {
    pub fn keys(&self) -> Vec<String> {
        match self {
            KeySpec::One(s) => vec![s.clone()],
            KeySpec::Many(v) => v.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiConfig {
    /// Vim-style modal editing and navigation.
    pub vim_mode: bool,
    /// A built-in theme (`auto`, `dusk`, `daylight`, `ember`, `linen`,
    /// `contrast`, `ansi`) or a user theme from a `themes/` directory.
    pub theme: String,
    /// `unicode` (works with any font) or `nerd` (Nerd Font icons).
    pub icons: String,
    /// `left` or `right`.
    pub sidebar: String,
    pub mouse: bool,
    /// Ask before quitting with unsaved changes.
    pub confirm_quit: bool,
    /// Keep the folders of the top tree row pinned while the tree scrolls.
    pub sticky_folders: bool,
    /// Default watch interval, in seconds (`:watch` re-sends the active
    /// request on this interval until stopped).
    pub watch_interval_s: u64,
    /// Key hints in the footer: `full` (as many as fit), `short` (the three
    /// most used) or `off` (only the menu and help keys).
    pub hints: String,
    /// Messages a gRPC or WebSocket stream keeps in the response; the oldest
    /// go first. The counts and sizes still cover every message.
    pub stream_messages_kept: usize,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            vim_mode: false,
            theme: "auto".into(),
            icons: "unicode".into(),
            sidebar: "left".into(),
            mouse: true,
            confirm_quit: true,
            sticky_folders: true,
            watch_interval_s: 5,
            hints: "short".into(),
            stream_messages_kept: 10_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HttpConfig {
    /// Default request timeout when a request sets none.
    pub timeout_ms: u64,
    /// Response bodies larger than this are truncated.
    pub max_body_mb: u64,
    /// Whether requests use the collection cookie jar when the request's
    /// `[options] cookies` says nothing.
    pub cookie_jar: bool,
    /// Outbound proxy for HTTP requests and MCP Streamable HTTP servers:
    /// `"http://host:port"` (also `https://` and `socks4://`, `socks4a://`,
    /// `socks5://`, `socks5h://`), or `"env"` for `HTTPS_PROXY`,
    /// `HTTP_PROXY`, `ALL_PROXY` and `NO_PROXY` read at send time with curl's
    /// precedence. Unset keeps the client's default discovery — the same
    /// variables, `NO_PROXY` included; the OS proxy settings are not read.
    /// gRPC does not use it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<String>,
    /// TLS material (`[http.tls]`): a custom CA and a client certificate.
    pub tls: HttpTlsConfig,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 30_000,
            max_body_mb: 100,
            cookie_jar: false,
            proxy: None,
            tls: HttpTlsConfig::default(),
        }
    }
}

/// PEM files for TLS, used by HTTP, gRPC and MCP Streamable HTTP servers.
/// Paths are relative to the collection root; `~/` expands to the home
/// directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct HttpTlsConfig {
    /// PEM file whose certificates replace the platform roots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca: Option<String>,
    /// PEM file with the client certificate chain (needs `client_key`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_cert: Option<String>,
    /// PEM file with the client private key (needs `client_cert`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct GrpcConfig {
    /// Default deadline when a request sets none (0 = none).
    pub deadline_ms: u64,
}

/// WebSocket settings (`[websocket]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct WsConfig {
    /// Default handshake timeout when a request sets none (0 = none).
    pub timeout_ms: u64,
}

/// Authorization-code + PKCE settings for `[oauth.<name>]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct OAuthIdentity {
    pub authorize_url: String,
    pub token_url: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub scopes: Vec<String>,
}

/// MCP settings and the servers a request can call (`[mcp.servers.<name>]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct McpConfig {
    /// Default call timeout when a request sets none (handshake included).
    pub timeout_ms: u64,
    pub servers: BTreeMap<String, McpServer>,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 30_000,
            servers: BTreeMap::new(),
        }
    }
}

/// One MCP server: a child process speaking stdio, or an HTTP endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum McpServer {
    Stdio(McpStdioServer),
    Http(McpHttpServer),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpStdioServer {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment entries added to the child; values may use `{{variables}}`.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpHttpServer {
    pub url: String,
    /// Extra headers sent with every POST; values may use `{{variables}}`.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default = "default_true")]
    pub verify_tls: bool,
    #[serde(default)]
    pub auth: Option<McpServerAuth>,
}

impl McpServerAuth {
    /// OAuth with discovery and dynamic registration (no fields set).
    pub fn oauth() -> Self {
        McpServerAuth::Oauth {
            client_id: None,
            client_secret: None,
            authorize_url: None,
            token_url: None,
            registration_url: None,
            scopes: Vec::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

/// Authentication for an HTTP MCP server. OAuth tokens are kept in the OS
/// keychain; a static bearer token may reference a secret variable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum McpServerAuth {
    Bearer {
        token: String,
    },
    Oauth {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_secret: Option<String>,
        /// Overrides, for servers where discovery does not work.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        authorize_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        registration_url: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        scopes: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HistoryConfig {
    pub enabled: bool,
    pub max_entries: usize,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_entries: 10_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CollectionConfig {
    /// Collection name (used for keychain account names). Defaults to the directory name.
    pub name: Option<String>,
    /// Directory names skipped when scanning the tree.
    pub ignore: Vec<String>,
}

impl Default for CollectionConfig {
    fn default() -> Self {
        Self {
            name: None,
            ignore: ["target", "node_modules", "vendor", "dist", "build"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

pub fn global_path() -> Option<PathBuf> {
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME").filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(x).join("sendito").join("config.toml"));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/sendito/config.toml"))
}

pub fn project_path(root: &Path) -> PathBuf {
    root.join(META_DIR).join("config.toml")
}

/// Recursively merges `over` into `base` (tables merge, other values replace).
pub fn merge(base: &mut toml::Table, over: toml::Table) {
    for (k, v) in over {
        match (base.get_mut(&k), v) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge(b, o),
            (_, v) => {
                base.insert(k, v);
            }
        }
    }
}

fn read_table(p: &Path) -> std::result::Result<Option<toml::Table>, String> {
    match fs::read_to_string(p) {
        Ok(t) => toml::from_str(&t)
            .map(Some)
            .map_err(|e| format!("{}: {e}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
}

/// Loads the layered config. Problems are returned as warnings; the
/// offending layer is skipped so the app still starts.
pub fn load(global: Option<&Path>, project: Option<&Path>) -> (Config, Vec<String>) {
    let mut warnings = Vec::new();
    let mut merged = toml::Table::new();
    for p in [global, project].into_iter().flatten() {
        match read_table(p) {
            Ok(Some(t)) => {
                // Validate each layer on its own so errors name the right file.
                let mut probe = merged.clone();
                merge(&mut probe, t);
                match probe.clone().try_into::<Config>() {
                    Ok(_) => merged = probe,
                    Err(e) => warnings.push(format!("{}: {e}", p.display())),
                }
            }
            Ok(None) => {}
            Err(e) => warnings.push(e),
        }
    }
    let cfg = merged.try_into::<Config>().unwrap_or_default();
    (cfg, warnings)
}

/// Reads a config file for an edit that keeps its comments and layout.
fn read_doc(path: &Path) -> Result<toml_edit::DocumentMut> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(StoreError::io(path, e)),
    };
    text.parse()
        .map_err(|e| StoreError::Invalid(format!("{}: {e}", path.display())))
}

/// Reads `path`, lets `f` change it and writes it back, keeping comments.
/// Other Sendito processes that edit a file in the same directory wait, so
/// two edits do not overwrite each other.
fn edit_doc(path: &Path, f: impl FnOnce(&mut toml_edit::DocumentMut) -> Result<()>) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    crate::fsio::with_dir_lock(dir, || {
        let mut doc = read_doc(path)?;
        f(&mut doc)?;
        let text = doc.to_string();
        // A literal OAuth client secret: the file is readable by its owner
        // only. A `{{variable}}` reference is not a secret.
        let literal_secret = doc_has_literal_secret(doc.as_item());
        if literal_secret {
            crate::fsio::write_private(path, text.as_bytes())
        } else {
            write_atomic(path, text.as_bytes())
        }
    })
}

fn doc_has_literal_secret(item: &toml_edit::Item) -> bool {
    let Some(t) = item.as_table_like() else {
        return false;
    };
    t.iter().any(|(k, v)| {
        (k == "client_secret"
            && v.as_str()
                .is_some_and(|s| !s.trim_start().starts_with("{{")))
            || doc_has_literal_secret(v)
    })
}

/// The table at the dotted path `keys` in `doc`, with each missing table
/// created (without its own header line while it holds only sub-tables).
fn doc_table<'a>(
    doc: &'a mut toml_edit::DocumentMut,
    keys: &[&str],
) -> Result<&'a mut dyn toml_edit::TableLike> {
    let mut cur: &mut dyn toml_edit::TableLike = doc.as_table_mut();
    for (i, name) in keys.iter().enumerate() {
        let item = cur.entry(name).or_insert_with(|| {
            let mut t = toml_edit::Table::new();
            t.set_implicit(true);
            toml_edit::Item::Table(t)
        });
        cur = item.as_table_like_mut().ok_or_else(|| {
            StoreError::Invalid(format!("[{}] is not a table", keys[..=i].join(".")))
        })?;
    }
    Ok(cur)
}

/// Replaces the table at the dotted path `keys` (for example `["mcp",
/// "servers"]`) with `value`, keeping the rest of the file (comments too).
/// A value that serializes to an empty table removes the table.
pub fn set_table<T: Serialize>(path: &Path, keys: &[&str], value: &T) -> Result<()> {
    let (last, parent) = keys
        .split_last()
        .ok_or_else(|| StoreError::Invalid("no table to write".into()))?;
    let name = keys.join(".");
    let value = toml::Value::try_from(value)
        .map_err(|e| StoreError::Invalid(format!("cannot write [{name}]: {e}")))?;
    edit_doc(path, |doc| {
        let sec = doc_table(doc, parent)?;
        if value.as_table().is_some_and(toml::Table::is_empty) {
            sec.remove(last);
        } else {
            let mut wrap = toml::Table::new();
            wrap.insert((*last).to_string(), value);
            let text = toml::to_string(&wrap).map_err(|e| StoreError::Invalid(e.to_string()))?;
            let mut src: toml_edit::DocumentMut = text
                .parse()
                .map_err(|e| StoreError::Invalid(format!("cannot write [{name}]: {e}")))?;
            let item = src
                .remove(last)
                .ok_or_else(|| StoreError::Invalid(format!("cannot write [{name}]")))?;
            sec.insert(last, item);
        }
        Ok(())
    })
}

/// Replaces `[mcp.servers]` in a config file, keeping the rest of the file
/// (comments too). An empty map removes the table.
pub fn set_mcp_servers(path: &Path, servers: &BTreeMap<String, McpServer>) -> Result<()> {
    set_table(path, &["mcp", "servers"], servers)
}

/// Sets `[mcp.servers.<server>.auth]` in a config file, keeping the rest of
/// the file (comments too). The server must already be in that file.
pub fn set_mcp_server_auth(path: &Path, server: &str, auth: &McpServerAuth) -> Result<()> {
    edit_doc(path, |doc| set_auth_in(doc, path, server, auth))
}

fn set_auth_in(
    doc: &mut toml_edit::DocumentMut,
    path: &Path,
    server: &str,
    auth: &McpServerAuth,
) -> Result<()> {
    let entry = doc
        .get_mut("mcp")
        .and_then(|m| m.get_mut("servers"))
        .and_then(|s| s.get_mut(server))
        .and_then(toml_edit::Item::as_table_like_mut)
        .ok_or_else(|| {
            StoreError::Invalid(format!(
                "[mcp.servers.{server}] is not in {}",
                path.display()
            ))
        })?;
    let mut wrap = toml::Table::new();
    let value = toml::Value::try_from(auth)
        .map_err(|e| StoreError::Invalid(format!("cannot write the auth of {server}: {e}")))?;
    wrap.insert("auth".to_string(), value);
    let text = toml::to_string(&wrap).map_err(|e| StoreError::Invalid(e.to_string()))?;
    let mut src: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| StoreError::Invalid(format!("cannot write the auth of {server}: {e}")))?;
    let item = src
        .remove("auth")
        .ok_or_else(|| StoreError::Invalid(format!("cannot write the auth of {server}")))?;
    entry.insert("auth", item);
    Ok(())
}

/// Sets `section.key = value` in a config file, keeping other content
/// (comments too). `section` is a dotted path (`http.tls`); `None` removes
/// the key.
pub fn set_value(path: &Path, section: &str, key: &str, value: Option<toml::Value>) -> Result<()> {
    edit_doc(path, |doc| set_value_in(doc, section, key, value))
}

fn set_value_in(
    doc: &mut toml_edit::DocumentMut,
    section: &str,
    key: &str,
    value: Option<toml::Value>,
) -> Result<()> {
    let keys: Vec<&str> = section.split('.').collect();
    let sec = doc_table(doc, &keys)?;
    let Some(value) = value else {
        sec.remove(key);
        return Ok(());
    };
    let v: toml_edit::Value = value
        .to_string()
        .parse()
        .map_err(|e| StoreError::Invalid(format!("cannot write {section}.{key}: {e}")))?;
    match sec.get_mut(key).and_then(toml_edit::Item::as_value_mut) {
        // Keep the comment written after the old value on its line.
        Some(old) => {
            let decor = old.decor().clone();
            *old = v;
            *old.decor_mut() = decor;
        }
        None => {
            sec.insert(key, toml_edit::Item::Value(v));
        }
    }
    Ok(())
}

/// Whether a config file sets `section.key` (`section` is a dotted path).
/// A file that cannot be read sets nothing.
pub fn has_value(path: &Path, section: &str, key: &str) -> bool {
    let Ok(Some(t)) = read_table(path) else {
        return false;
    };
    let mut cur = &t;
    for name in section.split('.') {
        match cur.get(name).and_then(toml::Value::as_table) {
            Some(next) => cur = next,
            None => return false,
        }
    }
    cur.contains_key(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_mcp_server_auth_keeps_the_rest() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config.toml");
        fs::write(
            &p,
            "# servers\n[mcp.servers.a]\nurl = \"https://a/mcp\" # main\n\n[mcp.servers.b]\nurl = \"https://b/mcp\"\n",
        )
        .unwrap();
        let auth = McpServerAuth::oauth();
        set_mcp_server_auth(&p, "a", &auth).unwrap();
        let text = fs::read_to_string(&p).unwrap();
        assert!(
            text.contains("# servers") && text.contains("# main"),
            "{text}"
        );
        let (c, w) = load(None, Some(&p));
        assert!(w.is_empty(), "{w:?}");
        let McpServer::Http(a) = &c.mcp.servers["a"] else {
            panic!("a is not http")
        };
        assert_eq!(a.auth, Some(auth.clone()));
        let McpServer::Http(b) = &c.mcp.servers["b"] else {
            panic!("b is not http")
        };
        assert_eq!(b.auth, None);
        assert!(set_mcp_server_auth(&p, "missing", &auth).is_err());
    }

    #[test]
    fn layering() {
        let d = tempfile::tempdir().unwrap();
        let g = d.path().join("g.toml");
        let p = d.path().join("p.toml");
        fs::write(
            &g,
            "[ui]\nvim_mode = true\ntheme = \"dusk\"\n[keymap.normal]\nsend = \"f9\"\n",
        )
        .unwrap();
        fs::write(&p, "[ui]\ntheme = \"daylight\"\n[keymap.normal]\nquit = [\"ctrl+q\", \"f10\"]\n[collection]\nname = \"acme\"\n").unwrap();
        let (c, w) = load(Some(&g), Some(&p));
        assert!(w.is_empty(), "{w:?}");
        assert!(c.ui.vim_mode);
        assert_eq!(c.ui.theme, "daylight");
        assert_eq!(c.keymap["normal"]["send"].keys(), vec!["f9".to_string()]);
        assert_eq!(c.keymap["normal"]["quit"].keys().len(), 2);
        assert_eq!(c.collection.name.as_deref(), Some("acme"));
        assert_eq!(c.http.timeout_ms, 30_000);
    }

    #[test]
    fn mcp_servers_parse() {
        let text = r#"
[mcp]
timeout_ms = 5000

[mcp.servers.files]
command = "npx"
args = ["-y", "@mcp/server-fs", "."]
env = { ROOT = "{{root}}" }

[mcp.servers.remote]
url = "https://api.example.com/mcp/"
verify_tls = false

[mcp.servers.remote.headers]
X-Api-Version = "2"

[mcp.servers.remote.auth]
type = "oauth"
scopes = ["read"]
"#;
        let c: Config = toml::from_str(text).unwrap();
        assert_eq!(c.mcp.timeout_ms, 5000);
        let Some(McpServer::Stdio(s)) = c.mcp.servers.get("files") else {
            panic!("files must be stdio");
        };
        assert_eq!(s.command, "npx");
        assert_eq!(s.args.len(), 3);
        assert_eq!(s.env.get("ROOT").map(String::as_str), Some("{{root}}"));
        let Some(McpServer::Http(h)) = c.mcp.servers.get("remote") else {
            panic!("remote must be http");
        };
        assert!(!h.verify_tls);
        assert_eq!(
            h.headers.get("X-Api-Version").map(String::as_str),
            Some("2")
        );
        assert!(matches!(
            &h.auth,
            Some(McpServerAuth::Oauth { scopes, .. }) if scopes == &vec!["read".to_string()]
        ));
        // Defaults: bearer auth, TLS on, no scopes.
        let bearer = "[mcp.servers.b]\nurl = \"https://x\"\n[mcp.servers.b.auth]\ntype = \"bearer\"\ntoken = \"{{t}}\"\n";
        let c: Config = toml::from_str(bearer).unwrap();
        let Some(McpServer::Http(h)) = c.mcp.servers.get("b") else {
            panic!();
        };
        assert!(h.verify_tls);
        assert!(matches!(&h.auth, Some(McpServerAuth::Bearer { token }) if token == "{{t}}"));
    }

    #[test]
    fn http_tls_parses() {
        let c: Config = toml::from_str(
            "[http.tls]\nca = \"certs/ca.pem\"\nclient_cert = \"c.pem\"\nclient_key = \"k.pem\"\n",
        )
        .unwrap();
        assert_eq!(c.http.tls.ca.as_deref(), Some("certs/ca.pem"));
        assert_eq!(c.http.tls.client_cert.as_deref(), Some("c.pem"));
        assert_eq!(c.http.tls.client_key.as_deref(), Some("k.pem"));
        let c: Config = toml::from_str("[http]\ntimeout_ms = 1\n").unwrap();
        assert_eq!(c.http.tls, Default::default());
        assert!(toml::from_str::<HttpConfig>("[tls]\nbogus = 1\n").is_err());
    }

    #[test]
    fn http_proxy_parses() {
        let c: Config = toml::from_str("[http]\nproxy = \"socks5://127.0.0.1:1080\"\n").unwrap();
        assert_eq!(c.http.proxy.as_deref(), Some("socks5://127.0.0.1:1080"));
        let c: Config = toml::from_str("[http]\ntimeout_ms = 1\n").unwrap();
        assert_eq!(c.http.proxy, None);
    }

    #[test]
    fn bad_layers_warn() {
        let d = tempfile::tempdir().unwrap();
        let g = d.path().join("g.toml");
        let p = d.path().join("p.toml");
        fs::write(&g, "[ui]\nvim_mode = \"yes\"\n").unwrap();
        fs::write(&p, "not = [valid").unwrap();
        let (c, w) = load(Some(&g), Some(&p));
        assert_eq!(w.len(), 2);
        assert_eq!(c, Config::default());
        let (c, w) = load(None, Some(&d.path().join("missing.toml")));
        assert!(w.is_empty());
        assert_eq!(c, Config::default());
    }

    #[test]
    fn set_mcp_servers_preserves_the_rest() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        fs::write(
            &p,
            "[collection]\nname = \"x\"\n[mcp]\ntimeout_ms = 5\n\
             [mcp.servers.old]\nurl = \"https://old\"\n[mcp.servers.old.headers]\na = \"b\"\n",
        )
        .unwrap();
        let mut servers = BTreeMap::new();
        servers.insert(
            "new".to_string(),
            McpServer::Http(McpHttpServer {
                url: "https://new/mcp".into(),
                headers: BTreeMap::new(),
                verify_tls: false,
                auth: Some(McpServerAuth::Bearer {
                    token: "{{t}}".into(),
                }),
            }),
        );
        set_mcp_servers(&p, &servers).unwrap();
        let (c, w) = load(None, Some(&p));
        assert!(w.is_empty());
        // The other sections survive, and the old server is gone.
        assert_eq!(c.collection.name.as_deref(), Some("x"));
        assert_eq!(c.mcp.timeout_ms, 5);
        assert_eq!(c.mcp.servers, servers);
        // An empty map removes the table without touching [mcp] itself.
        set_mcp_servers(&p, &BTreeMap::new()).unwrap();
        let (c, w) = load(None, Some(&p));
        assert!(w.is_empty());
        assert!(c.mcp.servers.is_empty());
        assert_eq!(c.mcp.timeout_ms, 5);
        fs::write(&p, "mcp = 1").unwrap();
        assert!(set_mcp_servers(&p, &BTreeMap::new()).is_err());
    }

    #[test]
    fn set_value_preserves() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        fs::write(&p, "[http]\ntimeout_ms = 5\n").unwrap();
        set_value(&p, "ui", "vim_mode", Some(toml::Value::Boolean(true))).unwrap();
        let (c, w) = load(None, Some(&p));
        assert!(w.is_empty());
        assert!(c.ui.vim_mode);
        assert_eq!(c.http.timeout_ms, 5);
        set_value(
            &d.path().join("new/c.toml"),
            "ui",
            "theme",
            Some(toml::Value::String("ansi".into())),
        )
        .unwrap();
        // Comments and other lines survive an edit.
        fs::write(
            &p,
            "# my settings\n[ui]\ntheme = \"dusk\" # mine\nmouse = false\n",
        )
        .unwrap();
        set_value(&p, "ui", "theme", Some(toml::Value::String("ansi".into()))).unwrap();
        let text = fs::read_to_string(&p).unwrap();
        assert_eq!(
            text,
            "# my settings\n[ui]\ntheme = \"ansi\" # mine\nmouse = false\n"
        );
        set_mcp_servers(&p, &BTreeMap::new()).unwrap();
        assert!(
            fs::read_to_string(&p)
                .unwrap()
                .starts_with("# my settings\n")
        );
        fs::write(&p, "ui = 1").unwrap();
        assert!(set_value(&p, "ui", "x", Some(toml::Value::Boolean(true))).is_err());
        assert!(global_path().is_some());
        assert!(project_path(Path::new("/x")).ends_with(".sendito/config.toml"));
    }

    #[cfg(unix)]
    #[test]
    fn a_literal_client_secret_makes_the_config_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o077;
        set_value(&p, "oauth.api", "client_secret", Some("{{secret}}".into())).unwrap();
        assert_ne!(mode(&p), 0, "a variable reference is not a secret");
        set_value(&p, "oauth.api", "client_secret", Some("s3cr3t".into())).unwrap();
        assert_eq!(mode(&p), 0);
    }

    #[test]
    fn set_value_nested_and_remove() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        fs::write(&p, "[http]\ntimeout_ms = 5 # short\n").unwrap();
        set_value(
            &p,
            "http.tls",
            "ca",
            Some(toml::Value::String("ca.pem".into())),
        )
        .unwrap();
        let (c, w) = load(None, Some(&p));
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(c.http.tls.ca.as_deref(), Some("ca.pem"));
        assert_eq!(c.http.timeout_ms, 5);
        assert!(has_value(&p, "http.tls", "ca"));
        assert!(has_value(&p, "http", "timeout_ms"));
        assert!(!has_value(&p, "http", "proxy"));
        assert!(!has_value(&d.path().join("none.toml"), "http", "proxy"));
        set_value(&p, "http.tls", "ca", None).unwrap();
        set_value(&p, "http", "proxy", None).unwrap();
        let (c, _) = load(None, Some(&p));
        assert_eq!(c.http.tls.ca, None);
        assert!(fs::read_to_string(&p).unwrap().contains("# short"));
        fs::write(&p, "[http]\ntls = 1\n").unwrap();
        assert!(set_value(&p, "http.tls", "ca", None).is_err());
    }

    #[test]
    fn set_table_writes_and_removes() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        fs::write(&p, "# top\n[ui]\nmouse = false\n").unwrap();
        let mut ids = BTreeMap::new();
        ids.insert(
            "gh".to_string(),
            OAuthIdentity {
                authorize_url: "https://a".into(),
                token_url: "https://t".into(),
                client_id: "id".into(),
                client_secret: Some("{{s}}".into()),
                scopes: vec!["repo".into()],
            },
        );
        set_table(&p, &["oauth"], &ids).unwrap();
        let (c, w) = load(None, Some(&p));
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(c.oauth, ids);
        assert!(!c.ui.mouse);
        set_table(&p, &["oauth"], &BTreeMap::<String, OAuthIdentity>::new()).unwrap();
        let (c, _) = load(None, Some(&p));
        assert!(c.oauth.is_empty());
        assert!(fs::read_to_string(&p).unwrap().starts_with("# top\n"));
        assert!(set_table(&p, &[], &ids).is_err());
    }
}
