//! Event and key routing: terminal events, the focused widget, the request bar and body keys.

use super::*;

impl App {
    // ----- events -------------------------------------------------------

    pub fn on_event(&mut self, ev: Event) {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => self.on_key(k),
            Event::Mouse(m) if self.project.config.ui.mouse => self.on_mouse(m),
            Event::Paste(s) => self.on_paste(&s),
            Event::Resize(w, h) => self.size = (w, h),
            _ => {}
        }
    }

    pub fn on_paste(&mut self, s: &str) {
        if let Some(ov) = &mut self.overlay {
            match ov {
                Overlay::Import(i) => i.input.insert_str(s),
                Overlay::Prompt(p) => p.input.insert_str(s),
                Overlay::Picker(p) => p.filter.insert_str(s),
                Overlay::Palette(p) => {
                    p.input.insert_str(s);
                    p.sel = 0;
                    p.armed = true;
                }
                // A paste is an edit: Esc must ask before it is lost.
                Overlay::Env(e) => {
                    e.table.paste(s);
                    e.dirty = true;
                }
                Overlay::Mcp(m) => {
                    m.table.paste(s);
                    m.dirty = true;
                }
                Overlay::Cfg(e) => {
                    if let Some(t) = e.table() {
                        t.paste(s);
                        e.dirty = true;
                    }
                }
                _ => {}
            }
            return;
        }
        // A pasted curl command in the URL bar becomes a request.
        if matches!(self.focus, Focus::Bar(1)) && s.trim_start().starts_with("curl ") {
            self.import_curl(s);
            return;
        }
        match self.focus {
            Focus::Filter => {
                self.filter.insert_str(s);
                let f = self.filter.text();
                self.tree.set_filter(&f);
            }
            Focus::Bar(_) | Focus::ReqBody => {
                if let Some(e) = self.focused_text() {
                    e.insert_str(s);
                } else if let Some(t) = self.focused_table() {
                    t.paste(s);
                }
                self.changed();
            }
            _ => {}
        }
    }

    pub(super) fn changed(&mut self) {
        if let Some(t) = self.tab_mut() {
            t.refresh_model();
        }
        // An edit makes "created …" / "saved …" stale.
        self.clear_transient_status();
        self.session_dirty = true;
    }

    pub(super) fn focused_text(&mut self) -> Option<&mut TextEditor> {
        let focus = self.focus;
        let tab = self.tabs.get_mut(self.active)?;
        match focus {
            Focus::Bar(p) => tab.ed.bar_text(p),
            Focus::ReqBody => {
                let f = tab.current_field()?;
                tab.ed.text_mut(f)
            }
            _ => None,
        }
    }

    pub(super) fn focused_table(&mut self) -> Option<&mut crate::form::Table> {
        let tab = self.tabs.get_mut(self.active)?;
        if self.focus != Focus::ReqBody {
            return None;
        }
        let f = tab.current_field()?;
        tab.ed.table_mut(f)
    }

    /// Whether the focused widget is a text editor in vim normal mode (or not text).
    pub(super) fn in_vim_normal(&mut self) -> bool {
        if !self.vim {
            return false;
        }
        if let Some(e) = self.focused_text() {
            return vim_label(e) != VimMode::Insert;
        }
        if let Some(t) = self.focused_table() {
            return t.editor_mut().is_none_or(|e| e.vim != VimMode::Insert);
        }
        true
    }

    pub(super) fn contexts(&mut self) -> Vec<Ctx> {
        let is_table = self.focused_table().is_some();
        match self.focus {
            Focus::Sidebar if self.sidebar_mode == SidebarMode::Tree => {
                vec![Ctx::Tree, Ctx::Global]
            }
            Focus::RespBody | Focus::RespTabs => vec![Ctx::Response, Ctx::Global],
            Focus::ReqBody if is_table => vec![Ctx::Table, Ctx::Global],
            _ => vec![Ctx::Global],
        }
    }

    /// Runs `f` on the focused text editor together with the clipboard.
    pub(super) fn with_text<R>(
        &mut self,
        f: impl FnOnce(&mut TextEditor, &mut Clipboard) -> R,
    ) -> Option<R> {
        let focus = self.focus;
        let tab = self.tabs.get_mut(self.active)?;
        let cb = &mut self.clipboard;
        let ed = match focus {
            Focus::Bar(p) => tab.ed.bar_text(p)?,
            Focus::ReqBody => {
                let fld = tab.current_field()?;
                tab.ed.text_mut(fld)?
            }
            _ => return None,
        };
        Some(f(ed, cb))
    }

    /// Runs `f` on the focused table together with the clipboard.
    pub(super) fn with_table<R>(
        &mut self,
        f: impl FnOnce(&mut crate::form::Table, &mut Clipboard) -> R,
    ) -> Option<R> {
        if self.focus != Focus::ReqBody {
            return None;
        }
        let tab = self.tabs.get_mut(self.active)?;
        let cb = &mut self.clipboard;
        let fld = tab.current_field()?;
        let t = tab.ed.table_mut(fld)?;
        Some(f(t, cb))
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        // A confirmation or note lasts until the next key press; warnings and
        // errors stay until replaced.
        self.clear_transient_status();
        if self.overlay.is_some() {
            self.complete = None;
            self.overlay_key(key);
            return;
        }
        // The popup sees its own keys (and Enter/Tab acceptance) before the
        // editor or the keymap does.
        if self.complete.is_some() && self.complete_key(key) {
            return;
        }
        let chord = Chord::from_event(&key);
        // The query bar closes with a new response or on another tab.
        if self.focus == Focus::RespQuery && !self.query_shown() {
            self.focus = Focus::RespBody;
        }
        // The query bar keeps Tab (completion) and Up/Down (history).
        if self.focus == Focus::RespQuery
            && self.pending.is_empty()
            && matches!(key.code, KeyCode::Tab | KeyCode::Up | KeyCode::Down)
            && key.modifiers.is_empty()
        {
            self.query_key(key);
            return;
        }
        // Tab inside a multi-line editor indents (unless in vim normal mode).
        if chord == Chord::new(KeyCode::Tab, KeyModifiers::NONE)
            && self.pending.is_empty()
            && self.focus == Focus::ReqBody
        {
            let multiline = self
                .tab()
                .and_then(|t| t.current_field())
                .is_some_and(Field::is_multiline);
            if multiline && !self.in_vim_normal() {
                self.forward_to_widget(key);
                return;
            }
        }
        // Vim command mode.
        if self.vim
            && self.pending.is_empty()
            && chord == Chord::new(KeyCode::Char(':'), KeyModifiers::NONE)
            && self.in_vim_normal()
            && self.focus != Focus::Filter
        {
            self.open_palette(":");
            return;
        }
        let mut seq = self.pending.clone();
        seq.push(chord);
        let ctxs = self.contexts();
        // Text widgets get plain keys first; only global bindings apply there.
        let text_focus =
            matches!(self.focus, Focus::Filter | Focus::RespQuery) || self.focused_text().is_some();
        let ctxs: Vec<Ctx> = if text_focus { vec![Ctx::Global] } else { ctxs };
        // In vim normal mode inside tables, `dd` deletes rows like lines.
        match self.keymap.lookup(&ctxs, self.vim, &seq) {
            Lookup::Action(a) => {
                self.pending.clear();
                self.dispatch(a);
                // An overlay takes the screen; the popup does not outlive it.
                if self.overlay.is_some() {
                    self.complete = None;
                }
            }
            Lookup::Prefix => {
                self.pending = seq;
            }
            Lookup::None => {
                let had_pending = !self.pending.is_empty();
                self.pending.clear();
                if !had_pending {
                    self.forward_to_widget(key);
                }
            }
        }
    }

    pub(super) fn forward_to_widget(&mut self, key: KeyEvent) {
        let vim = self.vim;
        match self.focus {
            Focus::Sidebar => self.sidebar_key(key),
            Focus::Filter => {
                let out = self.filter.handle_key(&key, false, &mut self.clipboard);
                match out {
                    Outcome::Changed => {
                        let f = self.filter.text();
                        self.tree.set_filter(&f);
                    }
                    Outcome::Submit => self.focus = Focus::Sidebar,
                    Outcome::Leave => {
                        self.filter.set_text("");
                        self.tree.set_filter("");
                        self.focus = Focus::Sidebar;
                    }
                    Outcome::Ignored if matches!(key.code, KeyCode::Down) => {
                        self.focus = Focus::Sidebar
                    }
                    _ => {}
                }
            }
            Focus::Bar(part) => self.bar_key(part, key),
            Focus::ReqTabs => match key.code {
                KeyCode::Left => self.subtab(-1),
                KeyCode::Right => self.subtab(1),
                KeyCode::Char('h') if vim => self.subtab(-1),
                KeyCode::Char('l') if vim => self.subtab(1),
                KeyCode::Enter | KeyCode::Down => self.enter_req_body(0),
                KeyCode::Char('j') if vim => self.enter_req_body(0),
                KeyCode::Up => self.focus = self.top_bar_focus(),
                KeyCode::Char('k') if vim => self.focus = self.top_bar_focus(),
                _ => {}
            },
            Focus::ReqBody => self.req_body_key(key),
            Focus::RespTabs => match key.code {
                KeyCode::Left => self.subtab(-1),
                KeyCode::Right => self.subtab(1),
                KeyCode::Enter | KeyCode::Down => self.focus = Focus::RespBody,
                _ => {}
            },
            Focus::RespBody => {}
            Focus::RespQuery => self.query_key(key),
        }
    }

    pub(super) fn enter_req_body(&mut self, field: usize) {
        if let Some(t) = self.tab_mut() {
            if t.fields().is_empty() {
                return;
            }
            t.field = field.min(t.fields().len() - 1);
            self.focus = Focus::ReqBody;
        }
    }

    pub(super) fn bar_key(&mut self, part: usize, key: KeyEvent) {
        let Some(kind) = self.tab().map(|t| t.ed.kind) else {
            return;
        };
        if kind == Kind::Mcp && key.code == KeyCode::F(2) {
            // F2 on the server input lists the configured servers (a direct
            // URL needs none); on the tool chip it discovers the tools.
            if part == 1
                && self
                    .tab()
                    .is_some_and(|t| t.ed.mcp_server.text().is_empty())
            {
                self.open_mcp_server_picker();
            } else {
                self.load_mcp_tools(true);
            }
            return;
        }
        let vim = self.vim;
        // Part 1 is the endpoint input of every kind; the gRPC method and MCP
        // tool chips (part 0) are editable inline.
        let is_text = part == 1 || (matches!(kind, Kind::Grpc | Kind::Mcp) && part == 0);
        if is_text {
            let Some(out) = self.with_text(|e, cb| e.handle_key(&key, vim, cb)) else {
                return;
            };
            match out {
                Outcome::Changed => {
                    self.changed();
                    if self.complete.is_some() {
                        self.complete_typed();
                    } else {
                        self.refresh_complete();
                    }
                }
                Outcome::Submit => {
                    self.complete = None;
                    match (kind, part) {
                        // Enter on any endpoint input sends.
                        (
                            Kind::Http | Kind::Ws | Kind::Sse | Kind::Mqtt | Kind::Grpc | Kind::Mcp,
                            1,
                        ) => self.dispatch(Action::Send),
                        // Enter on the method chip opens the full list.
                        (Kind::Grpc, 0) => self.open_grpc_method_picker(),
                        // Enter on the tool chip discovers and lists the tools.
                        (Kind::Mcp, 0) => self.load_mcp_tools(true),
                        _ => {}
                    }
                }
                Outcome::Leave => self.complete = None,
                Outcome::Ignored => {
                    if matches!(key.code, KeyCode::Down) {
                        self.focus = Focus::ReqTabs;
                    }
                }
                // A cursor move (Handled, not Changed) leaves the key
                // position: the popup's span would be stale.
                Outcome::Handled => self.complete = None,
            }
            return;
        }
        match (part, key.code) {
            (2, KeyCode::Enter | KeyCode::Char(' ')) => self.press_send_button(),
            (0, KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Down) if kind == Kind::Http => {
                self.open_method_picker(None)
            }
            (0, KeyCode::Char(c)) if kind == Kind::Http && c.is_ascii_alphabetic() => {
                self.open_method_picker(Some(c))
            }
            (0, KeyCode::Left | KeyCode::Right) if kind == Kind::Http => {
                let dir: isize = if key.code == KeyCode::Left { -1 } else { 1 };
                if let Some(t) = self.tab_mut() {
                    let i = HTTP_METHODS
                        .iter()
                        .position(|m| *m == t.ed.method)
                        .unwrap_or(0) as isize;
                    t.ed.method = HTTP_METHODS
                        [crate::list::cycle(i as usize, dir, HTTP_METHODS.len())]
                    .to_string();
                }
                self.changed();
            }
            (_, KeyCode::Down) => self.focus = Focus::ReqTabs,
            _ => {}
        }
    }

    pub(super) fn req_body_key(&mut self, key: KeyEvent) {
        let vim = self.vim;
        let Some(field) = self.tab().and_then(|t| t.current_field()) else {
            self.focus = Focus::ReqTabs;
            return;
        };
        if key.code == KeyCode::F(2) {
            match field {
                Field::McpServer => {
                    self.open_mcp_server_picker();
                    return;
                }
                Field::McpTool => {
                    self.load_mcp_tools(true);
                    return;
                }
                _ => {}
            }
        }
        if field.is_table() {
            let Some(out) = self.with_table(|t, cb| t.handle_key(&key, vim, cb)) else {
                return;
            };
            match out {
                TableOutcome::Changed => self.changed(),
                TableOutcome::ExitUp => self.prev_field(),
                TableOutcome::ExitDown => {
                    self.next_field(false);
                }
                TableOutcome::Leave => self.focus = Focus::ReqTabs,
                TableOutcome::Handled | TableOutcome::Ignored => {
                    if vim && key.code == KeyCode::Char('d') {
                        // `dd` on a row in vim normal mode.
                    }
                }
            }
            return;
        }
        if field.is_choice() || field.is_toggle() {
            match key.code {
                KeyCode::Enter | KeyCode::Char(' ') if field.is_choice() => {
                    self.open_choice_picker(field)
                }
                KeyCode::Char(c)
                    if field.is_choice()
                        && c.is_alphanumeric()
                        && !(vim && matches!(c, 'j' | 'k')) =>
                {
                    self.open_choice_picker(field);
                    if let Some(Overlay::Picker(p)) = &mut self.overlay {
                        p.filter.set_text(&c.to_string());
                        p.filter.move_to_end();
                        p.sel = 0;
                    }
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    if let Some(t) = self.tab_mut() {
                        t.ed.cycle(field, 1);
                    }
                    self.changed();
                }
                KeyCode::Left | KeyCode::Right => {
                    let dir = if key.code == KeyCode::Left { -1 } else { 1 };
                    if let Some(t) = self.tab_mut() {
                        t.ed.cycle(field, dir);
                    }
                    self.changed();
                }
                KeyCode::Down => {
                    self.next_field(false);
                }
                KeyCode::Up => self.prev_field(),
                KeyCode::Char('j') if vim => {
                    self.next_field(false);
                }
                KeyCode::Char('k') if vim => self.prev_field(),
                KeyCode::Esc => self.focus = Focus::ReqTabs,
                _ => {}
            }
            return;
        }
        let Some(out) = self.with_text(|e, cb| e.handle_key(&key, vim, cb)) else {
            return;
        };
        match out {
            Outcome::Changed => {
                self.changed();
                if self.complete.is_some() {
                    self.complete_typed();
                } else {
                    self.refresh_complete();
                }
            }
            Outcome::Submit => {
                self.complete = None;
                self.next_field(false);
            }
            Outcome::Leave => {
                self.complete = None;
                self.focus = Focus::ReqTabs;
            }
            Outcome::Ignored => match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.prev_field(),
                KeyCode::Down | KeyCode::Char('j') => {
                    self.next_field(false);
                }
                _ => {}
            },
            // A cursor move (Handled, not Changed) leaves the key position:
            // the popup's span would be stale.
            Outcome::Handled => self.complete = None,
        }
    }

    pub(super) fn prev_field(&mut self) {
        self.complete = None;
        if let Some(t) = self.tab_mut()
            && t.field > 0
        {
            t.field -= 1;
            return;
        }
        self.focus = Focus::ReqTabs;
    }

    /// Moves to the next field; at the end, to the response (when `wrap`).
    pub(super) fn next_field(&mut self, wrap: bool) -> bool {
        self.complete = None;
        if let Some(t) = self.tab_mut()
            && t.field + 1 < t.fields().len()
        {
            t.field += 1;
            if let Some(f) = t.current_field()
                && let Some(tb) = t.ed.table_mut(f)
            {
                tb.sel_row = 0;
            }
            return true;
        }
        if wrap {
            self.focus = Focus::RespTabs;
        }
        false
    }
}
