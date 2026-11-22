//! Private, environment-scoped HTTP cookie storage.
//!
//! Cookie parsing and matching are delegated to `cookie_store`; public suffixes
//! are rejected using the bundled `psl` list. Writes are serialized across
//! processes and use a private temporary file and atomic rename.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{Cursor, Write};
use std::path::Path;

use cookie_store::{CookieDomain, CookieStore, RawCookie};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{Result, StoreError};

#[derive(Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct FileContents {
    // Each value is the cookie_store JSON representation for one environment.
    jars: BTreeMap<String, String>,
}

fn path(root: &Path) -> std::path::PathBuf {
    root.join(".sendito/cookies.toml")
}

fn parse_url(url: &str) -> Result<Url> {
    let url = Url::parse(url).map_err(|e| StoreError::Invalid(format!("cookie URL: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(StoreError::Invalid(
            "cookie URL must be HTTP or HTTPS".into(),
        ));
    }
    Ok(url)
}

/// Acquire a lock independent of the atomically replaced cookie file.
fn with_lock<T>(root: &Path, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let dir = root.join(".sendito");
    fs::create_dir_all(&dir).map_err(|e| StoreError::io(&dir, e))?;
    let lock_path = dir.join("cookies.lock");
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options
        .open(&lock_path)
        .map_err(|e| StoreError::io(&lock_path, e))?;
    lock.lock_exclusive()
        .map_err(|e| StoreError::io(&lock_path, e))?;
    let result = f();
    let _ = FileExt::unlock(&lock);
    result
}

fn read_file(root: &Path) -> Result<FileContents> {
    let p = path(root);
    let text = match fs::read_to_string(&p) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(FileContents::default()),
        Err(e) => return Err(StoreError::io(&p, e)),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = fs::metadata(&p).map_err(|e| StoreError::io(&p, e))?;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(StoreError::Invalid(
                "cookie jar permissions must not allow group or other access".into(),
            ));
        }
    }
    toml::from_str(&text).map_err(|_| StoreError::Parse {
        path: p,
        msg: "invalid cookie jar".into(),
    })
}

fn read_jar(file: &FileContents, env: &str) -> Result<CookieStore> {
    let Some(text) = file.jars.get(env) else {
        return Ok(CookieStore::default());
    };
    let store = cookie_store::serde::json::load_all(Cursor::new(text))
        .map_err(|_| StoreError::Invalid("invalid cookie jar contents".into()))?;
    // Do not trust a manually edited or old jar with an unsafe domain cookie.
    if store
        .iter_any()
        .any(|c| matches!(&c.domain, CookieDomain::Suffix(d) if public_suffix(d)))
    {
        return Err(StoreError::Invalid(
            "cookie jar contains a public-suffix cookie".into(),
        ));
    }
    Ok(store)
}

