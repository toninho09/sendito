//! Modal overlays: palette, help, settings, confirm, prompt, picker, env
//! editor, import, report.

use crate::editor::TextEditor;
use crate::form::{Cell, ColKind, Column, Table};
use crate::viewer::Viewer;

mod env_editor;
mod mcp_editor;
mod palette;
mod settings;
pub use env_editor::*;
pub use mcp_editor::*;
pub use palette::*;
pub use settings::*;

/// What to do when a confirm dialog's first button is chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmAction {
    SendAnyway,
    /// Send although a secret did not load (its `{{name}}` goes out as typed).
    SendMissingSecrets,
    Delete(String),
    Quit,
    /// Close tab: button 0 = save, 1 = discard.
    CloseTab(u64),
    /// Close tab with a live request or open session: 0 = close, 1 = keep it.
    CloseTabRunning(u64),
    /// Env editor closing with changes: 0 = save, 1 = discard.
    EnvClose,
    /// MCP servers editor closing with changes: 0 = save, 1 = discard.
    McpClose,
    /// A config editor (MCP server details, OAuth, keymap) closing with
    /// changes: 0 = save, 1 = discard.
    CfgClose,
    /// Prompt or import dialog closing with typed text: 0 = discard,
    /// 1 = keep editing.
    DialogClose,
    ClearHistory,
    /// Delete the cookie with this number from the active environment's jar.
    DeleteCookie(usize),
    /// Delete every cookie of the active environment.
    ClearCookies,
    /// An MCP server asks for a sign-in: 0 = sign in, then do `after` in `tab`.
    McpLogin {
        server: String,
        tab: u64,
        after: McpAfterLogin,
    },
}

