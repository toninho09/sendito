//! What a confirm, a picker choice or a prompt does when accepted.

use super::*;

impl App {
    pub(super) fn confirm(&mut self, action: ConfirmAction, sel: usize) {
        match action {
            ConfirmAction::SendAnyway { missing } => match sel {
                0 => self.send(true),
                1 if !missing.is_empty() => self.define_variables(&missing),
                _ => {}
            },
            ConfirmAction::SendMissingSecrets => {
                if sel == 0 {
                    self.secrets_ok = true;
                    self.send(true);
                }
            }
            ConfirmAction::Delete(rel) => {
                if sel != 0 {
                    self.set_status(format!("kept {rel}: nothing was deleted"), StatusKind::Info);
                    return;
                }
                match self.project.coll.delete(&rel) {
                    Ok(()) => {
                        for t in &mut self.tabs {
                            if t.path
                                .as_deref()
                                .is_some_and(|p| crate::tree::repath(p, &rel, "").is_some())
                            {
                                t.path = None;
                                t.set_saved(None);
                                t.name = format!("{} (deleted)", t.name);
                            }
                        }
                        self.tree.forget_favorites(&rel);
                        self.session_dirty = true;
                        self.rescan();
                        self.set_status(format!("deleted {}", name_of(&rel)), StatusKind::Ok);
                    }
                    Err(e) => self.set_status(e.to_string(), StatusKind::Error),
                }
            }
            ConfirmAction::Quit => {
                if sel == 0 {
                    self.quit = true;
                } else {
                    self.set_status("staying: the tabs are still open", StatusKind::Info);
                }
            }
            ConfirmAction::CloseTab(id) => {
                let Some(i) = self.tabs.iter().position(|t| t.id == id) else {
                    return;
                };
                self.active = i;
                match sel {
                    0 => {
                        let had_path = self.tabs[i].path.is_some();
                        self.save();
                        if had_path && !self.tabs.get(i).is_some_and(|t| t.dirty()) {
                            self.close_tab(true);
                        }
                    }
                    1 => self.close_tab(true),
                    _ => {
                        let name = self.tabs[i].title();
                        self.set_status(
                            format!("\"{name}\" kept open: nothing was saved or discarded"),
                            StatusKind::Info,
                        );
                    }
                }
            }
            ConfirmAction::CloseTabRunning(id) => {
                let Some(i) = self.tabs.iter().position(|t| t.id == id) else {
                    return;
                };
                self.active = i;
                if sel != 0 {
                    let what = if self.tabs[i]
                        .running
                        .as_ref()
                        .is_some_and(|r| r.streaming_input)
                    {
                        "session"
                    } else {
                        "request"
                    };
                    self.set_status(
                        format!("kept open: the {what} is still running"),
                        StatusKind::Info,
                    );
                } else if self.tabs[i].dirty() {
                    // Unsaved edits still get their Save/Discard/Cancel.
                    self.prompt_close_dirty();
                } else {
                    self.close_tab(true);
                }
            }
            ConfirmAction::EnvClose => {
                let Some(mut ed) = self.parked_env.take() else {
                    return;
                };
                match sel {
                    0 => {
                        if !self.save_env_editor(&mut ed) {
                            self.overlay = Some(Overlay::Env(ed));
                        }
                    }
                    1 => {}
                    _ => self.overlay = Some(Overlay::Env(ed)),
                }
            }
            ConfirmAction::KeymapClose => self.keymap_close(sel),
            ConfirmAction::RecordsClose => self.records_close(sel),
            ConfirmAction::DialogClose => {
                let Some(dialog) = self.parked_dialog.take() else {
                    return;
                };
                self.overlay = Some(dialog);
                // Discarding goes through Esc: it closes the dialog and
                // restores whatever was parked behind it (the env editor).
                if sel == 0 {
                    self.overlay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
                }
            }
            ConfirmAction::McpLogin { server, tab, after } => {
                if sel == 0 {
                    self.mcp_oauth_command(&server, true, Some((tab, after)));
                }
            }
            ConfirmAction::ClearHistory => {
                if sel == 0 {
                    let _ = sendito_store::history::clear(&sendito_store::history::history_path(
                        &self.project.root,
                    ));
                    self.history = Some(Vec::new());
                    self.set_status("history cleared", StatusKind::Ok);
                }
            }
            ConfirmAction::DeleteCookie(n) => {
                if sel == 0 {
                    self.cookies_command(&format!("delete {n}"));
                }
            }
            ConfirmAction::ClearCookies => {
                if sel == 0 {
                    self.cookies_command("clear");
                }
            }
        }
    }

