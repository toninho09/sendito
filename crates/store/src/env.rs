//! Environments and secret sources.
//!
//! * `.sendito/collection.toml` — collection-level variables (always active).
//! * `.sendito/environments/<name>.toml` — selectable environments.
//! * `.sendito/secrets.toml` — gitignored local secret values.
//!
//! Each variable has exactly one source: plain text in the env file, the local
//! secrets file, the OS keychain, the contents of a file, or a key of a
//! `.env` file.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sendito_core::VarValue;

use crate::collection::{META_DIR, make_private, write_atomic, write_private};
use crate::tomlfmt::{Writer, quote_basic};
use crate::{Result, StoreError};

pub const COLLECTION_ENV: &str = "_collection";
pub const KEYCHAIN_SERVICE: &str = "sendito";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VarSource {
    /// Value stored in the (git-tracked) environment file.
    Plain(String),
    /// `.sendito/secrets.toml`, section `[<env>]`, key `key` (default: variable name).
    Local { key: Option<String> },
    /// OS keychain, service `sendito`, account `key` (default `<collection>/<env>/<var>`).
    Keychain { key: Option<String> },
    /// Contents of a file (one trailing newline removed).
    File { path: String },
    /// The environment's `.env` file (see [`EnvFile::dotenv`]), key `key`
    /// (default: variable name). Read only.
    Dotenv { key: Option<String> },
}

