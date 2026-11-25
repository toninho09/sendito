//! Folder settings (`folder.toml`) in the interface: the cache the drawing
//! and the checks read, and what a tab inherits.

use sendito_core::inherit::ChainItem;

use crate::tab::folder_request;
use sendito_store::collection::folders_above;

use super::*;

impl App {
    /// Reads every folder's settings into the cache. A folder whose
    /// `folder.toml` cannot be read is left out and named in the status line;
    /// a send from below it reports the error again.
    pub(super) fn load_folders(&mut self) {
        let mut rels = Vec::new();
        collect_folders(&self.tree.root, &mut rels);
        self.folders.clear();
        let mut errors = Vec::new();
        for rel in rels {
            match self.project.coll.load_folder(&rel) {
                Ok(f) if !f.is_empty() => {
                    self.folders.insert(rel, f);
                }
                Ok(_) => {}
                Err(e) => errors.push(e.to_string()),
            }
        }
        if !errors.is_empty() {
            self.set_status(errors.join(" · "), StatusKind::Error);
        }
    }

    /// The cached folder chain of a request (`None`: an unsaved request,
    /// which gets the root's settings).
    pub fn chain_of(&self, path: Option<&str>) -> Vec<ChainItem> {
        folders_above(path.unwrap_or(""))
            .into_iter()
            .filter_map(|rel| self.folders.get(&rel).map(|f| (rel, f.clone())))
            .collect()
    }

    /// The path a tab inherits from: its own, or for a history copy the
    /// request it came from.
    pub fn inherit_path(tab: &Tab) -> Option<&str> {
        tab.path.as_deref().or(tab.origin_path.as_deref())
    }

    /// The folder chain a tab reads: for a request, the folders above it; for
    /// a folder tab, the folders above it and the settings it edits.
    fn names_chain(&self, tab: &Tab) -> Vec<ChainItem> {
        let mut chain = self.inherited_chain(tab);
        if let Some(rel) = &tab.folder {
            chain.push((rel.clone(), tab.folder_settings()));
        }
        chain
    }

    /// The variable names a tab sees: [`App::names`] plus its folders'
    /// variables.
    pub fn names_of(&self, tab: &Tab) -> Scope {
        let mut names = self.names.clone();
        runner::add_folder_vars(&mut names, &self.names_chain(tab));
        names
    }

    /// The active tab's names, or [`App::names`] without a tab.
    pub fn tab_names(&self) -> Scope {
        match self.tab() {
            Some(t) => self.names_of(t),
            None => self.names.clone(),
        }
    }

    /// A tab's request as it is sent (folders applied), and its names; for
    /// the checks before a send or a watch repeat. A folder tab is not sent:
    /// its settings stay as they are and its names include its own variables.
    pub fn effective(&self, tab: &Tab) -> (Request, Scope) {
        let mut names = self.names.clone();
        let chain = self.names_chain(tab);
        if tab.folder.is_some() {
            runner::add_folder_vars(&mut names, &chain);
            return (tab.model.clone(), names);
        }
        let req = runner::with_folders(&tab.model, &chain, &mut names);
        (req, names)
    }
}

impl App {
    /// The chain a tab inherits from: a request's folders, or for a folder
    /// tab the folders above it (its own settings are what it edits).
    pub fn inherited_chain(&self, tab: &Tab) -> Vec<ChainItem> {
        match &tab.folder {
            Some(rel) if rel.is_empty() => Vec::new(),
            Some(rel) => self.chain_of(Some(parent_rel(rel))),
            None => self.chain_of(Self::inherit_path(tab)),
        }
    }

    /// What an "Inherit" auth stands for, for the line beside the auth type.
    pub fn inherited_auth_note(&self, tab: &Tab) -> String {
        let chain = self.inherited_chain(tab);
        match sendito_core::inherit::auth_source(&chain) {
            Some((rel, auth)) => format!("→ {} from {}", auth.kind().label(), folder_label(rel)),
            None => "→ no folder sets an auth: none is sent".into(),
        }
    }

    /// The enabled rows a tab inherits for its headers (gRPC: metadata) that
    /// its own rows do not replace, with the folder each comes from.
    pub fn inherited_rows(&self, tab: &Tab) -> Vec<(String, String, String)> {
        let chain = self.inherited_chain(tab);
        let own = match &tab.model {
            Request::Grpc(g) if tab.folder.is_none() => g.metadata.clone(),
            Request::Http(h) => h.headers.clone(),
            Request::Ws(w) => w.headers.clone(),
            Request::Sse(s) => s.headers.clone(),
            _ => return Vec::new(),
        };
        let rows = sendito_core::inherit::inherited_rows(&chain, &own);
        rows.into_iter()
            .filter(|r| r.enabled && !r.name.trim().is_empty())
            .map(|r| {
                let from = chain
                    .iter()
                    .rev()
                    .find(|(_, f)| {
                        f.headers
                            .iter()
                            .any(|h| h.name == r.name && h.value == r.value)
                    })
                    .map(|(rel, _)| folder_label(rel))
                    .unwrap_or_default();
                (r.name, r.value, from)
            })
            .collect()
    }
}

