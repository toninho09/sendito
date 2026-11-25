//! Environment picker and editor.

use super::*;

impl App {
    // ----- environment --------------------------------------------------

    pub(super) fn open_env_picker(&mut self) {
        self.env_list = self.project.envs.list();
        let mut items = vec!["(no environment)".to_string()];
        items.extend(self.env_list.iter().cloned());
        items.push("Manage environments…".into());
        let cur = match &self.active_env {
            Some(e) => self.env_list.iter().position(|x| x == e).map(|i| i + 1),
            None => Some(0),
        };
        // How many variables each environment holds.
        let mut details = vec![String::new()];
        details.extend(self.env_list.iter().map(|name| {
            match self.project.envs.load(name).map(|e| e.vars.len()) {
                Ok(1) => "1 variable".to_string(),
                Ok(n) => format!("{n} variables"),
                Err(_) => "cannot be read".to_string(),
            }
        }));
        details.push("Ctrl+W V".into());
        self.overlay = Some(Overlay::Picker(Picker::new(
            "Environment",
            items,
            details,
            PickAction::Env,
            cur,
        )));
    }

    pub fn set_env(&mut self, env: Option<String>) {
        self.active_env = env;
        // A watch re-resolves variables at every send; a new environment makes
        // that a different request, so watches stop.
        let watched = self.tabs.iter_mut().filter_map(|t| t.watch.take()).count();
        self.refresh_names();
        self.session_dirty = true;
        // The status line and the tab bar show the new environment; an older
        // confirmation would now be stale.
        self.clear_transient_status();
        if watched > 0 {
            self.set_status("watch off (environment changed)", StatusKind::Info);
        }
    }

    pub fn open_env_editor(&mut self) {
        self.clear_transient_status();
        self.env_list = self.project.envs.list();
        let mut envs = vec![COLLECTION_ENV.to_string()];
        envs.extend(self.env_list.iter().cloned());
        let current = self
            .active_env
            .as_ref()
            .and_then(|e| envs.iter().position(|x| x == e))
            .unwrap_or(0);
        let mut ed = EnvEditor {
            envs,
            current,
            ..EnvEditor::default()
        };
        self.load_env_into(&mut ed);
        self.overlay = Some(Overlay::Env(ed));
    }

    /// Fits the rows to their types again (after a type or the `.env` path
    /// changed): masks, what each empty cell says and the `.env` status.
    pub(super) fn refresh_env_rows(&self, ed: &mut EnvEditor) {
        let name = ed.envs.get(ed.current).cloned().unwrap_or_default();
        let mut env = self
            .project
            .envs
            .load(&name)
            .unwrap_or_else(|_| sendito_store::env::EnvFile::new(&name));
        env.dotenv = ed.dotenv_value();
        // The status counts the `.env` rows as edited, not as saved.
        env.vars = ed.to_env(&name).0.vars;
        ed.refresh(|v| self.project.envs.is_set(&env, v));
        ed.dotenv_status = self.dotenv_status(&env);
    }

    /// What the `.env` file line says: whether the file of `env` exists and
    /// how many keys it holds. `Some(true)` is a problem; `None` is neutral
    /// (no path and no `.env` variable, so no file is needed).
    fn dotenv_status(&self, env: &sendito_store::env::EnvFile) -> (String, Option<bool>) {
        if !self.project.envs.dotenv_path(env).is_file() {
            let used = env.dotenv.is_some()
                || env
                    .vars
                    .iter()
                    .any(|v| matches!(v.source, sendito_store::env::VarSource::Dotenv { .. }));
            return if used {
                ("✗ not found".into(), Some(true))
            } else {
                ("no file".into(), None)
            };
        }
        match self.project.envs.dotenv_keys(env) {
            Ok(1) => ("✓ found · 1 key".into(), Some(false)),
            Ok(n) => (format!("✓ found · {n} keys"), Some(false)),
            Err(e) => (format!("✗ {e}"), Some(true)),
        }
    }

    pub(super) fn load_env_into(&self, ed: &mut EnvEditor) {
        let name = ed.envs.get(ed.current).cloned().unwrap_or_default();
        match self.project.envs.load(&name) {
            Ok(env) => {
                ed.load(&env, |v| self.project.envs.is_set(&env, v));
                ed.dotenv_status = self.dotenv_status(&env);
            }
            Err(e) => {
                // Never show (and so never save) another environment's rows.
                ed.table = EnvEditor::table();
                ed.dotenv = EnvEditor::dotenv_input("");
                ed.on_dotenv = false;
                ed.dotenv_status = (String::new(), None);
                ed.dirty = false;
                ed.error = Some(e.to_string());
            }
        }
    }

