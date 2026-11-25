//! The environment editor overlay.

use super::*;

/// Type labels in the editor. "secret" is the file's `local` source, so it
/// is not confused with an environment named "local". A "key" type names the
/// entry of an existing secret (the file's `key =`) instead of holding it.
pub const SOURCE_NAMES: &[&str] = &[
    "text",
    "secret",
    "secret key",
    "keychain",
    "keychain key",
    "file",
    ".env",
];

/// The types, as indices into [`SOURCE_NAMES`].
pub const TEXT: usize = 0;
pub const SECRET: usize = 1;
pub const SECRET_KEY: usize = 2;
pub const KEYCHAIN: usize = 3;
pub const KEYCHAIN_KEY: usize = 4;
pub const FILE: usize = 5;
pub const DOTENV: usize = 6;

/// Whether a type holds the secret itself, typed masked in Value.
pub fn holds_secret(src: usize) -> bool {
    matches!(src, SECRET | KEYCHAIN)
}

/// The columns of the table.
const NAME: usize = 0;
const TYPE: usize = 1;
const VALUE: usize = 2;

pub struct EnvEditor {
    /// Environment file names; index 0 is the collection-level env.
    pub envs: Vec<String>,
    pub current: usize,
    pub table: Table,
    pub dirty: bool,
    pub error: Option<String>,
    /// The path of the `.env` file (the file's top-level `dotenv` key).
    pub dotenv: TextEditor,
    /// The focus is on the `.env` file line, not on the table.
    pub on_dotenv: bool,
    /// What the `.env` file line says about the file, and whether that is
    /// a problem (`Some(true)`: not found or does not parse; `None`: no
    /// file is needed).
    pub dotenv_status: (String, Option<bool>),
}

impl Default for EnvEditor {
    fn default() -> Self {
        Self {
            envs: Vec::new(),
            current: 0,
            table: Self::table(),
            dirty: false,
            error: None,
            dotenv: Self::dotenv_input(""),
            on_dotenv: false,
            dotenv_status: (String::new(), None),
        }
    }
}

/// The variable a row describes (without the secret value typed in it);
/// `None` for a row without a name.
fn row_var(r: &crate::form::Row) -> Option<sendito_store::env::EnvVar> {
    use sendito_store::env::{EnvVar, VarSource};
    let name = r.cells[NAME].text().trim().to_string();
    if name.is_empty() {
        return None;
    }
    let value = r.cells[VALUE].text();
    let key = Some(value.trim().to_string()).filter(|s| !s.is_empty());
    let source = match r.cells[TYPE].choice() {
        SECRET => VarSource::Local { key: None },
        SECRET_KEY => VarSource::Local { key },
        KEYCHAIN => VarSource::Keychain { key: None },
        KEYCHAIN_KEY => VarSource::Keychain { key },
        FILE => VarSource::File { path: value },
        DOTENV => VarSource::Dotenv { key },
        _ => VarSource::Plain(value),
    };
    Some(EnvVar { name, source })
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
                    title: "Type",
                    kind: ColKind::Choice(SOURCE_NAMES),
                    weight: 1,
                },
                Column {
                    title: "Value",
                    kind: ColKind::Text,
                    weight: 5,
                },
            ],
            false,
        )
    }

    /// The input of the `.env` file line.
    pub fn dotenv_input(path: &str) -> TextEditor {
        let mut e = TextEditor::single(path);
        e.placeholder = ".env (default)".into();
        e
    }

    /// The `.env` path as typed; `None` when empty (the default `.env`).
    pub fn dotenv_value(&self) -> Option<String> {
        Some(self.dotenv.text().trim().to_string()).filter(|s| !s.is_empty())
    }

    /// Loads `env`; `is_set` tells whether a secret, file or `.env` key
    /// has a value (`None` when unknown, e.g. the keychain).
    pub fn load(
        &mut self,
        env: &sendito_store::env::EnvFile,
        is_set: impl Fn(&sendito_store::env::EnvVar) -> Option<bool>,
    ) {
        use sendito_store::env::VarSource;
        let mut t = Self::table();
        for v in &env.vars {
            let (src, value) = match &v.source {
                VarSource::Plain(s) => (TEXT, s.clone()),
                VarSource::Local { key: None } => (SECRET, String::new()),
                VarSource::Local { key: Some(k) } => (SECRET_KEY, k.clone()),
                VarSource::Keychain { key: None } => (KEYCHAIN, String::new()),
                VarSource::Keychain { key: Some(k) } => (KEYCHAIN_KEY, k.clone()),
                VarSource::File { path } => (FILE, path.clone()),
                VarSource::Dotenv { key } => (DOTENV, key.clone().unwrap_or_default()),
            };
            t.push(
                true,
                vec![
                    Cell::Text(TextEditor::single(&v.name)),
                    Cell::Choice(src),
                    Cell::Text(TextEditor::single(&value)),
                ],
            );
        }
        self.table = t;
        self.dotenv = Self::dotenv_input(env.dotenv.as_deref().unwrap_or_default());
        self.on_dotenv = false;
        self.refresh(is_set);
        self.dirty = false;
        self.error = None;
    }

    /// Fits each row to its type: a secret or keychain value is masked, and
    /// the empty cells say what the row holds (`is_set` as in `load`). A
    /// value typed under another type is cleared when the mask changes, so a
    /// secret never shows as text and text never becomes a secret.
    pub fn refresh(&mut self, is_set: impl Fn(&sendito_store::env::EnvVar) -> Option<bool>) {
        for r in &mut self.table.rows {
            let src = r.cells[TYPE].choice();
            let status = row_var(r).and_then(|v| is_set(&v));
            let hint = match (src, status) {
                (SECRET | KEYCHAIN, Some(true)) => "set · type to replace it",
                (SECRET | KEYCHAIN, Some(false)) => "NOT SET · type the value",
                (SECRET | KEYCHAIN, None) => "type the value to store it",
                (SECRET_KEY, _) => "the entry name in secrets.toml",
                (KEYCHAIN_KEY, _) => "the account name in the keychain",
                (FILE, _) => "path/to/file · F2 browses",
                (DOTENV, Some(false)) => "NOT SET in .env (empty = the variable name)",
                (DOTENV, _) => "the .env key (empty = the variable name)",
                _ => "",
            };
            if let Cell::Text(e) = &mut r.cells[VALUE] {
                let mask = holds_secret(src);
                if e.mask != mask {
                    e.set_text("");
                    e.mask = mask;
                }
                e.placeholder = hint.to_string();
            }
        }
    }

    /// The environment as edited, plus secret values to store.
    pub fn to_env(&self, name: &str) -> (sendito_store::env::EnvFile, Vec<(String, String)>) {
        use sendito_store::env::EnvFile;
        let mut env = EnvFile::new(name);
        env.dotenv = self.dotenv_value();
        let mut secrets = Vec::new();
        for r in self.table.values() {
            let Some(var) = row_var(r) else {
                continue;
            };
            let value = r.cells[VALUE].text();
            if holds_secret(r.cells[TYPE].choice()) && !value.is_empty() {
                secrets.push((var.name.clone(), value));
            }
            env.set(var);
        }
        (env, secrets)
    }

    /// The type of the selected row, when the row is a variable.
    pub fn selected_type(&self) -> Option<usize> {
        self.table
            .rows
            .get(self.table.sel_row)
            .and_then(|r| r.cells.get(TYPE))
            .map(|c| c.choice())
    }
}
