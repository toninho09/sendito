//! The hint row.

use super::*;

impl App {
    // ----- hints --------------------------------------------------------

    /// Context-sensitive hints for the hint row: (keys, label).
    /// The send keys for hints: the first two bindings ("Ctrl+Enter/F5"), as
    /// Ctrl+Enter only works in terminals that report it.
    pub fn send_hint(&self) -> String {
        let keys = self.keymap.keys_for(Action::Send, self.vim);
        match keys.as_slice() {
            [] => "(unbound)".into(),
            [one] => one.clone(),
            [a, b, ..] => format!("{a}/{b}"),
        }
    }

    /// The verb for running a request: "send" for HTTP and gRPC alike,
    /// matching the Send button.
    pub fn send_verb(&self) -> &'static str {
        "send"
    }

    pub fn hints(&mut self) -> Vec<(String, String)> {
        let k = |s: &Self, a: Action| s.keymap.hint(a, s.vim);
        let mut v: Vec<(String, String)> = Vec::new();
        if self.complete.is_some() && self.overlay.is_none() {
            v.extend([
                ("type".into(), "filter".into()),
                ("↑↓".into(), "move".into()),
                ("Enter/Tab".into(), "insert".into()),
                ("Esc".into(), "close".into()),
            ]);
            v.push((k(self, Action::Palette), "commands".into()));
            v.push((k(self, Action::Help), "all keys".into()));
            return v;
        }
        if let Some(ov) = &self.overlay {
            match ov {
                Overlay::Palette(_) => v.extend([
                    ("↑↓".into(), "move".into()),
                    ("Enter".into(), "run".into()),
                    (":".into(), "commands".into()),
                    ("Esc".into(), "close".into()),
                ]),
                Overlay::Picker(_) => v.extend([
                    ("type".into(), "filter".into()),
                    ("↑↓".into(), "move".into()),
                    ("Enter".into(), "choose".into()),
                    ("Esc".into(), "close".into()),
                ]),
                Overlay::Prompt(p) if p.open.is_some() => v.extend([
                    ("↑↓".into(), "move".into()),
                    ("Enter".into(), "choose".into()),
                    ("Esc".into(), "close list".into()),
                ]),
                Overlay::Prompt(p) if p.focus > 0 => v.extend([
                    ("Enter".into(), prompt_verb(&p.action).into()),
                    ("←→".into(), "change".into()),
                    ("Space".into(), "list".into()),
                    ("Tab".into(), "next field".into()),
                    ("Esc".into(), "cancel".into()),
                ]),
                Overlay::Prompt(p) => v.extend([
                    ("Enter".into(), prompt_verb(&p.action).into()),
                    ("Tab".into(), "next field".into()),
                    ("Esc".into(), "cancel".into()),
                ]),
                // The dialog's own keys, then Enter on the focused button.
                Overlay::Confirm(c) => {
                    let b = |i: usize| c.buttons.get(i).cloned().unwrap_or_default().to_lowercase();
                    if c.buttons.len() == 3 {
                        v.extend([("s".into(), b(0)), ("d".into(), b(1))]);
                    } else {
                        v.push(("y".into(), b(0)));
                    }
                    v.extend([
                        (
                            "Enter".into(),
                            c.buttons
                                .get(c.sel)
                                .cloned()
                                .unwrap_or_default()
                                .to_lowercase(),
                        ),
                        ("←→".into(), "choose".into()),
                        ("Esc".into(), "cancel".into()),
                    ]);
                }
                // Save, New env and Close are on the dialog's buttons.
                Overlay::Env(e) => {
                    use crate::overlay::env_editor as t;
                    if e.on_dotenv {
                        v.extend([
                            ("F2".into(), "browse".into()),
                            ("Tab".into(), "complete path".into()),
                            ("↓".into(), "variables".into()),
                        ]);
                    } else {
                        v.extend([
                            ("Ctrl+D".into(), "delete row".into()),
                            ("Space".into(), "type".into()),
                        ]);
                        match e.selected_type().filter(|_| e.table.sel_col == 2) {
                            Some(t::FILE) => v.push(("F2".into(), "browse".into())),
                            Some(s) if t::holds_secret(s) => {
                                v.push(("type".into(), "set secret".into()))
                            }
                            _ => {}
                        }
                    }
                    v.extend([
                        ("Ctrl+S".into(), "save".into()),
                        ("Ctrl+PgUp/Dn".into(), "env".into()),
                        (
                            "Esc".into(),
                            if e.dirty {
                                "close (asks to save)"
                            } else {
                                "close"
                            }
                            .into(),
                        ),
                    ]);
                }
                Overlay::Keymap(e) => {
                    if e.capture.is_some() {
                        v.extend([
                            ("keys".into(), "record".into()),
                            ("pause 1 s".into(), "keep".into()),
                            ("Esc".into(), "cancel".into()),
                        ]);
                    } else {
                        v.extend([
                            ("Enter".into(), "change".into()),
                            ("^A".into(), "add a key".into()),
                            ("Del".into(), "default".into()),
                            ("^D".into(), "no key".into()),
                            ("Tab".into(), "mode".into()),
                            ("^S".into(), "save".into()),
                            (
                                "Esc".into(),
                                if e.dirty {
                                    "close (asks to save)"
                                } else {
                                    "close"
                                }
                                .into(),
                            ),
                        ]);
                    }
                }
                Overlay::Records(e) => {
                    use crate::records::{Input, Pane, RecordKind};
                    v.push(("Ctrl+S".into(), "save".into()));
                    if e.pane == Pane::List {
                        v.extend([
                            ("↑↓".into(), "choose".into()),
                            ("Enter".into(), "edit".into()),
                            ("Ctrl+N".into(), "new".into()),
                            ("Ctrl+D".into(), "delete".into()),
                        ]);
                    } else {
                        let input = e.focused_field().map(|f| match &f.input {
                            Input::Choice { .. } => 1,
                            Input::Toggle(_) => 2,
                            Input::Table(_) => 3,
                            _ => 0,
                        });
                        match input {
                            Some(1) => v.push(("←→".into(), "change".into())),
                            Some(2) => v.push(("Space".into(), "toggle".into())),
                            Some(3) => v.extend([
                                ("Enter".into(), "next cell".into()),
                                ("Ctrl+D".into(), "delete row".into()),
                            ]),
                            _ => {}
                        }
                        v.push(("Tab ↑↓".into(), "fields".into()));
                    }
                    match e.kind {
                        RecordKind::Mcp => v.push(("Ctrl+T".into(), "test".into())),
                        RecordKind::OAuth => v.push(("Ctrl+L".into(), "sign in".into())),
                    }
                    v.push((
                        "Esc".into(),
                        match (e.pane, e.dirty) {
                            (Pane::Form, _) => "list",
                            (_, true) => "close (asks to save)",
                            _ => "close",
                        }
                        .into(),
                    ));
                }
                Overlay::Import(_) => v.extend([
                    ("F5 / Ctrl+S".into(), "import".into()),
                    ("Tab".into(), "complete path".into()),
                    ("Esc".into(), "cancel".into()),
                ]),
                Overlay::Report(r) => v.extend([
                    (
                        "y".into(),
                        if r.copied { "copy again" } else { "copy" }.into(),
                    ),
                    ("↑↓".into(), "scroll".into()),
                    ("Esc".into(), "close".into()),
                    (":report".into(), "shows this report again".into()),
                ]),
                Overlay::Help { .. } => v.extend([
                    ("type".into(), "search".into()),
                    ("↑↓ PgUp PgDn".into(), "scroll".into()),
                    ("Esc".into(), "close".into()),
                ]),
                Overlay::Settings(s) if s.edit.is_some() => v.extend([
                    ("Enter".into(), "save".into()),
                    ("Esc".into(), "cancel".into()),
                ]),
                Overlay::Settings(s) => v.extend([
                    ("↑↓".into(), "move".into()),
                    if SETTINGS.get(s.sel).is_some_and(|(id, _)| id.typed()) {
                        ("Enter".into(), "edit".into())
                    } else if SETTINGS.get(s.sel).is_some_and(|(id, _)| id.opens()) {
                        ("Enter".into(), "open the editor".into())
                    } else {
                        ("←→ Space".into(), "change".into())
                    },
                    ("type".into(), "filter".into()),
                    ("Tab".into(), "global / project".into()),
                    ("Esc".into(), "close".into()),
                ]),
            }
            return v;
        }
        // While a tree item is dragged, say what releasing it does.
        if let Some(Drag::Tree {
            rel, active: true, ..
        }) = &self.drag
        {
            let name = sendito_store::collection::name_of(rel);
            let what = match self.drop_target.as_deref() {
                Some("") => format!("to move {name} into / (collection root)"),
                Some(t) => format!("to move {name} into {t}/"),
                None => "outside a folder: nothing moves".to_string(),
            };
            v.push(("release".into(), what));
            return v;
        }
        if !self.pending.is_empty() {
            let p = self
                .pending
                .iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join(" ");
            v.push((format!("{p} …"), "next key".into()));
            return v;
        }
        let running = self.tab().is_some_and(|t| t.running.is_some());
        let streaming = self
            .tab()
            .and_then(|t| t.running.as_ref())
            .is_some_and(|r| r.streaming_input);
        // Keys of a running call come first, whichever pane has focus.
        if streaming {
            v.extend([
                (self.send_hint(), "send message".into()),
                (k(self, Action::EndStream), "end stream".into()),
                (k(self, Action::Cancel), "cancel".into()),
            ]);
        } else if running {
            v.push((k(self, Action::Cancel), "cancel".into()));
        }
        match self.focus {
            Focus::Sidebar if self.sidebar_mode == SidebarMode::History => {
                v.extend([
                    ("Enter".into(), "open".into()),
                    (self.send_hint(), "resend entry".into()),
                    ("s".into(), "save to tree".into()),
                    (k(self, Action::History), "back to tree".into()),
                ]);
            }
            Focus::Sidebar => {
                // Most used and destructive keys first: narrow screens drop the tail.
                if self.vim {
                    v.push(("j/k".into(), "up/down".into()));
                    v.push(("h/l".into(), "collapse/expand".into()));
                }
                v.extend([
                    ("Enter".into(), "open / fold".into()),
                    ("n".into(), "new request".into()),
                    ("Del/x".into(), "delete".into()),
                    ("/".into(), "filter".into()),
                    ("r".into(), "rename".into()),
                    ("N".into(), "new folder".into()),
                    ("m / drag".into(), "move".into()),
                    ("d".into(), "duplicate".into()),
                    ("f".into(), "favorite".into()),
                ]);
            }
            Focus::Filter => v.extend([
                ("type".into(), "filter".into()),
                ("Enter/↓".into(), "to results".into()),
                ("Esc".into(), "clear".into()),
            ]),
            Focus::Bar(_) | Focus::ReqTabs | Focus::ReqBody => {
                let no_method = self.tab().is_some_and(|t| {
                    matches!(&t.model, Request::Grpc(g) if g.service.is_empty() || g.method.is_empty())
                });
                if no_method {
                    v.push(("method ▾".into(), "pick one to enable Send".into()));
                } else if !running {
                    v.push((self.send_hint(), self.send_verb().into()));
                }
                // Save comes right after send so narrow terminals keep it.
                v.push((k(self, Action::Save), "save".into()));
                // In vim insert or visual mode, how to get back out.
                let mode = self.focused_text().map(|e| e.vim);
                if self.vim
                    && matches!(
                        mode,
                        Some(
                            crate::editor::VimMode::Insert
                                | crate::editor::VimMode::Visual
                                | crate::editor::VimMode::VisualLine
                        )
                    )
                {
                    v.push(("Esc".into(), "normal mode".into()));
                } else if self.vim && mode == Some(crate::editor::VimMode::Normal) {
                    v.push(("i".into(), "insert".into()));
                }
                let field = self.tab().and_then(|t| t.current_field());
                match (self.focus, field) {
                    (Focus::ReqTabs, _) => v.extend([
                        ("Enter/↓".into(), "edit".into()),
                        ("←→".into(), "switch tab".into()),
                    ]),
                    (Focus::ReqBody, Some(Field::McpServer)) => {
                        v.push(("F2".into(), "choose server".into()))
                    }
                    (Focus::ReqBody, Some(Field::McpTool)) => {
                        v.push(("F2".into(), "choose tool".into()))
                    }
                    (Focus::ReqBody, Some(Field::AuthOAuthName)) => {
                        v.push(("F2".into(), "choose identity".into()))
                    }
                    (Focus::Bar(1), _) if self.tab().is_some_and(|t| t.ed.kind == Kind::Mcp) => {
                        v.push(("F2".into(), "choose server".into()))
                    }
                    (Focus::Bar(0), _) if self.tab().is_some_and(|t| t.ed.kind == Kind::Mcp) => {
                        v.push(("Enter".into(), "choose tool".into()))
                    }
                    (Focus::ReqBody, Some(f)) if f.is_table() => {
                        // A choice column (e.g. a multipart part's kind).
                        let choice = self
                            .tab_mut()
                            .and_then(|t| t.ed.table_mut(f))
                            .and_then(|t| t.columns.get(t.sel_col).map(|c| (c.title, c.kind)))
                            .and_then(|(title, kind)| {
                                matches!(kind, crate::form::ColKind::Choice(_)).then_some(title)
                            });
                        if let Some(title) = choice {
                            v.push(("Space".into(), format!("change {}", title.to_lowercase())));
                        }
                        v.extend([
                            ("Enter".into(), "next cell".into()),
                            (k(self, Action::DeleteRow), "delete row".into()),
                            (k(self, Action::ToggleRow), "enable/disable".into()),
                        ]);
                    }
                    (Focus::ReqBody, Some(Field::BodyKind)) => v.extend([
                        ("←→".into(), "change type".into()),
                        ("Enter".into(), "choose".into()),
                    ]),
                    (Focus::ReqBody, Some(Field::DocsPreview)) => v.extend([
                        ("↑↓ PgUp/PgDn".into(), "scroll".into()),
                        ("Enter".into(), "edit".into()),
                    ]),
                    (Focus::ReqBody, Some(f))
                        if f.is_secret()
                            && self
                                .tab_mut()
                                .and_then(|t| t.ed.text_mut(f))
                                .is_some_and(|e| crate::tab::plain_secret(&e.text())) =>
                    {
                        v.push((k(self, Action::MoveToSecret), "move to secret".into()))
                    }
                    (Focus::ReqBody, Some(f)) if f.is_choice() => {
                        v.push(("Enter/Space".into(), "choose".into()))
                    }
                    (Focus::ReqBody, Some(f)) if f.is_toggle() => {
                        v.push(("Space".into(), "toggle".into()))
                    }
                    (Focus::ReqBody, Some(f)) if f.is_multiline() => {
                        // The way out of the editor comes right after Send, so
                        // it survives on narrow screens.
                        let at = v
                            .iter()
                            .position(|(_, l)| l == "send" || l == "send message")
                            .map_or(v.len(), |i| i + 1);
                        v.insert(at, ("Esc".into(), "leave editor".into()));
                        if !self.vim {
                            v.push((k(self, Action::SelectAll), "select all".into()));
                        }
                        if f != Field::Description {
                            v.push((k(self, Action::FormatBody), "format JSON".into()));
                        }
                    }
                    (Focus::Bar(0), _)
                        if self
                            .tab()
                            .is_some_and(|t| matches!(t.ed.kind, Kind::Http | Kind::Grpc)) =>
                    {
                        v.push(("Enter".into(), "choose method".into()))
                    }
                    _ => {}
                }
            }
            Focus::RespQuery => {
                v.push(("Enter".into(), "keep".into()));
                v.push(("Esc".into(), "close".into()));
                let jq = self
                    .tab()
                    .and_then(|t| t.response.as_ref())
                    .and_then(|r| r.query.as_ref())
                    .is_some_and(|q| q.mode == QueryMode::Jq);
                if jq {
                    v.push(("Tab".into(), "complete".into()));
                    v.push(("↑↓".into(), "history".into()));
                }
            }
            Focus::RespTabs | Focus::RespBody => {
                v.push(("Alt+1-4".into(), "tabs".into()));
                // View modes, folding and saving only apply to the body.
                if self.tab().is_some_and(|t| t.resp_tab == 0) {
                    let stream = self
                        .tab()
                        .and_then(|t| t.response.as_ref())
                        .is_some_and(|r| r.stream.kept() > 1);
                    if stream {
                        v.push(("[ ]".into(), "message".into()));
                    }
                    v.extend([
                        ("y".into(), "copy".into()),
                        ("p/r/x".into(), "view".into()),
                        ("/".into(), "search".into()),
                        (k(self, Action::Filter), "jq".into()),
                        ("s".into(), "save body".into()),
                        ("Enter".into(), "fold".into()),
                        ("w".into(), "wrap".into()),
                    ]);
                } else {
                    v.push(("y".into(), "copy".into()));
                    // Timing is a short table: nothing to search or wrap.
                    if self.tab().is_some_and(|t| t.resp_tab != 2) {
                        v.push(("/".into(), "search".into()));
                        v.push(("w".into(), "wrap".into()));
                    }
                }
            }
        }
        v.push((k(self, Action::Palette), "commands".into()));
        // The footer always keeps the last two: the Ctrl+W menu and F1.
        v.push((crate::keymap::MENU_PREFIX.into(), "menu".into()));
        v.push((k(self, Action::Help), "all keys".into()));
        v
    }
}
