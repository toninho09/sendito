//! Programs outside the UI: the focused text in `$VISUAL` / `$EDITOR`, and
//! the response body in `$PAGER`. The event loop leaves the terminal, runs
//! the program on a private temporary file and enters the terminal again.

use std::io::Write;
use std::path::{Path, PathBuf};

/// A program the event loop runs between two frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum External {
    /// Edit `text`; the result replaces the focused text. `ext` is the file
    /// extension the editor uses for highlighting (`json`, `lua`, …).
    Edit { text: String, ext: &'static str },
    /// Read `text` in the pager.
    Pager { text: String },
}

/// The shell command of the program, from the environment: `$VISUAL`, then
/// `$EDITOR`, then `vi` to edit; `$PAGER`, then `less` to read. A value can
/// hold arguments (`code -w`).
pub fn command(job: &External, env: impl Fn(&str) -> Option<String>) -> String {
    let set = |name: &str| env(name).filter(|v| !v.trim().is_empty());
    match job {
        External::Edit { .. } => set("VISUAL")
            .or_else(|| set("EDITOR"))
            .unwrap_or_else(|| "vi".into()),
        External::Pager { .. } => set("PAGER").unwrap_or_else(|| "less".into()),
    }
}

/// A new private file in the temporary directory, with `text` in it.
fn temp_file(text: &str, ext: &str) -> std::io::Result<PathBuf> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("sendito-{}-{n}.{ext}", std::process::id()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&path)?;
    f.write_all(text.as_bytes())?;
    Ok(path)
}

/// Runs `cmd` on `path` through `sh`, so the command can hold arguments.
fn run_shell(cmd: &str, path: &Path) -> Result<(), String> {
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{cmd} \"$1\""))
        .arg("sh")
        .arg(path)
        .status()
        .map_err(|e| format!("cannot start {cmd}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{cmd} exited with {status}"))
    }
}

/// Runs the program of `job` on a temporary file. The caller leaves the
/// terminal before and enters it after. Returns the edited text for
/// [`External::Edit`], `None` for the pager.
pub fn run(job: &External) -> Result<Option<String>, String> {
    let cmd = command(job, |name| std::env::var(name).ok());
    let (text, ext) = match job {
        External::Edit { text, ext } => (text.as_str(), *ext),
        External::Pager { text } => (text.as_str(), "txt"),
    };
    let path = temp_file(text, ext).map_err(|e| format!("cannot write a temporary file: {e}"))?;
    let result = run_shell(&cmd, &path).and_then(|()| match job {
        External::Edit { .. } => std::fs::read_to_string(&path)
            .map(Some)
            .map_err(|e| format!("cannot read the edited file: {e}")),
        External::Pager { .. } => Ok(None),
    });
    let _ = std::fs::remove_file(&path);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            vars.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn the_command_follows_the_environment() {
        let edit = External::Edit {
            text: String::new(),
            ext: "json",
        };
        let pager = External::Pager {
            text: String::new(),
        };
        assert_eq!(command(&edit, env(&[])), "vi");
        assert_eq!(command(&edit, env(&[("EDITOR", "nano")])), "nano");
        assert_eq!(
            command(&edit, env(&[("EDITOR", "nano"), ("VISUAL", "code -w")])),
            "code -w"
        );
        // An empty value does not count.
        assert_eq!(
            command(&edit, env(&[("VISUAL", " "), ("EDITOR", "nano")])),
            "nano"
        );
        assert_eq!(command(&pager, env(&[])), "less");
        assert_eq!(command(&pager, env(&[("PAGER", "more")])), "more");
    }

    #[cfg(unix)]
    #[test]
    fn a_command_with_arguments_edits_a_private_file() {
        let path = temp_file("a b", "txt").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        // `sed -i.bak` stands in for an editor that takes arguments.
        run_shell("sed -i.bak s/a/z/", &path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "z b");
        assert!(run_shell("false", &path).unwrap_err().contains("exited"));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("txt.bak"));
    }
}
