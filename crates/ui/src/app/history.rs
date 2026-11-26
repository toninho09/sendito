//! History panel: load, open and save an entry.

use super::*;

impl App {
    // ----- history ------------------------------------------------------

    pub(super) fn toggle_history(&mut self) {
        if self.sidebar_mode == SidebarMode::History && matches!(self.focus, Focus::Sidebar) {
            self.sidebar_mode = SidebarMode::Tree;
            return;
        }
        self.sidebar_mode = SidebarMode::History;
        self.layout.sidebar_visible = true;
        self.focus = Focus::Sidebar;
        self.load_history();
    }

    pub fn load_history(&mut self) {
        let path = sendito_store::history::history_path(&self.project.root);
        let max = self.project.config.history.max_entries.max(1);
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let h = tokio::task::spawn_blocking(move || {
                sendito_store::history::load(&path, max).unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            let _ = tx.send(AppMsg::History(h));
        });
    }

    pub(super) fn open_history(&mut self, i: usize, resend: bool) {
        let Some(e) = self.history.as_ref().and_then(|h| h.get(i)).cloned() else {
            return;
        };
        match sendito_store::request_file::parse(&e.request) {
            Ok(req) => {
                self.new_tab(req);
                if let Some(t) = self.tab_mut() {
                    t.name = e.name.clone();
                    let when = e.ts.get(11..19).unwrap_or(&e.ts);
                    t.origin = Some(format!("from history ({when})"));
                    t.origin_path = e.path.clone();
                    // Unchanged from the history entry: not an unsaved edit.
                    t.set_saved(Some(t.model.clone()));
                }
                if !resend {
                    self.show_history_response(&e);
                }
                if resend {
                    self.status = None;
                    self.send(false);
                    if self.status.is_none() {
                        let when = e.ts.get(11..19).unwrap_or(&e.ts);
                        self.set_status(
                            format!("resent the history entry of {when} ({})", e.name),
                            StatusKind::Info,
                        );
                    }
                }
            }
            Err(err) => self.set_status(
                format!("cannot read history entry: {err}"),
                StatusKind::Error,
            ),
        }
    }

    /// Shows the response kept with a history entry in the active tab.
    fn show_history_response(&mut self, e: &HistoryEntry) {
        let Some(id) = &e.response else { return };
        let path = sendito_store::history::history_path(&self.project.root);
        match sendito_store::history::load_response(&path, id) {
            Ok(ex) => {
                let when = e.ts.get(11..19).unwrap_or(&e.ts);
                let label = if e.size > ex.body.len() as u64 {
                    format!("{when} · body cut")
                } else {
                    when.to_string()
                };
                let resp = Response::from_stored(ex, crate::tab::Stored::History(label));
                if let Some(t) = self.tab_mut() {
                    t.response = Some(resp);
                    t.resp_tab = 0;
                }
            }
            Err(err) => self.set_status(
                format!("the response of this history entry did not load: {err}"),
                StatusKind::Warn,
            ),
        }
    }

    pub(super) fn save_history_prompt(&mut self, i: usize) {
        let Some(e) = self.history.as_ref().and_then(|h| h.get(i)).cloned() else {
            return;
        };
        let folders = self.tree.all_folders();
        let labels: Vec<String> = folders
            .iter()
            .map(|f| {
                if f.is_empty() {
                    "/ (collection root)".into()
                } else {
                    format!("/{f}")
                }
            })
            .collect();
        // The entry's own folder, else the folder selected in the tree (as for
        // new requests and folders).
        let target = self.tree.target_folder();
        let idx = e
            .path
            .as_deref()
            .map(parent_rel)
            .and_then(|p| folders.iter().position(|f| f == p))
            .or_else(|| folders.iter().position(|f| *f == target))
            .unwrap_or(0);
        // The original request usually still exists: offer a free name, and
        // say why it differs.
        let folder = folders.get(idx).cloned().unwrap_or_default();
        let exists = |n: &str| {
            self.project
                .root
                .join(sendito_store::collection::join_rel(
                    &folder,
                    &format!("{n}.toml"),
                ))
                .exists()
        };
        let mut name = e.name.clone();
        let mut note = None;
        if exists(&name) {
            let mut k = 2;
            while exists(&format!("{} {k}", e.name)) {
                k += 1;
            }
            name = format!("{} {k}", e.name);
            // No-break spaces keep each name on one line when the note wraps.
            let nb = |t: &str| t.replace(' ', "\u{a0}");
            note = Some(format!(
                "\"{}\" exists in /{}; suggested name \"{}\" is selected: typing replaces it.",
                nb(&e.name),
                nb(&folder),
                nb(&name)
            ));
        }
        let mut input = TextEditor::single(&name);
        input.move_to_end();
        if note.is_some() {
            // A suggested name: selected, so typing replaces it.
            input.select_all();
        }
        let mut prompt = Prompt::new(
            "Save request (from history)",
            "Name",
            input,
            vec![("In folder".into(), labels, idx)],
            PromptAction::SaveHistory(Box::new(e)),
        );
        prompt.note = note;
        self.overlay = Some(Overlay::Prompt(prompt));
    }
}
