//! Completion popups: which completion applies, its pool, and how a choice is accepted.

use super::*;

impl App {
    // ----- completion ---------------------------------------------------

    /// Which completion belongs to the focused widget, if any.
    pub(super) fn complete_kind(&self) -> Option<CompleteKind> {
        let t = self.tab()?;
        match (self.focus, t.ed.kind) {
            // The gRPC method chip completes from the schema; the MCP tool
            // input completes from the discovered tool list.
            (Focus::Bar(0), Kind::Grpc) => return Some(CompleteKind::GrpcMethod),
            (Focus::Bar(1), Kind::Mcp) => return Some(CompleteKind::McpServer),
            (Focus::Bar(0), Kind::Mcp) => return Some(CompleteKind::McpTool),
            _ => {}
        }
        if self.focus != Focus::ReqBody {
            return None;
        }
        match t.current_field()? {
            Field::Message if matches!(t.ed.kind, Kind::Grpc | Kind::Mcp) => {
                Some(CompleteKind::Fields)
            }
            Field::BodyText if t.ed.body_kind == BodyKind::GraphQL && t.ed.graphql_section == 0 => {
                let ctx = crate::complete::graphql_context(
                    &t.ed.body_text.text(),
                    t.ed.body_text.cursor,
                )?;
                Some(match ctx.position {
                    sendito_core::graphql::GqlPosition::Field => CompleteKind::GraphqlField,
                    sendito_core::graphql::GqlPosition::Argument => CompleteKind::GraphqlArgument,
                    sendito_core::graphql::GqlPosition::OnType => CompleteKind::GraphqlOnType,
                })
            }
            Field::McpServer => Some(CompleteKind::McpServer),
            Field::McpTool => Some(CompleteKind::McpTool),
            _ => None,
        }
    }

    /// The input tree of the message/tool being edited.
    pub(super) fn complete_tree(&self) -> Option<FieldNode> {
        let t = self.tab()?;
        match t.ed.kind {
            Kind::Grpc => {
                let svcs = t.services.as_ref()?;
                let (svc, m) = (t.ed.service(), t.ed.method());
                svcs.iter()
                    .find(|s| s.name == svc)
                    .and_then(|s| s.methods.iter().find(|mi| mi.name == m))
                    .map(|mi| mi.input_fields.clone())
            }
            Kind::Ws | Kind::Folder => None,
            // Argument keys come from a tool's input schema only.
            Kind::Mcp if t.ed.mcp_op != McpOp::Tool => None,
            Kind::Mcp => {
                let tools = self.mcp_tools(t);
                tools
                    .iter()
                    .find(|tool| tool.name == t.ed.mcp_tool.text())
                    .map(|tool| crate::complete::schema_node(&tool.input_schema, &tool.name))
            }
            Kind::Http | Kind::Sse | Kind::Mqtt => None,
        }
    }

    /// The tool list of a tab, when it belongs to the server it was loaded from.
    pub(super) fn mcp_tools(&self, t: &Tab) -> Vec<sendito_pipeline::mcp::Tool> {
        if t.mcp_tools_server == t.ed.mcp_server.text() {
            t.mcp.tools.clone()
        } else {
            Vec::new()
        }
    }

    /// The list of names a popup kind offers, with a detail column.
    pub(super) fn complete_pool(
        &self,
        kind: CompleteKind,
        t: &Tab,
    ) -> Option<Vec<(String, String)>> {
        match kind {
            CompleteKind::GrpcMethod => t.services.as_ref().map(|svcs| {
                svcs.iter()
                    .flat_map(|s| {
                        s.methods.iter().map(|m| {
                            (
                                format!("{}/{}", s.name, m.name),
                                format!("{} · {} → {}", m.kind.label(), m.input, m.output),
                            )
                        })
                    })
                    .collect()
            }),
            CompleteKind::McpServer => Some(
                self.project
                    .config
                    .mcp
                    .servers
                    .iter()
                    .map(|(name, cfg)| match cfg {
                        sendito_store::config::McpServer::Stdio(s) => {
                            (name.clone(), s.command.clone())
                        }
                        sendito_store::config::McpServer::Http(h) => (name.clone(), h.url.clone()),
                    })
                    .collect(),
            ),
            // The targets of the tab's operation: tools, resources (with the
            // templates) or prompts.
            CompleteKind::McpTool => {
                if self.mcp_tools(t).is_empty() {
                    return None;
                }
                let items: Vec<(String, String)> = super::ingest::mcp_items(&t.mcp)
                    .into_iter()
                    .filter(|(op, _, _)| *op == t.ed.mcp_op)
                    .map(|(_, target, detail)| (target, detail))
                    .collect();
                (!items.is_empty()).then_some(items)
            }
            CompleteKind::Fields
            | CompleteKind::JqKey
            | CompleteKind::GraphqlField
            | CompleteKind::GraphqlArgument
            | CompleteKind::GraphqlOnType => None,
        }
    }