/// What runs again after a sign-in that a failed MCP call started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpAfterLogin {
    Send,
    Tools { open_picker: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptAction {
    NewRequest {
        folder: String,
    },
    NewFolder {
        folder: String,
    },
    Rename(String),
    SaveAs {
        tab: u64,
    },
    SaveBody,
    NewEnv,
    /// The entry itself, not its row: the list can reload under the prompt.
    SaveHistory(Box<sendito_store::history::HistoryEntry>),
    CustomMethod,
    /// A new server for the MCP editor: (name, URL or command).
    McpServer {
        name: String,
    },
    /// A typed setting of the settings dialog (number, path or text).
    Setting(SettingId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickAction {
    Env,
    Method,
    GrpcMethod,
    McpServer,
    McpTool,
    MoveTo(String),
    Choice(crate::tab::Field),
    RequestKind,
    Theme,
    /// The cookie jar: item i is cookie i + 1; the last item clears all.
    Cookie,
    /// Sign in to (or out of) an OAuth identity or an MCP server.
    Auth {
        login: bool,
    },
    /// Open the details editor of an MCP server.
    McpServerDetails,
}

pub struct Prompt {
    pub title: String,
    pub label: String,
    pub input: TextEditor,
    /// The field's text when the dialog opened: a different text is pending.
    pub initial: String,
    /// Choice rows (e.g. HTTP / gRPC, target folder): (label, options, selected).
    pub choices: Vec<(String, Vec<String>, usize)>,
    /// 0 = input, 1.. = choice rows.
    pub focus: usize,
    pub action: PromptAction,
    pub error: Option<String>,
    /// Information about a pre-filled value (shown until the input changes).
    pub note: Option<String>,
    /// The option list of the focused choice row is open, with this row highlighted.
    pub open: Option<usize>,
}

impl Prompt {
    /// A prompt whose pending state is the text its field was opened with.
    pub fn new(
        title: &str,
        label: &str,
        input: TextEditor,
        choices: Vec<(String, Vec<String>, usize)>,
        action: PromptAction,
    ) -> Self {
        let initial = input.text();
        Self {
            title: title.to_string(),
            label: label.to_string(),
            input,
            initial,
            choices,
            focus: 0,
            action,
            error: None,
            note: None,
            open: None,
        }
    }

    /// The field holds text typed after the dialog opened (closing would drop it).
    pub fn pending(&self) -> bool {
        self.input.text() != self.initial
    }
}

pub struct Picker {
    pub title: String,
    pub items: Vec<String>,
    /// Secondary text shown dimmed after each item.
    pub details: Vec<String>,
    pub filter: TextEditor,
    pub sel: usize,
    pub scroll: usize,
    pub action: PickAction,
}

impl Picker {
    pub fn new(
        title: &str,
        items: Vec<String>,
        details: Vec<String>,
        action: PickAction,
        current: Option<usize>,
    ) -> Self {
        Self {
            title: title.to_string(),
            items,
            details,
            filter: TextEditor::single("").with_placeholder("type to filter"),
            sel: current.unwrap_or(0),
            scroll: 0,
            action,
        }
    }

    /// Indices of items matching the filter.
    pub fn visible(&self) -> Vec<usize> {
        let f = self.filter.text().to_lowercase();
        self.items
            .iter()
            .enumerate()
            .filter(|(i, s)| {
                f.is_empty()
                    || fuzzy(&s.to_lowercase(), &f)
                    || self
                        .details
                        .get(*i)
                        .is_some_and(|d| fuzzy(&d.to_lowercase(), &f))
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn selected(&self) -> Option<usize> {
        self.visible().get(self.sel).copied()
    }
}

/// Subsequence match: all query characters appear in order.
pub fn fuzzy(hay: &str, q: &str) -> bool {
    if hay.contains(q) {
        return true;
    }
    let mut it = hay.chars();
    q.chars()
        .filter(|c| !c.is_whitespace())
        .all(|c| it.any(|h| h == c))
}

pub struct Confirm {
    pub title: String,
    pub message: String,
    pub buttons: Vec<String>,
    pub sel: usize,
    pub action: ConfirmAction,
}

pub struct Report {
    pub title: String,
    pub viewer: Viewer,
    /// Text copied with `y`.
    pub copy_text: String,
    /// Already on the clipboard (the button then reads "Copy again").
    pub copied: bool,
    /// Status line message left when the dialog closes (what was done).
    pub close_status: Option<String>,
}

pub struct Import {
    pub input: TextEditor,
    /// The field's text when the dialog opened: a different text is pending.
    pub initial: String,
    pub error: Option<String>,
}

impl Import {
    /// An import dialog whose pending state is the text its field was opened with.
    pub fn new(input: TextEditor) -> Self {
        let initial = input.text();
        Self {
            input,
            initial,
            error: None,
        }
    }

    /// The field holds text typed after the dialog opened (closing would drop it).
    pub fn pending(&self) -> bool {
        self.input.text() != self.initial
    }
}

pub enum Overlay {
    Palette(Palette),
    Help { viewer: Viewer, filter: TextEditor },
    Settings { sel: usize },
    Confirm(Confirm),
    Prompt(Prompt),
    Picker(Picker),
    Env(EnvEditor),
    Mcp(McpEditor),
    Cfg(crate::config_editor::CfgEditor),
    Import(Import),
    Report(Report),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_and_picker() {
        assert!(fuzzy("users/list users", "lsu"));
        assert!(!fuzzy("abc", "abd"));
        let mut p = Picker::new(
            "t",
            vec!["dev".into(), "prod".into()],
            vec![],
            PickAction::Env,
            Some(1),
        );
        assert_eq!(p.selected(), Some(1));
        p.filter.set_text("pr");
        p.sel = 0;
        assert_eq!(p.selected(), Some(1));
    }

    #[test]
    fn env_editor_roundtrip() {
        use sendito_store::env::{EnvFile, EnvVar, VarSource};
        let mut env = EnvFile::new("dev");
        env.set(EnvVar {
            name: "a".into(),
            source: VarSource::Plain("1".into()),
        });
        env.set(EnvVar {
            name: "b".into(),
            source: VarSource::Local { key: None },
        });
        env.set(EnvVar {
            name: "c".into(),
            source: VarSource::Keychain {
                key: Some("k".into()),
            },
        });
        env.set(EnvVar {
            name: "d".into(),
            source: VarSource::File { path: "f".into() },
        });
        let mut ed = EnvEditor {
            envs: vec![],
            current: 0,
            table: EnvEditor::table(),
            dirty: false,
            error: None,
        };
        ed.load(&env, |_| None);
        if let Cell::Text(e) = &mut ed.table.rows[1].cells[3] {
            e.set_text("s3cret");
        }
        let (back, secrets) = ed.to_env("dev");
        assert_eq!(back, env);
        assert_eq!(secrets, vec![("b".to_string(), "s3cret".to_string())]);
    }

    #[test]
    fn env_command_lists_real_environments_first() {
        let mut items: Vec<(PaletteItem, String, String)> = COMMANDS
            .iter()
            .map(|(c, d)| (PaletteItem::Command(c, d), c.to_string(), d.to_string()))
            .collect();
        items.push((
            PaletteItem::Env(None),
            "No environment".into(),
            String::new(),
        ));
        items.push((
            PaletteItem::Env(Some("local".into())),
            "local".into(),
            String::new(),
        ));
        let p = Palette {
            input: TextEditor::single(":env"),
            items,
            sel: 0,
            scroll: 0,
            armed: true,
        };
        let v: Vec<PaletteItem> = p.visible().iter().map(|i| p.items[*i].0.clone()).collect();
        assert_eq!(
            v,
            vec![
                PaletteItem::Env(Some("local".into())),
                PaletteItem::Env(None)
            ]
        );
    }

    #[test]
    fn palette_filters() {
        let p = Palette {
            input: TextEditor::single(":en"),
            items: COMMANDS
                .iter()
                .map(|(c, d)| (PaletteItem::Command(c, d), c.to_string(), d.to_string()))
                .collect(),
            sel: 0,
            scroll: 0,
            armed: true,
        };
        let v = p.visible();
        assert!(v.len() >= 2);
    }

    #[test]
    fn commands_match_without_colon() {
        let mut items: Vec<(PaletteItem, String, String)> = vec![(
            PaletteItem::Action(crate::keymap::Action::Help),
            "Show all keys and actions".into(),
            "F1".into(),
        )];
        items.extend(
            COMMANDS
                .iter()
                .map(|(c, d)| (PaletteItem::Command(c, d), c.to_string(), d.to_string())),
        );
        let mut p = Palette {
            input: TextEditor::single("cookie"),
            items,
            sel: 0,
            scroll: 0,
            armed: true,
        };
        let v: Vec<&str> = p.visible().iter().map(|i| p.items[*i].1.as_str()).collect();
        assert!(v.contains(&":cookies"), "{v:?}");
        // The untouched palette still starts with the key list.
        p.input = TextEditor::single("");
        assert_eq!(p.visible().first(), Some(&0));
    }

    #[test]
    fn command_words() {
        assert_eq!(command_base(":saveas <name>"), ("saveas", true));
        assert_eq!(command_base(":theme [name]"), ("theme", false));
        assert_eq!(command_base(":auth login [name]"), ("auth login", false));
        assert_eq!(
            command_base(":cookies delete <number>"),
            ("cookies delete", true)
        );
        assert_eq!(command_base(":send"), ("send", false));
    }
}
