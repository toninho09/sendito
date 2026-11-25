//! The environment editor overlay.

use super::*;

/// Source labels in the editor, in the order of the file's `source` values
/// (plain, "local", "keychain", "file", "dotenv"). "local" reads as "secrets"
/// here so it is not confused with an environment named "local".
pub const SOURCE_NAMES: &[&str] = &["plain", "secrets", "keychain", "file", "dotenv"];

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
        let mut t = Table::new(
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
        );
        // Rows added through the trailing "add" row must mask secrets too.
        t.masked_cols = vec![3];
        t
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
                VarSource::Dotenv { key } => (4, key.clone().unwrap_or_default()),
            };
            let hint = match (src, is_set(v)) {
                (1 | 2, Some(true)) => "set",
                (1 | 2, Some(false)) => "NOT SET",
                (1 | 2, None) => "type to set",
                (3, Some(false)) => "file not found",
                (3, _) => "from the file",
                (4, Some(false)) => "NOT SET",
                (4, _) => "from .env",
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

    /// The environment as edited, plus secret values to store. The editor
    /// does not show the `dotenv` path: the caller keeps the file's value.
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
                4 => VarSource::Dotenv { key: opt(value) },
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