impl VarSource {
    pub fn label(&self) -> &'static str {
        match self {
            VarSource::Plain(_) => "plain",
            VarSource::Local { .. } => "local",
            VarSource::Keychain { .. } => "keychain",
            VarSource::File { .. } => "file",
            VarSource::Dotenv { .. } => "dotenv",
        }
    }

    pub fn is_secret(&self) -> bool {
        !matches!(self, VarSource::Plain(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvVar {
    pub name: String,
    pub source: VarSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvFile {
    pub name: String,
    /// Path of the `.env` file that `dotenv` variables read; `None` means
    /// `.env` at the collection root.
    pub dotenv: Option<String>,
    pub vars: Vec<EnvVar>,
}

impl EnvFile {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            dotenv: None,
            vars: Vec::new(),
        }
    }

    pub fn get(&self, name: &str) -> Option<&EnvVar> {
        self.vars.iter().find(|v| v.name == name)
    }

    pub fn set(&mut self, var: EnvVar) {
        if let Some(v) = self.vars.iter_mut().find(|v| v.name == var.name) {
            *v = var;
        } else {
            self.vars.push(var);
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.vars.retain(|v| v.name != name);
    }
}

pub fn parse_env(name: &str, text: &str) -> std::result::Result<EnvFile, String> {
    let t: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut env = EnvFile::new(name);
    for k in t.keys() {
        if !matches!(k.as_str(), "variables" | "dotenv") {
            return Err(format!(
                "unknown key {k:?} (expected `dotenv` or [variables])"
            ));
        }
    }
    if let Some(p) = t.get("dotenv") {
        let p = p
            .as_str()
            .ok_or_else(|| "`dotenv` must be a string (a path)".to_string())?;
        env.dotenv = Some(p.to_string());
    }
    let Some(vars) = t.get("variables") else {
        return Ok(env);
    };
    let vars = vars
        .as_table()
        .ok_or_else(|| "[variables] must be a table".to_string())?;
    for (k, v) in vars {
        let source = match v {
            toml::Value::String(s) => VarSource::Plain(s.clone()),
            toml::Value::Table(tt) => {
                let get = |f: &str| tt.get(f).and_then(|x| x.as_str()).map(str::to_string);
                for key in tt.keys() {
                    if !matches!(key.as_str(), "source" | "key" | "path") {
                        return Err(format!("variable {k:?}: unknown field {key:?}"));
                    }
                }
                match get("source").as_deref() {
                    Some("local") => VarSource::Local { key: get("key") },
                    Some("keychain") => VarSource::Keychain { key: get("key") },
                    Some("dotenv") => VarSource::Dotenv { key: get("key") },
                    Some("file") => VarSource::File {
                        path: get("path")
                            .ok_or_else(|| format!("variable {k:?}: file source needs `path`"))?,
                    },
                    o => {
                        return Err(format!(
                            "variable {k:?}: source must be \"local\", \"keychain\", \"file\" or \"dotenv\", got {o:?}"
                        ));
                    }
                }
            }
            other => VarSource::Plain(match other {
                toml::Value::Integer(i) => i.to_string(),
                toml::Value::Float(f) => f.to_string(),
                toml::Value::Boolean(b) => b.to_string(),
                _ => return Err(format!("variable {k:?}: unsupported value")),
            }),
        };
        env.vars.push(EnvVar {
            name: k.clone(),
            source,
        });
    }
    Ok(env)
}

pub fn env_to_toml(env: &EnvFile) -> String {
    let mut w = Writer::new();
    if let Some(p) = &env.dotenv {
        w.str("dotenv", p);
    }
    w.table("variables");
    for v in &env.vars {
        match &v.source {
            VarSource::Plain(s) => w.str(&v.name, s),
            VarSource::Local { key } => match key {
                Some(k) => w.inline_table(&v.name, &[("source", "local"), ("key", k)]),
                None => w.inline_table(&v.name, &[("source", "local")]),
            },
            VarSource::Keychain { key } => match key {
                Some(k) => w.inline_table(&v.name, &[("source", "keychain"), ("key", k)]),
                None => w.inline_table(&v.name, &[("source", "keychain")]),
            },
            VarSource::File { path } => {
                w.inline_table(&v.name, &[("source", "file"), ("path", path)])
            }
            VarSource::Dotenv { key } => match key {
                Some(k) => w.inline_table(&v.name, &[("source", "dotenv"), ("key", k)]),
                None => w.inline_table(&v.name, &[("source", "dotenv")]),
            },
        }
    }
    w.finish()
}

/// Access to an OS keychain (or a test double).
pub trait Keychain: Send + Sync {
    fn get(&self, account: &str) -> std::result::Result<Option<String>, String>;
    fn set(&self, account: &str, value: &str) -> std::result::Result<(), String>;
    fn delete(&self, account: &str) -> std::result::Result<(), String>;
}

/// The platform keychain: Secret Service on Linux, Keychain on macOS.
pub struct OsKeychain;

impl Keychain for OsKeychain {
    fn get(&self, account: &str) -> std::result::Result<Option<String>, String> {
        let e = keyring::Entry::new(KEYCHAIN_SERVICE, account).map_err(|e| e.to_string())?;
        match e.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn set(&self, account: &str, value: &str) -> std::result::Result<(), String> {
        keyring::Entry::new(KEYCHAIN_SERVICE, account)
            .and_then(|e| e.set_password(value))
            .map_err(|e| e.to_string())
    }

    fn delete(&self, account: &str) -> std::result::Result<(), String> {
        let e = keyring::Entry::new(KEYCHAIN_SERVICE, account).map_err(|e| e.to_string())?;
        match e.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// In-memory keychain for tests and for `SENDITO_KEYCHAIN=memory`.
#[derive(Default)]
pub struct MemoryKeychain {
    map: Mutex<BTreeMap<String, String>>,
}

impl Keychain for MemoryKeychain {
    fn get(&self, account: &str) -> std::result::Result<Option<String>, String> {
        Ok(self
            .map
            .lock()
            .map_err(|e| e.to_string())?
            .get(account)
            .cloned())
    }

    fn set(&self, account: &str, value: &str) -> std::result::Result<(), String> {
        self.map
            .lock()
            .map_err(|e| e.to_string())?
            .insert(account.into(), value.into());
        Ok(())
    }

    fn delete(&self, account: &str) -> std::result::Result<(), String> {
        self.map.lock().map_err(|e| e.to_string())?.remove(account);
        Ok(())
    }
}

/// Environments, the local secrets file and the keychain for one collection.
pub struct Envs {
    pub root: PathBuf,
    pub collection_name: String,
    pub keychain: Arc<dyn Keychain>,
    /// The directories of the Lua function catalog, in load order (see
    /// [`crate::lua_lib::dirs`]); the collection's only by default.
    pub lua_dirs: Vec<(&'static str, PathBuf)>,
}

/// Outcome of resolving the variables of one environment.
#[derive(Debug, Default)]
pub struct ResolvedVars {
    pub vars: BTreeMap<String, VarValue>,
    /// Variables whose source failed, with the reason.
    pub errors: Vec<(String, String)>,
}

impl Envs {
    pub fn new(
        root: impl Into<PathBuf>,
        collection_name: impl Into<String>,
        keychain: Arc<dyn Keychain>,
    ) -> Self {
        let root = root.into();
        Self {
            lua_dirs: crate::lua_lib::dirs(None, &root),
            root,
            collection_name: collection_name.into(),
            keychain,
        }
    }

    fn meta(&self) -> PathBuf {
        self.root.join(META_DIR)
    }

    pub fn env_dir(&self) -> PathBuf {
        self.meta().join("environments")
    }

    pub fn secrets_path(&self) -> PathBuf {
        self.meta().join("secrets.toml")
    }

    fn env_path(&self, name: &str) -> PathBuf {
        if name == COLLECTION_ENV {
            self.meta().join("collection.toml")
        } else {
            self.env_dir().join(format!("{name}.toml"))
        }
    }

    /// Names of the selectable environments, sorted.
    pub fn list(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(self.env_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                (p.extension().is_some_and(|x| x == "toml"))
                    .then(|| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                    .flatten()
            })
            .filter(|n| !n.starts_with('.'))
            .collect();
        v.sort();
        v
    }

    pub fn load(&self, name: &str) -> Result<EnvFile> {
        let p = self.env_path(name);
        match fs::read_to_string(&p) {
            Ok(t) => parse_env(name, &t).map_err(|msg| StoreError::Parse { path: p, msg }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && name == COLLECTION_ENV => {
                Ok(EnvFile::new(name))
            }
            Err(e) => Err(StoreError::io(&p, e)),
        }
    }

    pub fn save(&self, env: &EnvFile) -> Result<()> {
        if env.name != COLLECTION_ENV {
            crate::collection::validate_name(&env.name)?;
            if env.name.starts_with('_') {
                return Err(StoreError::Invalid(
                    "environment names cannot start with '_'".into(),
                ));
            }
            // Script values live in a scope layer with this name.
            if env.name == sendito_core::script::RUNTIME_LAYER {
                return Err(StoreError::Invalid(format!(
                    "\"{}\" is reserved for values set by scripts",
                    env.name
                )));
            }
        }
        let p = self.env_path(&env.name);
        let text = env_to_toml(env);
        if fs::read_to_string(&p)
            .ok()
            .and_then(|t| parse_env(&env.name, &t).ok())
            .as_ref()
            == Some(env)
        {
            return Ok(());
        }
        write_atomic(&p, text.as_bytes())
    }

    pub fn delete(&self, name: &str) -> Result<()> {
        let p = self.env_path(name);
        fs::remove_file(&p).map_err(|e| StoreError::io(&p, e))
    }

    pub fn rename(&self, from: &str, to: &str) -> Result<()> {
        crate::collection::validate_name(to)?;
        let (a, b) = (self.env_path(from), self.env_path(to));
        if b.exists() {
            return Err(StoreError::Exists(b));
        }
        fs::rename(&a, &b).map_err(|e| StoreError::io(&a, e))
    }

    pub fn keychain_account(&self, env: &str, var: &str, key: &Option<String>) -> String {
        key.clone()
            .unwrap_or_else(|| format!("{}/{}/{}", self.collection_name, env, var))
    }

    fn load_local(&self) -> std::result::Result<toml::Table, String> {
        let path = self.secrets_path();
        match fs::read_to_string(&path) {
            Ok(t) => {
                make_private(&path);
                // The full error quotes the failing line, which holds a
                // secret: report the position and the reason only.
                toml::from_str(&t).map_err(|e: toml::de::Error| {
                    let line = e
                        .span()
                        .and_then(|r| t.get(..r.start))
                        .map(|head| head.matches('\n').count() + 1);
                    match line {
                        Some(l) => format!("secrets.toml line {l}: {}", e.message()),
                        None => format!("secrets.toml: {}", e.message()),
                    }
                })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(toml::Table::new()),
            Err(e) => Err(format!("secrets.toml: {e}")),
        }
    }

    /// The `.env` file that the `dotenv` variables of `env` read.
    pub fn dotenv_path(&self, env: &EnvFile) -> PathBuf {
        expand(&self.root, env.dotenv.as_deref().unwrap_or(".env"))
    }

    /// Reads the `.env` file of `env`. An error names the file and the line,
    /// never the line's text: it can hold a secret.
    fn load_dotenv(&self, env: &EnvFile) -> std::result::Result<BTreeMap<String, String>, String> {
        let path = self.dotenv_path(env);
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut out = BTreeMap::new();
        for item in dotenvy::from_read_iter(text.as_bytes()) {
            match item {
                Ok((k, v)) => {
                    out.insert(k, v);
                }
                Err(dotenvy::Error::LineParse(line, _)) => {
                    // The failing line is a part of the file text.
                    let at = (!line.is_empty())
                        .then(|| text.find(&line))
                        .flatten()
                        .map(|i| text[..i].matches('\n').count() + 1);
                    return Err(match at {
                        Some(l) => format!("{} line {l}: cannot parse the line", path.display()),
                        None => format!("{}: cannot parse the file", path.display()),
                    });
                }
                Err(e) => return Err(format!("{}: {e}", path.display())),
            }
        }
        Ok(out)
    }

    /// How many keys the `.env` file of `env` holds. An error names the file
    /// and the line, never the line's text.
    pub fn dotenv_keys(&self, env: &EnvFile) -> std::result::Result<usize, String> {
        self.load_dotenv(env).map(|m| m.len())
    }

    /// Whether a secret has a value, when that can be known without blocking:
    /// `Some` for `local` (secrets.toml), `file` (the file exists) and
    /// `dotenv` (the `.env` file has the key), `None` for plain values and
    /// the keychain.
    pub fn is_set(&self, env: &EnvFile, var: &EnvVar) -> Option<bool> {
        match &var.source {
            VarSource::Local { key } => {
                let k = key.as_deref().unwrap_or(&var.name);
                Some(self.load_local().ok().is_some_and(|t| {
                    t.get(&env.name)
                        .and_then(|s| s.get(k))
                        .is_some_and(|x| x.is_str())
                }))
            }
            VarSource::File { path } => Some(expand(&self.root, path).is_file()),
            VarSource::Dotenv { key } => {
                let k = key.as_deref().unwrap_or(&var.name);
                Some(self.load_dotenv(env).is_ok_and(|m| m.contains_key(k)))
            }
            _ => None,
        }
    }

    /// Reads every variable of `env` from its source. Keychain access may block,
    /// so call this off the UI thread.
    pub fn resolve(&self, env: &EnvFile) -> ResolvedVars {
        let mut out = ResolvedVars::default();
        let mut local: Option<std::result::Result<toml::Table, String>> = None;
        let mut dotenv: Option<std::result::Result<BTreeMap<String, String>, String>> = None;
        for v in &env.vars {
            let r: std::result::Result<String, String> = match &v.source {
                VarSource::Plain(s) => {
                    out.vars.insert(v.name.clone(), VarValue::plain(s.clone()));
                    continue;
                }
                VarSource::Local { key } => {
                    let table = local.get_or_insert_with(|| self.load_local());
                    match table {
                        Err(e) => Err(e.clone()),
                        Ok(t) => {
                            let k = key.as_deref().unwrap_or(&v.name);
                            t.get(&env.name)
                                .and_then(|s| s.get(k))
                                .and_then(|x| x.as_str())
                                .map(str::to_string)
                                .ok_or_else(|| {
                                    format!("not set in .sendito/secrets.toml [{}] {k}", env.name)
                                })
                        }
                    }
                }
                VarSource::Keychain { key } => {
                    let account = self.keychain_account(&env.name, &v.name, key);
                    match self.keychain.get(&account) {
                        Ok(Some(s)) => Ok(s),
                        Ok(None) => Err(format!("no keychain entry for {account}")),
                        Err(e) => Err(format!("keychain: {e}")),
                    }
                }
                VarSource::File { path } => {
                    let p = expand(&self.root, path);
                    fs::read_to_string(&p)
                        .map(strip_one_newline)
                        .map_err(|e| format!("{}: {e}", p.display()))
                }
                VarSource::Dotenv { key } => {
                    let map = dotenv.get_or_insert_with(|| self.load_dotenv(env));
                    match map {
                        Err(e) => Err(e.clone()),
                        Ok(m) => {
                            let k = key.as_deref().unwrap_or(&v.name);
                            m.get(k).cloned().ok_or_else(|| {
                                format!("{k} not set in {}", self.dotenv_path(env).display())
                            })
                        }
                    }
                }
            };
            match r {
                Ok(s) => {
                    out.vars.insert(v.name.clone(), VarValue::secret(s));
                }
                Err(e) => out.errors.push((v.name.clone(), e)),
            }
        }
        out
    }

    /// Stores a secret value in the variable's own source (local file,
    /// keychain or file). A `dotenv` variable is read only.
    pub fn store_secret(&self, env: &str, var: &EnvVar, value: &str) -> Result<()> {
        match &var.source {
            // Locked: two processes that save a secret at once both keep it.
            VarSource::Local { key } => crate::fsio::with_dir_lock(&self.meta(), || {
                let mut t = self.load_local().map_err(StoreError::Invalid)?;
                let section = t
                    .entry(env.to_string())
                    .or_insert_with(|| toml::Value::Table(toml::Table::new()));
                let toml::Value::Table(s) = section else {
                    return Err(StoreError::Invalid(format!(
                        "secrets.toml: [{env}] is not a table"
                    )));
                };
                s.insert(
                    key.clone().unwrap_or_else(|| var.name.clone()),
                    toml::Value::String(value.into()),
                );
                write_private(&self.secrets_path(), secrets_to_toml(&t).as_bytes())
            }),
            VarSource::Keychain { key } => self
                .keychain
                .set(&self.keychain_account(env, &var.name, key), value)
                .map_err(StoreError::Secret),
            VarSource::File { path } => {
                let p = expand(&self.root, path);
                write_private(&p, value.as_bytes())
            }
            VarSource::Plain(_) => Err(StoreError::Invalid(
                "plain variables are stored in the environment file".into(),
            )),
            VarSource::Dotenv { .. } => Err(StoreError::Invalid(
                "dotenv variables are read from the .env file; edit that file".into(),
            )),
        }
    }
}

fn secrets_to_toml(t: &toml::Table) -> String {
    let mut w = Writer::new();
    w.raw_line("# Local secret values. Keep this file out of version control.");
    for (section, vals) in t {
        if let toml::Value::Table(vals) = vals {
            w.table(&crate::tomlfmt::bare_or_quoted_key(section));
            for (k, v) in vals {
                match v.as_str() {
                    Some(s) => w.str(k, s),
                    // Values this program does not write survive a save.
                    None => w.raw_line(&format!("{} = {v}", crate::tomlfmt::bare_or_quoted_key(k))),
                }
            }
        } else {
            let key = crate::tomlfmt::bare_or_quoted_key(section);
            match vals.as_str() {
                Some(s) => w.raw_line(&format!("{key} = {}", quote_basic(s))),
                None => w.raw_line(&format!("{key} = {vals}")),
            }
        }
    }
    w.finish()
}

fn strip_one_newline(mut s: String) -> String {
    if s.ends_with('\n') {
        s.pop();
        if s.ends_with('\r') {
            s.pop();
        }
    }
    s
}

/// Expands `~/` and anchors relative paths at the collection root.
pub fn expand(root: &Path, p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/")
        && let Some(h) = std::env::var_os("HOME")
    {
        return PathBuf::from(h).join(rest);
    }
    let pb = PathBuf::from(p);
    if pb.is_absolute() { pb } else { root.join(pb) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secrets_toml_error_does_not_quote_the_secret() {
        let d = tempfile::tempdir().unwrap();
        let envs = Envs::new(d.path(), "c", Arc::new(MemoryKeychain::default()));
        let p = envs.secrets_path();
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "[local]\ntoken = \"s3cr3t\"\ntoken = \"s3cr3t\"\n").unwrap();
        let e = envs.load_local().unwrap_err();
        assert!(!e.contains("s3cr3t"), "{e}");
        assert!(e.contains("line 3"), "{e}");
    }

    fn envs() -> (tempfile::TempDir, Envs) {
        let d = tempfile::tempdir().unwrap();
        let e = Envs::new(d.path(), "acme", Arc::new(MemoryKeychain::default()));
        (d, e)
    }

    #[test]
    fn parse_and_write() {
        let text = "[variables]\nbase = \"http://x\"\nport = 8080\non = true\nf = 1.5\ntok = { source = \"local\" }\nk = { source = \"keychain\", key = \"acme/k\" }\nc = { source = \"file\", path = \"~/c.txt\" }\n\"odd key\" = \"v\"\n";
        let env = parse_env("dev", text).unwrap();
        assert_eq!(env.vars.len(), 8);
        assert_eq!(
            env.get("port").map(|v| &v.source),
            Some(&VarSource::Plain("8080".into()))
        );
        assert_eq!(env.vars[0].name, "base");
        let out = env_to_toml(&env);
        let back = parse_env("dev", &out).unwrap();
        assert_eq!(
            back,
            EnvFile {
                name: "dev".into(),
                dotenv: None,
                vars: env.vars.clone(),
            }
        );
        assert_eq!(env_to_toml(&back), out);
        assert!(parse_env("x", "nope = 1").is_err());
        assert!(parse_env("x", "variables = 1").is_err());
        assert!(parse_env("x", "[variables]\na = { source = \"vault\" }").is_err());
        assert!(parse_env("x", "[variables]\na = { source = \"file\" }").is_err());
        assert!(parse_env("x", "[variables]\na = { source = \"local\", extra = 1 }").is_err());
        assert!(parse_env("x", "[variables]\na = [1]").is_err());
        assert!(parse_env("x", "").unwrap().vars.is_empty());
        assert!(VarSource::Keychain { key: None }.is_secret());
        assert_eq!(VarSource::Plain("".into()).label(), "plain");
    }

    #[test]
    fn all_four_sources() {
        let (d, e) = envs();
        fs::write(d.path().join("token.txt"), "file-secret\r\n").unwrap();
        let mut env = EnvFile::new("dev");
        env.set(EnvVar {
            name: "plain".into(),
            source: VarSource::Plain("p".into()),
        });
        env.set(EnvVar {
            name: "loc".into(),
            source: VarSource::Local { key: None },
        });
        env.set(EnvVar {
            name: "kc".into(),
            source: VarSource::Keychain { key: None },
        });
        env.set(EnvVar {
            name: "f".into(),
            source: VarSource::File {
                path: "token.txt".into(),
            },
        });
        env.set(EnvVar {
            name: "missing".into(),
            source: VarSource::Keychain {
                key: Some("zz".into()),
            },
        });
        e.save(&env).unwrap();
        e.save(&env).unwrap();
        assert_eq!(e.list(), vec!["dev".to_string()]);
        assert_eq!(e.is_set(&env, env.get("loc").unwrap()), Some(false));
        e.store_secret("dev", env.get("loc").unwrap(), "local-secret")
            .unwrap();
        assert_eq!(e.is_set(&env, env.get("loc").unwrap()), Some(true));
        assert_eq!(
            e.is_set(&EnvFile::new("prod"), env.get("loc").unwrap()),
            Some(false)
        );
        assert_eq!(e.is_set(&env, env.get("f").unwrap()), Some(true));
        assert_eq!(e.is_set(&env, env.get("kc").unwrap()), None);
        assert_eq!(e.is_set(&env, env.get("plain").unwrap()), None);
        let gone = EnvVar {
            name: "g".into(),
            source: VarSource::File {
                path: "nope.txt".into(),
            },
        };
        assert_eq!(e.is_set(&env, &gone), Some(false));
        e.store_secret("dev", env.get("kc").unwrap(), "kc-secret")
            .unwrap();
        assert!(
            e.store_secret("dev", env.get("plain").unwrap(), "x")
                .is_err()
        );
        let loaded = e.load("dev").unwrap();
        assert_eq!(loaded, env);
        let r = e.resolve(&loaded);
        assert_eq!(r.vars["plain"], VarValue::plain("p"));
        assert_eq!(r.vars["loc"], VarValue::secret("local-secret"));
        assert_eq!(r.vars["kc"], VarValue::secret("kc-secret"));
        assert_eq!(r.vars["f"], VarValue::secret("file-secret"));
        assert_eq!(r.errors.len(), 1);
        assert_eq!(r.errors[0].0, "missing");
        let secrets = fs::read_to_string(e.secrets_path()).unwrap();
        assert!(secrets.contains("[dev]\nloc = \"local-secret\""));
        assert_eq!(e.keychain_account("dev", "kc", &None), "acme/dev/kc");
        e.store_secret("dev", env.get("f").unwrap(), "new").unwrap();
        assert_eq!(e.resolve(&loaded).vars["f"], VarValue::secret("new"));
    }

    #[test]
    fn dotenv_parse_and_write() {
        let text = "dotenv = \".env.dev\"\n\n[variables]\ntok = { source = \"dotenv\", key = \"API_TOKEN\" }\nuser = { source = \"dotenv\" }\n";
        let env = parse_env("dev", text).unwrap();
        assert_eq!(env.dotenv.as_deref(), Some(".env.dev"));
        assert_eq!(
            env.get("tok").map(|v| &v.source),
            Some(&VarSource::Dotenv {
                key: Some("API_TOKEN".into())
            })
        );
        assert_eq!(
            env.get("user").map(|v| &v.source),
            Some(&VarSource::Dotenv { key: None })
        );
        assert_eq!(env_to_toml(&env), text);
        assert_eq!(parse_env("dev", &env_to_toml(&env)).unwrap(), env);
        assert!(VarSource::Dotenv { key: None }.is_secret());
        assert_eq!(VarSource::Dotenv { key: None }.label(), "dotenv");
        assert!(parse_env("x", "dotenv = 1").is_err());
        assert!(
            parse_env(
                "x",
                "[variables]\na = { source = \"dotenv\", path = \"p\" }"
            )
            .is_ok()
        );
        assert!(parse_env("x", "other = \"x\"").is_err());
    }

    #[test]
    fn dotenv_values_resolve_as_secrets() {
        let (d, e) = envs();
        fs::write(
            d.path().join(".env"),
            "# comment\nexport API_TOKEN=abc123\nuser=\"ann b\"\n",
        )
        .unwrap();
        let mut env = EnvFile::new("dev");
        env.set(EnvVar {
            name: "tok".into(),
            source: VarSource::Dotenv {
                key: Some("API_TOKEN".into()),
            },
        });
        env.set(EnvVar {
            name: "user".into(),
            source: VarSource::Dotenv { key: None },
        });
        env.set(EnvVar {
            name: "gone".into(),
            source: VarSource::Dotenv { key: None },
        });
        let r = e.resolve(&env);
        assert_eq!(r.vars["tok"], VarValue::secret("abc123"));
        assert_eq!(r.vars["user"], VarValue::secret("ann b"));
        assert_eq!(r.errors.len(), 1);
        assert_eq!(r.errors[0].0, "gone");
        assert!(
            r.errors[0].1.contains("gone not set in"),
            "{}",
            r.errors[0].1
        );
        assert_eq!(e.is_set(&env, env.get("tok").unwrap()), Some(true));
        assert_eq!(e.is_set(&env, env.get("gone").unwrap()), Some(false));
        assert!(e.store_secret("dev", env.get("tok").unwrap(), "x").is_err());
        assert!(
            fs::read_to_string(d.path().join(".env"))
                .unwrap()
                .contains("abc123")
        );

        // A custom path, relative to the collection root.
        fs::create_dir_all(d.path().join("conf")).unwrap();
        fs::write(d.path().join("conf/.env.dev"), "API_TOKEN=other\n").unwrap();
        env.dotenv = Some("conf/.env.dev".into());
        assert_eq!(e.resolve(&env).vars["tok"], VarValue::secret("other"));

        // A missing file fails every dotenv variable.
        env.dotenv = Some("nope.env".into());
        let r = e.resolve(&env);
        assert!(r.vars.is_empty());
        assert_eq!(r.errors.len(), 3);
        assert!(r.errors[0].1.contains("nope.env"), "{}", r.errors[0].1);
        assert_eq!(e.is_set(&env, env.get("tok").unwrap()), Some(false));
    }

    #[test]
    fn a_dotenv_error_does_not_quote_the_secret() {
        let (d, e) = envs();
        fs::write(d.path().join(".env"), "A=1\nB=2\nTOKEN=\"s3cr3t\n").unwrap();
        let mut env = EnvFile::new("dev");
        env.set(EnvVar {
            name: "A".into(),
            source: VarSource::Dotenv { key: None },
        });
        let r = e.resolve(&env);
        assert_eq!(r.errors.len(), 1);
        let msg = &r.errors[0].1;
        assert!(!msg.contains("s3cr3t"), "{msg}");
        assert!(msg.contains("line 3"), "{msg}");
    }

    #[test]
    fn secrets_toml_keeps_values_it_does_not_write() {
        let t: toml::Table = "[dev]\ntoken = \"a\"\nport = 8080\n".parse().unwrap();
        let back: toml::Table = secrets_to_toml(&t).parse().unwrap();
        assert_eq!(back, t);
    }

    #[cfg(unix)]
    #[test]
    fn secret_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let (d, e) = envs();
        let mut env = EnvFile::new("dev");
        env.set(EnvVar {
            name: "loc".into(),
            source: VarSource::Local { key: None },
        });
        env.set(EnvVar {
            name: "f".into(),
            source: VarSource::File {
                path: "token.txt".into(),
            },
        });
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let (secrets, file) = (e.secrets_path(), d.path().join("token.txt"));
        e.store_secret("dev", env.get("loc").unwrap(), "local-secret")
            .unwrap();
        e.store_secret("dev", env.get("f").unwrap(), "file-secret")
            .unwrap();
        assert_eq!(mode(&secrets), 0o600);
        assert_eq!(mode(&file), 0o600);
        for p in [&secrets, &file] {
            fs::set_permissions(p, fs::Permissions::from_mode(0o644)).unwrap();
        }
        e.store_secret("dev", env.get("loc").unwrap(), "rotated")
            .unwrap();
        e.store_secret("dev", env.get("f").unwrap(), "rotated")
            .unwrap();
        assert_eq!(mode(&secrets), 0o600);
        assert_eq!(mode(&file), 0o600);
        fs::set_permissions(&secrets, fs::Permissions::from_mode(0o664)).unwrap();
        let r = e.resolve(&env);
        assert_eq!(r.vars["loc"], VarValue::secret("rotated"));
        assert_eq!(mode(&secrets), 0o600);
        assert_eq!(r.vars["f"], VarValue::secret("rotated"));
        let leftovers: Vec<_> = fs::read_dir(e.meta())
            .unwrap()
            .flatten()
            .map(|x| x.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn collection_env_and_management() {
        let (_d, e) = envs();
        assert!(e.load(COLLECTION_ENV).unwrap().vars.is_empty());
        let mut c = EnvFile::new(COLLECTION_ENV);
        c.set(EnvVar {
            name: "a".into(),
            source: VarSource::Plain("1".into()),
        });
        e.save(&c).unwrap();
        assert_eq!(e.load(COLLECTION_ENV).unwrap(), c);
        assert!(e.list().is_empty());
        assert!(e.save(&EnvFile::new("_bad")).is_err());
        assert!(e.save(&EnvFile::new("a/b")).is_err());
        e.save(&EnvFile::new("staging")).unwrap();
        e.save(&EnvFile::new("prod")).unwrap();
        assert_eq!(e.list(), vec!["prod".to_string(), "staging".to_string()]);
        e.rename("staging", "stage").unwrap();
        assert!(e.rename("stage", "prod").is_err());
        e.delete("stage").unwrap();
        assert_eq!(e.list(), vec!["prod".to_string()]);
        assert!(e.load("nope").is_err());
        let mut env = EnvFile::new("prod");
        env.set(EnvVar {
            name: "x".into(),
            source: VarSource::Plain("1".into()),
        });
        env.set(EnvVar {
            name: "x".into(),
            source: VarSource::Plain("2".into()),
        });
        assert_eq!(env.vars.len(), 1);
        env.remove("x");
        assert!(env.vars.is_empty());
        fs::write(e.secrets_path(), "not toml [").unwrap();
        let mut s = EnvFile::new("prod");
        s.set(EnvVar {
            name: "l".into(),
            source: VarSource::Local {
                key: Some("k".into()),
            },
        });
        let r = e.resolve(&s);
        assert_eq!(r.errors.len(), 1);
        assert!(e.store_secret("prod", &s.vars[0], "v").is_err());
    }

    #[test]
    fn memory_keychain() {
        let k = MemoryKeychain::default();
        k.set("a", "1").unwrap();
        assert_eq!(k.get("a").unwrap(), Some("1".into()));
        k.delete("a").unwrap();
        assert_eq!(k.get("a").unwrap(), None);
        assert_eq!(strip_one_newline("x\n\n".into()), "x\n");
        assert_eq!(expand(Path::new("/r"), "/abs"), PathBuf::from("/abs"));
        assert_eq!(expand(Path::new("/r"), "rel"), PathBuf::from("/r/rel"));
    }
}

/// Real OS keychain round trip; runs only with `SENDITO_TEST_OS_KEYCHAIN=1`
/// (CI sets up GNOME Keyring on Linux; macOS uses the login keychain).
#[cfg(test)]
mod os_keychain {
    use super::*;

    #[test]
    fn os_keychain_roundtrip() {
        if std::env::var("SENDITO_TEST_OS_KEYCHAIN").is_err() {
            return;
        }
        let k = OsKeychain;
        let account = format!("sendito-test/{}", std::process::id());
        k.set(&account, "value-1").unwrap();
        assert_eq!(k.get(&account).unwrap().as_deref(), Some("value-1"));
        k.delete(&account).unwrap();
        assert_eq!(k.get(&account).unwrap(), None);
    }
}