    /// Rebuilds the popup from the cursor position, or closes it when the cursor
    /// is not on a key. Starts the missing schema/tools load in the background.
    pub(super) fn refresh_complete(&mut self) {
        // A load already in flight keeps its row: the list fills when it lands;
        // a failed one keeps its message instead of being retried.
        let (loading, failed) = self
            .complete
            .as_ref()
            .map_or((None, None), |c| (c.loading, c.failed));
        self.complete = None;
        let Some(kind) = self.complete_kind() else {
            return;
        };
        let Some(t) = self.tab() else {
            return;
        };
        let (id, field) = (t.id, t.current_field());
        if kind.is_graphql() {
            let Some(ctx) =
                crate::complete::graphql_context(&t.ed.body_text.text(), t.ed.body_text.cursor)
            else {
                return;
            };
            let mut c = Completion::new(kind, id, field);
            c.start = Pos {
                row: t.ed.body_text.cursor.row,
                col: t.ed.body_text.cursor.col.saturating_sub(ctx.prefix.len()),
            };
            if t.graphql_source == self.graphql_source(t)
                && let Some(schema) = &t.graphql_schema
            {
                c.set_pool(
                    crate::complete::graphql_candidates(schema, &ctx),
                    ctx.prefix,
                );
                self.complete = Some(c);
            } else {
                c.loading = loading;
                c.failed = failed;
                self.complete = Some(c);
                self.start_complete_fetch(kind);
            }
            return;
        }
        if kind == CompleteKind::Fields {
            let Some(ctx) =
                crate::complete::json_context(&t.ed.message.text(), t.ed.message.cursor)
            else {
                return;
            };
            let Some(tree) = self.complete_tree() else {
                let mut c = Completion::new(kind, id, field);
                c.loading = loading;
                c.failed = failed;
                self.complete = Some(c);
                self.start_complete_fetch(kind);
                return;
            };
            let mut c = Completion::new(kind, id, field);
            c.start = ctx.start;
            c.quoted = ctx.quoted;
            c.set_pool(
                crate::complete::field_candidates(&tree, &ctx, ""),
                ctx.prefix,
            );
            self.complete = Some(c);
            return;
        }
        let query = match kind {
            CompleteKind::GrpcMethod => t.ed.grpc_method_text.text(),
            CompleteKind::McpServer => t.ed.mcp_server.text(),
            _ => t.ed.mcp_tool.text(),
        };
        let Some(pool) = self.complete_pool(kind, t) else {
            let mut c = Completion::new(kind, id, field);
            c.loading = loading;
            c.failed = failed;
            self.complete = Some(c);
            self.start_complete_fetch(kind);
            return;
        };
        let mut c = Completion::new(kind, id, field);
        c.set_pool(crate::complete::plain_candidates(pool, ""), query);
        self.complete = Some(c);
    }

