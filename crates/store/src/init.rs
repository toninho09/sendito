//! `sendito init` and collection discovery.

use std::fs;
use std::path::{Path, PathBuf};

use crate::collection::{META_DIR, write_atomic};
use crate::env::{EnvFile, EnvVar, VarSource};
use crate::tomlfmt::Writer;
use crate::{Result, StoreError};

/// Entries `sendito init` adds to the collection's `.gitignore`.
pub const GITIGNORE_ENTRIES: &[&str] = &[
    ".sendito/secrets.toml",
    ".sendito/session.toml",
    ".sendito/history.jsonl",
];

/// Finds the nearest directory at or above `start` that contains `.sendito/`.
pub fn find_root(start: &Path) -> Option<PathBuf> {
    let mut cur = Some(start);
    while let Some(d) = cur {
        if d.join(META_DIR).is_dir() {
            return Some(d.to_path_buf());
        }
        cur = d.parent();
    }
    None
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct InitReport {
    pub created: Vec<PathBuf>,
    pub gitignore_added: Vec<String>,
}

/// Creates a collection in `dir`. Existing files are left as they are.
pub fn init(dir: &Path) -> Result<InitReport> {
    let mut rep = InitReport::default();
    let meta = dir.join(META_DIR);
    fs::create_dir_all(meta.join("environments")).map_err(|e| StoreError::io(&meta, e))?;
    let name = dir
        .canonicalize()
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "collection".into());

    let cfg = meta.join("config.toml");
    if !cfg.exists() {
        let mut w = Writer::new();
        w.raw_line("# Project settings. They override ~/.config/sendito/config.toml.");
        w.table("collection");
        w.str("name", &name);
        write_atomic(&cfg, w.finish().as_bytes())?;
        rep.created.push(cfg);
    }
    let coll = meta.join("collection.toml");
    if !coll.exists() {
        let env = EnvFile::new(crate::env::COLLECTION_ENV);
        write_atomic(&coll, crate::env::env_to_toml(&env).as_bytes())?;
        rep.created.push(coll);
    }
    let dev = meta.join("environments").join("local.toml");
    if !dev.exists()
        && fs::read_dir(meta.join("environments"))
            .map(|mut d| d.next().is_none())
            .unwrap_or(true)
    {
        let mut env = EnvFile::new("local");
        env.set(EnvVar {
            name: "base_url".into(),
            source: VarSource::Plain("http://localhost:8080".into()),
        });
        write_atomic(&dev, crate::env::env_to_toml(&env).as_bytes())?;
        rep.created.push(dev);
    }

    let gi = dir.join(".gitignore");
    let existing = fs::read_to_string(&gi).unwrap_or_default();
    let have: Vec<&str> = existing.lines().map(str::trim).collect();
    let mut add = String::new();
    for e in GITIGNORE_ENTRIES {
        if !have.contains(e) && !have.contains(&format!("/{e}").as_str()) {
            add.push_str(e);
            add.push('\n');
            rep.gitignore_added.push(e.to_string());
        }
    }
    if !add.is_empty() {
        let mut text = existing.clone();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        if existing.is_empty() {
            text.push_str("# Sendito local files\n");
        }
        text.push_str(&add);
        write_atomic(&gi, text.as_bytes())?;
    }
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_is_idempotent() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join(".gitignore"), "target").unwrap();
        let r = init(d.path()).unwrap();
        assert_eq!(r.created.len(), 3);
        assert_eq!(r.gitignore_added.len(), 3);
        let gi = fs::read_to_string(d.path().join(".gitignore")).unwrap();
        assert_eq!(
            gi,
            "target\n.sendito/secrets.toml\n.sendito/session.toml\n.sendito/history.jsonl\n"
        );
        let r2 = init(d.path()).unwrap();
        assert_eq!(r2, InitReport::default());
        let sub = d.path().join("a/b");
        fs::create_dir_all(&sub).unwrap();
        assert_eq!(find_root(&sub).as_deref(), Some(d.path()));
        let other = tempfile::tempdir().unwrap();
        assert!(
            find_root(other.path()).is_none()
                || find_root(other.path()) != Some(other.path().to_path_buf())
        );
    }
}
