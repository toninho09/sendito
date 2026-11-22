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
    /// Default watch interval, in seconds (`:watch` re-sends the active
    /// request on this interval until stopped).
    pub watch_interval_s: u64,
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
            watch_interval_s: 5,
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

/// Sets `section.key = value` in a config file, keeping other content.
pub fn set_value(path: &Path, section: &str, key: &str, value: toml::Value) -> Result<()> {
    let mut t = read_table(path)
        .map_err(StoreError::Invalid)?
        .unwrap_or_default();
    let sec = t
        .entry(section.to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let toml::Value::Table(sec) = sec else {
        return Err(StoreError::Invalid(format!("[{section}] is not a table")));
    };
    sec.insert(key.to_string(), value);
    let text = toml::to_string(&t).map_err(|e| StoreError::Invalid(e.to_string()))?;
    write_atomic(path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn set_value_preserves() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("c.toml");
        fs::write(&p, "[http]\ntimeout_ms = 5\n").unwrap();
        set_value(&p, "ui", "vim_mode", toml::Value::Boolean(true)).unwrap();
        let (c, w) = load(None, Some(&p));
        assert!(w.is_empty());
        assert!(c.ui.vim_mode);
        assert_eq!(c.http.timeout_ms, 5);
        set_value(
            &d.path().join("new/c.toml"),
            "ui",
            "theme",
            toml::Value::String("ansi".into()),
        )
        .unwrap();
        fs::write(&p, "ui = 1").unwrap();
        assert!(set_value(&p, "ui", "x", toml::Value::Boolean(true)).is_err());
        assert!(global_path().is_some());
        assert!(project_path(Path::new("/x")).ends_with(".sendito/config.toml"));
    }
}