fn write_file(root: &Path, file: &FileContents) -> Result<()> {
    let p = path(root);
    let dir = root.join(".sendito");
    // Ignore rules cannot protect a file already tracked or staged by git.
    let canonical_root = root.canonicalize().map_err(|e| StoreError::io(root, e))?;
    let in_git = canonical_root
        .ancestors()
        .any(|ancestor| ancestor.join(".git").exists());
    if in_git {
        let status = std::process::Command::new("git")
            .current_dir(root)
            .args(["ls-files", "--error-unmatch", "--", ".sendito/cookies.toml"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|_| {
                StoreError::Invalid("cannot check whether cookie jar is tracked by git".into())
            })?;
        match status.code() {
            Some(0) => {
                return Err(StoreError::Invalid(
                    "cookie jar is tracked by git; untrack it before storing cookies".into(),
                ));
            }
            Some(1) => {}
            _ => {
                return Err(StoreError::Invalid(
                    "cannot check whether cookie jar is tracked by git".into(),
                ));
            }
        }
    }
    // Existing collections may predate the cookie jar. Ensure newly created
    // credentials are ignored even if `sendito init` was run before this feature.
    let ignore = dir.join(".gitignore");
    let existing = match fs::read_to_string(&ignore) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(StoreError::io(&ignore, e)),
    };
    // `/cookies.toml` and `/cookies.lock` protect the jar; `/.gitignore`
    // makes this file ignore itself, so it never shows up as an untracked
    // file after the first save.
    let missing: Vec<&str> = ["/cookies.toml", "/cookies.lock", "/.gitignore"]
        .into_iter()
        .filter(|entry| !existing.lines().any(|line| line.trim() == *entry))
        .collect();
    if !missing.is_empty() {
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        let mut f = options
            .open(&ignore)
            .map_err(|e| StoreError::io(&ignore, e))?;
        if !existing.is_empty() && !existing.ends_with('\n') {
            f.write_all(b"\n").map_err(|e| StoreError::io(&ignore, e))?;
        }
        for entry in missing {
            writeln!(f, "{entry}").map_err(|e| StoreError::io(&ignore, e))?;
        }
    }
    if in_git {
        let status = std::process::Command::new("git")
            .current_dir(root)
            .args(["check-ignore", "--quiet", "--", ".sendito/cookies.toml"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|_| StoreError::Invalid("cannot check cookie jar ignore rules".into()))?;
        if !status.success() {
            return Err(StoreError::Invalid(
                "cookie jar is not ignored by git; fix ignore rules before storing cookies".into(),
            ));
        }
    }
    let text = toml::to_string(file)
        .map_err(|e| StoreError::Invalid(format!("cannot serialize cookie jar: {e}")))?;
    let mut tmp = tempfile::NamedTempFile::new_in(&dir).map_err(|e| StoreError::io(&dir, e))?;
    tmp.write_all(text.as_bytes())
        .map_err(|e| StoreError::io(&p, e))?;
    tmp.persist(&p).map_err(|e| StoreError::io(&p, e.error))?;
    Ok(())
}

fn public_suffix(domain: &str) -> bool {
    let d = domain.trim_start_matches('.').trim_end_matches('.');
    // Canonicalize IDNA before checking the public-suffix list. Reject IP
    // addresses and malformed Domain attributes rather than broadening scope.
    let Ok(url::Host::Domain(d)) = url::Host::parse(d) else {
        return true;
    };
    psl::suffix_str(&d).is_none_or(|s| s.eq_ignore_ascii_case(&d))
}

/// Select matching cookies. Returns the header and all unexpired values, which
/// callers must treat as secrets when masking response contents.
pub fn request(root: &Path, env: &str, url: &str) -> Result<(Option<String>, Vec<String>)> {
    let url = parse_url(url)?;
    with_lock(root, || {
        let store = read_jar(&read_file(root)?, env)?;
        let mut matches = store.matches(&url);
        matches.sort_by_key(|c| std::cmp::Reverse(c.path.len()));
        let header = matches
            .iter()
            .map(|c| format!("{}={}", c.name(), c.value()))
            .collect::<Vec<_>>()
            .join("; ");
        let values = store
            .iter_unexpired()
            .map(|c| c.value().to_string())
            .filter(|v| !v.is_empty())
            .collect();
        Ok(((!header.is_empty()).then_some(header), values))
    })
}

/// Parse and apply all Set-Cookie headers from a response. The whole update is
/// done under a cross-process lock, preserving updates by concurrent requests.
pub fn store_response(root: &Path, env: &str, url: &str, headers: &[String]) -> Result<()> {
    let url = parse_url(url)?;
    with_lock(root, || {
        let mut file = read_file(root)?;
        let mut store = read_jar(&file, env)?;
        for header in headers {
            let Ok(raw) = RawCookie::parse(header.clone()) else {
                continue;
            };
            if raw.domain().is_some_and(public_suffix) {
                continue;
            }
            // HTTP must not set Secure cookies or overwrite an existing Secure
            // cookie of the same name (conservative against cookie tossing).
            if url.scheme() != "https"
                && (raw.secure() == Some(true)
                    || store
                        .iter_unexpired()
                        .any(|c| c.secure() == Some(true) && c.name() == raw.name()))
            {
                continue;
            }
            // Prefixes impose additional security requirements.
            if raw.name().starts_with("__Secure-")
                && (url.scheme() != "https" || raw.secure() != Some(true))
            {
                continue;
            }
            if raw.name().starts_with("__Host-")
                && (url.scheme() != "https"
                    || raw.secure() != Some(true)
                    || raw.domain().is_some()
                    || raw.path() != Some("/"))
            {
                continue;
            }
            let _ = store.insert_raw(&raw, &url);
        }
        save_jar(root, &mut file, env, &store)
    })
}

/// Values in Set-Cookie response headers, including rejected cookies, to mask
/// before displaying the response.
pub fn response_secrets(headers: &[String]) -> Vec<String> {
    headers
        .iter()
        .filter_map(|s| RawCookie::parse(s.clone()).ok())
        .map(|c| c.value().to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

fn save_jar(root: &Path, file: &mut FileContents, env: &str, store: &CookieStore) -> Result<()> {
    let mut bytes = Vec::new();
    cookie_store::serde::json::save_incl_expired_and_nonpersistent(store, &mut bytes)
        .map_err(|_| StoreError::Invalid("cannot serialize cookies".into()))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| StoreError::Invalid("invalid cookie encoding".into()))?;
    file.jars.insert(env.to_string(), text);
    write_file(root, file)
}

// The same deterministic order is used by the report and deletion command.
fn ordered(store: &CookieStore) -> Vec<&cookie_store::Cookie<'static>> {
    let mut items: Vec<_> = store.iter_unexpired().collect();
    items.sort_by_key(|c| {
        (
            c.name().to_owned(),
            c.domain.as_cow().unwrap_or_default().into_owned(),
            c.path.to_string(),
        )
    });
    items
}

/// Describe cookies without exposing values.
pub fn describe(root: &Path, env: &str) -> Result<Vec<String>> {
    with_lock(root, || {
        let store = read_jar(&read_file(root)?, env)?;
        Ok(ordered(&store)
            .into_iter()
            .enumerate()
            .map(|(i, c)| {
                format!(
                    "{}  {} = ••••••  · {}  · {}{}",
                    i + 1,
                    c.name(),
                    c.domain.as_cow().unwrap_or_default(),
                    c.path.as_ref(),
                    if c.secure() == Some(true) {
                        " · secure"
                    } else {
                        ""
                    }
                )
            })
            .collect())
    })
}

/// Delete one cookie by its one-based report index.
pub fn delete(root: &Path, env: &str, index: usize) -> Result<()> {
    with_lock(root, || {
        let mut file = read_file(root)?;
        let mut store = read_jar(&file, env)?;
        let key = index
            .checked_sub(1)
            .and_then(|i| {
                ordered(&store).get(i).map(|c| {
                    (
                        c.domain.as_cow().unwrap_or_default().into_owned(),
                        c.path.to_string(),
                        c.name().to_string(),
                    )
                })
            })
            .ok_or_else(|| StoreError::Invalid("cookie number not found".into()))?;
        store.remove(&key.0, &key.1, &key.2);
        save_jar(root, &mut file, env, &store)
    })
}

/// Remove all cookies in one environment.
pub fn clear(root: &Path, env: &str) -> Result<()> {
    with_lock(root, || {
        let mut file = read_file(root)?;
        file.jars.remove(env);
        write_file(root, &file)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_security_and_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let source = "https://api.example.com/login";
        store_response(
            root,
            "dev",
            source,
            &[
                "id=one; Path=/; Secure".into(),
                "other=two; Domain=example.com; Path=/api".into(),
                "evil=bad; Domain=com; Path=/".into(),
                "bad=bad; Domain=other.com".into(),
                "__Host-invalid=bad; Secure; Domain=example.com; Path=/".into(),
            ],
        )
        .unwrap();
        let (header, _) = request(root, "dev", "https://api.example.com/api/a").unwrap();
        assert_eq!(header.as_deref(), Some("other=two; id=one"));
        assert_eq!(request(root, "other", source).unwrap().0, None);
        assert_eq!(
            request(root, "dev", "http://api.example.com/").unwrap().0,
            None
        );
        assert_eq!(
            request(root, "dev", "https://www.example.com/api")
                .unwrap()
                .0
                .as_deref(),
            Some("other=two")
        );
        store_response(
            root,
            "dev",
            source,
            &["id=deleted; Path=/; Max-Age=0".into()],
        )
        .unwrap();
        assert_eq!(request(root, "dev", source).unwrap().0, None);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path(root)).unwrap().permissions().mode() & 0o077,
                0
            );
        }
        clear(root, "dev").unwrap();
        assert_eq!(
            request(root, "dev", "https://www.example.com/api")
                .unwrap()
                .0,
            None
        );
    }

    #[test]
    fn ignore_file_ignores_itself_and_is_not_duplicated() {
        let d = tempfile::tempdir().unwrap();
        store_response(d.path(), "dev", "https://example.com/", &["a=b".into()]).unwrap();
        let gi = fs::read_to_string(d.path().join(".sendito/.gitignore")).unwrap();
        for entry in ["/cookies.toml", "/cookies.lock", "/.gitignore"] {
            assert!(gi.lines().any(|l| l.trim() == entry), "{gi}");
        }
        // A second save appends nothing.
        store_response(d.path(), "dev", "https://example.com/", &["c=d".into()]).unwrap();
        let gi = fs::read_to_string(d.path().join(".sendito/.gitignore")).unwrap();
        assert_eq!(gi.matches("/cookies.toml").count(), 1);
    }

    #[test]
    fn public_suffix_and_idna_are_rejected() {
        let d = tempfile::tempdir().unwrap();
        store_response(
            d.path(),
            "dev",
            "https://site.xn--p1ai/",
            &["bad=x; Domain=рф; Path=/".into(), "ok=y; Path=/".into()],
        )
        .unwrap();
        assert_eq!(
            request(d.path(), "dev", "https://other.xn--p1ai/")
                .unwrap()
                .0,
            None
        );
        assert_eq!(
            request(d.path(), "dev", "https://site.xn--p1ai/")
                .unwrap()
                .0
                .as_deref(),
            Some("ok=y")
        );
    }

    #[test]
    fn numbered_deletion_and_concurrent_updates() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let root = root.clone();
                std::thread::spawn(move || {
                    store_response(
                        &root,
                        "dev",
                        "https://example.com/",
                        &[format!("c{i}=v{i}; Path=/")],
                    )
                    .unwrap()
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(describe(&root, "dev").unwrap().len(), 8);
        delete(&root, "dev", 1).unwrap();
        assert_eq!(describe(&root, "dev").unwrap().len(), 7);
        assert!(
            !describe(&root, "dev")
                .unwrap()
                .iter()
                .any(|r| r.contains("c0 ="))
        );
        assert!(delete(&root, "dev", 0).is_err());
    }

    #[test]
    fn ipv6_and_path_scoping() {
        let d = tempfile::tempdir().unwrap();
        store_response(
            d.path(),
            "dev",
            "http://[::1]/api/login",
            &["x=v; Path=/api".into()],
        )
        .unwrap();
        assert_eq!(
            request(d.path(), "dev", "http://[::1]/api/x")
                .unwrap()
                .0
                .as_deref(),
            Some("x=v")
        );
        assert_eq!(
            request(d.path(), "dev", "http://[::1]/apix").unwrap().0,
            None
        );
    }

    #[test]
    fn refuses_to_overwrite_a_tracked_cookie_file() {
        let d = tempfile::tempdir().unwrap();
        let status = std::process::Command::new("git")
            .current_dir(d.path())
            .args(["init", "-q"])
            .status()
            .unwrap();
        assert!(status.success());
        fs::create_dir(d.path().join(".sendito")).unwrap();
        fs::write(path(d.path()), "[jars]\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path(d.path()), fs::Permissions::from_mode(0o600)).unwrap();
        }
        let status = std::process::Command::new("git")
            .current_dir(d.path())
            .args(["add", "--", ".sendito/cookies.toml"])
            .status()
            .unwrap();
        assert!(status.success());
        let result = store_response(
            d.path(),
            "dev",
            "https://example.test/",
            &["session=private-value".into()],
        );
        assert!(result.is_err());
        assert!(
            !fs::read_to_string(path(d.path()))
                .unwrap()
                .contains("private-value")
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_insecure_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        store_response(d.path(), "dev", "https://example.com/", &["a=b".into()]).unwrap();
        fs::set_permissions(path(d.path()), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(request(d.path(), "dev", "https://example.com/").is_err());
    }
}
