//! Sidebar tree state: flattening, expansion, filtering and selection.

use std::collections::BTreeSet;

use sendito_store::collection::{Folder, parent_rel};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKind {
    Folder {
        expanded: bool,
        empty: bool,
    },
    Request {
        badge: String,
    },
    /// A heading (the name) or an empty separator row. It cannot be selected.
    Section,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub rel: String,
    pub name: String,
    pub depth: usize,
    pub kind: RowKind,
    /// The row is in the Favorites section, not in the tree.
    pub fav: bool,
}

impl Row {
    pub fn is_folder(&self) -> bool {
        matches!(self.kind, RowKind::Folder { .. })
    }

    pub fn is_section(&self) -> bool {
        matches!(self.kind, RowKind::Section)
    }
}

/// `p` moved into `new` when the node `old` is renamed or moved: `p` itself
/// or a path inside the folder `old`. `None` when `p` is not affected.
pub fn repath(p: &str, old: &str, new: &str) -> Option<String> {
    if p == old {
        Some(new.to_string())
    } else {
        p.strip_prefix(&format!("{old}/"))
            .map(|rest| format!("{new}/{rest}"))
    }
}

#[derive(Debug, Clone)]
pub struct TreeState {
    pub root: Folder,
    pub expanded: BTreeSet<String>,
    pub filter: String,
    /// Favorite requests, in the order they were added. A path with no
    /// request on disk is kept but not shown.
    pub favorites: Vec<String>,
    pub rows: Vec<Row>,
    pub selected: usize,
    pub scroll: usize,
    /// Whether the next draw should scroll the selection into view. Selection
    /// changes arm it; the mouse wheel disarms it to allow free browsing.
    pub follow: bool,
}

impl TreeState {
    pub fn new(
        root: Folder,
        expanded: impl IntoIterator<Item = String>,
        favorites: Vec<String>,
    ) -> Self {
        let mut t = Self {
            root,
            expanded: expanded.into_iter().collect(),
            filter: String::new(),
            favorites,
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
        self.root = root;
        self.rebuild_keep();
    }

    /// Rebuilds the rows and keeps the same row selected when it still exists.
    fn rebuild_keep(&mut self) {
        let sel = self.selected_row().map(|r| (r.rel.clone(), r.fav));
        self.rebuild();
        if let Some((rel, fav)) = sel {
            let at = fav
                .then(|| self.rows.iter().position(|r| r.fav && r.rel == rel))
                .flatten();
            match at {
                Some(i) => self.selected = i,
                None => {
                    self.select_rel(&rel);
                }
            }
        }
    }

    pub fn rebuild(&mut self) {
        let mut rows = Vec::new();
        let filter = self.filter.to_lowercase();
        let root = self.root.clone();
        if filter.is_empty() {
            self.favorite_rows(&mut rows);
            self.flatten(&root, 0, &mut rows);
        } else {
            Self::flatten_filtered(&root, 0, &filter, &mut rows);
        }
        self.rows = rows;
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len().saturating_sub(1);
        }
        self.skip_section(1);
        self.follow = true;
    }

    /// The Favorites section: a heading, one row per favorite request on
    /// disk, and a separator. Nothing when no favorite is on disk.
    fn favorite_rows(&self, out: &mut Vec<Row>) {
        let favs: Vec<Row> = self
            .favorites
            .iter()
            .filter_map(|rel| self.root.find_request(rel))
            .map(|r| Row {
                rel: r.rel.clone(),
                name: r.name.clone(),
                depth: 0,
                kind: RowKind::Request {
                    badge: r.badge.clone(),
                },
                fav: true,
            })
            .collect();
        if favs.is_empty() {
            return;
        }
        let section = |name: &str| Row {
            rel: String::new(),
            name: name.into(),
            depth: 0,
            kind: RowKind::Section,
            fav: true,
        };
        out.push(section("Favorites"));
        out.extend(favs);
        out.push(section(""));
    }

