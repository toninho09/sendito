//! The response query bar: find, filter and jq.

use super::*;

impl App {
    pub(super) fn open_search(&mut self) {
        self.open_query(QueryMode::Find);
    }

    pub(super) fn open_filter(&mut self) {
        self.open_query(QueryMode::Jq);
    }

    /// Opens the query bar above the response view (jq: above the body), or
    /// focuses it when it is open in that mode.
    pub(super) fn open_query(&mut self, mode: QueryMode) {
        if self.tab().and_then(|t| t.response.as_ref()).is_none() {
            let msg = match mode {
                QueryMode::Jq => "no response to filter yet",
                QueryMode::Find => "no response to search yet",
            };
            self.set_status(msg, StatusKind::Info);
            return;
        }
        let Some(t) = self.tabs.get_mut(self.active) else {
            return;
        };
        if mode == QueryMode::Jq {
            t.resp_tab = 0;
        }
        let tab = t.resp_tab;
        let Some(resp) = t.response.as_mut() else {
            return;
        };
        let open = resp
            .query
            .as_ref()
            .is_some_and(|q| q.mode == mode && q.tab == tab);
        if !open {
            let text = match mode {
                QueryMode::Jq => resp.jq.clone().unwrap_or_default(),
                QueryMode::Find => resp
                    .viewer_mut(tab)
                    .search
                    .as_ref()
                    .map(|s| s.query.clone())
                    .unwrap_or_default(),
            };
            resp.query = Some(QueryBar::new(mode, &text, tab));
        }
        self.focus = Focus::RespQuery;
    }

    /// Whether the active tab shows a query bar on its current response tab.
    pub(crate) fn query_shown(&self) -> bool {
        self.tab().is_some_and(|t| {
            t.response
                .as_ref()
                .and_then(|r| r.query.as_ref())
                .is_some_and(|q| q.tab == t.resp_tab)
        })
    }

    /// A key typed in the query bar: the text applies at each change (a jq
    /// filter on a large body waits for Enter).
    pub(super) fn query_key(&mut self, key: KeyEvent) {
        let Some(t) = self.tabs.get_mut(self.active) else {
            return;
        };
        let history = t.jq_history.clone();
        let Some(resp) = t.response.as_mut() else {
            return;
        };
        let live = resp.live_jq();
        let Some(q) = resp.query.as_mut() else {
            return;
        };
        let mode = q.mode;
        if key.code == KeyCode::Tab {
            if mode == QueryMode::Jq {
                self.jq_complete();
            }
            return;
        }
        if mode == QueryMode::Jq && matches!(key.code, KeyCode::Up | KeyCode::Down) {
            let pos = match (key.code, q.history_pos) {
                (_, _) if history.is_empty() => return,
                (KeyCode::Up, None) => Some(history.len() - 1),
                (KeyCode::Up, Some(i)) => Some(i.saturating_sub(1)),
                (_, Some(i)) if i + 1 < history.len() => Some(i + 1),
                _ => None,
            };
            q.history_pos = pos;
            q.input.set_text(pos.map_or("", |i| history[i].as_str()));
            q.input.move_to_end();
            if live {
                resp.run_query();
            }
            return;
        }
        match q.input.handle_key(&key, false, &mut self.clipboard) {
            Outcome::Changed => {
                q.history_pos = None;
                if live || mode == QueryMode::Find {
                    resp.run_query();
                } else {
                    q.error = None;
                }
                self.complete_typed();
            }
            Outcome::Submit => {
                if !live || mode == QueryMode::Find {
                    resp.run_query();
                }
                self.query_submit();
            }
            Outcome::Leave => self.query_leave(),
            _ => {}
        }
    }

