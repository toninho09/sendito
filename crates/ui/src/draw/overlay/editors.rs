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
    use crate::keymap::Action;
    use crate::overlay::env_editor as t;
    let browse = app.dialog_key(Action::DialogBrowse);
    let save = app.dialog_key(Action::DialogConfirm);
    let explain: String = match e.selected_type().filter(|_| !e.on_dotenv) {
        Some(t::TEXT) => "text: the value is written in this file and shared through git.".into(),
        Some(t::SECRET) => {
            "secret: type the value; it goes to .sendito/secrets.toml on this machine, never committed.".into()
        }
        Some(t::SECRET_KEY) => {
            "secret key: the name of an entry in .sendito/secrets.toml, e.g. one that other variables share.".into()
        }
        Some(t::KEYCHAIN) => "keychain: type the value; it goes to your OS keychain.".into(),
        Some(t::KEYCHAIN_KEY) => {
            "keychain key: the name of an account in your OS keychain.".into()
        }
        Some(t::FILE) => {
            format!(
                "file: the value is read from this file (relative to the collection). {browse} browses; a pasted path works too."
            )
        }
        Some(t::DOTENV) => {
            "dotenv: read from the .env file above. Value: its key, or empty to use the variable name.".into()
        }
        _ if e.on_dotenv => {
            format!(
                ".env file: the file the .env variables read, relative to the collection; empty = .env. {browse} browses."
            )
        }
        _ => "Type a name in the last row to add a variable.".into(),
    };
    let legend = format!(
        "Type says where a value lives (Space changes it). A secret or keychain value \
            is typed here but stored outside this file. ✕ deletes a row ({}). {save} saves; \
            Esc asks before discarding unsaved changes.",
        app.dialog_key(Action::DialogDelete)
    );
    let labels_btn = [
        format!("Save ({save})"),
        format!("New env ({})", app.dialog_key(Action::DialogNew)),
        "Close (Esc)".to_string(),
    ];
    let buttons: Vec<&str> = labels_btn.iter().map(String::as_str).collect();
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
            legend: &legend,
            explain: &explain,
            error: e.error.as_deref(),
            dirty: e.dirty,
            buttons: &buttons,
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