    pub(super) fn save_env_editor(&mut self, ed: &mut EnvEditor) -> bool {
        let name = ed.envs.get(ed.current).cloned().unwrap_or_default();
        // A file that does not parse was never loaded: saving would replace
        // it with what the editor shows.
        match self.project.envs.load(&name) {
            // A deleted file is written again.
            Ok(_) => {}
            Err(sendito_store::StoreError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                ed.error = Some(format!("not saved: fix the file first ({e})"));
                return false;
            }
        }
        // `to_env` keeps the last of two rows with one name: say so instead.
        let mut seen = std::collections::HashSet::new();
        for r in ed.table.values() {
            let var = r.cells[0].text().trim().to_string();
            if !var.is_empty() && !seen.insert(var.clone()) {
                ed.error = Some(format!("{var}: two variables have this name"));
                return false;
            }
        }
        let (env, secrets) = ed.to_env(&name);
        if let Err(e) = self.project.envs.save(&env) {
            ed.error = Some(e.to_string());
            return false;
        }
        for (var, value) in secrets {
            if let Some(v) = env.get(&var)
                && let Err(e) = self.project.envs.store_secret(&name, v, &value)
            {
                ed.error = Some(format!("{var}: {e}"));
                return false;
            }
        }
        self.load_env_into(ed);
        self.refresh_names();
        let label = if name == COLLECTION_ENV {
            "collection variables".to_string()
        } else {
            name
        };
        self.set_status(format!("saved {label}"), StatusKind::Ok);
        true
    }

    /// F2 in the environment editor: a file picker for the `.env` file line
    /// or for the Value of the selected `file` row. The editor waits behind
    /// it and comes back with the chosen path, or unchanged on Esc.
    pub(super) fn open_env_file_picker(&mut self, ed: EnvEditor) {
        let current = if ed.on_dotenv {
            ed.dotenv.text()
        } else {
            ed.table
                .rows
                .get(ed.table.sel_row)
                .map(|r| r.cells[2].text())
                .unwrap_or_default()
        };
        let root = self.project.root.clone();
        let p = sendito_store::env::expand(&root, current.trim());
        let dir = if current.trim().is_empty() {
            root
        } else if p.is_dir() {
            p
        } else {
            p.parent()
                .filter(|d| d.is_dir())
                .map(|d| d.to_path_buf())
                .unwrap_or(root)
        };
        let dotenv = ed.on_dotenv;
        self.parked_env = Some(ed);
        self.show_file_picker(dir, dotenv);
    }

