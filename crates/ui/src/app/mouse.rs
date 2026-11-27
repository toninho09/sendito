//! Mouse: hit test, wheel, drag and clicks.

use super::*;

impl App {
    // ----- mouse --------------------------------------------------------

    pub(super) fn hit_at(&self, x: u16, y: u16) -> Option<(Rect, Hit)> {
        self.hits
            .iter()
            .rev()
            .find(|(r, _)| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height)
            .cloned()
    }

    pub fn on_mouse(&mut self, m: MouseEvent) {
        let (x, y) = (m.column, m.row);
        self.hover = Some((x, y));
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // A click starts a new action: an old message no longer applies.
                self.clear_transient_status();
                let double = self.last_click.is_some_and(|(t, lx, ly)| {
                    t.elapsed() < Duration::from_millis(400) && lx == x && ly == y
                });
                self.last_click = Some((Instant::now(), x, y));
                if let Some((rect, hit)) = self.hit_at(x, y) {
                    self.click(
                        hit,
                        rect,
                        x,
                        y,
                        double,
                        m.modifiers.contains(KeyModifiers::SHIFT),
                    );
                } else {
                    // A click on nothing still closes the Ctrl+W menu.
                    self.pending.clear();
                }
            }
            MouseEventKind::Down(MouseButton::Middle) => {
                if self.overlay.is_none()
                    && let Some((_, Hit::Tab(i))) = self.hit_at(x, y)
                {
                    self.active = i;
                    self.close_tab(false);
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => self.on_drag(x, y),
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(Drag::Tree {
                    rel, active: true, ..
                }) = self.drag.take()
                    && let Some(target) = self.drop_target.take()
                    && target != parent_rel(&rel)
                {
                    self.move_node(&rel, &target);
                }
                self.drag = None;
                self.drop_target = None;
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let d: isize = if m.kind == MouseEventKind::ScrollUp {
                    -3
                } else {
                    3
                };
                self.scroll_at(x, y, d);
            }
            _ => {}
        }
    }

    pub(super) fn scroll_at(&mut self, x: u16, y: u16, d: isize) {
        let inside = |r: Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        if self.overlay.is_some() {
            // Only the wheel over the dialog acts; the screen behind is modal.
            let over_dialog = matches!(
                self.hit_at(x, y),
                Some((_, Hit::Overlay(h))) if h != OverlayHit::Outside
            );
            if !over_dialog {
                return;
            }
            if let Some(ov) = &mut self.overlay {
                match ov {
                    Overlay::Report(r) => r.viewer.scroll_by(d),
                    Overlay::Help(h) => h.viewer.scroll_by(d),
                    Overlay::Picker(p) => {
                        let n = p.visible().len();
                        p.sel = crate::list::step(p.sel, d.signum(), n);
                    }
                    Overlay::Palette(p) => {
                        p.armed = true;
                        let n = p.visible().len();
                        p.sel = crate::list::step(p.sel, d.signum(), n);
                    }
                    Overlay::Settings(v) => v.step(d.signum()),
                    Overlay::Env(e) => e.table.move_selection(d.signum()),
                    Overlay::Records(e) => e.wheel(d.signum()),
                    Overlay::Keymap(e) => e.wheel(d.signum()),
                    Overlay::Prompt(p) => {
                        if let Some(hl) = p.open {
                            let n = p
                                .choices
                                .get(p.focus.wrapping_sub(1))
                                .map_or(0, |c| c.1.len());
                            if n > 0 {
                                p.open = Some(crate::list::cycle(hl, d.signum(), n));
                            }
                        }
                    }
                    _ => {}
                }
            }
            self.preview_picked_theme();
            return;
        }
        if inside(self.areas.sidebar) {
            if self.sidebar_mode == SidebarMode::Tree {
                let max = self.tree.max_scroll(
                    self.areas.tree.height as usize,
                    self.project.config.ui.sticky_folders,
                );
                self.tree.scroll =
                    ((self.tree.scroll as isize + d).clamp(0, max as isize)) as usize;
            } else {
                let n = self.history.as_ref().map(|h| h.len()).unwrap_or(0);
                self.history_scroll = ((self.history_scroll as isize + d)
                    .clamp(0, n.saturating_sub(1) as isize))
                    as usize;
            }
        } else if inside(self.areas.resp_body) {
            self.with_viewer(|v| v.scroll_by(d));
        } else if inside(self.areas.req_body) {
            let rects = self.areas.field_rects.clone();
            if let Some(t) = self.tab_mut() {
                let fields = t.fields();
                for (i, r) in rects.iter().enumerate() {
                    if inside(*r)
                        && let Some(f) = fields.get(i)
                    {
                        if let Some(e) = t.ed.text_mut(*f) {
                            e.scroll_by(d);
                        } else if let Some(tb) = t.ed.table_mut(*f) {
                            let max = tb.rows.len() as isize;
                            tb.scroll = ((tb.scroll as isize + d).clamp(0, max)) as usize;
                        }
                    }
                }
            }
        }
    }

    pub(super) fn on_drag(&mut self, x: u16, y: u16) {
        match self.drag.clone() {
            Some(Drag::Sidebar) => {
                let w = if self.project.config.ui.sidebar == "right" {
                    self.size.0.saturating_sub(x)
                } else {
                    x.saturating_sub(self.areas.sidebar.x) + 1
                };
                self.layout.sidebar_width = w.clamp(16, self.size.0.saturating_sub(30).max(16));
                self.session_dirty = true;
            }
            Some(Drag::Split) => {
                let main = self.areas.main;
                if main.height > 4 {
                    let rel = y.saturating_sub(main.y) as u32 * 100 / main.height as u32;
                    self.layout.request_pct = (rel as u16).clamp(20, 80);
                    self.session_dirty = true;
                }
            }
            Some(Drag::Tree { rel, start, active }) => {
                let moved = start.0.abs_diff(x) > 1 || start.1 != y;
                let active = active || moved;
                self.drag = Some(Drag::Tree {
                    rel: rel.clone(),
                    start,
                    active,
                });
                if active {
                    self.drop_target = match self.hit_at(x, y) {
                        Some((_, Hit::TreeRow(i))) => self.tree.rows.get(i).map(|r| {
                            if r.is_folder() {
                                r.rel.clone()
                            } else {
                                parent_rel(&r.rel).to_string()
                            }
                        }),
                        Some((_, Hit::SidebarArea)) => Some(String::new()),
                        _ => None,
                    }
                    .filter(|t| *t != rel && !t.starts_with(&format!("{rel}/")));
                }
            }
            Some(Drag::Text { field, bar }) => {
                let rect = if bar {
                    self.areas.bar_url
                } else {
                    field
                        .and_then(|f| self.areas.field_rects.get(f).copied())
                        .unwrap_or_default()
                };
                let rx = x.saturating_sub(rect.x);
                let ry = y.saturating_sub(rect.y).min(rect.height.saturating_sub(1));
                if let Some(e) = self.focused_text() {
                    e.click(rx, ry, true);
                }
            }
            None => {}
        }
    }

    pub(super) fn click(
        &mut self,
        hit: Hit,
        rect: Rect,
        x: u16,
        y: u16,
        double: bool,
        shift: bool,
    ) {
        if self.overlay.is_some() {
            if let Hit::Overlay(oh) = hit {
                self.overlay_click(oh, rect, x, y, double);
            }
            return;
        }
        // A click on the Ctrl+W menu runs that entry; a click elsewhere
        // closes the menu and acts as usual.
        if !self.pending.is_empty() {
            self.pending.clear();
            if let Hit::Menu(a) = hit {
                if let Some(a) = a {
                    self.dispatch(a);
                }
                return;
            }
        }
        if let Hit::CompleteItem(i) = hit {
            if let Some(c) = &mut self.complete {
                // The list may have re-filtered since the frame was drawn.
                c.sel = i.min(c.items.len().saturating_sub(1));
            }
            self.complete_accept();
            return;
        }
        self.complete = None;
        match hit {
            Hit::Tab(i) => {
                self.active = i;
                self.session_dirty = true;
            }
            Hit::TabClose(i) => {
                self.active = i;
                self.close_tab(false);
            }
            Hit::NewTab => self.new_tab(Request::new_http()),
            Hit::EnvSelector => self.open_env_picker(),
            Hit::HelpButton => self.open_help(),
            Hit::SidebarMode(m) => {
                self.sidebar_mode = m;
                self.focus = Focus::Sidebar;
                if m == SidebarMode::History {
                    self.load_history();
                }
            }
            Hit::SidebarArea => self.focus = Focus::Sidebar,
            Hit::TreeRow(i) => {
                self.focus = Focus::Sidebar;
                self.tree.selected = i;
                // A click on a pinned folder scrolls the tree to it.
                self.tree.follow = true;
                let row = self.tree.rows.get(i).cloned();
                if let Some(row) = row {
                    // Click on the folder arrow toggles, double-click opens requests.
                    let arrow_x = rect.x + (row.depth as u16) * 2;
                    if row.is_folder() && (double || x <= arrow_x + 1) {
                        self.tree.toggle(&row.rel);
                    } else if !row.is_folder() && double {
                        self.open_request(&row.rel);
                    }
                    self.drag = Some(Drag::Tree {
                        rel: row.rel,
                        start: (x, y),
                        active: false,
                    });
                }
            }
            Hit::TreeFilter => {
                self.focus = Focus::Filter;
                self.sidebar_mode = SidebarMode::Tree;
            }
            Hit::HistoryRow(i) => {
                self.focus = Focus::Sidebar;
                self.history_sel = i;
                if double {
                    self.open_history(i, false);
                }
            }
            Hit::RecentRow(i) => self.open_history(i, false),
            Hit::FavoriteRow(rel) => self.open_request(&rel),
            Hit::Bar(p) => {
                // The stop button (3) of an open stream is mouse-only: it stays
                // out of the keyboard focus cycle (ctrl+w x cancels).
                if p != 3 {
                    self.focus = Focus::Bar(p);
                }
                let kind = self.tab().map(|t| t.ed.kind);
                match (kind, p) {
                    (Some(_), 2) => self.press_send_button(),
                    (Some(_), 3) => {
                        // Disabled does nothing: cancel() would also stop
                        // background jobs that do not belong to this tab.
                        if self.tab().is_some_and(|t| t.running.is_some()) {
                            self.dispatch(Action::Cancel);
                        }
                    }
                    (Some(Kind::Http), 0) => self.open_method_picker(None),
                    (Some(Kind::Grpc), 0) => self.open_grpc_method_picker(),
                    // The tool chip: a click places the caret (part 1 does the
                    // same for the server input); the ▾ area opens the list.
                    (Some(Kind::Mcp), 0) => {
                        let (rx, ry) = (x.saturating_sub(rect.x), y.saturating_sub(rect.y));
                        if rx + 3 >= rect.width {
                            self.load_mcp_tools(true);
                        } else {
                            if let Some(e) = self.focused_text() {
                                e.click(rx, ry, shift);
                            }
                            self.drag = Some(Drag::Text {
                                field: None,
                                bar: true,
                            });
                        }
                    }
                    // The endpoint input of every kind: place the caret.
                    (Some(_), 1) => {
                        let (rx, ry) = (x.saturating_sub(rect.x), y.saturating_sub(rect.y));
                        if let Some(e) = self.focused_text() {
                            e.click(rx, ry, shift);
                        }
                        self.drag = Some(Drag::Text {
                            field: None,
                            bar: true,
                        });
                    }
                    _ => {}
                }
            }
            Hit::ReqTab(i) => {
                self.focus = Focus::ReqTabs;
                if let Some(t) = self.tab_mut() {
                    t.req_tab = i;
                    t.field = 0;
                }
            }
            Hit::ReqField(i) => {
                self.focus = Focus::ReqBody;
                if let Some(t) = self.tab_mut() {
                    t.field = i;
                    if let Some(f) = t.current_field() {
                        if f.is_toggle() {
                            t.ed.cycle(f, 1);
                        } else if f.is_choice() {
                            self.open_choice_picker(f);
                            return;
                        }
                    }
                }
                self.changed();
            }
            Hit::JsonError { line, col } => {
                if let Some(t) = self.tab_mut()
                    && let Some(i) = t.fields().iter().position(|f| f.is_multiline())
                {
                    t.field = i;
                    if let Some(f) = t.current_field()
                        && let Some(e) = t.ed.text_mut(f)
                    {
                        e.move_to(Pos { row: line, col });
                    }
                    self.focus = Focus::ReqBody;
                }
            }
            Hit::ChoiceOption { field, index } => {
                self.focus = Focus::ReqBody;
                if let Some(t) = self.tab_mut() {
                    t.field = field;
                    if let Some(f) = t.current_field() {
                        t.ed.set_choice(f, index);
                    }
                }
                self.changed();
            }
            Hit::TextArea { field } => {
                self.focus = Focus::ReqBody;
                let (rx, ry) = (x.saturating_sub(rect.x), y.saturating_sub(rect.y));
                if let Some(t) = self.tab_mut() {
                    t.field = field;
                    if let Some(f) = t.current_field()
                        && let Some(e) = t.ed.text_mut(f)
                    {
                        e.click(rx, ry, shift);
                    }
                }
                self.drag = Some(Drag::Text {
                    field: Some(field),
                    bar: false,
                });
            }
            Hit::TableCell { field, row, col } => {
                self.focus = Focus::ReqBody;
                let (rx, ry) = (x.saturating_sub(rect.x), y.saturating_sub(rect.y));
                let mut changed = false;
                if let Some(t) = self.tab_mut() {
                    t.field = field;
                    if let Some(f) = t.current_field()
                        && let Some(tb) = t.ed.table_mut(f)
                    {
                        tb.select(row, col);
                        if tb.editing_text() {
                            // Clicking the trailing "add" row creates it.
                            tb.ensure_row();
                            if let Some(e) = tb.editor_mut() {
                                e.click(rx, ry, false);
                            }
                        } else {
                            changed = tb.activate(1);
                        }
                    }
                }
                if changed {
                    self.changed();
                }
            }
            Hit::TableToggle { field, row } => {
                if let Some(t) = self.tab_mut() {
                    t.field = field;
                    if let Some(f) = t.current_field()
                        && let Some(tb) = t.ed.table_mut(f)
                    {
                        tb.select(row, 0);
                        tb.toggle_row();
                    }
                }
                self.focus = Focus::ReqBody;
                self.changed();
            }
            Hit::TableDelete { field, row } => {
                if let Some(t) = self.tab_mut() {
                    t.field = field;
                    if let Some(f) = t.current_field()
                        && let Some(tb) = t.ed.table_mut(f)
                    {
                        tb.select(row, 0);
                        tb.delete_row();
                    }
                }
                self.focus = Focus::ReqBody;
                self.changed();
            }
            Hit::RespTab(i) => {
                self.focus = Focus::RespTabs;
                if let Some(t) = self.tab_mut() {
                    t.resp_tab = i;
                }
            }
            Hit::RespView => {
                self.focus = Focus::RespBody;
                let ry = y.saturating_sub(rect.y);
                let gutter_click = x < rect.x + 2;
                self.with_viewer(|v| {
                    v.click(ry);
                    if gutter_click || double {
                        v.toggle_fold();
                    }
                });
            }
            Hit::RespQuery => self.focus = Focus::RespQuery,
            Hit::StreamMsg(i) => {
                self.focus = Focus::RespBody;
                if let Some(r) = self.tab_mut().and_then(|t| t.response.as_mut()) {
                    r.set_message(i);
                }
            }
            Hit::ViewMode(m) => {
                self.focus = Focus::RespBody;
                self.set_view(m);
            }
            Hit::RespAction(a) => match a {
                RespAction::Copy => self.copy_body(),
                RespAction::Save => self.prompt_save_body(),
                RespAction::Wrap => self.with_viewer(Viewer::toggle_wrap),
                RespAction::Cancel => self.cancel(),
                RespAction::SendMessage => self.send(false),
                RespAction::EndStream => self.end_stream(),
                RespAction::Template => self.load_schema(false, true, false),
                RespAction::Format => self.format_body(),
            },
            Hit::SidebarBorder => self.drag = Some(Drag::Sidebar),
            Hit::SplitBorder => self.drag = Some(Drag::Split),
            Hit::Overlay(_) => {}
            // Handled at the top of `click`, before the panes.
            Hit::CompleteItem(_) | Hit::Menu(_) => {}
        }
    }

    pub(super) fn overlay_click(
        &mut self,
        oh: OverlayHit,
        rect: Rect,
        x: u16,
        y: u16,
        double: bool,
    ) {
        let Some(mut ov) = self.overlay.take() else {
            return;
        };
        let mut keep = true;
        match (&mut ov, oh) {
            // The env and MCP editors route outside clicks through Esc so a
            // dirty editor still asks to save instead of dropping changes.
            (Overlay::Env(_), OverlayHit::Outside) => {
                self.overlay = Some(ov);
                self.overlay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
                return;
            }
            (Overlay::Picker(p), OverlayHit::Outside)
                if matches!(p.action, PickAction::EnvFile { .. }) || p.action.is_theme() =>
            {
                self.overlay = Some(ov);
                self.overlay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
                return;
            }
            (Overlay::Records(_) | Overlay::Keymap(_), OverlayHit::Outside) => {
                self.overlay = Some(ov);
                self.overlay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
                return;
            }
            // A "new environment" prompt returns to the env editor parked
            // behind it, as Esc does, so the editor's changes are kept.
            (Overlay::Prompt(p), OverlayHit::Outside)
                if !p.pending() && matches!(p.action, PromptAction::NewEnv) =>
            {
                self.overlay = Some(ov);
                self.overlay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
                return;
            }
            // A prompt or import dialog drops what was typed in it: ask first.
            (Overlay::Prompt(p), OverlayHit::Outside) if p.pending() => {
                let title = p.title.clone();
                self.parked_dialog = Some(ov);
                self.overlay = Some(Overlay::Confirm(Confirm {
                    title: "Unsaved text".into(),
                    message: format!("Discard the text typed into \"{title}\"?"),
                    buttons: vec!["Discard".into(), "Keep editing".into()],
                    sel: 1,
                    action: ConfirmAction::DialogClose,
                }));
                return;
            }
            (Overlay::Import(i), OverlayHit::Outside) if i.pending() => {
                self.parked_dialog = Some(ov);
                self.overlay = Some(Overlay::Confirm(Confirm {
                    title: "Unsaved text".into(),
                    message: "Discard the text typed into \"Import\"?".into(),
                    buttons: vec!["Discard".into(), "Keep editing".into()],
                    sel: 1,
                    action: ConfirmAction::DialogClose,
                }));
                return;
            }
            (Overlay::Import(_), OverlayHit::Outside) => keep = false,
            (_, OverlayHit::Outside) => keep = matches!(ov, Overlay::Confirm(_)),
            (Overlay::Picker(p), OverlayHit::Input) => {
                p.filter.click(x.saturating_sub(rect.x), 0, false);
            }
            (Overlay::Picker(p), OverlayHit::Item(i)) => {
                p.sel = i;
                if let Some(idx) = p.selected() {
                    let (action, item, detail) = (
                        p.action.clone(),
                        p.items[idx].clone(),
                        p.details.get(idx).cloned(),
                    );
                    self.pick(action, idx, &item, detail);
                }
                keep = false;
            }
            (Overlay::Palette(p), OverlayHit::Input) => {
                p.input.click(x.saturating_sub(rect.x), 0, false);
            }
            (Overlay::Palette(p), OverlayHit::Item(i)) => {
                p.sel = i;
                // Running the row is what Enter would do: it needs `armed`.
                p.armed = true;
                self.overlay = Some(ov);
                self.overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                return;
            }
            (Overlay::Confirm(c), OverlayHit::Button(i)) => {
                let action = c.action.clone();
                self.confirm(action, i);
                keep = false;
            }
            // A click selects a setting; the value chip (or a double click)
            // changes it, matching the keyboard's ←/→.
            (Overlay::Settings(v), OverlayHit::Item(i)) => {
                v.sel = i;
                v.edit = None;
                if double {
                    self.click_setting(v, i);
                }
            }
            (Overlay::Settings(v), OverlayHit::Value(i)) => {
                v.sel = i;
                self.click_setting(v, i);
            }
            // The Global / Project tabs choose the file.
            (Overlay::Settings(_), OverlayHit::Choice(i)) => {
                let want = if i == 0 {
                    ConfigScope::Global
                } else {
                    ConfigScope::Project
                };
                if self.config_scope != want {
                    self.change_setting(SettingId::Scope, 1);
                }
            }
            (Overlay::Prompt(p), OverlayHit::Choice(k)) => {
                p.focus = k + 1;
                p.open = p.choices.get(k).map(|c| c.2);
            }
            (Overlay::Prompt(p), OverlayHit::PromptOption(j)) => {
                if let Some(c) = p.choices.get_mut(p.focus.wrapping_sub(1)) {
                    c.2 = j.min(c.1.len().saturating_sub(1));
                }
                p.open = None;
                p.focus = 0;
            }
            (Overlay::Prompt(p), OverlayHit::Input) => {
                p.focus = 0;
                p.input.click(x.saturating_sub(rect.x), 0, false);
            }
            (Overlay::Prompt(_), OverlayHit::Button(0)) => {
                self.overlay = Some(ov);
                self.overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                return;
            }
            (Overlay::Prompt(_), OverlayHit::Button(_)) => {
                self.overlay = Some(ov);
                self.overlay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
                return;
            }
            (Overlay::Import(i), OverlayHit::Input) => {
                i.input
                    .click(x.saturating_sub(rect.x), y.saturating_sub(rect.y), false)
            }
            (Overlay::Import(_), OverlayHit::Button(0)) => {
                self.overlay = Some(ov);
                self.overlay_key(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE));
                return;
            }
            (Overlay::Import(_), OverlayHit::Button(_)) => keep = false,
            (Overlay::Env(e), OverlayHit::EnvDelete(row)) => {
                // The row's ✕, same as Ctrl+D.
                e.table.sel_row = row.min(e.table.rows.len());
                if e.table.delete_row() {
                    e.dirty = true;
                }
            }
            (Overlay::Env(e), OverlayHit::EnvTab(i)) => {
                if e.dirty {
                    e.error = Some("save (Ctrl+S) or discard changes before switching".into());
                } else {
                    e.current = i;
                    let mut ed = std::mem::take(e);
                    self.load_env_into(&mut ed);
                    *e = ed;
                }
            }
            (Overlay::Env(e), OverlayHit::EnvDotenv) => {
                e.focus = EnvFocus::Dotenv;
                e.dotenv.click(x.saturating_sub(rect.x), 0, false);
            }
            // A click on the value lists the themes, as in Settings.
            (Overlay::Env(e), OverlayHit::EnvTheme) => {
                e.focus = EnvFocus::Theme;
                let ed = std::mem::take(e);
                self.open_env_theme_picker(ed);
                return;
            }
            (Overlay::Env(e), OverlayHit::EnvCell { row, col }) => {
                e.focus = EnvFocus::Table;
                e.table.select(row, col);
                if e.table.editing_text() {
                    e.table.ensure_row();
                    if let Some(ed) = e.table.editor_mut() {
                        ed.click(x.saturating_sub(rect.x), 0, false);
                    }
                } else if e.table.activate(1) {
                    e.dirty = true;
                    self.refresh_env_rows(e);
                }
            }
            (Overlay::Env(_), OverlayHit::Button(0)) => {
                self.overlay = Some(ov);
                self.overlay_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
                return;
            }
            (Overlay::Env(_), OverlayHit::Button(1)) => {
                self.overlay = Some(ov);
                self.overlay_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
                return;
            }
            (Overlay::Env(_), OverlayHit::Button(_)) => {
                self.overlay = Some(ov);
                self.overlay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
                return;
            }
            (Overlay::Records(e), OverlayHit::RecButton(i)) => {
                let key = match (i, e.kind) {
                    (0, _) => KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
                    (1, _) => KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
                    (2, crate::records::RecordKind::Mcp) => {
                        KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL)
                    }
                    (2, _) => KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
                    _ => {
                        // Close: Esc from the list closes (it asks first
                        // when there are changes).
                        e.pane = crate::records::Pane::List;
                        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
                    }
                };
                self.overlay = Some(ov);
                self.overlay_key(key);
                return;
            }
            (
                Overlay::Records(e),
                hit @ (OverlayHit::RecList(_)
                | OverlayHit::RecField(_)
                | OverlayHit::RecCell { .. }
                | OverlayHit::RecDelete { .. }),
            ) => e.click(hit, x.saturating_sub(rect.x)),
            (Overlay::Keymap(e), OverlayHit::KeyTab(i)) => {
                e.mode = i.min(crate::keymap_editor::KEYMAP_MODES.len() - 1);
            }
            (Overlay::Keymap(e), OverlayHit::KeyRow(i)) => e.sel = i,
            (Overlay::Keymap(_), OverlayHit::KeyButton(i)) => {
                let key = match i {
                    0 => KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
                    1 => KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                    2 => KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
                    _ => KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
                };
                self.overlay = Some(ov);
                self.overlay_key(key);
                return;
            }
            (Overlay::Report(r), OverlayHit::Button(0)) => {
                self.clipboard.set(r.copy_text.clone(), false);
                r.copied = true;
                self.set_status("copied (sent to the terminal clipboard)", StatusKind::Ok);
            }
            (Overlay::Report(r), OverlayHit::Button(_)) => {
                if let Some(m) = r.close_status.clone() {
                    self.set_status(m, StatusKind::Ok);
                }
                keep = false;
            }
            (Overlay::Help(h), OverlayHit::Input) => {
                h.filter.click(x.saturating_sub(rect.x), 0, false);
            }
            (Overlay::Help(h), OverlayHit::Choice(i)) => h.select(i),
            (Overlay::Help(h), OverlayHit::Item(off)) => h
                .viewer
                .click((y.saturating_sub(rect.y) as usize + off).min(u16::MAX as usize) as u16),
            _ => {
                let _ = double;
            }
        }
        if keep && self.overlay.is_none() {
            self.overlay = Some(ov);
        }
    }
}
