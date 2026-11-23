//! Sidebar tree state: flattening, expansion, filtering and selection.

use std::collections::BTreeSet;

use sendito_store::collection::{Folder, parent_rel};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKind {
    Folder { expanded: bool, empty: bool },
    Request { badge: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub rel: String,
    pub name: String,
    pub depth: usize,
    pub kind: RowKind,
}

impl Row {
    pub fn is_folder(&self) -> bool {
        matches!(self.kind, RowKind::Folder { .. })
    }
}

#[derive(Debug, Clone)]
pub struct TreeState {
    pub root: Folder,
    pub expanded: BTreeSet<String>,
    pub filter: String,
    pub rows: Vec<Row>,
    pub selected: usize,
    pub scroll: usize,
    /// Whether the next draw should scroll the selection into view. Selection
    /// changes arm it; the mouse wheel disarms it to allow free browsing.
    pub follow: bool,
}

impl TreeState {
    pub fn new(root: Folder, expanded: impl IntoIterator<Item = String>) -> Self {
        let mut t = Self {
            root,
            expanded: expanded.into_iter().collect(),
            filter: String::new(),
            rows: Vec::new(),
            selected: 0,
            scroll: 0,
            follow: true,
        };
        t.rebuild();
        t
    }

    /// Replaces the scanned tree, keeping the selection on the same path when possible.
    pub fn set_root(&mut self, root: Folder) {
        let sel = self.selected_rel();
        self.root = root;
        self.rebuild();
        if let Some(s) = sel {
            self.select_rel(&s);
        }
    }

    pub fn rebuild(&mut self) {
        let mut rows = Vec::new();
        let filter = self.filter.to_lowercase();
        let root = self.root.clone();
        if filter.is_empty() {
            self.flatten(&root, 0, &mut rows);
        } else {
            Self::flatten_filtered(&root, 0, &filter, &mut rows);
        }
        self.rows = rows;
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len().saturating_sub(1);
        }
        self.follow = true;
    }

    fn flatten(&self, f: &Folder, depth: usize, out: &mut Vec<Row>) {
        for sub in &f.folders {
            let expanded = self.expanded.contains(&sub.rel);
            out.push(Row {
                rel: sub.rel.clone(),
                name: sub.name.clone(),
                depth,
                kind: RowKind::Folder {
                    expanded,
                    empty: sub.folders.is_empty() && sub.requests.is_empty(),
                },
            });
            if expanded {
                self.flatten(sub, depth + 1, out);
            }
        }
        for r in &f.requests {
            out.push(Row {
                rel: r.rel.clone(),
                name: r.name.clone(),
                depth,
                kind: RowKind::Request {
                    badge: r.badge.clone(),
                },
            });
        }
    }

    /// Shows matching requests and folders with their ancestors (all expanded).
    fn flatten_filtered(f: &Folder, depth: usize, q: &str, out: &mut Vec<Row>) -> bool {
        let mut any = false;
        for sub in &f.folders {
            let mut child_rows = Vec::new();
            let name_match = sub.name.to_lowercase().contains(q);
            let child_match = Self::flatten_filtered(sub, depth + 1, q, &mut child_rows);
            if name_match || child_match {
                any = true;
                out.push(Row {
                    rel: sub.rel.clone(),
                    name: sub.name.clone(),
                    depth,
                    kind: RowKind::Folder {
                        expanded: true,
                        empty: sub.folders.is_empty() && sub.requests.is_empty(),
                    },
                });
                out.extend(child_rows);
            }
        }
        for r in &f.requests {
            if r.name.to_lowercase().contains(q) || r.badge.to_lowercase() == q {
                any = true;
                out.push(Row {
                    rel: r.rel.clone(),
                    name: r.name.clone(),
                    depth,
                    kind: RowKind::Request {
                        badge: r.badge.clone(),
                    },
                });
            }
        }
        any
    }

    pub fn set_filter(&mut self, f: &str) {
        let keep = self.selected_row().map(|r| r.rel.clone());
        self.filter = f.to_string();
        self.selected = 0;
        self.scroll = 0;
        self.rebuild();
        // Clearing the filter keeps the chosen item selected (and its folders open).
        if self.filter.is_empty() {
            if let Some(rel) = keep {
                self.select_rel(&rel);
            }
            return;
        }
        // Prefer the first request when filtering.
        if !self.filter.is_empty()
            && let Some(i) = self.rows.iter().position(|r| !r.is_folder())
        {
            self.selected = i;
        }
    }

    pub fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    pub fn selected_rel(&self) -> Option<String> {
        self.selected_row().map(|r| r.rel.clone())
    }

    pub fn select_rel(&mut self, rel: &str) -> bool {
        // Expand ancestors so the node is visible.
        let mut p = parent_rel(rel).to_string();
        let mut changed = false;
        while !p.is_empty() {
            changed |= self.expanded.insert(p.clone());
            p = parent_rel(&p).to_string();
        }
        if changed && self.filter.is_empty() {
            self.rebuild();
        }
        if let Some(i) = self.rows.iter().position(|r| r.rel == rel) {
            self.selected = i;
            self.follow = true;
            true
        } else {
            false
        }
    }

    pub fn move_sel(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let max = self.rows.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, max) as usize;
        self.follow = true;
    }

    pub fn toggle(&mut self, rel: &str) {
        if !self.expanded.remove(rel) {
            self.expanded.insert(rel.to_string());
        }
        self.rebuild();
    }

    pub fn expand(&mut self) -> bool {
        if let Some(r) = self.selected_row().cloned() {
            if let RowKind::Folder {
                expanded: false, ..
            } = r.kind
            {
                self.expanded.insert(r.rel);
                self.rebuild();
                return true;
            }
            if let RowKind::Folder { expanded: true, .. } = r.kind {
                // Move into the first child.
                if self
                    .rows
                    .get(self.selected + 1)
                    .is_some_and(|n| n.depth > r.depth)
                {
                    self.selected += 1;
                    self.follow = true;
                }
                return true;
            }
        }
        false
    }

    pub fn collapse(&mut self) {
        let Some(r) = self.selected_row().cloned() else {
            return;
        };
        if let RowKind::Folder { expanded: true, .. } = r.kind
            && self.filter.is_empty()
        {
            self.expanded.remove(&r.rel);
            self.rebuild();
            return;
        }
        let parent = parent_rel(&r.rel).to_string();
        if !parent.is_empty() {
            self.select_rel(&parent);
        }
    }

    /// Folder that receives new items: the selected folder, or the selected request's folder.
    pub fn target_folder(&self) -> String {
        match self.selected_row() {
            Some(r) if r.is_folder() => r.rel.clone(),
            Some(r) => parent_rel(&r.rel).to_string(),
            None => String::new(),
        }
    }

    pub fn ensure_visible(&mut self, height: usize) {
        if height == 0 {
            return;
        }
        self.scroll = crate::list::keep_visible(self.selected, self.scroll, height);
        let max_scroll = self.rows.len().saturating_sub(height);
        self.scroll = self.scroll.min(max_scroll);
    }

    /// All folder paths (for the move picker), "" for the root.
    pub fn all_folders(&self) -> Vec<String> {
        fn walk(f: &Folder, out: &mut Vec<String>) {
            out.push(f.rel.clone());
            for s in &f.folders {
                walk(s, out);
            }
        }
        let mut v = Vec::new();
        walk(&self.root, &mut v);
        v
    }

    pub fn expanded_list(&self) -> Vec<String> {
        self.expanded.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sendito_store::collection::RequestEntry;

    fn tree() -> Folder {
        let req = |rel: &str, badge: &str| RequestEntry {
            name: sendito_store::collection::name_of(rel).to_string(),
            rel: rel.to_string(),
            badge: badge.to_string(),
        };
        Folder {
            name: "root".into(),
            rel: String::new(),
            folders: vec![
                Folder {
                    name: "Users".into(),
                    rel: "Users".into(),
                    folders: vec![Folder {
                        name: "Admin".into(),
                        rel: "Users/Admin".into(),
                        folders: vec![],
                        requests: vec![req("Users/Admin/ban.toml", "POST")],
                    }],
                    requests: vec![req("Users/list.toml", "GET")],
                },
                Folder {
                    name: "empty".into(),
                    rel: "empty".into(),
                    folders: vec![],
                    requests: vec![],
                },
            ],
            requests: vec![req("ping.toml", "GRPC")],
        }
    }

    #[test]
    fn flatten_expand_filter() {
        let mut t = TreeState::new(tree(), Vec::<String>::new());
        assert_eq!(t.rows.len(), 3);
        assert!(t.expand());
        assert_eq!(t.rows.len(), 5);
        t.expand();
        assert_eq!(t.selected_rel().as_deref(), Some("Users/Admin"));
        t.collapse();
        t.collapse();
        assert_eq!(t.selected_rel().as_deref(), Some("Users"));
        t.collapse();
        assert_eq!(t.rows.len(), 3);
        assert!(t.select_rel("Users/Admin/ban.toml"));
        assert_eq!(t.rows.len(), 6);
        assert_eq!(t.target_folder(), "Users/Admin");
        t.set_filter("ban");
        assert_eq!(t.rows.len(), 3);
        assert_eq!(t.selected_rel().as_deref(), Some("Users/Admin/ban.toml"));
        t.set_filter("grpc");
        assert_eq!(t.rows.len(), 1);
        t.set_filter("");
        t.move_sel(100);
        assert_eq!(t.selected_rel().as_deref(), Some("ping.toml"));
        assert_eq!(t.target_folder(), "");
        t.toggle("empty");
        assert!(t.expanded.contains("empty"));
        t.ensure_visible(2);
        assert!(t.scroll > 0);
        assert_eq!(t.all_folders().len(), 4);
        t.set_root(tree());
        assert_eq!(t.selected_rel().as_deref(), Some("ping.toml"));
        assert!(!t.expanded_list().is_empty());
    }

    #[test]
    fn clearing_the_filter_keeps_the_chosen_request_visible() {
        let mut t = TreeState::new(tree(), Vec::<String>::new());
        t.set_filter("ban");
        assert_eq!(t.selected_rel().as_deref(), Some("Users/Admin/ban.toml"));
        t.set_filter("");
        assert_eq!(t.selected_rel().as_deref(), Some("Users/Admin/ban.toml"));
        assert!(t.rows.iter().any(|r| r.rel == "Users/Admin/ban.toml"));
    }
}