impl App {
    /// Opens the settings of the folder `rel` (`""`: the collection root) in
    /// a tab, or goes to the tab that has them.
    pub(super) fn open_folder(&mut self, rel: &str) {
        self.complete = None;
        self.clear_transient_status();
        if let Some(i) = self
            .tabs
            .iter()
            .position(|t| t.folder.as_deref() == Some(rel))
        {
            self.active = i;
            self.focus = Focus::ReqTabs;
            return;
        }
        let settings = match self.project.coll.load_folder(rel) {
            Ok(f) => f,
            Err(e) => {
                self.set_status(
                    format!("cannot open the folder settings: {e}"),
                    StatusKind::Error,
                );
                return;
            }
        };
        let name = if rel.is_empty() {
            self.project.name.clone()
        } else {
            name_of(rel).to_string()
        };
        let id = self.alloc_id();
        let mut tab = Tab::new_folder(id, rel, name, &settings, Some(&settings));
        tab.ed.set_vim(self.vim);
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        self.focus = Focus::ReqTabs;
        self.session_dirty = true;
    }

    /// Reopens a folder tab of the last session, with its unsaved edits. A
    /// folder that is gone, or whose file cannot be read, is left out.
    pub(super) fn restore_folder_tab(&mut self, rel: &str, draft: Option<&str>) {
        if self.tree.root.find_folder(rel).is_none() {
            return;
        }
        let Ok(saved) = self.project.coll.load_folder(rel) else {
            return;
        };
        let current = draft
            .and_then(|d| sendito_store::folder::parse(d).ok())
            .unwrap_or_else(|| saved.clone());
        let name = if rel.is_empty() {
            self.project.name.clone()
        } else {
            name_of(rel).to_string()
        };
        let id = self.alloc_id();
        let mut tab = Tab::new_folder(id, rel, name, &current, Some(&saved));
        tab.ed.set_vim(self.vim);
        self.tabs.push(tab);
    }

    /// The tree's selection: a folder's own settings, or for a request the
    /// settings of its folder.
    pub(super) fn tree_folder_settings(&mut self) {
        let rel = match self.tree.selected_row() {
            Some(r) if r.is_folder() => r.rel.clone(),
            Some(r) => parent_rel(&r.rel).to_string(),
            None => String::new(),
        };
        self.open_folder(&rel);
    }

    /// `:folder`: the settings of the active request's folder (the root for
    /// an unsaved request), or of the tree's selection without a tab.
    pub(super) fn folder_settings_of_active(&mut self) {
        match self.tab() {
            Some(t) if t.folder.is_some() => {}
            Some(t) => {
                let rel = parent_rel(Self::inherit_path(t).unwrap_or("")).to_string();
                self.open_folder(&rel);
            }
            None => self.tree_folder_settings(),
        }
    }

    /// Writes the active folder tab to its `folder.toml` (empty settings
    /// remove the file) and refreshes the cache.
    pub(super) fn save_folder_tab(&mut self) {
        let Some(t) = self.tab() else { return };
        let Some(rel) = t.folder.clone() else { return };
        let settings = t.folder_settings();
        match self.project.coll.save_folder(&rel, &settings) {
            Ok(_) => {
                if let Some(t) = self.tab_mut() {
                    t.set_saved(Some(folder_request(&settings)));
                }
                if settings.is_empty() {
                    self.folders.remove(&rel);
                } else {
                    self.folders.insert(rel.clone(), settings.clone());
                }
                let file = if rel.is_empty() {
                    sendito_store::folder::FILE.to_string()
                } else {
                    format!("{rel}/{}", sendito_store::folder::FILE)
                };
                let what = if settings.is_empty() {
                    "removed"
                } else {
                    "saved"
                };
                self.set_status(format!("{what} {file}"), StatusKind::Ok);
                self.session_dirty = true;
            }
            Err(e) => self.set_status(format!("save failed: {e}"), StatusKind::Error),
        }
    }
}

/// How a folder is named in notes: its path, or "the collection root".
pub fn folder_label(rel: &str) -> String {
    if rel.is_empty() {
        "the collection root".into()
    } else {
        rel.to_string()
    }
}

fn collect_folders(f: &sendito_store::Folder, out: &mut Vec<String>) {
    out.push(f.rel.clone());
    for sub in &f.folders {
        collect_folders(sub, out);
    }
}
