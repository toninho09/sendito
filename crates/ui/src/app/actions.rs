//! Keymap actions: focus, cursor, tabs, close, quit and save.

use super::*;

impl App {
    // ----- actions ------------------------------------------------------

    pub fn dispatch(&mut self, a: Action) {
        match a {
            Action::Send => {
                if self.focus == Focus::Sidebar && self.sidebar_mode == SidebarMode::History {
                    self.open_history(self.history_sel, true);
                } else {
                    self.send(false);
                }
            }
            Action::Cancel => self.cancel(),
            Action::WatchToggle => self.toggle_watch(None),
            Action::EndStream => self.end_stream(),
            Action::Save => self.save(),
            Action::Quit => self.request_quit(),
            Action::NewRequest => self.new_tab(Request::new_http()),
            Action::NewGrpcRequest => self.new_tab(Request::new_grpc()),
            Action::NewSseRequest => self.new_tab(Request::new_sse()),
            Action::NewMqttRequest => self.new_tab(Request::new_mqtt()),
            Action::NewWsRequest => self.new_tab(Request::new_ws()),
            Action::CloseTab => self.close_tab(false),
            Action::NextTab => self.switch_tab(1),
            Action::PrevTab => self.switch_tab(-1),
            Action::TogglePin => self.toggle_pin(),
            Action::ToggleFavorite => self.toggle_favorite(),
            Action::FocusLeft => self.focus_dir(-1, 0),
            Action::FocusRight => self.focus_dir(1, 0),
            Action::FocusUp => self.focus_dir(0, -1),
            Action::FocusDown => self.focus_dir(0, 1),
            Action::FocusNext => self.focus_cycle(1),
            Action::FocusPrev => self.focus_cycle(-1),
            Action::NextSubTab => self.subtab(1),
            Action::PrevSubTab => self.subtab(-1),
            Action::SubTab1
            | Action::SubTab2
            | Action::SubTab3
            | Action::SubTab4
            | Action::SubTab5
            | Action::SubTab6 => {
                let i = match a {
                    Action::SubTab1 => 0,
                    Action::SubTab2 => 1,
                    Action::SubTab3 => 2,
                    Action::SubTab4 => 3,
                    Action::SubTab5 => 4,
                    _ => 5,
                };
                self.set_subtab(i);
            }
            Action::Palette => self.open_palette(""),
            Action::Complete => self.refresh_complete(),
            Action::Help => self.open_help(),
            Action::Settings => self.open_settings(SettingId::Theme),
            Action::SelectEnv => self.open_env_picker(),
            Action::ManageEnvs => self.open_env_editor(),
            Action::ManageMcpServers => self.open_mcp_editor(),
            Action::History => self.toggle_history(),
            Action::ToggleSidebar => {
                self.layout.sidebar_visible = !self.layout.sidebar_visible;
                if !self.layout.sidebar_visible
                    && matches!(self.focus, Focus::Sidebar | Focus::Filter)
                {
                    self.focus = self.top_bar_focus();
                }
                self.session_dirty = true;
            }
            Action::Import => self.open_import(""),
            Action::ExportCurl => self.export_curl(false),
            Action::FormatBody => self.format_body(),
            Action::Zoom => self.zoom = !self.zoom,
            Action::GrowSidebar => self.resize_sidebar(4),
            Action::ShrinkSidebar => self.resize_sidebar(-4),
            Action::GrowRequest => self.resize_split(5),
            Action::ShrinkRequest => self.resize_split(-5),
            Action::TreeOpen => self.tree_open(),
            Action::TreeExpand => {
                if !self.tree.expand() {
                    self.tree_open();
                }
                self.session_dirty = true;
            }
            Action::TreeCollapse => {
                self.tree.collapse();
                self.session_dirty = true;
            }
            Action::TreeNewRequest => self.prompt_new_request(),
            Action::TreeNewFolder => self.prompt_new_folder(),
            Action::TreeRename => self.prompt_rename(),
            Action::TreeFolderSettings => self.tree_folder_settings(),
            Action::TreeDuplicate => self.tree_duplicate(),
            Action::TreeDelete => self.tree_delete(),
            Action::TreeMove => self.tree_move(),
            Action::TreeFilter => {
                self.focus = Focus::Filter;
                self.sidebar_mode = SidebarMode::Tree;
            }
            Action::CursorUp => self.cursor_move(-1),
            Action::CursorDown => self.cursor_move(1),
            Action::PageUp => self.cursor_page(-1),
            Action::PageDown => self.cursor_page(1),
            Action::Top => self.cursor_edge(false),
            Action::Bottom => self.cursor_edge(true),
            Action::ScrollLeft => self.with_viewer(|v| v.hscroll_by(-8)),
            Action::ScrollRight => self.with_viewer(|v| v.hscroll_by(8)),
            Action::ViewPretty => self.set_view(ViewMode::Pretty),
            Action::ViewRaw => self.set_view(ViewMode::Raw),
            Action::ViewHex => self.set_view(ViewMode::Hex),
            Action::Search => self.open_search(),
            Action::Filter => self.open_filter(),
            Action::FilterHere => self.filter_here(),
            Action::CopyPath => self.copy_path(),
            Action::PrevMessage => self.step_message(-1),
            Action::NextMessage => self.step_message(1),
            Action::SearchNext => self.with_viewer(|v| {
                v.next_match(1);
            }),
            Action::SearchPrev => self.with_viewer(|v| {
                v.next_match(-1);
            }),
            Action::FoldToggle => self.with_viewer(|v| {
                v.toggle_fold();
            }),
            Action::FoldAll => self.with_viewer(Viewer::fold_all),
            Action::UnfoldAll => self.with_viewer(Viewer::unfold_all),
            Action::ToggleWrap => self.with_viewer(Viewer::toggle_wrap),
            Action::CopyBody => self.copy_body(),
            Action::SaveBody => self.prompt_save_body(),
            Action::NextCell => {
                if let Some(t) = self.focused_table() {
                    t.next_cell();
                }
            }
            Action::ToggleRow => {
                if let Some(t) = self.focused_table() {
                    t.toggle_row();
                }
                self.changed();
            }
            Action::DeleteRow => {
                if let Some(t) = self.focused_table() {
                    t.delete_row();
                }
                self.changed();
            }
            Action::Undo
            | Action::Redo
            | Action::SelectAll
            | Action::Copy
            | Action::Cut
            | Action::Paste => {}
        }
    }