    pub(super) fn pick(
        &mut self,
        action: PickAction,
        index: usize,
        item: &str,
        detail: Option<String>,
    ) {
        match action {
            PickAction::Env => {
                let n = self.env_list.len();
                if index == 0 {
                    self.set_env(None);
                } else if index <= n {
                    self.set_env(Some(item.to_string()));
                } else {
                    self.open_env_editor();
                }
            }
            PickAction::Method => {
                if item == "Custom…" {
                    self.overlay = Some(Overlay::Prompt(Prompt::new(
                        "Custom HTTP method",
                        "Method",
                        TextEditor::single(""),
                        vec![],
                        PromptAction::CustomMethod,
                    )));
                    return;
                }
                if let Some(t) = self.tab_mut() {
                    t.ed.method = item.to_string();
                }
                self.changed();
            }
            PickAction::GrpcMethod => {
                if let Some((svc, m)) = item.rsplit_once('/') {
                    let mut fill = false;
                    if let Some(t) = self.tab_mut() {
                        let label = format!("{svc}/{m}");
                        let changed = t.ed.grpc_method_text.text() != label;
                        t.ed.grpc_method_text.set_text(&label);
                        t.ed.grpc_method_text.move_to_end();
                        let msg = t.ed.message.text();
                        fill = changed && (msg.trim().is_empty() || msg.trim() == "{}");
                    }
                    self.changed();
                    if fill {
                        self.load_schema(false, true, false);
                    }
                }
            }
            PickAction::McpServer => {
                if let Some(t) = self.tab_mut() {
                    t.ed.mcp_server.set_text(item);
                }
                self.changed();
            }
            PickAction::McpTool => {
                if let Some(t) = self.tab_mut() {
                    let items = super::ingest::mcp_items(&t.mcp);
                    let op = items.get(index).map(|(op, _, _)| *op).unwrap_or_default();
                    t.ed.mcp_op = op;
                    t.ed.mcp_tool.set_text(item);
                    let args = t.ed.message.text();
                    let unchanged =
                        matches!(t.saved(), Some(Request::Mcp(saved)) if saved.arguments == args);
                    // Arguments that are still another tool's template are
                    // replaced too: they were not typed by the user.
                    let a_template = t.mcp.tools.iter().any(|tl| {
                        sendito_core::json_schema::object_template(&tl.input_schema) == args
                    });
                    let blank = unchanged && (args.trim().is_empty() || args.trim() == "{}");
                    if (blank || a_template)
                        && op == McpOp::Tool
                        && let Some(tool) = t.mcp.tools.get(index)
                    {
                        let template =
                            sendito_core::json_schema::object_template(&tool.input_schema);
                        t.ed.message.replace_all(&template);
                    }
                    // A prompt's arguments start as an object of its argument names.
                    let first_prompt = items.len() - t.mcp.prompts.len();
                    if blank
                        && op == McpOp::Prompt
                        && let Some(p) = t.mcp.prompts.get(index.saturating_sub(first_prompt))
                        && !p.arguments.is_empty()
                    {
                        let object: serde_json::Map<String, serde_json::Value> = p
                            .arguments
                            .iter()
                            .map(|(name, _)| {
                                (name.clone(), serde_json::Value::String(String::new()))
                            })
                            .collect();
                        let text = serde_json::to_string_pretty(&object).unwrap_or_default();
                        t.ed.message.replace_all(&text);
                    }
                }
                self.changed();
            }
            PickAction::MoveTo(rel) => {
                let folder = detail.unwrap_or_default();
                self.move_node(&rel, &folder);
            }
            PickAction::Choice(f) => {
                if let Some(t) = self.tab_mut() {
                    t.ed.set_choice(f, index);
                }
                self.changed();
            }
            PickAction::RequestKind => {}
            PickAction::SendValue => self.answer(item.to_string()),
            PickAction::Theme { settings } => {
                self.theme_command(item);
                if settings {
                    self.open_settings(SettingId::Theme);
                }
            }
            PickAction::Cookie => {
                let (title, message, action) = if item == CLEAR_COOKIES {
                    (
                        "Clear cookies",
                        "Delete every cookie of the active environment?".to_string(),
                        ConfirmAction::ClearCookies,
                    )
                } else {
                    (
                        "Delete cookie",
                        format!("Delete this cookie?\n{item}"),
                        ConfirmAction::DeleteCookie(index + 1),
                    )
                };
                self.overlay = Some(Overlay::Confirm(Confirm {
                    title: title.into(),
                    message,
                    buttons: vec!["Delete".into(), "Cancel".into()],
                    sel: 1,
                    action,
                }));
            }
            PickAction::Auth { login } => {
                let verb = if login { "login" } else { "logout" };
                self.http_oauth_command(&format!("{verb} {item}"));
            }
            PickAction::Example => self.show_example(item),
            PickAction::EnvFile { dir, dotenv } => self.pick_env_file(&dir, dotenv, item),
            PickAction::EnvTheme => self.close_env_theme_picker(Some(item)),
            PickAction::OAuthIdentity => {
                if index >= self.project.config.oauth.len() {
                    self.open_oauth_editor();
                    if let Some(Overlay::Records(e)) = &mut self.overlay {
                        e.add();
                    }
                    return;
                }
                if let Some(t) = self.tab_mut() {
                    t.ed.auth_oauth_name.set_text(item);
                }
                self.changed();
            }
        }
    }

