//! Modal overlays: palette, help, settings, confirm, prompt, picker, env
//! editor, import, report.

use crate::editor::TextEditor;
use crate::form::{Cell, ColKind, Column, Table};
use crate::viewer::Viewer;

/// What to do when a confirm dialog's first button is chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmAction {
    SendAnyway,
    Delete(String),
    Quit,
    /// Close tab: button 0 = save, 1 = discard.
    CloseTab(u64),
    /// Env editor closing with changes: 0 = save, 1 = discard.
    EnvClose,
    ClearHistory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptAction {
    NewRequest { folder: String },
    NewFolder { folder: String },
    Rename(String),
    SaveAs { tab: u64 },
    SaveBody,
    NewEnv,
    SaveHistory(usize),
    CustomMethod,
    Search,
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
}

pub struct Prompt {
    pub title: String,
    pub label: String,
    pub input: TextEditor,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteItem {
    Action(crate::keymap::Action),
    Open(String),
    Env(Option<String>),
    Command(&'static str, &'static str),
}

pub struct Palette {
    pub input: TextEditor,
    pub items: Vec<(PaletteItem, String, String)>,
    pub sel: usize,
    pub scroll: usize,
    /// A row is highlighted and Enter runs it. Always true for the app's
    /// palette (its untouched first row is harmless, see `visible`).
    pub armed: bool,
}

pub const COMMANDS: &[(&str, &str)] = &[
    (":send", "Send the current request"),
    (":cancel", "Cancel the running request"),
    (":end", "Finish sending on a client/bidi stream"),
    (
        ":env <name>",
        "Switch environment (names listed as you type)",
    ),
    (":envs", "Edit environments"),
    (":w", "Save the request"),
    (":saveas <name>", "Save the request under a new name"),
    (":wq", "Save and quit"),
    (":q", "Quit (asks when there are unsaved changes)"),
    (":q!", "Quit without saving"),
    (":new", "New HTTP request (Ctrl+N)"),
    (":newgrpc", "New gRPC request (Ctrl+W g)"),
    (":newmcp", "New MCP request"),
    (":mcpservers", "Choose an MCP server"),
    (":mcptools", "Discover and choose an MCP tool"),
    (
        ":import [path]",
        "Import a Postman collection (path) or paste a curl command",
    ),
    (
        ":curl",
        "Copy the request as curl (secret variables stay {{placeholders}})",
    ),
    (":curl!", "Copy the request as curl including secret values"),
    (":history", "Show history"),
    (":clearhistory", "Delete all history entries"),
    (":format", "Pretty-print the JSON body"),
    (
        ":template",
        "Fill the gRPC message with a template from the schema",
    ),
    (":reload", "Reload the gRPC schema"),
    (":settings", "Open settings"),
    (
        ":theme <name>",
        "Switch theme; :theme reload reads your theme files again",
    ),
    (":help", "Show keys"),
    (
        ":report",
        "Show the last report (import, curl export) again",
    ),
    (":savebody [path]", "Save the response body to a file"),
];

impl Palette {
    pub fn visible(&self) -> Vec<usize> {
        let q = self.input.text().to_lowercase();
        let q = q.trim();
        if let Some(cmd) = q.strip_prefix(':') {
            // ":env" and ":env <prefix>" list the matching environments, real
            // ones first; "no environment" comes last.
            if cmd == "env" || cmd.starts_with("env ") {
                let arg = cmd[3..].trim();
                let mut v: Vec<usize> = self
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, (it, _, _))| {
                        matches!(it, PaletteItem::Env(Some(name)) if name.to_lowercase().starts_with(arg))
                    })
                    .map(|(i, _)| i)
                    .collect();
                if arg.is_empty() {
                    v.extend(
                        self.items
                            .iter()
                            .position(|(it, _, _)| *it == PaletteItem::Env(None)),
                    );
                }
                return v;
            }
            let word = cmd.split_whitespace().next().unwrap_or("");
            return self
                .items
                .iter()
                .enumerate()
                .filter(|(_, (it, label, _))| {
                    matches!(it, PaletteItem::Command(..)) && label[1..].starts_with(word)
                })
                .map(|(i, _)| i)
                .collect();
        }
        let mut v: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, (it, label, detail))| {
                !matches!(it, PaletteItem::Command(..))
                    && (q.is_empty()
                        || fuzzy(&label.to_lowercase(), q)
                        || fuzzy(&detail.to_lowercase(), q))
            })
            .map(|(i, _)| i)
            .collect();
        // The untouched palette highlights its first row, so that row must be
        // harmless: the key list, never an action that sends or deletes.
        if q.is_empty()
            && let Some(pos) = v
                .iter()
                .position(|i| self.items[*i].0 == PaletteItem::Action(crate::keymap::Action::Help))
        {
            let help = v.remove(pos);
            v.insert(0, help);
        }
        v
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingId {
    Vim,
    Theme,
    Icons,
    Sidebar,
    Mouse,
    ConfirmQuit,
    History,
}

