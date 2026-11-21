//! Request history: `.sendito/history.jsonl`, one JSON object per line.
//!
//! Entries store the *masked* request (secret variables left as placeholders),
//! never response bodies.

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::collection::{META_DIR, write_atomic};
use crate::{Result, StoreError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// RFC 3339 UTC timestamp.
    pub ts: String,
    /// Request name (file stem) or "untitled".
    pub name: String,
    /// Relative path of the saved request, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    /// `GET`, `POST`, ..., `GRPC`.
    pub badge: String,
    /// URL (gRPC: target/service/method) as sent, with secrets masked.
    pub target: String,
    /// URL (gRPC: target/service/method) as written, with `{{variables}}`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub template: String,
    /// The request as written (variables unsubstituted), in canonical TOML
    /// form; resending resolves it again with the active environment.
    pub request: String,
    /// HTTP status or gRPC status code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<i64>,
    pub duration_ms: u64,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn history_path(root: &Path) -> PathBuf {
    root.join(META_DIR).join("history.jsonl")
}

/// Appends an entry. When the file exceeds `max` entries by 10% it is trimmed
/// to the newest `max`.
pub fn append(path: &Path, entry: &HistoryEntry, max: usize) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| StoreError::io(dir, e))?;
    }
    let line = serde_json::to_string(entry).map_err(|e| StoreError::Invalid(e.to_string()))?;
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| StoreError::io(path, e))?;
    writeln!(f, "{line}").map_err(|e| StoreError::io(path, e))?;
    drop(f);
    let count = count_lines(path);
    if max > 0 && count > max + max / 10 {
        let entries = load(path, max)?;
        let mut out = String::new();
        for e in &entries {
            out.push_str(
                &serde_json::to_string(e).map_err(|e| StoreError::Invalid(e.to_string()))?,
            );
            out.push('\n');
        }
        write_atomic(path, out.as_bytes())?;
    }
    Ok(())
}

fn count_lines(path: &Path) -> usize {
    fs::File::open(path)
        .map(|f| BufReader::new(f).lines().count())
        .unwrap_or(0)
}

/// Loads up to `limit` newest entries, oldest first. Corrupt lines are skipped.
pub fn load(path: &Path, limit: usize) -> Result<Vec<HistoryEntry>> {
    let f = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(StoreError::io(path, e)),
    };
    let mut v: Vec<HistoryEntry> = BufReader::new(f)
        .lines()
        .map_while(std::result::Result::ok)
        .filter_map(|l| serde_json::from_str(&l).ok())
        .collect();
    if v.len() > limit {
        v.drain(..v.len() - limit);
    }
    Ok(v)
}

pub fn clear(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(StoreError::io(path, e)),
    }
}

pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(i: usize) -> HistoryEntry {
        HistoryEntry {
            ts: now_rfc3339(),
            name: format!("r{i}"),
            path: Some(format!("r{i}.toml")),
            env: None,
            badge: "GET".into(),
            target: "http://x".into(),
            template: "{{base}}".into(),
            request: "kind = \"http\"\n".into(),
            status: Some(200),
            duration_ms: 3,
            size: 10,
            error: None,
        }
    }

    #[test]
    fn append_load_trim() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(".sendito/history.jsonl");
        assert!(load(&p, 10).unwrap().is_empty());
        for i in 0..25 {
            append(&p, &entry(i), 10).unwrap();
        }
        let all = load(&p, 100).unwrap();
        assert!(all.len() <= 11, "{}", all.len());
        assert_eq!(all.last().map(|e| e.name.as_str()), Some("r24"));
        let few = load(&p, 3).unwrap();
        assert_eq!(few.len(), 3);
        assert_eq!(few[0].name, "r22");
        fs::OpenOptions::new()
            .append(true)
            .open(&p)
            .unwrap()
            .write_all(b"garbage\n")
            .unwrap();
        assert_eq!(load(&p, 100).unwrap().len(), all.len());
        clear(&p).unwrap();
        clear(&p).unwrap();
        assert!(load(&p, 10).unwrap().is_empty());
        assert!(history_path(d.path()).ends_with("history.jsonl"));
    }
}