    /// The file picker on `dir`: `../`, then the folders, then the files.
    pub(super) fn show_file_picker(&mut self, dir: PathBuf, dotenv: bool) {
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if e.path().is_dir() {
                dirs.push(format!("{name}/"));
            } else {
                files.push(name);
            }
        }
        dirs.sort_by_key(|n| n.to_lowercase());
        files.sort_by_key(|n| n.to_lowercase());
        let mut items = Vec::new();
        if dir.parent().is_some() {
            items.push("../".to_string());
        }
        items.extend(dirs);
        items.extend(files);
        let details = vec![String::new(); items.len()];
        let title = format!("Choose a file · {}", self.path_value(&dir));
        self.overlay = Some(Overlay::Picker(Picker::new(
            &title,
            items,
            details,
            PickAction::EnvFile { dir, dotenv },
            None,
        )));
    }

    /// A picked item of the file picker: a folder opens it; a file goes
    /// into the environment editor.
    pub(super) fn pick_env_file(&mut self, dir: &Path, dotenv: bool, item: &str) {
        if item == "../" {
            let up = dir.parent().map(Path::to_path_buf).unwrap_or_default();
            return self.show_file_picker(up, dotenv);
        }
        if let Some(name) = item.strip_suffix('/') {
            return self.show_file_picker(dir.join(name), dotenv);
        }
        let Some(mut ed) = self.parked_env.take() else {
            return;
        };
        let value = self.path_value(&dir.join(item));
        if dotenv {
            ed.dotenv.set_text(&value);
            ed.dotenv.move_to_end();
        } else if let Some(crate::form::Cell::Text(e)) = ed
            .table
            .rows
            .get_mut(ed.table.sel_row)
            .and_then(|r| r.cells.get_mut(2))
        {
            e.set_text(&value);
        }
        ed.dirty = true;
        self.refresh_env_rows(&mut ed);
        self.overlay = Some(Overlay::Env(ed));
    }

    /// A path as an environment file writes it: relative to the collection
    /// when inside it (`.` for the root), else with `~` for the home folder.
    fn path_value(&self, p: &Path) -> String {
        match p.strip_prefix(&self.project.root) {
            Ok(rel) if rel.as_os_str().is_empty() => ".".into(),
            Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
            Err(_) => display_path(p, self.home.as_deref()),
        }
    }

    /// "Define" in the undefined-variables dialog: the environment editor
    /// of the active environment (else the collection variables) with a row
    /// for each name, ready for its value.
    pub(super) fn define_variables(&mut self, names: &[String]) {
        self.open_env_editor();
        let Some(Overlay::Env(ed)) = &mut self.overlay else {
            return;
        };
        let first = ed.table.rows.len();
        for n in names {
            let mut row = ed.table.new_row();
            if let Some(crate::form::Cell::Text(e)) = row.cells.get_mut(0) {
                e.set_text(n);
            }
            ed.table.rows.push(row);
        }
        ed.table.select(first, 2);
        ed.dirty = true;
        self.set_status(
            format!("type the value of {} and save (Ctrl+S)", names.join(", ")),
            StatusKind::Info,
        );
    }

    /// The environment a moved secret goes to: the active one, else the
    /// collection variables.
    fn secret_env(&self) -> String {
        self.active_env
            .clone()
            .unwrap_or_else(|| COLLECTION_ENV.to_string())
    }

    /// `Ctrl+W K`: asks the name of the secret that takes the plain value of
    /// the focused credential field.
    pub(super) fn prompt_move_to_secret(&mut self) {
        let field = self
            .tab()
            .filter(|_| self.focus == Focus::ReqBody)
            .and_then(|t| t.current_field())
            .filter(|f| f.is_secret());
        let Some(field) = field else {
            self.set_status(
                "select a password, token, key or secret field first",
                StatusKind::Info,
            );
            return;
        };
        let value = self
            .tab_mut()
            .and_then(|t| t.ed.text_mut(field))
            .map(|e| e.text())
            .unwrap_or_default();
        if !crate::tab::plain_secret(&value) {
            self.set_status(
                "the field is empty or already a {{variable}}",
                StatusKind::Info,
            );
            return;
        }
        let mut input = TextEditor::single(secret_name(field));
        input.select_all();
        let env = self.secret_env();
        let mut prompt = Prompt::new(
            "Move to secret",
            "Name",
            input,
            Vec::new(),
            PromptAction::MoveToSecret(field),
        );
        prompt.note = Some(format!(
            "The value goes to .sendito/secrets.toml (never committed) for {}; the field becomes {{{{name}}}}.",
            env_label(&env)
        ));
        self.overlay = Some(Overlay::Prompt(prompt));
    }

    /// Stores the plain value of `field` as the secret `name` of the active
    /// environment (`local` source) and writes `{{name}}` in the field.
    pub fn move_to_secret(&mut self, field: Field, name: &str) -> Result<String, String> {
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err("a name uses letters, digits, _ and - only".into());
        }
        let value = self
            .tab_mut()
            .and_then(|t| t.ed.text_mut(field))
            .map(|e| e.text())
            .ok_or("no request is open")?;
        let env_name = self.secret_env();
        let mut env = self
            .project
            .envs
            .load(&env_name)
            .map_err(|e| format!("fix the environment file first: {e}"))?;
        if env.get(name).is_some() {
            return Err(format!(
                "{name} is already defined in {}: choose another name",
                env_label(&env_name)
            ));
        }
        let var = sendito_store::env::EnvVar {
            name: name.to_string(),
            source: sendito_store::env::VarSource::Local { key: None },
        };
        // The secret first: a failed write leaves the environment file as it was.
        self.project
            .envs
            .store_secret(&env_name, &var, &value)
            .map_err(|e| e.to_string())?;
        env.set(var);
        self.project.envs.save(&env).map_err(|e| e.to_string())?;
        if let Some(ed) = self.tab_mut().and_then(|t| t.ed.text_mut(field)) {
            ed.set_text(&format!("{{{{{name}}}}}"));
        }
        self.changed();
        self.refresh_names();
        Ok(format!(
            "moved to the secret {name} of {}; save the request to drop the plain value from its file",
            env_label(&env_name)
        ))
    }
}

/// A suggested secret name for a credential field.
fn secret_name(field: Field) -> &'static str {
    match field {
        Field::AuthToken => "token",
        Field::AuthValue => "api_key",
        Field::AwsSecretKey => "aws_secret_access_key",
        Field::AwsSessionToken => "aws_session_token",
        Field::JwtSecret => "jwt_secret",
        Field::MqttPass => "mqtt_password",
        _ => "password",
    }
}

/// An environment as the interface names it.
fn env_label(name: &str) -> String {
    if name == COLLECTION_ENV {
        "the collection variables".to_string()
    } else {
        format!("\"{name}\"")
    }
}
