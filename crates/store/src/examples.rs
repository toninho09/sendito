//! Saved example responses of a request: `<name>.examples/<example>.toml`
//! next to `<name>.toml`. Schema in FORMAT.md.

use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

use crate::collection::{Collection, EXT, validate_name, write_atomic};
use crate::request_file::RawKv;
use crate::tomlfmt::Writer;
use crate::{Result, StoreError};

/// The suffix of the directory that holds a request's examples.
pub const DIR_SUFFIX: &str = ".examples";

/// One saved response.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Example {
    /// The file stem: `200-ok`.
    pub name: String,
    pub status: u16,
    pub reason: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Example {
    /// The name a new example gets from its status: `200-ok`, `404-not-found`.
    pub fn default_name(status: u16, reason: &str) -> String {
        let reason: String = reason
            .trim()
            .to_ascii_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let reason = reason
            .split('-')
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        if reason.is_empty() {
            status.to_string()
        } else {
            format!("{status}-{reason}")
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExample {
    status: u16,
    #[serde(default)]
    reason: String,
    body: Option<String>,
    body_file: Option<String>,
    #[serde(default)]
    headers: Vec<RawKv>,
}

/// The examples directory of the request at `rel` (`Users/login.toml` →
/// `Users/login.examples`).
pub fn dir_rel(request_rel: &str) -> String {
    let stem = request_rel
        .strip_suffix(&format!(".{EXT}"))
        .unwrap_or(request_rel);
    format!("{stem}{DIR_SUFFIX}")
}

fn dir(coll: &Collection, request_rel: &str) -> PathBuf {
    coll.abs(&dir_rel(request_rel))
}

/// The names of a request's examples, sorted; none when it has no directory.
pub fn list(coll: &Collection, request_rel: &str) -> Result<Vec<String>> {
    let d = dir(coll, request_rel);
    let entries = match fs::read_dir(&d) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(StoreError::io(&d, e)),
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter_map(|n| n.strip_suffix(&format!(".{EXT}")).map(str::to_string))
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    Ok(names)
}

pub fn load(coll: &Collection, request_rel: &str, name: &str) -> Result<Example> {
    validate_name(name)?;
    let d = dir(coll, request_rel);
    let path = d.join(format!("{name}.{EXT}"));
    let text = fs::read_to_string(&path).map_err(|e| StoreError::io(&path, e))?;
    let raw: RawExample = toml::from_str(&text).map_err(|e| StoreError::Parse {
        path: path.clone(),
        msg: e.to_string(),
    })?;
    let body = match (raw.body, raw.body_file) {
        (_, Some(file)) => {
            validate_name(&file)?;
            let p = d.join(&file);
            fs::read(&p).map_err(|e| StoreError::io(&p, e))?
        }
        (Some(b), None) => b.into_bytes(),
        (None, None) => Vec::new(),
    };
    Ok(Example {
        name: name.to_string(),
        status: raw.status,
        reason: raw.reason,
        headers: crate::request_file::kvs(raw.headers)
            .into_iter()
            .map(|h| (h.name, h.value))
            .collect(),
        body,
    })
}

/// Serializes an example: a UTF-8 body inline, another one as `body_file`.
fn to_toml(ex: &Example, body_file: Option<&str>) -> String {
    let mut w = Writer::new();
    w.int("status", u64::from(ex.status));
    if !ex.reason.is_empty() {
        w.str("reason", &ex.reason);
    }
    match (body_file, std::str::from_utf8(&ex.body)) {
        (Some(f), _) => w.str("body_file", f),
        (None, Ok(text)) if !text.is_empty() => w.str("body", text),
        _ => {}
    }
    for (n, v) in &ex.headers {
        w.array_table("headers");
        w.str("name", n);
        w.str("value", v);
    }
    w.finish()
}

/// Saves `ex` under its name, or `name (2)`, `name (3)`... when that name is
/// taken. Returns the name used.
pub fn save(coll: &Collection, request_rel: &str, ex: &Example) -> Result<String> {
    validate_name(&ex.name)?;
    let d = dir(coll, request_rel);
    fs::create_dir_all(&d).map_err(|e| StoreError::io(&d, e))?;
    let taken = list(coll, request_rel)?;
    let mut name = ex.name.clone();
    let mut n = 2;
    while taken.iter().any(|t| t.eq_ignore_ascii_case(&name)) {
        name = format!("{} ({n})", ex.name);
        n += 1;
    }
    let binary = std::str::from_utf8(&ex.body).is_err();
    let body_file = binary.then(|| format!("{name}.bin"));
    if let Some(f) = &body_file {
        write_atomic(&d.join(f), &ex.body)?;
    }
    let ex = Example {
        name: name.clone(),
        ..ex.clone()
    };
    write_atomic(
        &d.join(format!("{name}.{EXT}")),
        to_toml(&ex, body_file.as_deref()).as_bytes(),
    )?;
    Ok(name)
}

/// Deletes an example (and its body file); the directory goes when empty.
pub fn delete(coll: &Collection, request_rel: &str, name: &str) -> Result<()> {
    validate_name(name)?;
    let d = dir(coll, request_rel);
    let path = d.join(format!("{name}.{EXT}"));
    fs::remove_file(&path).map_err(|e| StoreError::io(&path, e))?;
    let _ = fs::remove_file(d.join(format!("{name}.bin")));
    if fs::read_dir(&d).is_ok_and(|mut e| e.next().is_none()) {
        let _ = fs::remove_dir(&d);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coll() -> (tempfile::TempDir, Collection) {
        let d = tempfile::tempdir().unwrap();
        let c = Collection::new(d.path(), vec![]);
        (d, c)
    }

    #[test]
    fn save_list_load_delete() {
        let (_d, c) = coll();
        let rel = "Users/login.toml";
        let ex = Example {
            name: Example::default_name(200, "OK"),
            status: 200,
            reason: "OK".into(),
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: b"{\"ok\": true}\n".to_vec(),
        };
        assert_eq!(save(&c, rel, &ex).unwrap(), "200-ok");
        assert_eq!(save(&c, rel, &ex).unwrap(), "200-ok (2)");
        assert_eq!(list(&c, rel).unwrap(), ["200-ok", "200-ok (2)"]);
        assert_eq!(load(&c, rel, "200-ok").unwrap(), ex);
        let text = fs::read_to_string(c.abs("Users/login.examples/200-ok.toml")).unwrap();
        assert!(
            text.starts_with("status = 200\nreason = \"OK\"\n"),
            "{text}"
        );
        delete(&c, rel, "200-ok").unwrap();
        delete(&c, rel, "200-ok (2)").unwrap();
        assert!(!c.abs("Users/login.examples").exists());
        assert!(list(&c, rel).unwrap().is_empty());
    }

    #[test]
    fn a_binary_body_goes_to_a_file() {
        let (_d, c) = coll();
        let ex = Example {
            name: "png".into(),
            status: 200,
            body: vec![0x89, b'P', b'N', b'G', 0xff],
            ..Default::default()
        };
        save(&c, "img.toml", &ex).unwrap();
        assert!(c.abs("img.examples/png.bin").is_file());
        assert_eq!(load(&c, "img.toml", "png").unwrap().body, ex.body);
    }

    #[test]
    fn default_names() {
        assert_eq!(Example::default_name(404, "Not Found"), "404-not-found");
        assert_eq!(Example::default_name(204, ""), "204");
        assert_eq!(dir_rel("a/b.toml"), "a/b.examples");
    }
}
