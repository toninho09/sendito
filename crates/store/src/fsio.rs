//! Safe file writes and cross-process locks for the store.
//!
//! A write goes to a temporary file in the same directory, is flushed to disk,
//! and is renamed over the target, so a crash leaves the old file or the new
//! one, never a truncated file.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{Result, StoreError};

/// Writes through a temporary file and renames it into place. A new file gets
/// the default mode (umask); see [`write_private`] for owner-only files.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    write_temp(path, bytes, false)
}

/// Like [`write_atomic`], but the file is readable by its owner only.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    write_temp(path, bytes, true)
}

fn write_temp(path: &Path, bytes: &[u8], private: bool) -> Result<()> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
    if let Some(dir) = dir {
        fs::create_dir_all(dir).map_err(|e| StoreError::io(dir, e))?;
    }
    // Unique per process and per call: two threads saving the same file do
    // not write into one temporary file.
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    if private {
        owner_only(&mut options);
    }
    let written = options.open(&tmp).and_then(|mut f| {
        f.write_all(bytes)?;
        f.sync_all()
    });
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(StoreError::io(&tmp, e));
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        StoreError::io(path, e)
    })?;
    if let Some(dir) = dir {
        sync_dir(dir);
    }
    Ok(())
}

/// Makes the rename itself durable. Best effort: not every platform or file
/// system lets a directory be synced.
fn sync_dir(_dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = fs::File::open(_dir) {
        let _ = d.sync_all();
    }
}

/// Runs `f` while this process holds an exclusive lock on `lock_path`, which
/// is created when missing. Other Sendito processes that lock the same path
/// wait. The lock file is separate from the data file, because atomic writes
/// replace the data file.
pub(crate) fn with_lock<T>(lock_path: &Path, f: impl FnOnce() -> Result<T>) -> Result<T> {
    if let Some(dir) = lock_path.parent() {
        fs::create_dir_all(dir).map_err(|e| StoreError::io(dir, e))?;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    owner_only(&mut options);
    let lock = options
        .open(lock_path)
        .map_err(|e| StoreError::io(lock_path, e))?;
    lock.lock().map_err(|e| StoreError::io(lock_path, e))?;
    let result = f();
    let _ = lock.unlock();
    result
}

/// Runs `f` while this process holds an exclusive lock on `dir` itself. It
/// serializes read-modify-write of the files in that directory without a lock
/// file that would show up in the collection. Where a directory cannot be
/// locked (not Unix), `f` runs without the lock. An empty `dir` (the parent of
/// a bare file name) locks the current directory.
pub(crate) fn with_dir_lock<T>(dir: &Path, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    #[cfg(unix)]
    {
        fs::create_dir_all(dir).map_err(|e| StoreError::io(dir, e))?;
        let d = fs::File::open(dir).map_err(|e| StoreError::io(dir, e))?;
        d.lock().map_err(|e| StoreError::io(dir, e))?;
        let result = f();
        let _ = d.unlock();
        result
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        f()
    }
}

#[cfg(unix)]
fn owner_only(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

#[cfg(not(unix))]
fn owner_only(_: &mut OpenOptions) {}

/// Removes group and other access from an existing file.
pub(crate) fn make_private(_path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(_path)
            && meta.permissions().mode() & 0o077 != 0
        {
            let mode = meta.permissions().mode() & !0o077;
            let _ = fs::set_permissions(_path, fs::Permissions::from_mode(mode));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub/file.toml");
        write_atomic(&p, b"one").unwrap();
        write_private(&p, b"two").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "two");
        let names: Vec<_> = fs::read_dir(dir.path().join("sub"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("file.toml")]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&p).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0);
        }
    }

    fn bump_concurrently(dir: &Path, lock: impl Fn(&dyn Fn() -> Result<()>) + Sync) -> u32 {
        let counter = dir.join("n");
        fs::write(&counter, "0").unwrap();
        let read_add = || {
            let n: u32 = fs::read_to_string(&counter).unwrap().parse().unwrap();
            std::thread::yield_now();
            fs::write(&counter, (n + 1).to_string()).unwrap();
            Ok(())
        };
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    for _ in 0..25 {
                        lock(&read_add);
                    }
                });
            }
        });
        fs::read_to_string(&counter).unwrap().parse().unwrap()
    }

    #[test]
    fn file_lock_serializes_threads() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("n.lock");
        let n = bump_concurrently(dir.path(), |f| with_lock(&lock_path, f).unwrap());
        assert_eq!(n, 200);
    }

    #[cfg(unix)]
    #[test]
    fn dir_lock_serializes_threads() {
        let dir = tempfile::tempdir().unwrap();
        let n = bump_concurrently(dir.path(), |f| with_dir_lock(dir.path(), f).unwrap());
        assert_eq!(n, 200);
    }

    #[cfg(unix)]
    #[test]
    fn an_empty_dir_locks_the_current_directory() {
        // The parent of a bare file name is `""`: the lock must still run `f`.
        assert_eq!(with_dir_lock(Path::new(""), || Ok(7)).unwrap(), 7);
    }
}
