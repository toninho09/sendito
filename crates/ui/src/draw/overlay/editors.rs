//! The environment editor dialog.

use super::*;

/// Where the environment editor's clicks go.
pub(super) struct EnvHits;

impl EditorHits for EnvHits {
    fn tab(&self, i: usize) -> OverlayHit {
        OverlayHit::EnvTab(i)
    }
    fn cell(&self, row: usize, col: usize) -> OverlayHit {
        OverlayHit::EnvCell { row, col }
    }
    fn delete(&self, row: usize) -> OverlayHit {
        OverlayHit::EnvDelete(row)
    }
    fn button(&self, i: usize) -> OverlayHit {
        OverlayHit::Button(i)
    }
}

pub(super) fn draw_env(
    app: &mut App,
    buf: &mut Buffer,
    frame: Frame<'_>,
    is_def: &impl Fn(&str) -> bool,
    e: &mut EnvEditor,
    cursor: &mut Option<(u16, u16)>,
) {
    let labels: Vec<String> = e
        .envs
        .iter()
        .map(|name| {
            if name == sendito_store::env::COLLECTION_ENV {
                "Collection variables".to_string()
            } else if app.active_env.as_deref() == Some(name.as_str()) {
                // The highlight shows the env being edited; this marks the one in use.
                format!("{name} (active)")
            } else {
                name.clone()
            }
        })
        .collect();
    use crate::overlay::env_editor as t;
    let explain = match e.selected_type().filter(|_| !e.on_dotenv) {
        Some(t::TEXT) => "text: the value is written in this file and shared through git.",
        Some(t::SECRET) => {
            "secret: type the value; it goes to .sendito/secrets.toml on this machine, never committed."
        }
        Some(t::SECRET_KEY) => {
            "secret key: the name of an entry in .sendito/secrets.toml, e.g. one that other variables share."
        }
        Some(t::KEYCHAIN) => "keychain: type the value; it goes to your OS keychain.",
        Some(t::KEYCHAIN_KEY) => "keychain key: the name of an account in your OS keychain.",
        Some(t::FILE) => {
            "file: the value is read from this file (relative to the collection). F2 browses; a pasted path works too."
        }
        Some(t::DOTENV) => {
            "dotenv: read from the .env file above. Value: its key, or empty to use the variable name."
        }
        _ if e.on_dotenv => {
            ".env file: the file the .env variables read, relative to the collection; empty = .env. F2 browses."
        }
        _ => "Type a name in the last row to add a variable.",
    };
    let legend = "Type says where a value lives (Space changes it). A secret or keychain value \
            is typed here but stored outside this file. ✕ deletes a row (Ctrl+D). Ctrl+S saves; \
            Esc asks before discarding unsaved changes.";
    *cursor = editor::render(
        app,
        buf,
        frame.body,
        frame.theme,
        EditorDialog {
            title: "Environments",
            width: 112,
            height: frame.area.height.saturating_sub(4).max(12),
            tabs: labels,
            tab_sel: e.current,
            reserved: 4,
            legend,
            explain,
            error: e.error.as_deref(),
            dirty: e.dirty,
            buttons: &["Save (Ctrl+S)", "New env (Ctrl+N)", "Close (Esc)"],
            is_def,
            head: Some(Head {
                label: ".env file",
                input: &mut e.dotenv,
                focused: e.on_dotenv,
                status: &e.dotenv_status.0,
                problem: e.dotenv_status.1,
                hit: OverlayHit::EnvDotenv,
            }),
        },
        &mut e.table,
        &EnvHits,
    );
}