    /// What a name prompt will create, checked as it is typed: the path
    /// (`false`) or why the name cannot be used (`true`).
    pub(crate) fn prompt_preview(&self, p: &Prompt) -> Option<(String, bool)> {
        use sendito_store::collection::{
            EXT, join_rel, validate_folder_name, validate_request_name,
        };
        let name = p.input.text();
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        let folder = || {
            p.choices
                .iter()
                .find(|(l, _, _)| l == "In folder")
                .and_then(|(_, _, i)| self.tree.all_folders().get(*i).cloned())
        };
        let (check, parent) = match &p.action {
            PromptAction::NewRequest { folder: f } => {
                (validate_request_name(name), folder().unwrap_or(f.clone()))
            }
            PromptAction::SaveAs { .. } | PromptAction::SaveHistory(_) => {
                (validate_request_name(name), folder().unwrap_or_default())
            }
            PromptAction::NewFolder { folder } => (validate_folder_name(name), folder.clone()),
            PromptAction::Rename(rel) => {
                let parent = rel.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
                let check = if rel.ends_with(&format!(".{EXT}")) {
                    validate_request_name(name)
                } else {
                    validate_folder_name(name)
                };
                (check, parent.to_string())
            }
            _ => return None,
        };
        if let Err(e) = check {
            return Some((e.to_string(), true));
        }
        let file = match &p.action {
            PromptAction::NewFolder { .. } => name.to_string(),
            PromptAction::Rename(rel) if !rel.ends_with(&format!(".{EXT}")) => name.to_string(),
            _ => format!("{name}.{EXT}"),
        };
        let rel = join_rel(&parent, &file);
        if matches!(&p.action, PromptAction::Rename(old) if *old == rel) {
            return None;
        }
        if self.project.coll.abs(&rel).exists() {
            let place = if parent.is_empty() {
                "the collection root".to_string()
            } else {
                format!("/{parent}")
            };
            return Some((
                format!("\"{name}\" already exists in {place}: choose another name"),
                true,
            ));
        }
        Some((format!("→ {rel}"), false))
    }

