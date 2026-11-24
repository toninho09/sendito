//! Session state restored on restart: open tabs, unsaved drafts, layout and the
//! active environment. Stored in the gitignored `.sendito/session.toml`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::collection::META_DIR;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Session {
    pub active_env: Option<String>,
    pub active_tab: usize,
    pub tabs: Vec<SessionTab>,
    pub layout: Layout,
    /// Folders expanded in the tree.
    pub expanded: Vec<String>,
    /// Non-secret runtime variables set by scripts.
    pub runtime_vars: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SessionTab {
    /// Relative path of the saved request; `None` for a never-saved request.
    pub path: Option<String>,
    /// Display name for unsaved requests.
    pub name: String,
    /// Unsaved edits as request TOML (absent when the tab is clean).
    pub draft: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub sidebar_width: u16,
    pub sidebar_visible: bool,
    /// Percentage of the main area given to the request editor.
    pub request_pct: u16,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            sidebar_width: 30,
            sidebar_visible: true,
            request_pct: 50,
        }
    }
}

pub fn session_path(root: &Path) -> PathBuf {
    root.join(META_DIR).join("session.toml")
}

/// Loads the session; a missing file yields the default. A file that does not
/// parse also yields the default, but it is first moved to
/// `session.toml.corrupt` (its drafts are not overwritten by the next save),
/// and the returned warning says so.
pub fn load(path: &Path) -> (Session, Option<String>) {
    let Ok(text) = fs::read_to_string(path) else {
        return (Session::default(), None);
    };
    match toml::from_str(&text) {
        Ok(s) => (s, None),
        Err(_) => {
            let aside = path.with_extension("toml.corrupt");
            let warning = match fs::rename(path, &aside) {
                Ok(()) => format!(
                    "the session file could not be read; it was kept as {}",
                    aside.display()
                ),
                Err(_) => "the session file could not be read; open tabs were not restored".into(),
            };
            (Session::default(), Some(warning))
        }
    }
}

pub fn save(path: &Path, s: &Session) -> Result<()> {
    let text = toml::to_string(s).map_err(|e| crate::StoreError::Invalid(e.to_string()))?;
    if fs::read_to_string(path).ok().as_deref() == Some(text.as_str()) {
        return Ok(());
    }
    // Drafts can hold a typed token: owner-only, like secrets.toml.
    crate::collection::write_private(path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let p = session_path(d.path());
        assert_eq!(load(&p), (Session::default(), None));
        let mut s = Session {
            active_env: Some("dev".into()),
            tabs: vec![SessionTab {
                path: Some("a.toml".into()),
                name: "a".into(),
                draft: Some("kind = \"http\"\nurl = '''\nx'''\n".into()),
            }],
            ..Default::default()
        };
        s.runtime_vars.insert("k".into(), "v".into());
        s.layout.sidebar_width = 40;
        save(&p, &s).unwrap();
        save(&p, &s).unwrap();
        assert_eq!(load(&p).0, s);
        fs::write(&p, "garbage [").unwrap();
        let (loaded, warning) = load(&p);
        assert_eq!(loaded, Session::default());
        assert!(warning.is_some_and(|w| w.contains("session.toml.corrupt")));
        assert_eq!(
            fs::read_to_string(p.with_extension("toml.corrupt")).unwrap(),
            "garbage ["
        );
        assert!(!p.exists());
    }
}
