//! An opened collection, its settings and its keychain.

use super::*;

/// An opened collection with its settings.
pub struct Project {
    pub root: PathBuf,
    pub coll: Collection,
    pub envs: Arc<Envs>,
    pub config: Config,
    pub warnings: Vec<String>,
    pub name: String,
    /// Global config file (settings are written here); `None` disables writing.
    pub global_config: Option<PathBuf>,
    /// Where user themes are looked for, lowest precedence first.
    pub theme_dirs: Vec<PathBuf>,
    /// `[http] tls` with paths resolved against the collection root.
    pub tls: sendito_http::TlsMaterial,
    /// The last HTTP responses of this session, for `$response`.
    pub responses: super::ResponseCache,
}

/// The keychain to use: `SENDITO_KEYCHAIN=memory` selects an in-memory one (tests, demos).
pub fn default_keychain() -> Arc<dyn Keychain> {
    if std::env::var("SENDITO_KEYCHAIN").is_ok_and(|v| v == "memory") {
        Arc::new(MemoryKeychain::default())
    } else {
        Arc::new(OsKeychain)
    }
}

impl Project {
    pub fn open(root: &Path, keychain: Arc<dyn Keychain>) -> Self {
        Self::open_with(root, keychain, sendito_store::config::global_path())
    }

    /// Opens a collection with an explicit global config path (tests pass `None`).
    pub fn open_with(root: &Path, keychain: Arc<dyn Keychain>, global: Option<PathBuf>) -> Self {
        let project = sendito_store::config::project_path(root);
        let (config, warnings) = sendito_store::config::load(global.as_deref(), Some(&project));
        let mut p = Self {
            root: root.to_path_buf(),
            coll: Collection::new(root, Vec::new()),
            envs: Arc::new(Envs::new(root, String::new(), keychain)),
            config,
            warnings,
            name: String::new(),
            theme_dirs: sendito_store::themes::dirs(global.as_deref(), root),
            global_config: global,
            tls: sendito_http::TlsMaterial::default(),
            responses: Default::default(),
        };
        let problems = p.apply_config();
        p.warnings.extend(problems);
        p
    }

    /// Recomputes what depends on `config` after it changed: the TLS
    /// material, the collection name (and so the keychain accounts) and the
    /// ignore list. Returns the problems found (a bad proxy, half a client
    /// certificate pair).
    pub fn apply_config(&mut self) -> Vec<String> {
        let mut problems = Vec::new();
        let root = self.root.clone();
        if let Some(p) = &self.config.http.proxy
            && let Err(e) = sendito_http::ProxySpec::parse(p)
        {
            problems.push(e);
        }
        let t = &self.config.http.tls;
        let path = |p: &Option<String>| p.as_deref().map(|p| sendito_store::env::expand(&root, p));
        self.tls = sendito_http::TlsMaterial {
            ca: path(&t.ca),
            client_cert: path(&t.client_cert),
            client_key: path(&t.client_key),
        };
        if let Some(e) = self.tls.pair_error() {
            problems.push(e);
        }
        self.name = self.config.collection.name.clone().unwrap_or_else(|| {
            root.canonicalize()
                .ok()
                .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "collection".into())
        });
        self.coll = Collection::new(&root, self.config.collection.ignore.clone());
        let mut envs = Envs::new(&root, self.name.clone(), self.envs.keychain.clone());
        envs.lua_dirs = sendito_store::lua_lib::dirs(self.global_config.as_deref(), &root);
        self.envs = Arc::new(envs);
        problems
    }

    /// `[http] tls` and `[http] proxy` for OAuth requests.
    pub fn oauth_network(&self) -> sendito_mcp::oauth::Network {
        let o = self.send_options();
        sendito_mcp::oauth::Network {
            tls: o.tls,
            proxy: o.proxy,
        }
    }

    pub fn send_options(&self) -> SendOptions {
        SendOptions {
            default_timeout: Duration::from_millis(self.config.http.timeout_ms.max(1)),
            max_body: (self.config.http.max_body_mb.max(1) as usize) * 1024 * 1024,
            proxy: self
                .config
                .http
                .proxy
                .as_deref()
                .and_then(|p| sendito_http::ProxySpec::parse(p).ok()),
            tls: self.tls.clone(),
        }
    }
}