    pub(super) fn with_viewer(&mut self, f: impl FnOnce(&mut Viewer)) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let rt = tab.resp_tab;
        if let Some(r) = &mut tab.response {
            f(r.viewer_mut(rt));
        }
    }

    pub(super) fn cursor_move(&mut self, d: isize) {
        match self.focus {
            Focus::Sidebar => {
                if self.sidebar_mode == SidebarMode::Tree {
                    self.tree.move_sel(d);
                }
            }
            _ => self.with_viewer(|v| v.move_cursor(d)),
        }
    }

    pub(super) fn cursor_page(&mut self, d: isize) {
        match self.focus {
            Focus::Sidebar => {
                let h = self.areas.tree.height.max(2) as isize - 1;
                self.tree.move_sel(d * h);
            }
            _ => self.with_viewer(|v| v.page(d)),
        }
    }

    pub(super) fn cursor_edge(&mut self, bottom: bool) {
        match self.focus {
            Focus::Sidebar => {
                let n = self.tree.rows.len() as isize;
                self.tree.move_sel(if bottom { n } else { -n });
            }
            _ => self.with_viewer(|v| if bottom { v.bottom() } else { v.top() }),
        }
    }

    pub(super) fn set_view(&mut self, m: ViewMode) {
        if let Some(t) = self.tab_mut() {
            t.resp_tab = 0;
        }
        self.with_viewer(|v| v.set_mode(m));
    }

    pub(super) fn resize_sidebar(&mut self, d: i32) {
        let w = (self.layout.sidebar_width as i32 + d).clamp(16, 80) as u16;
        self.layout.sidebar_width = w;
        self.session_dirty = true;
    }

    pub(super) fn resize_split(&mut self, d: i32) {
        self.layout.request_pct = (self.layout.request_pct as i32 + d).clamp(20, 80) as u16;
        self.session_dirty = true;
    }

    /// The top bar part to focus for the active tab: every kind draws the
    /// same `[picker chip][endpoint input]` strip, and the input is part 1.
    /// Focus for an opened request: the URL for HTTP and WebSocket (whose
    /// part 0 is a static badge), the method or tool chip for gRPC and MCP.
    pub(super) fn open_focus(kind: Kind) -> Focus {
        match kind {
            Kind::Http | Kind::Ws | Kind::Sse | Kind::Mqtt => Focus::Bar(1),
            // A folder tab has no request bar.
            Kind::Folder => Focus::ReqTabs,
            _ => Focus::Bar(0),
        }
    }

    pub(super) fn top_bar_focus(&self) -> Focus {
        Focus::Bar(1)
    }

    pub(super) fn focus_order(&self) -> Vec<Focus> {
        let mut v = Vec::new();
        if self.layout.sidebar_visible {
            v.push(Focus::Sidebar);
        }
        if !self.tabs.is_empty() {
            // Every kind has the chip (part 0) except WebSocket, whose chip is
            // a static badge; the endpoint input (part 1) follows. gRPC and MCP
            // draw [input][chip], so their cycle reads left to right: the
            // input first, then the chip.
            match self.tab().map(|t| t.ed.kind) {
                Some(Kind::Grpc | Kind::Mcp) => {
                    v.push(Focus::Bar(1));
                    v.push(Focus::Bar(0));
                }
                Some(Kind::Ws | Kind::Sse | Kind::Mqtt) => v.push(Focus::Bar(1)),
                Some(Kind::Folder) => {}
                _ => {
                    v.push(Focus::Bar(0));
                    v.push(Focus::Bar(1));
                }
            }
            if self.tab().is_some_and(|t| t.folder.is_none()) {
                v.push(Focus::Bar(2));
            }
            v.extend([
                Focus::ReqTabs,
                Focus::ReqBody,
                Focus::RespTabs,
                Focus::RespBody,
            ]);
        }
        v
    }

    pub fn focus_cycle(&mut self, dir: isize) {
        self.complete = None;
        // Inside the request body, Tab walks through the fields first.
        if self.focus == Focus::ReqBody {
            if dir > 0 && self.next_field(false) {
                return;
            }
            if dir < 0
                && let Some(t) = self.tab_mut()
                && t.field > 0
            {
                t.field -= 1;
                return;
            }
        }
        let order = self.focus_order();
        if order.is_empty() {
            return;
        }
        let cur = match self.focus {
            Focus::Filter => Focus::Sidebar,
            f => f,
        };
        let i = order.iter().position(|f| *f == cur).unwrap_or(0) as isize;
        let next = order[crate::list::cycle(i as usize, dir, order.len())];
        if next == Focus::ReqBody {
            let count = self.tab().map(|t| t.fields().len()).unwrap_or(0);
            if count == 0 {
                self.focus = if dir > 0 {
                    Focus::RespTabs
                } else {
                    Focus::ReqTabs
                };
                return;
            }
            let start = if dir > 0 { 0 } else { count - 1 };
            self.enter_req_body(start);
            return;
        }
        self.focus = next;
    }

    pub(super) fn focus_dir(&mut self, dx: i32, dy: i32) {
        self.complete = None;
        let right_sidebar = self.project.config.ui.sidebar == "right";
        let in_sidebar = matches!(self.focus, Focus::Sidebar | Focus::Filter);
        if dx != 0 {
            let toward_sidebar = (dx < 0) != right_sidebar;
            if toward_sidebar && !in_sidebar && self.layout.sidebar_visible {
                self.focus = Focus::Sidebar;
            } else if !toward_sidebar && in_sidebar && !self.tabs.is_empty() {
                self.focus = Focus::Bar(1);
            } else if let Focus::Bar(p) = self.focus {
                // WebSocket has no chip to focus; its strip starts at the URL.
                let min = if self
                    .tab()
                    .is_some_and(|t| matches!(t.ed.kind, Kind::Ws | Kind::Sse | Kind::Mqtt))
                {
                    1
                } else {
                    0
                };
                self.focus = Focus::Bar(((p as i32 + dx).clamp(min, 2)) as usize);
            }
            return;
        }
        if in_sidebar || self.tabs.is_empty() {
            return;
        }
        let in_request = self.focus.in_request();
        if dy > 0 && in_request {
            self.focus = Focus::RespBody;
        } else if dy < 0 && !in_request {
            self.focus = Focus::Bar(1);
        }
    }

    pub(super) fn subtab(&mut self, dir: isize) {
        self.complete = None;
        let resp = self.focus.in_response();
        let Some(t) = self.tab_mut() else { return };
        if resp {
            t.resp_tab = crate::list::cycle(t.resp_tab, dir, crate::tab::RESP_TABS.len());
        } else {
            let n = t.ed.tab_names().len();
            t.req_tab = crate::list::cycle(t.req_tab, dir, n);
            t.field = 0;
            let empty = t.fields().is_empty();
            if self.focus == Focus::ReqBody && empty {
                self.focus = Focus::ReqTabs;
            }
        }
    }

    pub(super) fn set_subtab(&mut self, i: usize) {
        self.complete = None;
        let resp = self.focus.in_response();
        let Some(t) = self.tab_mut() else { return };
        if resp {
            if i < crate::tab::RESP_TABS.len() {
                t.resp_tab = i;
            }
        } else if i < t.ed.tab_names().len() {
            t.req_tab = i;
            t.field = 0;
            if !matches!(self.focus, Focus::ReqTabs | Focus::ReqBody) {
                self.focus = Focus::ReqTabs;
            }
            if self.focus == Focus::ReqBody && self.tab().is_some_and(|t| t.fields().is_empty()) {
                self.focus = Focus::ReqTabs;
            }
        }
    }

    pub(super) fn switch_tab(&mut self, dir: isize) {
        self.complete = None;
        if self.tabs.is_empty() {
            return;
        }
        self.active = crate::list::cycle(self.active, dir, self.tabs.len());
        self.session_dirty = true;
    }

    pub fn new_tab(&mut self, req: Request) {
        self.complete = None;
        self.clear_transient_status();
        let id = self.alloc_id();
        // The lowest free name: two open tabs never share one.
        let label = |n: usize| {
            if n == 1 {
                "untitled".to_string()
            } else {
                format!("untitled {n}")
            }
        };
        let name = (1..)
            .map(label)
            .find(|l| !self.tabs.iter().any(|t| t.path.is_none() && t.name == *l))
            .unwrap_or_default();
        let blank = req == Request::new_http()
            || req == Request::new_grpc()
            || req == Request::new_ws()
            || req == Request::new_mcp();
        let mut tab = Tab::new(id, None, name, &req, None);
        tab.ed.set_vim(self.vim);
        tab.refresh_model();
        // An empty new request has nothing to lose: it is not "unsaved" until
        // something is typed.
        if blank {
            tab.set_saved(Some(tab.model.clone()));
        }
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        self.focus = self.top_bar_focus();
        self.session_dirty = true;
    }

    pub fn open_request(&mut self, rel: &str) {
        self.complete = None;
        self.clear_transient_status();
        if let Some(i) = self
            .tabs
            .iter()
            .position(|t| t.path.as_deref() == Some(rel))
        {
            self.active = i;
            self.focus = Self::open_focus(self.tabs[i].ed.kind);
            return;
        }
        match self.project.coll.load(rel) {
            Ok(req) => {
                let id = self.alloc_id();
                let mut tab = Tab::new(
                    id,
                    Some(rel.to_string()),
                    name_of(rel).to_string(),
                    &req,
                    Some(req.clone()),
                );
                tab.ed.set_vim(self.vim);
                tab.refresh_model();
                let kind = tab.ed.kind;
                self.tabs.push(tab);
                self.active = self.tabs.len() - 1;
                self.focus = Self::open_focus(kind);
                self.session_dirty = true;
            }
            Err(e) => self.set_status(format!("cannot open: {e}"), StatusKind::Error),
        }
    }

    pub(super) fn close_tab(&mut self, force: bool) {
        self.complete = None;
        let Some(t) = self.tab() else { return };
        if !force && t.running.is_some() {
            let id = t.id;
            let name = t.title();
            let what = if t.running.as_ref().is_some_and(|r| r.streaming_input) {
                "an open session"
            } else {
                "a request in flight"
            };
            self.overlay = Some(Overlay::Confirm(Confirm {
                title: "Still running".into(),
                message: format!("\"{name}\" has {what}. Close the tab anyway?"),
                buttons: vec!["Close".into(), "Cancel".into()],
                sel: 0,
                action: ConfirmAction::CloseTabRunning(id),
            }));
            return;
        }
        if t.dirty() && !force {
            self.prompt_close_dirty();
            return;
        }
        if let Some(mut t) = (self.active < self.tabs.len()).then(|| self.tabs.remove(self.active))
            && let Some(r) = t.running.take()
        {
            r.task.abort();
        }
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        }
        if self.tabs.is_empty() {
            // With no tab left, the tree is the only place for focus: show it.
            self.layout.sidebar_visible = true;
            self.focus = Focus::Sidebar;
        }
        self.session_dirty = true;
    }

    /// Pins or unpins the active tab. A pinned tab moves to the end of the
    /// pinned tabs, an unpinned one to the start of the others.
    pub(super) fn toggle_pin(&mut self) {
        let Some(t) = self.tab_mut() else { return };
        t.pinned = !t.pinned;
        let pinned = t.pinned;
        let tab = self.tabs.remove(self.active);
        let to = self.tabs.iter().take_while(|t| t.pinned).count();
        self.tabs.insert(to, tab);
        self.active = to;
        self.session_dirty = true;
        let msg = if pinned { "tab pinned" } else { "tab unpinned" };
        self.set_status(msg, StatusKind::Ok);
    }

    /// The Save/Discard/Cancel prompt for closing the active dirty tab.
    pub(super) fn prompt_close_dirty(&mut self) {
        let Some(t) = self.tab() else { return };
        let id = t.id;
        let name = t.title();
        let message = if t.path.is_none() {
            format!("\"{name}\" has no file yet. Save it? (Save asks for a name and a folder.)")
        } else {
            format!("\"{name}\" has unsaved changes.")
        };
        self.overlay = Some(Overlay::Confirm(Confirm {
            title: "Unsaved changes".into(),
            message,
            buttons: vec!["Save".into(), "Discard".into(), "Cancel".into()],
            sel: 0,
            action: ConfirmAction::CloseTab(id),
        }));
    }

    pub(super) fn request_quit(&mut self) {
        let dirty = self.tabs.iter().filter(|t| t.dirty()).count();
        let live = self.tabs.iter().filter(|t| t.running.is_some()).count();
        if (dirty > 0 || live > 0) && self.project.config.ui.confirm_quit {
            let mut message = String::new();
            if dirty > 0 {
                message.push_str(&format!(
                    "{} unsaved changes, kept as drafts and restored next time.",
                    if dirty == 1 {
                        "1 tab has".to_string()
                    } else {
                        format!("{dirty} tabs have")
                    }
                ));
            }
            if live > 0 {
                message.push_str(&format!(
                    " {} a request or session still running.",
                    if live == 1 {
                        "1 tab has".to_string()
                    } else {
                        format!("{live} tabs have")
                    }
                ));
            }
            self.overlay = Some(Overlay::Confirm(Confirm {
                title: "Quit Sendito?".into(),
                message: format!("{} Quit anyway?", message.trim()),
                buttons: vec!["Quit".into(), "Cancel".into()],
                sel: 0,
                action: ConfirmAction::Quit,
            }));
        } else {
            self.quit = true;
        }
    }

    pub fn save(&mut self) {
        let Some(t) = self.tab() else { return };
        if t.folder.is_some() {
            self.save_folder_tab();
            return;
        }
        let Some(path) = t.path.clone() else {
            let id = t.id;
            let name = t.name.clone();
            self.prompt_save_as(id, &name);
            return;
        };
        let model = t.model.clone();
        match self.project.coll.save(&path, &model) {
            Ok(_) => {
                if let Some(t) = self.tab_mut() {
                    t.set_saved(Some(model));
                }
                self.rescan();
                self.set_status(format!("saved {path}"), StatusKind::Ok);
                self.session_dirty = true;
            }
            Err(e) => self.set_status(format!("save failed: {e}"), StatusKind::Error),
        }
    }

    pub(super) fn prompt_save_as(&mut self, tab: u64, name: &str) {
        let folders = self.tree.all_folders();
        // A request that has a file is saved next to it; a new one goes to the
        // collection root unless the user picks a folder (the tree's selection
        // is often just its first row, which says nothing about this request).
        let target = self
            .tab_by_id(tab)
            .and_then(|t| t.path.clone().or_else(|| t.origin_path.clone()))
            .map(|p| sendito_store::collection::parent_rel(&p).to_string())
            .unwrap_or_default();
        let idx = folders.iter().position(|f| *f == target).unwrap_or(0);
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
        let mut input = TextEditor::single(if name.starts_with("untitled") {
            ""
        } else {
            name
        });
        input.move_to_end();
        self.overlay = Some(Overlay::Prompt(Prompt::new(
            "Save request",
            "Name",
            input,
            vec![("In folder".into(), labels, idx)],
            PromptAction::SaveAs { tab },
        )));
    }

    pub fn rescan(&mut self) {
        match self.project.coll.scan() {
            Ok(root) => {
                self.tree.set_root(root);
                self.load_folders();
            }
            Err(e) => self.set_status(format!("cannot read collection: {e}"), StatusKind::Error),
        }
    }
}