    /// Enter in the query bar: keep the result and go back to the view. An
    /// empty bar closes; a jq error keeps the bar focused.
    pub(super) fn query_submit(&mut self) {
        let Some(t) = self.tabs.get_mut(self.active) else {
            return;
        };
        let Some(resp) = t.response.as_mut() else {
            return;
        };
        let Some(q) = resp.query.as_ref() else {
            return;
        };
        let (mode, text) = (q.mode, q.input.text());
        // The bar shows the error; the text stays for editing.
        if q.error.is_some() {
            return;
        }
        match resp.query.as_mut() {
            Some(_) if text.trim().is_empty() => resp.query = None,
            Some(q) => q.committed = text.clone(),
            None => {}
        }
        self.focus = Focus::RespBody;
        match mode {
            QueryMode::Jq => {
                t.remember_jq(&text);
                let msg = if text.trim().is_empty() {
                    "jq filter cleared"
                } else {
                    "jq filter applied"
                };
                self.set_status(msg, StatusKind::Ok);
            }
            QueryMode::Find => {
                let n = resp.query.as_ref().map(|q| q.tab).map_or(0, |tab| {
                    resp.viewer_mut(tab)
                        .search
                        .as_ref()
                        .map_or(0, |s| s.matches.len())
                });
                if n == 0 && !text.is_empty() {
                    self.set_status(format!("no match for \"{text}\""), StatusKind::Warn);
                } else if n > 0 {
                    self.set_status(
                        format!("{n} match(es) · n/N next/previous"),
                        StatusKind::Info,
                    );
                }
            }
        }
    }

    /// The jq key context at the query bar's cursor.
    pub(super) fn jq_complete_context(&self) -> Option<(String, String, usize)> {
        let q = self.tab()?.response.as_ref()?.query.as_ref()?;
        if q.mode != QueryMode::Jq {
            return None;
        }
        crate::complete::jq_context(&q.input.text(), q.input.cursor.col)
    }

    /// Tab in the jq bar: completes the object key at the cursor. One match
    /// is inserted at once; more open the popup.
    pub(super) fn jq_complete(&mut self) {
        let Some((base, prefix, start)) = self.jq_complete_context() else {
            self.set_status("Tab completes a key after a \".\"", StatusKind::Info);
            return;
        };
        let Some(t) = self.tab() else {
            return;
        };
        let Some(r) = t.response.as_ref() else {
            return;
        };
        let src = r.jq_src.as_deref().unwrap_or(r.body.doc.bytes());
        let mut c = Completion::new(CompleteKind::JqKey, t.id, None);
        c.start = Pos { row: 0, col: start };
        c.set_pool(crate::complete::jq_keys(&base, src), prefix);
        match c.items.len() {
            0 => self.set_status("no keys to complete here", StatusKind::Info),
            1 => {
                let name = c.items[0].name.clone();
                self.jq_complete_insert(c.start, &name);
            }
            _ => self.complete = Some(c),
        }
    }

    /// Puts a key over the one typed in the jq bar and runs the filter.
    pub(super) fn jq_complete_insert(&mut self, start: Pos, name: &str) -> bool {
        let Some(resp) = self
            .tabs
            .get_mut(self.active)
            .and_then(|t| t.response.as_mut())
        else {
            return false;
        };
        let live = resp.live_jq();
        let Some(q) = resp.query.as_mut() else {
            return false;
        };
        let line: Vec<char> = q.input.text().chars().collect();
        let mut end = q.input.cursor.col;
        while line
            .get(end)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
        {
            end += 1;
        }
        let span_end = Pos { row: 0, col: end };
        q.input
            .replace_span(start, span_end, &crate::complete::jq_key_text(name));
        if live {
            resp.run_query();
        }
        true
    }

    /// Esc in the query bar: back to the view with the text the last Enter
    /// kept; the bar closes when that text is empty.
    pub(super) fn query_leave(&mut self) {
        self.focus = Focus::RespBody;
        let Some(resp) = self
            .tabs
            .get_mut(self.active)
            .and_then(|t| t.response.as_mut())
        else {
            return;
        };
        let Some(q) = resp.query.as_mut() else {
            return;
        };
        q.history_pos = None;
        let committed = q.committed.clone();
        if q.input.text() != committed || q.error.is_some() {
            q.input.set_text(&committed);
            q.input.move_to_end();
            resp.run_query();
        }
        if committed.trim().is_empty() {
            resp.query = None;
        }
    }

    /// Applies (or clears) the jq filter on the active tab's response body and
    /// shows the body tab, where the result is drawn (`:jq <expression>`).
    pub(super) fn apply_filter(&mut self, expr: &str) -> Result<bool, String> {
        let t = self
            .tab_mut()
            .ok_or_else(|| "no open request".to_string())?;
        let resp = t
            .response
            .as_mut()
            .ok_or_else(|| "no response to filter yet".to_string())?;
        let applied = resp.apply_jq(expr)?;
        // The query bar shows the filter that applies.
        resp.query = applied.then(|| QueryBar::new(QueryMode::Jq, expr.trim(), 0));
        t.resp_tab = 0;
        t.remember_jq(expr);
        self.focus = Focus::RespBody;
        Ok(applied)
    }
}
