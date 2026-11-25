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
        self.overlay = Some(Overlay::Picker(Picker::new(
            "Environment",
            items,
            vec![],
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
            table: EnvEditor::table(),
            dirty: false,
            error: None,
        };
        self.load_env_into(&mut ed);
        self.overlay = Some(Overlay::Env(ed));
    }

    pub(super) fn load_env_into(&self, ed: &mut EnvEditor) {
        let name = ed.envs.get(ed.current).cloned().unwrap_or_default();
        match self.project.envs.load(&name) {
            Ok(env) => ed.load(&env, |v| self.project.envs.is_set(&env, v)),
            Err(e) => {
                // Never show (and so never save) another environment's rows.
                ed.table = EnvEditor::table();
                ed.dirty = false;
                ed.error = Some(e.to_string());
            }
        }
    }

    pub(super) fn save_env_editor(&mut self, ed: &mut EnvEditor) -> bool {
        let name = ed.envs.get(ed.current).cloned().unwrap_or_default();
        // A file that does not parse was never loaded: saving would replace
        // it with what the editor shows.
        // The editor does not show the `dotenv` path: keep the file's value.
        let dotenv = match self.project.envs.load(&name) {
            Ok(old) => old.dotenv,
            // A deleted file is written again.
            Err(sendito_store::StoreError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                None
            }
            Err(e) => {
                ed.error = Some(format!("not saved: fix the file first ({e})"));
                return false;
            }
        };
        // `to_env` keeps the last of two rows with one name: say so instead.
        let mut seen = std::collections::HashSet::new();
        for r in ed.table.values() {
            let var = r.cells[0].text().trim().to_string();
            if !var.is_empty() && !seen.insert(var.clone()) {
                ed.error = Some(format!("{var}: two variables have this name"));
                return false;
            }
        }
        let (mut env, secrets) = ed.to_env(&name);
        env.dotenv = dotenv;
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
}