    /// Moves the selection off a section row, first in direction `dir`
    /// (1 or -1), then the other way.
    fn skip_section(&mut self, dir: isize) {
        for d in [dir, -dir] {
            let mut i = self.selected as isize;
            while (0..self.rows.len() as isize).contains(&i) && self.rows[i as usize].is_section() {
                i += d;
            }
            if (0..self.rows.len() as isize).contains(&i) {
                self.selected = i as usize;
                return;
            }
        }
    }

    pub fn is_favorite(&self, rel: &str) -> bool {
        self.favorites.iter().any(|f| f == rel)
    }

    /// Adds `rel` to the favorites or removes it. Returns whether it is now a
    /// favorite.
    pub fn toggle_favorite(&mut self, rel: &str) -> bool {
        let now = if let Some(i) = self.favorites.iter().position(|f| f == rel) {
            self.favorites.remove(i);
            false
        } else {
            self.favorites.push(rel.to_string());
            true
        };
        self.rebuild_keep();
        now
    }

    /// Moves the favorites into a renamed or moved node.
    pub fn repath_favorites(&mut self, old: &str, new: &str) {
        for f in &mut self.favorites {
            if let Some(p) = repath(f, old, new) {
                *f = p;
            }
        }
    }

    /// Removes the favorites of a deleted node (a request or a folder).
    pub fn forget_favorites(&mut self, rel: &str) {
        self.favorites.retain(|f| repath(f, rel, "").is_none());
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
                fav: false,
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
                fav: false,
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
                    fav: false,
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
                    fav: false,
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

    /// The selected row; never a section row.
    pub fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected).filter(|r| !r.is_section())
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
        if let Some(i) = self.rows.iter().position(|r| !r.fav && r.rel == rel) {
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
        self.skip_section(if delta < 0 { -1 } else { 1 });
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

    /// Scrolls the selection into view. With `sticky`, the pinned folder rows
    /// take their lines from the list.
    pub fn ensure_visible(&mut self, height: usize, sticky: bool) {
        if height == 0 {
            return;
        }
        self.scroll = crate::list::keep_visible(self.selected, self.scroll, height);
        while sticky && self.selected >= self.scroll + height - self.sticky(height).len() {
            self.scroll += 1;
        }
        self.scroll = self.scroll.min(self.max_scroll(height, sticky));
    }

    /// The largest scroll that still fills a list `height` rows tall.
    pub fn max_scroll(&self, height: usize, sticky: bool) -> usize {
        let len = self.rows.len();
        let mut s = len.saturating_sub(height);
        while sticky && s < len && s + height - self.pins_at(s, height).len() < len {
            s += 1;
        }
        s
    }

    /// The folder rows that contain row `idx`, from the root down.
    fn ancestors(&self, idx: usize) -> Vec<usize> {
        let Some(mut depth) = self.rows.get(idx).map(|r| r.depth) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for i in (0..idx).rev() {
            if depth == 0 {
                break;
            }
            let r = &self.rows[i];
            if r.is_folder() && r.depth < depth {
                out.push(i);
                depth = r.depth;
            }
        }
        out.reverse();
        out
    }

    /// The folder rows pinned above the list in a list `height` rows tall:
    /// the folders of the top row, at most half the height. The list starts
    /// below them, so they hide no row.
    pub fn sticky(&self, height: usize) -> Vec<usize> {
        self.pins_at(self.scroll, height)
    }

    fn pins_at(&self, scroll: usize, height: usize) -> Vec<usize> {
        let mut pins = self.ancestors(scroll);
        pins.truncate(height / 2);
        pins
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
            description: String::new(),
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
        let mut t = TreeState::new(tree(), Vec::<String>::new(), Vec::new());
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
        t.ensure_visible(2, false);
        assert!(t.scroll > 0);
        assert_eq!(t.all_folders().len(), 4);
        t.set_root(tree());
        assert_eq!(t.selected_rel().as_deref(), Some("ping.toml"));
        assert!(!t.expanded_list().is_empty());
    }

    #[test]
    fn sticky_pins_the_folders_of_the_top_row() {
        let mut t = TreeState::new(tree(), Vec::<String>::new(), Vec::new());
        // Users, Admin, ban, list, empty, ping
        assert!(t.select_rel("Users/Admin/ban.toml"));
        assert!(t.sticky(6).is_empty());
        t.scroll = 2;
        assert_eq!(t.sticky(6), vec![0, 1]);
        assert_eq!(t.sticky(2), vec![0], "at most half the height");
        t.scroll = 3;
        assert_eq!(t.sticky(6), vec![0]);
        t.scroll = 4;
        assert!(t.sticky(6).is_empty());
        // The pins take lines, so the last row needs one more scroll step.
        assert_eq!(t.max_scroll(4, false), 2);
        assert_eq!(t.max_scroll(4, true), 3);
        t.scroll = 0;
        t.move_sel(100);
        t.ensure_visible(4, true);
        assert_eq!(t.scroll, 3);
        // Moving up past the pins shows the row at the top, not under them.
        t.selected = 2;
        t.ensure_visible(4, true);
        assert_eq!(t.scroll, 2);
        assert!(!t.sticky(4).contains(&2));
    }

    #[test]
    fn clearing_the_filter_keeps_the_chosen_request_visible() {
        let mut t = TreeState::new(tree(), Vec::<String>::new(), Vec::new());
        t.set_filter("ban");
        assert_eq!(t.selected_rel().as_deref(), Some("Users/Admin/ban.toml"));
        t.set_filter("");
        assert_eq!(t.selected_rel().as_deref(), Some("Users/Admin/ban.toml"));
        assert!(t.rows.iter().any(|r| r.rel == "Users/Admin/ban.toml"));
    }

    #[test]
    fn favorites_section_on_top() {
        let favs = vec!["ping.toml".to_string(), "gone.toml".to_string()];
        let mut t = TreeState::new(tree(), Vec::<String>::new(), favs);
        // Favorites, ping (favorite), separator, Users, empty, ping
        assert_eq!(t.rows.len(), 6);
        assert!(t.rows[0].is_section() && t.rows[2].is_section());
        assert!(t.rows[1].fav && t.rows[1].rel == "ping.toml");
        assert_eq!(t.selected, 1, "a section row is never selected");
        t.move_sel(1);
        assert_eq!(t.selected_rel().as_deref(), Some("Users"));
        t.move_sel(-1);
        assert_eq!(t.selected, 1);
        t.move_sel(-1);
        assert_eq!(t.selected, 1);
        // select_rel finds the row in the tree, not the favorite.
        assert!(t.select_rel("ping.toml"));
        assert!(!t.selected_row().unwrap().fav);
        // The filter hides the section.
        t.set_filter("ping");
        assert!(t.rows.iter().all(|r| !r.fav));
        t.set_filter("");
        assert!(t.toggle_favorite("Users/list.toml"));
        assert_eq!(t.selected_rel().as_deref(), Some("ping.toml"));
        assert!(!t.toggle_favorite("ping.toml"));
        assert!(t.rows[1].fav && t.rows[1].rel == "Users/list.toml");
        assert_eq!(t.favorites, vec!["gone.toml", "Users/list.toml"]);
    }

    #[test]
    fn favorites_follow_renames_and_deletes() {
        let favs = vec!["Users/list.toml".to_string(), "ping.toml".to_string()];
        let mut t = TreeState::new(tree(), Vec::<String>::new(), favs);
        t.repath_favorites("Users", "People");
        assert_eq!(t.favorites, vec!["People/list.toml", "ping.toml"]);
        t.forget_favorites("People");
        assert_eq!(t.favorites, vec!["ping.toml"]);
        assert_eq!(repath("Usersx/a", "Users", "P"), None);
    }
}
