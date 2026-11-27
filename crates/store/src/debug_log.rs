//! The debug log: errors, warnings, panics and one line per send, written to
//! a file while it is on (`--debug` on the command line, `:debug` in the
//! app). One log per process; every crate writes to it through [`log`].

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use crate::{Result, StoreError};

struct Log {
    path: PathBuf,
    file: File,
}

static LOG: Mutex<Option<Log>> = Mutex::new(None);

fn lock() -> MutexGuard<'static, Option<Log>> {
    LOG.lock().unwrap_or_else(|e| e.into_inner())
}

/// `$XDG_STATE_HOME/sendito/sendito-<UTC time>.log`, else the same name in
/// `~/.local/state/sendito/`.
pub fn default_path() -> Option<PathBuf> {
    let dir = state_dir(std::env::var_os("XDG_STATE_HOME"), std::env::var_os("HOME"))?;
    let stamp = time::OffsetDateTime::now_utc()
        .format(time::macros::format_description!(
            "[year][month][day]-[hour][minute][second]"
        ))
        .unwrap_or_default();
    Some(dir.join(format!("sendito-{stamp}.log")))
}

fn state_dir(xdg: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    match xdg.filter(|s| !s.is_empty()) {
        Some(x) => Some(PathBuf::from(x).join("sendito")),
        None => home.map(|h| PathBuf::from(h).join(".local/state/sendito")),
    }
}

/// Starts the log in `path` ([`default_path`] when `None`), after the log
/// that runs, if any. The file is readable by its owner only (a URL can hold
/// a token); a file that exists gets the new lines at its end.
pub fn start(path: Option<PathBuf>) -> Result<PathBuf> {
    let path = path
        .or_else(default_path)
        .ok_or_else(|| StoreError::Invalid("no HOME for the debug log".into()))?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|e| StoreError::io(dir, e))?;
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    crate::fsio::owner_only(&mut options);
    let file = options.open(&path).map_err(|e| StoreError::io(&path, e))?;
    *lock() = Some(Log {
        path: path.clone(),
        file,
    });
    let args: Vec<String> = std::env::args().collect();
    log(
        "START",
        &format!(
            "sendito {} ({} {}): {}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            args.join(" ")
        ),
    );
    Ok(path)
}

/// Stops the log. Returns its file, `None` when it was off.
pub fn stop() -> Option<PathBuf> {
    log("STOP", "debug log off");
    lock().take().map(|l| l.path)
}

/// The file of the log, `None` when it is off.
pub fn path() -> Option<PathBuf> {
    lock().as_ref().map(|l| l.path.clone())
}

/// Writes one entry: `<UTC time> <LEVEL> <msg>`. The other lines of a
/// multi-line message start with two spaces. Does nothing while the log is
/// off; a write error is ignored (the log must not stop the app).
pub fn log(level: &str, msg: &str) {
    let mut guard = lock();
    let Some(l) = guard.as_mut() else {
        return;
    };
    let text = msg.trim_end().replace('\n', "\n  ");
    let _ = writeln!(l.file, "{} {level} {text}", crate::history::now_rfc3339());
}

/// Writes each panic to the log while it is on, with a backtrace, then runs
/// the hook that was there before. Install it once, at the start.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if path().is_some() {
            let trace = std::backtrace::Backtrace::force_capture();
            log("PANIC", &format!("{info}\n{trace}"));
        }
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_dir_prefers_xdg_state_home() {
        assert_eq!(
            state_dir(Some("/x".into()), Some("/h".into())),
            Some(PathBuf::from("/x/sendito"))
        );
        assert_eq!(
            state_dir(Some("".into()), Some("/h".into())),
            Some(PathBuf::from("/h/.local/state/sendito"))
        );
        assert_eq!(state_dir(None, None), None);
        let name = default_path().map(|p| p.file_name().unwrap().to_string_lossy().into_owned());
        if let Some(name) = name {
            assert!(
                name.starts_with("sendito-") && name.ends_with(".log"),
                "{name}"
            );
        }
    }

    // One test: the log is global to the process.
    #[test]
    fn debug_log_writes_while_on_only() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sub/debug.log");
        log("ERROR", "before start");
        assert_eq!(start(Some(file.clone())).unwrap(), file);
        assert_eq!(path(), Some(file.clone()));
        log("ERROR", "first line\nsecond line\n");
        assert_eq!(stop(), Some(file.clone()));
        log("ERROR", "after stop");
        assert_eq!(stop(), None);
        let text = fs::read_to_string(&file).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4, "{text}");
        assert!(lines[0].contains(" START sendito "), "{text}");
        assert!(lines[1].ends_with(" ERROR first line"), "{text}");
        assert_eq!(lines[2], "  second line");
        assert!(lines[3].ends_with(" STOP debug log off"), "{text}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0);
        }
    }
}
