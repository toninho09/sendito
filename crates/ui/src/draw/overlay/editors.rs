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
    use crate::overlay::EnvFocus;
    let on_table = e.focus == EnvFocus::Table;
    let explain: String = match e.selected_type().filter(|_| on_table) {
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
        _ if e.focus == EnvFocus::Theme => {
            "theme: the app shows it while this environment is active. ← → change it; Space or Enter lists the themes.".into()
        }
        _ if e.focus == EnvFocus::Dotenv => {
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
    let has_theme = e.has_theme();
    let mut heads = Vec::new();
    if has_theme {
        let names = app.themes.names();
        let base = app.project.config.ui.theme.clone();
        let shown = e.theme.clone().unwrap_or_else(|| BASE_THEME.to_string());
        let value = match e
            .theme
            .as_ref()
            .and_then(|t| names.iter().position(|n| n.eq_ignore_ascii_case(t)))
        {
            Some(i) => format!("‹ {shown} › ({} of {})", i + 2, names.len() + 1),
            None if e.theme.is_none() => format!("‹ {shown} › (1 of {})", names.len() + 1),
            None => format!("‹ {shown} ›"),
        };
        let t = app.themes.theme(e.theme.as_deref().unwrap_or(&base));
        heads.push(Head {
            label: "Theme",
            input: HeadInput::Choice { value, theme: t },
            focused: e.focus == EnvFocus::Theme,
            status: &e.theme_status.0,
            problem: e.theme_status.1,
            hit: OverlayHit::EnvTheme,
        });
    }
    heads.push(Head {
        label: ".env file",
        input: HeadInput::Text(&mut e.dotenv),
        focused: e.focus == EnvFocus::Dotenv,
        status: &e.dotenv_status.0,
        problem: e.dotenv_status.1,
        hit: OverlayHit::EnvDotenv,
    });
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
            heads,
        },
        &mut e.table,
        &EnvHits,
    );
}