    /// Loads the schema or tools a missing completion needs, without opening a
    /// picker when they arrive. While one is in flight, typing does not start
    /// another, and a load that already failed is not retried here (Enter on
    /// the method chip or F2 on the tool field asks again).
    pub(super) fn start_complete_fetch(&mut self, kind: CompleteKind) {
        let Some(t) = self.tab() else {
            return;
        };
        // (what it waits for, a load is in flight, a load already failed)
        let wait: Option<(&'static str, bool, bool)> = match kind {
            CompleteKind::Fields | CompleteKind::GrpcMethod
                if t.ed.kind == Kind::Grpc && t.services.is_none() =>
            {
                Some((
                    "schema",
                    self.schema_job.is_some(),
                    t.schema_error.is_some(),
                ))
            }
            CompleteKind::Fields | CompleteKind::McpTool
                if t.ed.kind == Kind::Mcp
                    && self.mcp_tools(t).is_empty()
                    && !t.ed.mcp_server.text().trim().is_empty() =>
            {
                Some((
                    "tools",
                    self.mcp_discovery.is_some(),
                    t.mcp_tools_error.is_some() && t.mcp_tools_server == t.ed.mcp_server.text(),
                ))
            }
            kind if kind.is_graphql() => Some((
                "graphql",
                self.schema_job.is_some(),
                t.graphql_source == self.graphql_source(t) && t.schema_error.is_some(),
            )),
            _ => None,
        };
        let Some((what, busy, failed)) = wait else {
            return;
        };
        if busy || failed {
            if let Some(c) = &mut self.complete {
                if busy {
                    c.loading = Some(what);
                } else {
                    c.loading = None;
                    c.failed = Some(what);
                }
            }
            return;
        }
        if let Some(c) = &mut self.complete {
            c.loading = Some(what);
        }
        match what {
            "schema" => self.load_schema(false, false, false),
            "graphql" => self.load_graphql_schema(false),
            _ => self.load_mcp_tools(false),
        }
    }

    /// Handles the keys the popup owns. Returns `false` for anything else, so
    /// typing keeps filtering and global chords (Ctrl+P palette, Ctrl+N new
    /// tab) still work while it is open.
    pub(super) fn complete_key(&mut self, key: KeyEvent) -> bool {
        let Some(c) = self.complete.as_mut() else {
            return false;
        };
        match key.code {
            KeyCode::Esc => self.complete = None,
            KeyCode::Up => c.up(),
            KeyCode::Down => c.down(),
            KeyCode::PageUp => c.page_up(),
            KeyCode::PageDown => c.page_down(),
            KeyCode::Enter | KeyCode::Tab => {
                if !self.complete_accept() {
                    return false;
                }
            }
            _ => return false,
        }
        true
    }

    /// While the popup is open, typing narrows the list; a key that leaves the
    /// key position closes it.
    pub(super) fn complete_typed(&mut self) {
        let Some(c) = self.complete.as_ref() else {
            return;
        };
        let (kind, tab) = (c.kind, c.tab);
        let Some(t) = self.tab() else {
            self.complete = None;
            return;
        };
        if t.id != tab {
            self.complete = None;
            return;
        }
        if kind.is_graphql() {
            self.refresh_complete();
            return;
        }
        if kind == CompleteKind::JqKey {
            match self.jq_complete_context() {
                Some((_, prefix, start)) => {
                    if let Some(c) = &mut self.complete {
                        if c.start.col != start {
                            self.complete = None;
                            return;
                        }
                        c.filter(prefix);
                    }
                }
                None => self.complete = None,
            }
            return;
        }
        if kind == CompleteKind::Fields {
            let Some(ctx) =
                crate::complete::json_context(&t.ed.message.text(), t.ed.message.cursor)
            else {
                self.complete = None;
                return;
            };
            if let Some(c) = &mut self.complete {
                if c.start != ctx.start {
                    self.complete = None;
                    return;
                }
                c.filter(ctx.prefix);
            }
            return;
        }
        let query = match kind {
            CompleteKind::GrpcMethod => t.ed.grpc_method_text.text(),
            CompleteKind::McpServer => t.ed.mcp_server.text(),
            _ => t.ed.mcp_tool.text(),
        };
        if let Some(c) = &mut self.complete {
            c.filter(query);
        }
    }

