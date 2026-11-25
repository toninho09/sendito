//! Collection tree operations: open, new, rename, duplicate, delete and move.

use super::*;
use crate::tree::repath;

impl App {
    // ----- tree operations ----------------------------------------------

    pub(super) fn tree_open(&mut self) {
        let Some(row) = self.tree.selected_row().cloned() else {
            return;
        };
        match row.kind {
            RowKind::Folder { .. } => {
                if self.tree.filter.is_empty() {
                    self.tree.toggle(&row.rel);
                    self.session_dirty = true;
                }
            }
            RowKind::Request { .. } => self.open_request(&row.rel),
            RowKind::Section => {}
        }
    }

    /// Folder choice row: all folders, the tree's target folder selected.
    pub(super) fn folder_choice(&self) -> (String, Vec<String>, usize) {
        let folders = self.tree.all_folders();
        let target = self.tree.target_folder();
        let idx = folders.iter().position(|f| *f == target).unwrap_or(0);
        let labels = folders
            .iter()
            .map(|f| {
                if f.is_empty() {
                    "/ (collection root)".to_string()
                } else {
                    format!("/{f}")
                }
            })
            .collect();
        ("In folder".into(), labels, idx)
    }

    pub(super) fn prompt_new_request(&mut self) {
        let folder = self.tree.target_folder();
        self.overlay = Some(Overlay::Prompt(Prompt::new(
            "New request",
            "Name",
            TextEditor::single(""),
            vec![
                (
                    "Type".into(),
                    vec![
                        "HTTP".into(),
                        "gRPC".into(),
                        "WebSocket".into(),
                        "MCP".into(),
                        "SSE".into(),
                        "MQTT".into(),
                    ],
                    0,
                ),
                self.folder_choice(),
            ],
            PromptAction::NewRequest { folder },
        )));
    }

    pub(super) fn prompt_new_folder(&mut self) {
        let folder = self.tree.target_folder();
        self.overlay = Some(Overlay::Prompt(Prompt::new(
            "New folder",
            "Name",
            TextEditor::single(""),
            vec![self.folder_choice()],
            PromptAction::NewFolder { folder },
        )));
    }

    pub(super) fn prompt_rename(&mut self) {
        let Some(row) = self.tree.selected_row().cloned() else {
            return;
        };
        let mut input = TextEditor::single(&row.name);
        input.select_all();
        self.overlay = Some(Overlay::Prompt(Prompt::new(
            &format!(
                "Rename {}",
                if row.is_folder() { "folder" } else { "request" }
            ),
            "New name",
            input,
            vec![],
            PromptAction::Rename(row.rel),
        )));
    }

    pub(super) fn tree_duplicate(&mut self) {
        let Some(rel) = self.tree.selected_rel() else {
            return;
        };
        match self.project.coll.duplicate(&rel) {
            Ok(new_rel) => {
                self.rescan();
                self.tree.select_rel(&new_rel);
                self.set_status(
                    format!("duplicated as {}", name_of(&new_rel)),
                    StatusKind::Ok,
                );
            }
            Err(e) => self.set_status(e.to_string(), StatusKind::Error),
        }
    }

    pub(super) fn tree_delete(&mut self) {
        let Some(row) = self.tree.selected_row().cloned() else {
            return;
        };
        let what = if row.is_folder() {
            "folder and everything in it"
        } else {
            "request"
        };
        self.overlay = Some(Overlay::Confirm(Confirm {
            title: "Delete".into(),
            message: format!(
                "Delete the {what} \"{}\"? This removes it from disk.",
                row.name
            ),
            buttons: vec!["Delete".into(), "Cancel".into()],
            sel: 1,
            action: ConfirmAction::Delete(row.rel),
        }));
    }

    pub(super) fn tree_move(&mut self) {
        let Some(rel) = self.tree.selected_rel() else {
            return;
        };
        let folders: Vec<String> = self
            .tree
            .all_folders()
            .into_iter()
            .filter(|f| *f != rel && !f.starts_with(&format!("{rel}/")) && f != parent_rel(&rel))
            .collect();
        if folders.is_empty() {
            self.set_status(
                "there is no other folder to move to (create one with N)",
                StatusKind::Info,
            );
            return;
        }
        let labels = folders
            .iter()
            .map(|f| {
                if f.is_empty() {
                    "/ (collection root)".to_string()
                } else {
                    format!("/{f}")
                }
            })
            .collect();
        self.overlay = Some(Overlay::Picker(Picker {
            details: folders,
            ..Picker::new(
                &format!("Move \"{}\" to", name_of(&rel)),
                labels,
                vec![],
                PickAction::MoveTo(rel),
                None,
            )
        }));
    }

    /// Moves a node, updating open tabs that point into it.
    pub fn move_node(&mut self, rel: &str, folder: &str) {
        let was_folder = self.tree.root.find_folder(rel).is_some();
        match self.project.coll.move_to(rel, folder) {
            Ok(new_rel) => {
                self.repath_tabs(rel, &new_rel, was_folder);
                self.rescan();
                self.tree.select_rel(&new_rel);
                self.set_status(format!("moved to {new_rel}"), StatusKind::Ok);
            }
            Err(e) => self.set_status(e.to_string(), StatusKind::Error),
        }
    }

    /// Toggles the favorite of the tree's selected request, or with another
    /// focus, of the active tab's request.
    pub(super) fn toggle_favorite(&mut self) {
        let rel = if self.focus == Focus::Sidebar && self.sidebar_mode == SidebarMode::Tree {
            match self.tree.selected_row() {
                Some(r) if r.is_folder() => {
                    self.set_status("only requests can be favorites", StatusKind::Info);
                    return;
                }
                r => r.map(|r| r.rel.clone()),
            }
        } else {
            match self.tab() {
                Some(t) if t.folder.is_some() => {
                    self.set_status("only requests can be favorites", StatusKind::Info);
                    return;
                }
                Some(t) if t.path.is_none() => {
                    self.set_status("save the request first", StatusKind::Info);
                    return;
                }
                t => t.and_then(|t| t.path.clone()),
            }
        };
        let Some(rel) = rel else { return };
        let msg = if self.tree.toggle_favorite(&rel) {
            "added to favorites"
        } else {
            "removed from favorites"
        };
        self.session_dirty = true;
        self.set_status(format!("{msg}: {}", name_of(&rel)), StatusKind::Ok);
    }

    /// Moves the paths of open tabs and favorites into a renamed or moved
    /// node. `folder` also moves the folder a folder tab edits.
    pub(super) fn repath_tabs(&mut self, old: &str, new: &str, folder: bool) {
        for t in &mut self.tabs {
            if let Some(p) = t.path.as_deref().and_then(|p| repath(p, old, new)) {
                if t.path.as_deref() == Some(old) {
                    t.name = name_of(new).to_string();
                }
                t.path = Some(p);
            }
            if folder && let Some(f) = t.folder.as_deref().and_then(|f| repath(f, old, new)) {
                t.folder = Some(f);
            }
        }
        self.tree.repath_favorites(old, new);
        self.session_dirty = true;
    }
}