    pub(super) fn prompt_ok(&mut self, p: &mut Prompt) -> Result<(), String> {
        let value = p.input.text();
        let choice_at = |k: usize| p.choices.get(k).map(|(_, _, i)| *i).unwrap_or(0);
        // Folder choice is the last row when present.
        let folder_choice = p
            .choices
            .iter()
            .position(|(l, _, _)| l == "In folder")
            .map(choice_at);
        let choice = choice_at(0);
        match p.action.clone() {
            PromptAction::NewRequest { folder } => {
                let folder = folder_choice
                    .and_then(|i| self.tree.all_folders().get(i).cloned())
                    .unwrap_or(folder);
                let req = match choice {
                    1 => Request::new_grpc(),
                    2 => Request::new_ws(),
                    4 => Request::new_sse(),
                    5 => Request::new_mqtt(),
                    3 => Request::new_mcp(),
                    _ => Request::new_http(),
                };
                let rel = self
                    .project
                    .coll
                    .create_request(&folder, value.trim(), &req)
                    .map_err(|e| e.to_string())?;
                self.rescan();
                self.tree.select_rel(&rel);
                self.open_request(&rel);
                self.set_status(
                    format!(
                        "created {rel} (an empty file) · your edits show ● until Ctrl+S saves them"
                    ),
                    StatusKind::Ok,
                );
            }
            PromptAction::NewFolder { folder } => {
                let folder = folder_choice
                    .and_then(|i| self.tree.all_folders().get(i).cloned())
                    .unwrap_or(folder);
                let rel = self
                    .project
                    .coll
                    .create_folder(&folder, value.trim())
                    .map_err(|e| e.to_string())?;
                self.tree.expanded.insert(rel.clone());
                self.rescan();
                self.tree.select_rel(&rel);
                self.set_status(format!("created folder {rel}"), StatusKind::Ok);
            }
            PromptAction::Rename(rel) => {
                let was_folder = self.tree.root.find_folder(&rel).is_some();
                let new_rel = self
                    .project
                    .coll
                    .rename(&rel, value.trim())
                    .map_err(|e| e.to_string())?;
                if self.tree.expanded.remove(&rel) {
                    self.tree.expanded.insert(new_rel.clone());
                }
                self.repath_tabs(&rel, &new_rel, was_folder);
                self.rescan();
                self.tree.select_rel(&new_rel);
            }
            PromptAction::SaveAs { tab } => {
                let folders = self.tree.all_folders();
                let folder = folders.get(choice).cloned().unwrap_or_default();
                let model = self
                    .tabs
                    .iter()
                    .find(|t| t.id == tab)
                    .map(|t| t.model.clone())
                    .ok_or("tab closed")?;
                let rel = self
                    .project
                    .coll
                    .create_request(&folder, value.trim(), &model)
                    .map_err(|e| e.to_string())?;
                if let Some(t) = self.tab_by_id(tab) {
                    t.path = Some(rel.clone());
                    t.name = name_of(&rel).to_string();
                    t.set_saved(Some(model));
                }
                self.rescan();
                self.tree.select_rel(&rel);
                self.set_status(format!("saved {rel}"), StatusKind::Ok);
                self.session_dirty = true;
            }
            PromptAction::SaveHistory(e) => {
                let folders = self.tree.all_folders();
                let folder = folders.get(choice).cloned().unwrap_or_default();
                let req = sendito_store::request_file::parse(&e.request)?;
                let rel = self
                    .project
                    .coll
                    .create_request(&folder, value.trim(), &req)
                    .map_err(|e| match e {
                        sendito_store::StoreError::Exists(_) => format!(
                            "\"{}\" exists in /{folder}: choose another name",
                            value.trim()
                        ),
                        e => e.to_string(),
                    })?;
                self.rescan();
                self.tree.select_rel(&rel);
                self.set_status(format!("saved history entry as {rel}"), StatusKind::Ok);
            }
            PromptAction::SaveBody => {
                let m = self.save_body(&value)?;
                self.set_status(m, StatusKind::Ok);
            }
            PromptAction::NewEnv => {
                let name = value.trim().to_string();
                let env = sendito_store::env::EnvFile::new(&name);
                if self.project.envs.list().contains(&name) {
                    return Err(format!("environment \"{name}\" already exists"));
                }
                self.project.envs.save(&env).map_err(|e| e.to_string())?;
                self.env_list = self.project.envs.list();
                let mut envs = vec![COLLECTION_ENV.to_string()];
                envs.extend(self.env_list.iter().cloned());
                let current = envs.iter().position(|e| *e == name).unwrap_or(0);
                let mut ed = self.parked_env.take().unwrap_or_default();
                ed.envs = envs;
                ed.current = current;
                self.load_env_into(&mut ed);
                self.overlay = Some(Overlay::Env(ed));
            }
            PromptAction::CustomMethod => {
                let m = value.trim().to_ascii_uppercase();
                if m.is_empty()
                    || !m
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
                {
                    return Err("a method is a single token such as PURGE".into());
                }
                if let Some(t) = self.tab_mut() {
                    t.ed.method = m;
                }
                self.changed();
            }
            PromptAction::McpServer { name } => {
                self.add_mcp_server(&name, choice, &value)?;
            }
            PromptAction::SendValue => self.answer(value),
            PromptAction::MoveToSecret(field) => {
                let m = self.move_to_secret(field, value.trim())?;
                self.set_status(m, StatusKind::Ok);
            }
        }
        Ok(())
    }
}