    /// Inserts the selected candidate over the key typed so far.
    pub(super) fn complete_accept(&mut self) -> bool {
        let Some(c) = self.complete.take() else {
            return false;
        };
        let Some(cand) = c.selected().cloned() else {
            return false;
        };
        if c.kind == CompleteKind::JqKey {
            return self.jq_complete_insert(c.start, &cand.name);
        }
        let Some(t) = self.tab_mut() else {
            return false;
        };
        if t.id != c.tab {
            return false;
        }
        let ed = match c.kind {
            CompleteKind::JqKey => None,
            CompleteKind::GraphqlField
            | CompleteKind::GraphqlArgument
            | CompleteKind::GraphqlOnType => Some(&mut t.ed.body_text),
            CompleteKind::Fields => t.ed.text_mut(Field::Message),
            CompleteKind::GrpcMethod => Some(&mut t.ed.grpc_method_text),
            CompleteKind::McpServer => Some(&mut t.ed.mcp_server),
            CompleteKind::McpTool => Some(&mut t.ed.mcp_tool),
        };
        let Some(ed) = ed else {
            return false;
        };
        match c.kind {
            kind if kind.is_graphql() => {
                let line: Vec<char> = ed
                    .lines()
                    .get(ed.cursor.row)
                    .map(|s| s.chars().collect())
                    .unwrap_or_default();
                let mut end = ed.cursor.col;
                while line
                    .get(end)
                    .is_some_and(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
                {
                    end += 1;
                }
                let after = line[end..].iter().find(|ch| !ch.is_whitespace()).copied();
                let suffix = if kind == CompleteKind::GraphqlArgument && after != Some(':') {
                    ": "
                } else {
                    ""
                };
                ed.replace_span(
                    c.start,
                    Pos {
                        row: ed.cursor.row,
                        col: end,
                    },
                    &format!("{}{suffix}", cand.name),
                );
            }
            // The key is replaced in place; the rest of the JSON stays.
            CompleteKind::Fields => {
                // The rest of the key after the cursor is replaced too, and an
                // existing `: value` is kept: only the name changes then.
                let line: Vec<char> = ed
                    .lines()
                    .get(ed.cursor.row)
                    .map(|l| l.chars().collect())
                    .unwrap_or_default();
                let mut end = ed.cursor.col;
                while line.get(end).is_some_and(|ch| {
                    !matches!(ch, '"' | ':' | ',' | '{' | '}') && !ch.is_whitespace()
                }) {
                    end += 1;
                }
                let closed = c.quoted && line.get(end) == Some(&'"');
                if closed {
                    end += 1;
                }
                let mut after = end;
                while line.get(after).is_some_and(|ch| ch.is_whitespace()) {
                    after += 1;
                }
                let has_value = line.get(after) == Some(&':');
                let span_end = Pos {
                    row: ed.cursor.row,
                    col: end,
                };
                if has_value {
                    let name = if c.quoted {
                        format!("{}\"", cand.name)
                    } else {
                        format!("\"{}\"", cand.name)
                    };
                    ed.replace_span(c.start, span_end, &name);
                } else {
                    let insert = cand.insert(c.quoted);
                    ed.replace_span(c.start, span_end, &insert);
                    if cand.cursor_back > 0 {
                        let back = cand.cursor_back as usize;
                        ed.cursor.col = ed.cursor.col.saturating_sub(back);
                    }
                }
            }
            // Single-line name fields take the whole suggestion.
            _ => {
                ed.set_text(&cand.name);
                ed.move_to_end();
            }
        }
        self.changed();
        true
    }

    pub(super) fn sidebar_key(&mut self, key: KeyEvent) {
        if self.sidebar_mode == SidebarMode::History {
            let n = self.history.as_ref().map(|h| h.len()).unwrap_or(0);
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.history_sel = crate::list::step(self.history_sel, -1, n)
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.history_sel = crate::list::step(self.history_sel, 1, n)
                }
                KeyCode::Home => self.history_sel = 0,
                KeyCode::End => self.history_sel = n.saturating_sub(1),
                KeyCode::PageUp => self.history_sel = crate::list::step(self.history_sel, -10, n),
                KeyCode::PageDown => self.history_sel = crate::list::step(self.history_sel, 10, n),
                KeyCode::Enter => self.open_history(self.history_sel, false),
                KeyCode::Char('s') => self.save_history_prompt(self.history_sel),
                KeyCode::Esc => self.toggle_history(),
                _ => {}
            }
            self.history_follow = true;
        }
    }
}
