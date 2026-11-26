//! Request history: `.sendito/history.jsonl`, one JSON object per line.
//!
//! Entries store the *masked* request (secret variables left as placeholders).
//! An HTTP entry can name a response file in `.sendito/history/`, in the
//! format of a saved example, with its secrets masked.

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::collection::META_DIR;
use crate::examples::Example;
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
    /// The response file in [`responses_dir`] (its stem), when one was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<String>,
}

pub fn history_path(root: &Path) -> PathBuf {
    root.join(META_DIR).join("history.jsonl")
}

/// The directory of the response files of the history file at `path`.
pub fn responses_dir(path: &Path) -> PathBuf {
    path.with_file_name("history")
}

/// A response file name no other append uses: the time, the process and a
/// counter.
fn new_response_id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{ms}-{}-{n}", std::process::id())
}

/// The first `max` bytes of `body`; a UTF-8 body is cut before a character
/// that does not fit, so it stays text.
pub fn cut_body(body: &[u8], max: usize) -> Vec<u8> {
    if body.len() <= max {
        return body.to_vec();
    }
    let head = &body[..max];
    match std::str::from_utf8(head) {
        Err(e) if e.error_len().is_none() => head[..e.valid_up_to()].to_vec(),
        _ => head.to_vec(),
    }
}

/// Reads the response file `id` of the history file at `path`.
pub fn load_response(path: &Path, id: &str) -> Result<Example> {
    crate::examples::read_in(&responses_dir(path), id)
}

/// Deletes the response files that no entry in `kept` names.
fn prune_responses(path: &Path, kept: &[HistoryEntry]) {
    let keep: std::collections::HashSet<&str> =
        kept.iter().filter_map(|e| e.response.as_deref()).collect();
    let Ok(files) = fs::read_dir(responses_dir(path)) else {
        return;
    };
    for f in files.flatten() {
        let name = f.file_name().to_string_lossy().into_owned();
        let stem = name
            .strip_suffix(".toml")
            .or_else(|| name.strip_suffix(".bin"))
            .unwrap_or(&name);
        if !keep.contains(stem) {
            let _ = fs::remove_file(f.path());
        }
    }
}

/// Appends an entry. When the file exceeds `max` entries by 10% it is trimmed
/// to the newest `max`. Appends and trims from other Sendito processes wait
/// for each other, so a trim does not drop a line another process just wrote.
pub fn append(path: &Path, entry: &HistoryEntry, max: usize) -> Result<()> {
    append_with_response(path, entry, max, None)
}

/// Appends an entry and, when `response` is given, writes it as the entry's
/// response file first (inside the same lock, so a trim of another process
/// cannot remove it before the entry names it).
pub fn append_with_response(
    path: &Path,
    entry: &HistoryEntry,
    max: usize,
    response: Option<&Example>,
) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    crate::fsio::with_dir_lock(dir, || {
        let mut entry = entry.clone();
        if let Some(ex) = response {
            let rdir = responses_dir(path);
            fs::create_dir_all(&rdir).map_err(|e| StoreError::io(&rdir, e))?;
            let id = new_response_id();
            crate::examples::write_in(&rdir, &id, ex, true)?;
            entry.response = Some(id);
        }
        let line = serde_json::to_string(&entry).map_err(|e| StoreError::Invalid(e.to_string()))?;
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(path)
            .map_err(|e| StoreError::io(path, e))?;
        // Requests are stored as written, with any literal token: owner-only.
        crate::collection::make_private(path);
        // A crash can leave a last line with no newline: end it first, so this
        // entry does not join it and become unreadable. One write keeps the
        // line whole.
        let mut out = String::with_capacity(line.len() + 2);
        if ends_without_newline(&mut f) {
            out.push('\n');
        }
        out.push_str(&line);
        out.push('\n');
        f.write_all(out.as_bytes())
            .map_err(|e| StoreError::io(path, e))?;
        drop(f);
        let count = bump_count(path);
        if max > 0 && count > max + max / 10 {
            let entries = load(path, max)?;
            let mut out = String::new();
            for e in &entries {
                out.push_str(
                    &serde_json::to_string(e).map_err(|e| StoreError::Invalid(e.to_string()))?,
                );
                out.push('\n');
            }
            crate::collection::write_private(path, out.as_bytes())?;
            set_count(path, entries.len());
            prune_responses(path, &entries);
        }
        Ok(())
    })
}

/// Entries per history file, counted once per process and then kept up to
/// date, so an append does not read the whole file. Appends from other
/// processes are not seen; that only delays a trim.
fn counts() -> &'static std::sync::Mutex<std::collections::HashMap<PathBuf, usize>> {
    static COUNTS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PathBuf, usize>>,
    > = std::sync::OnceLock::new();
    COUNTS.get_or_init(Default::default)
}

/// The entry count after one more append.
fn bump_count(path: &Path) -> usize {
    let Ok(mut map) = counts().lock() else {
        return count_lines(path);
    };
    let n = map
        .entry(path.to_path_buf())
        .and_modify(|n| *n += 1)
        .or_insert_with(|| count_lines(path));
    *n
}

