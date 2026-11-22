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
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 30_000,
            max_body_mb: 100,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct GrpcConfig {
    /// Default deadline when a request sets none (0 = none).
    pub deadline_ms: u64,
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
