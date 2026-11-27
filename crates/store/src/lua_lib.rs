//! The Lua function catalog: `*.lua` files in `<config dir>/lua/` (next to
//! the global `config.toml`) and `<collection>/.sendito/lua/`. This module
//! only reads the files; `sendito_core::lua_script::LuaCatalog` runs them.

use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use sendito_core::model::LuaScript;

use crate::collection::META_DIR;

/// The directories read, in load order, with the label that names their files
/// in messages: the global one, then the collection's.
pub fn dirs(global_config: Option<&Path>, root: &Path) -> Vec<(&'static str, PathBuf)> {
    let mut out = Vec::new();
    if let Some(dir) = global_config.and_then(Path::parent) {
        out.push(("global lua", dir.join("lua")));
    }
    out.push(("lua", root.join(META_DIR).join("lua")));
    out
}

/// Reads the `*.lua` files of `dirs`, each directory in name order. The origin
/// of a file is `<label>/<file name>`. A missing directory gives no files; a
/// file that cannot be read gives a message and is skipped.
pub fn load(dirs: &[(&str, PathBuf)]) -> (Vec<LuaScript>, Vec<String>) {
    let mut files = Vec::new();
    let mut errors = Vec::new();
    for (label, dir) in dirs {
        for p in crate::fsio::files_with_ext(dir, "lua") {
            let name = p.file_name().unwrap_or_default().to_string_lossy();
            match fs::read_to_string(&p) {
                Ok(code) => files.push(LuaScript {
                    origin: format!("{label}/{name}"),
                    code,
                }),
                Err(e) => errors.push(format!("{label}/{name}: {e}")),
            }
        }
    }
    (files, errors)
}

/// A number that changes when a `*.lua` file of `dirs` is added, removed or
/// written (its path, size and modification time).
pub fn stamp(dirs: &[(&str, PathBuf)]) -> u64 {
    let mut h = DefaultHasher::new();
    for (_, dir) in dirs {
        for p in crate::fsio::files_with_ext(dir, "lua") {
            let meta = fs::metadata(&p).ok();
            (&p, meta.as_ref().map(fs::Metadata::len)).hash(&mut h);
            meta.and_then(|m| m.modified().ok()).hash(&mut h);
        }
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_files_come_first_in_name_order() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("cfg/config.toml");
        let root = tmp.path().join("coll");
        let d = dirs(Some(&global), &root);
        assert_eq!(d.len(), 2);
        // Missing directories are not an error.
        assert_eq!(load(&d), (vec![], vec![]));
        for (dir, name) in [(&d[1].1, "b.lua"), (&d[1].1, "a.lua"), (&d[0].1, "z.lua")] {
            fs::create_dir_all(dir).unwrap();
            fs::write(dir.join(name), format!("-- {name}")).unwrap();
        }
        fs::write(d[1].1.join("notes.txt"), "").unwrap();
        let (files, errors) = load(&d);
        assert!(errors.is_empty());
        let origins: Vec<&str> = files.iter().map(|f| f.origin.as_str()).collect();
        assert_eq!(origins, ["global lua/z.lua", "lua/a.lua", "lua/b.lua"]);
        assert_eq!(files[1].code, "-- a.lua");
        // No global config: only the collection directory.
        assert_eq!(dirs(None, &root).len(), 1);
    }

    #[test]
    fn stamp_changes_with_the_files() {
        let tmp = tempfile::tempdir().unwrap();
        let d = dirs(None, tmp.path());
        let empty = stamp(&d);
        fs::create_dir_all(&d[0].1).unwrap();
        assert_eq!(stamp(&d), empty);
        fs::write(d[0].1.join("a.lua"), "x").unwrap();
        let one = stamp(&d);
        assert_ne!(one, empty);
        assert_eq!(stamp(&d), one);
        fs::write(d[0].1.join("a.lua"), "xy").unwrap();
        assert_ne!(stamp(&d), one);
        fs::remove_file(d[0].1.join("a.lua")).unwrap();
        assert_eq!(stamp(&d), empty);
    }
}