pub const SETTINGS: &[(SettingId, &str)] = &[
    (SettingId::Theme, "Theme"),
    (SettingId::Icons, "Icons"),
    (SettingId::Vim, "Vim mode"),
    (SettingId::Sidebar, "Sidebar position"),
    (SettingId::Mouse, "Mouse support"),
    (SettingId::ConfirmQuit, "Confirm quit with unsaved changes"),
    (SettingId::History, "Record history"),
];

/// Source labels in the editor, in the order of the file's `source` values
/// (plain, "local", "keychain", "file"). "local" reads as "secrets" here so it
/// is not confused with an environment named "local".
pub const SOURCE_NAMES: &[&str] = &["plain", "secrets", "keychain", "file"];

pub struct EnvEditor {
    /// Environment file names; index 0 is the collection-level env.
    pub envs: Vec<String>,
    pub current: usize,
    pub table: Table,
    pub dirty: bool,
    pub error: Option<String>,
}

impl EnvEditor {
    pub fn table() -> Table {
        Table::new(
            vec![
                Column {
                    title: "Variable",
                    kind: ColKind::Text,
                    weight: 2,
                },
                Column {
                    title: "Source",
                    kind: ColKind::Choice(SOURCE_NAMES),
                    weight: 1,
                },
                Column {
                    title: "Value, key or path",
                    kind: ColKind::Text,
                    weight: 3,
                },
                Column {
                    title: "Secret value",
                    kind: ColKind::Text,
                    weight: 2,
                },
            ],
            false,
        )
    }

    /// Loads `env`; `is_set` tells whether a secret already has a value
    /// (`None` when unknown, e.g. the keychain).
    pub fn load(
        &mut self,
        env: &sendito_store::env::EnvFile,
        is_set: impl Fn(&sendito_store::env::EnvVar) -> Option<bool>,
    ) {
        use sendito_store::env::VarSource;
        let mut t = Self::table();
        for v in &env.vars {
            let (src, value) = match &v.source {
                VarSource::Plain(s) => (0, s.clone()),
                VarSource::Local { key } => (1, key.clone().unwrap_or_default()),
                VarSource::Keychain { key } => (2, key.clone().unwrap_or_default()),
                VarSource::File { path } => (3, path.clone()),
            };
            let hint = match (src, is_set(v)) {
                (1 | 2, Some(true)) => "set",
                (1 | 2, Some(false)) => "NOT SET",
                (1 | 2, None) => "type to set",
                (3, Some(false)) => "file not found",
                (3, _) => "from the file",
                _ => "",
            };
            let mut secret = TextEditor::single("").with_placeholder(hint);
            secret.mask = true;
            t.push(
                true,
                vec![
                    Cell::Text(TextEditor::single(&v.name)),
                    Cell::Choice(src),
                    Cell::Text(TextEditor::single(&value)),
                    Cell::Text(secret),
                ],
            );
        }
        self.table = t;
        self.dirty = false;
        self.error = None;
    }

    /// The environment as edited, plus secret values to store.
    pub fn to_env(&self, name: &str) -> (sendito_store::env::EnvFile, Vec<(String, String)>) {
        use sendito_store::env::{EnvFile, EnvVar, VarSource};
        let mut env = EnvFile::new(name);
        let mut secrets = Vec::new();
        for r in self.table.values() {
            let var = r.cells[0].text().trim().to_string();
            if var.is_empty() {
                continue;
            }
            let value = r.cells[2].text();
            let opt = |s: String| {
                if s.trim().is_empty() {
                    None
                } else {
                    Some(s.trim().to_string())
                }
            };
            let source = match r.cells[1].choice() {
                1 => VarSource::Local { key: opt(value) },
                2 => VarSource::Keychain { key: opt(value) },
                3 => VarSource::File { path: value },
                _ => VarSource::Plain(value),
            };
            let secret = r.cells[3].text();
            if !secret.is_empty()
                && matches!(source, VarSource::Local { .. } | VarSource::Keychain { .. })
            {
                secrets.push((var.clone(), secret));
            }
            env.set(EnvVar { name: var, source });
        }
        (env, secrets)
    }
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
    pub error: Option<String>,
}

pub enum Overlay {
    Palette(Palette),
    Help { viewer: Viewer, filter: TextEditor },
    Settings { sel: usize },
    Confirm(Confirm),
    Prompt(Prompt),
    Picker(Picker),
    Env(EnvEditor),
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
}
