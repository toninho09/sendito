//! Import from Postman Collection v2.1 and curl; export to curl.

pub mod curl;
pub mod postman;

use sendito_core::Request;
use sendito_store::collection::{Collection, join_rel, validate_name};
use sendito_store::env::{COLLECTION_ENV, EnvVar, Envs, VarSource};

/// An imported tree, not yet written to disk.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImportFolder {
    pub name: String,
    pub folders: Vec<ImportFolder>,
    pub requests: Vec<(String, Request)>,
}

impl ImportFolder {
    pub fn count_requests(&self) -> usize {
        self.requests.len()
            + self
                .folders
                .iter()
                .map(ImportFolder::count_requests)
                .sum::<usize>()
    }
}

/// Something that was not imported, or imported differently, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// Where it was found, e.g. `Users / Create user`.
    pub item: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImportResult {
    pub root: ImportFolder,
    /// Collection-level variables (written to the collection environment).
    pub variables: Vec<(String, String)>,
    pub skipped: Vec<Skipped>,
}

/// Makes a file-system-safe, unique name.
pub fn safe_name(name: &str, taken: &mut Vec<String>) -> String {
    let mut s: String = name
        .trim()
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | '\0') || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    while s.starts_with('.') {
        s.remove(0);
    }
    if s.is_empty() {
        s = "untitled".into();
    }
    if s.len() > 120 {
        let mut cut = 120;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    let base = s.trim().to_string();
    let mut candidate = base.clone();
    let mut i = 2;
    while taken.iter().any(|t| t.eq_ignore_ascii_case(&candidate))
        || validate_name(&candidate).is_err()
    {
        candidate = format!("{base} ({i})");
        i += 1;
        if i > 10_000 {
            candidate = format!("untitled {i}");
            break;
        }
    }
    taken.push(candidate.clone());
    candidate
}

/// What [`write`] created.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WriteReport {
    pub folder: String,
    pub requests: usize,
    pub folders: usize,
    pub variables: usize,
}

/// Writes an import under `parent` in a new folder named after the root.
pub fn write(
    result: &ImportResult,
    coll: &Collection,
    envs: Option<&Envs>,
    parent: &str,
) -> Result<WriteReport, sendito_store::StoreError> {
    let mut rep = WriteReport::default();
    let mut taken: Vec<String> = std::fs::read_dir(coll.abs(parent))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    let name = safe_name(&result.root.name, &mut taken);
    let rel = coll.create_folder(parent, &name)?;
    rep.folder = rel.clone();
    write_folder(&result.root, coll, &rel, &mut rep)?;
    if let Some(envs) = envs
        && !result.variables.is_empty()
    {
        let mut env = envs.load(COLLECTION_ENV)?;
        for (k, v) in &result.variables {
            if env.get(k).is_none() {
                env.set(EnvVar {
                    name: k.clone(),
                    source: VarSource::Plain(v.clone()),
                });
                rep.variables += 1;
            }
        }
        envs.save(&env)?;
    }
    Ok(rep)
}

fn write_folder(
    f: &ImportFolder,
    coll: &Collection,
    rel: &str,
    rep: &mut WriteReport,
) -> Result<(), sendito_store::StoreError> {
    let mut taken = Vec::new();
    for sub in &f.folders {
        let name = safe_name(&sub.name, &mut taken);
        let r = coll.create_folder(rel, &name)?;
        rep.folders += 1;
        write_folder(sub, coll, &r, rep)?;
    }
    for (name, req) in &f.requests {
        let name = safe_name(name, &mut taken);
        coll.create_request(rel, &name, req)?;
        rep.requests += 1;
    }
    let _ = join_rel;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_tree_and_variables() {
        let d = tempfile::tempdir().unwrap();
        sendito_store::init::init(d.path()).unwrap();
        let coll = Collection::new(d.path(), vec![]);
        let envs = Envs::new(
            d.path(),
            "t",
            std::sync::Arc::new(sendito_store::env::MemoryKeychain::default()),
        );
        let result = ImportResult {
            root: ImportFolder {
                name: "API/v1".into(),
                folders: vec![ImportFolder {
                    name: "Users".into(),
                    folders: vec![],
                    requests: vec![
                        ("list".into(), Request::new_http()),
                        ("list".into(), Request::new_grpc()),
                    ],
                }],
                requests: vec![("ping".into(), Request::new_http())],
            },
            variables: vec![("base".into(), "http://x".into())],
            skipped: vec![],
        };
        let rep = write(&result, &coll, Some(&envs), "").unwrap();
        assert_eq!(rep.folder, "API-v1");
        assert_eq!((rep.requests, rep.folders, rep.variables), (3, 1, 1));
        assert!(d.path().join("API-v1/Users/list (2).toml").is_file());
        // A second import gets a fresh folder and does not duplicate variables.
        let rep2 = write(&result, &coll, Some(&envs), "").unwrap();
        assert_eq!(rep2.folder, "API-v1 (2)");
        assert_eq!(rep2.variables, 0);
        assert_eq!(result.root.count_requests(), 3);
    }

    #[test]
    fn names() {
        let mut t = vec![];
        assert_eq!(safe_name("a/b", &mut t), "a-b");
        assert_eq!(safe_name("a/b", &mut t), "a-b (2)");
        assert_eq!(safe_name("..hidden", &mut t), "hidden");
        assert_eq!(safe_name("", &mut t), "untitled");
        assert_eq!(safe_name(&"x".repeat(300), &mut t).len(), 120);
    }
}
