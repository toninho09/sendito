//! User color themes: `*.toml` files in `<config dir>/themes/` (next to the
//! global `config.toml`) and `<collection>/.sendito/themes/`.
//!
//! ```toml
//! name = "harbor"     # optional; defaults to the file name
//! extends = "dusk"    # optional; a built-in or another user theme
//! [colors]
//! bg = "#0f1720"
//! accent = "#5ccfe6"
//! get = "green"
//! border = 240
//! ```
//!
//! This module only finds and parses the files. Turning color values into a
//! theme (and checking token names) is the UI's job, see `docs/THEMES.md`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::collection::META_DIR;

/// A parsed theme file.
#[derive(Debug, Clone, PartialEq)]
pub struct ThemeFile {
    pub name: String,
    pub extends: Option<String>,
    /// Token name → color value as written (a string or an xterm-256 index).
    pub colors: BTreeMap<String, ColorValue>,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColorValue {
    Text(String),
    Index(i64),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    name: Option<String>,
    extends: Option<String>,
    #[serde(default)]
    colors: BTreeMap<String, toml::Value>,
}

/// The directories searched, lowest precedence first: next to the global
/// config file, then the collection's `.sendito/themes`.
pub fn dirs(global_config: Option<&Path>, root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(dir) = global_config.and_then(Path::parent) {
        out.push(dir.join("themes"));
    }
    out.push(root.join(META_DIR).join("themes"));
    out
}

/// Parses one theme file.
pub fn parse(text: &str, path: &Path) -> Result<ThemeFile, String> {
    let raw: Raw = toml::from_str(text).map_err(|e| format!("{}: {e}", path.display()))?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = raw.name.unwrap_or(stem).trim().to_string();
    if name.is_empty() {
        return Err(format!("{}: the theme has no name", path.display()));
    }
    let mut colors = BTreeMap::new();
    let mut bad = Vec::new();
    for (k, v) in raw.colors {
        match v {
            toml::Value::String(s) => {
                colors.insert(k, ColorValue::Text(s));
            }
            toml::Value::Integer(i) => {
                colors.insert(k, ColorValue::Index(i));
            }
            _ => bad.push(k),
        }
    }
    if !bad.is_empty() {
        return Err(format!(
            "{}: colors must be strings or numbers ({})",
            path.display(),
            bad.join(", ")
        ));
    }
    Ok(ThemeFile {
        name,
        extends: raw.extends.map(|e| e.trim().to_string()),
        colors,
        path: path.to_path_buf(),
    })
}

/// Finds every theme in `dirs`. A later directory wins over an earlier one
/// for the same name. Problems are returned as warnings; the file is skipped.
pub fn discover(dirs: &[PathBuf]) -> (Vec<ThemeFile>, Vec<String>) {
    let mut by_name: BTreeMap<String, ThemeFile> = BTreeMap::new();
    let mut warnings = Vec::new();
    for dir in dirs {
        for p in crate::fsio::files_with_ext(dir, "toml") {
            match fs::read_to_string(&p) {
                Ok(text) => match parse(&text, &p) {
                    Ok(t) => {
                        by_name.insert(t.name.clone(), t);
                    }
                    Err(e) => warnings.push(e),
                },
                Err(e) => warnings.push(format!("{}: {e}", p.display())),
            }
        }
    }
    (by_name.into_values().collect(), warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) {
        fs::create_dir_all(dir).ok();
        fs::write(dir.join(name), text).ok();
    }

    #[test]
    fn parses_names_values_and_extends() {
        let t = parse(
            "extends = \"dusk\"\n[colors]\nbg = \"#101010\"\nborder = 240\n",
            Path::new("/x/harbor.toml"),
        )
        .unwrap();
        assert_eq!(t.name, "harbor");
        assert_eq!(t.extends.as_deref(), Some("dusk"));
        assert_eq!(t.colors["bg"], ColorValue::Text("#101010".into()));
        assert_eq!(t.colors["border"], ColorValue::Index(240));
        let named = parse("name = \"Sea\"\n", Path::new("/x/a.toml")).unwrap();
        assert_eq!(named.name, "Sea");
        assert!(named.colors.is_empty());
    }

    #[test]
    fn rejects_bad_files() {
        assert!(parse("[colors]\nbg = true\n", Path::new("/x/a.toml")).is_err());
        assert!(parse("unknown = 1\n", Path::new("/x/a.toml")).is_err());
        assert!(parse("name = \"  \"\n", Path::new("/x/a.toml")).is_err());
        assert!(parse("[colors\n", Path::new("/x/a.toml")).is_err());
    }

    #[test]
    fn discovery_order_and_warnings() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("cfg/config.toml");
        let root = tmp.path().join("coll");
        let d = dirs(Some(&global), &root);
        assert_eq!(d.len(), 2);
        write(&d[0], "harbor.toml", "[colors]\nbg = \"#000000\"\n");
        write(&d[0], "only-global.toml", "");
        write(&d[0], "notes.txt", "not a theme");
        write(&d[1], "harbor.toml", "[colors]\nbg = \"#ffffff\"\n");
        write(&d[1], "broken.toml", "[colors]\nbg = [1]\n");
        let (themes, warnings) = discover(&d);
        let names: Vec<&str> = themes.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["harbor", "only-global"]);
        // The collection's file wins.
        assert_eq!(themes[0].colors["bg"], ColorValue::Text("#ffffff".into()));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("broken.toml"));
        // No global config: only the collection directory.
        assert_eq!(dirs(None, &root).len(), 1);
        // Missing directories are fine.
        assert!(discover(&[tmp.path().join("nope")]).0.is_empty());
    }
}