fn set_count(path: &Path, n: usize) {
    if let Ok(mut map) = counts().lock() {
        map.insert(path.to_path_buf(), n);
    }
}

fn ends_without_newline(f: &mut fs::File) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let mut last = [0u8; 1];
    f.seek(SeekFrom::End(-1)).is_ok() && f.read_exact(&mut last).is_ok() && last[0] != b'\n'
}

fn count_lines(path: &Path) -> usize {
    fs::File::open(path)
        .map(|f| {
            BufReader::new(f)
                .split(b'\n')
                .filter(|l| l.as_ref().is_ok_and(|l| !l.is_empty()))
                .count()
        })
        .unwrap_or(0)
}

/// Loads up to `limit` newest entries, oldest first. Corrupt lines are skipped.
pub fn load(path: &Path, limit: usize) -> Result<Vec<HistoryEntry>> {
    let f = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(StoreError::io(path, e)),
    };
    // Split on bytes: one line that is not UTF-8 is skipped alone, and the
    // entries after it still load (and survive the next trim).
    let mut v: Vec<HistoryEntry> = BufReader::new(f)
        .split(b'\n')
        .map_while(std::result::Result::ok)
        .filter_map(|l| serde_json::from_slice(&l).ok())
        .collect();
    if v.len() > limit {
        v.drain(..v.len() - limit);
    }
    Ok(v)
}

pub fn clear(path: &Path) -> Result<()> {
    set_count(path, 0);
    match fs::remove_dir_all(responses_dir(path)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(StoreError::io(&responses_dir(path), e)),
    }
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
            response: None,
        }
    }

    fn response(body: &[u8]) -> Example {
        Example {
            name: String::new(),
            status: 200,
            reason: "OK".into(),
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: body.to_vec(),
        }
    }

    #[test]
    fn responses_are_kept_pruned_and_cleared() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(".sendito/history.jsonl");
        // An entry of an older version has no response key and still loads.
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(
            &p,
            serde_json::to_string(&entry(0))
                .unwrap()
                .replace(",\"response\":null", "")
                + "\n",
        )
        .unwrap();
        append_with_response(&p, &entry(1), 4, Some(&response(b"{\"a\":1}"))).unwrap();
        append_with_response(&p, &entry(2), 4, Some(&response(&[0xff, 0x00]))).unwrap();
        let all = load(&p, 10).unwrap();
        assert_eq!(all[0].response, None);
        let id = all[1].response.clone().unwrap();
        let ex = load_response(&p, &id).unwrap();
        assert_eq!((ex.status, ex.body.as_slice()), (200, &b"{\"a\":1}"[..]));
        let bin = load_response(&p, all[2].response.as_deref().unwrap()).unwrap();
        assert_eq!(bin.body, [0xff, 0x00]);
        assert!(responses_dir(&p).join(format!("{id}.toml")).is_file());
        // Trims drop the files of the entries they drop: keep 4 of 7.
        for i in 3..7 {
            append_with_response(&p, &entry(i), 4, Some(&response(b"x"))).unwrap();
        }
        let kept = load(&p, 100).unwrap();
        assert_eq!(kept.len(), 4);
        let files = fs::read_dir(responses_dir(&p)).unwrap().count();
        assert_eq!(files, kept.iter().filter(|e| e.response.is_some()).count());
        assert!(load_response(&p, &id).is_err());
        clear(&p).unwrap();
        assert!(!responses_dir(&p).exists());
    }

    #[test]
    fn cut_body_keeps_text_whole() {
        assert_eq!(cut_body(b"abc", 5), b"abc");
        assert_eq!(cut_body(b"abcdef", 4), b"abcd");
        // "é" is two bytes: a cut in its middle drops it.
        assert_eq!(cut_body("aé".as_bytes(), 2), b"a");
        assert_eq!(cut_body(&[0xff, 0xfe, 0xfd], 2), [0xff, 0xfe]);
    }

    #[test]
    fn bad_bytes_and_partial_lines_do_not_hide_later_entries() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(".sendito/history.jsonl");
        append(&p, &entry(1), 100).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&p)
            .unwrap()
            .write_all(b"\xff\xfe not utf-8\n{\"ts\":\"cut")
            .unwrap();
        append(&p, &entry(2), 100).unwrap();
        let names: Vec<String> = load(&p, 100).unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["r1", "r2"]);
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

    #[test]
    fn concurrent_appends_with_trims_keep_every_line_whole() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(".sendito/history.jsonl");
        std::thread::scope(|s| {
            for t in 0..4 {
                let p = &p;
                s.spawn(move || {
                    for i in 0..30 {
                        append(p, &entry(t * 100 + i), 20).unwrap();
                    }
                });
            }
        });
        let text = fs::read_to_string(&p).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines.len() <= 22, "{}", lines.len());
        for l in &lines {
            assert!(serde_json::from_str::<HistoryEntry>(l).is_ok(), "{l}");
        }
    }
}
