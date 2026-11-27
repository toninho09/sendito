//! Import, export and response body actions.

use super::*;

impl App {
    pub fn open_import(&mut self, prefill: &str) {
        let mut input = TextEditor::multi(prefill).with_placeholder(
            "curl https://…   or   a collection, OpenAPI, .http, HAR file or Bruno directory",
        );
        input.move_to_end();
        self.overlay = Some(Overlay::Import(Import::new(input)));
    }

    pub(super) fn import_curl(&mut self, text: &str) {
        match sendito_import::curl::import(text) {
            Ok(r) => {
                // An empty, never-saved tab is reused instead of left behind.
                let blank = {
                    let mut b = Tab::new(0, None, String::new(), &Request::new_http(), None);
                    b.refresh_model();
                    b.model
                };
                let reuse = self.tab().is_some_and(|t| {
                    t.path.is_none()
                        && t.running.is_none()
                        && t.response.is_none()
                        && t.model == blank
                });
                let req = Request::Http(r.request);
                let where_ = if reuse {
                    let (id, name) = self
                        .tab()
                        .map(|t| (t.id, t.name.clone()))
                        .unwrap_or_default();
                    let mut tab = Tab::new(id, None, name, &req, None);
                    tab.ed.set_vim(self.vim);
                    tab.refresh_model();
                    self.tabs[self.active] = tab;
                    self.focus = Focus::Bar(1);
                    self.session_dirty = true;
                    "this tab"
                } else {
                    self.new_tab(req);
                    "a new tab"
                };
                if r.notes.is_empty() {
                    self.set_status(
                        format!("imported the curl command into {where_}"),
                        StatusKind::Ok,
                    );
                } else {
                    self.show_report(
                        "curl import notes",
                        &r.notes
                            .iter()
                            .map(|n| format!("• {n}"))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                }
            }
            Err(e) => self.set_status(format!("curl: {e}"), StatusKind::Error),
        }
    }

    /// A typed import path: the working directory first, then the
    /// collection folder.
    fn import_path(&self, t: &str) -> std::path::PathBuf {
        let cwd = std::env::current_dir().unwrap_or_default();
        let p = sendito_store::env::expand(&cwd, t);
        if p.exists() {
            return p;
        }
        let in_root = sendito_store::env::expand(&self.project.root, t);
        if in_root.exists() { in_root } else { p }
    }

    /// What the import dialog's text would import, for the line under its
    /// instructions: `(text, is a problem)`.
    pub(crate) fn import_preview(&self, text: &str) -> Option<(String, bool)> {
        let t = text.trim();
        if t.is_empty() {
            return None;
        }
        let summary = |r: Result<
            (sendito_import::ImportFormat, sendito_import::ImportResult),
            String,
        >| match r {
            Ok((format, result)) => {
                let n = result.root.count_requests();
                (
                    format!(
                        "✓ {} \"{}\" · {n} request{}",
                        format.label(),
                        result.root.name,
                        if n == 1 { "" } else { "s" }
                    ),
                    false,
                )
            }
            Err(e) => (format!("✗ {e}"), true),
        };
        if t.starts_with("curl ") || t == "curl" {
            return Some(match sendito_import::curl::import(t) {
                Ok(c) => (
                    format!("✓ curl · {} {}", c.request.method, c.request.url),
                    false,
                ),
                Err(e) => (format!("✗ curl: {e}"), true),
            });
        }
        if t.starts_with('{') {
            return Some(summary(sendito_import::import_json(t)));
        }
        if t.contains('\n') {
            return None;
        }
        let p = self.import_path(t);
        if !p.exists() {
            return Some((
                format!("✗ no file or directory at {} (Tab completes)", p.display()),
                true,
            ));
        }
        Some(summary(sendito_import::import_path(&p)))
    }

    /// Tab in the import dialog: completes the path typed so far (see
    /// [`complete_path`]).
    pub(crate) fn complete_import_path(&self, text: &str) -> Option<String> {
        let t = text.trim();
        if t.starts_with("curl") || t.starts_with('{') {
            return None;
        }
        complete_path(t, |p| self.import_path(p))
    }

    pub(super) fn do_import(&mut self, text: &str) -> Result<(), String> {
        let t = text.trim();
        if t.is_empty() {
            return Err("paste a curl command or enter a file path".into());
        }
        if t.starts_with("curl ") || t == "curl" {
            // Check first: on an error the dialog stays open with the text.
            sendito_import::curl::import(t).map_err(|e| format!("curl: {e}"))?;
            self.overlay = None;
            self.import_curl(t);
            return Ok(());
        }
        let (format, result) = if t.starts_with('{') {
            sendito_import::import_json(t)?
        } else {
            sendito_import::import_path(&self.import_path(t))?
        };
        let rep = sendito_import::write(&result, &self.project.coll, Some(&self.project.envs), "")
            .map_err(|e| e.to_string())?;
        self.overlay = None;
        self.rescan();
        self.tree.select_rel(&rep.folder);
        self.refresh_names();
        let here = self.project.name.clone();
        let mut text = format!(
            "{} collection \"{}\" imported as the folder {}/ in this collection ({here}).\n\n  requests:  {}\n  folders:   {}\n  variables: {} (collection variables)\n",
            format.label(),
            result.root.name,
            rep.folder,
            rep.requests,
            rep.folders,
            rep.variables
        );
        if rep.environments > 0 {
            text.push_str(&format!("  environments: {}\n", rep.environments));
        }
        for name in &rep.variable_conflicts {
            text.push_str(&format!(
                "  kept the existing collection variable {name:?}\n"
            ));
        }
        for (original, renamed) in &rep.renamed_environments {
            text.push_str(&format!(
                "  environment {original:?} imported as {renamed:?}\n"
            ));
        }
        if result.skipped.is_empty()
            && rep.variable_conflicts.is_empty()
            && rep.renamed_environments.is_empty()
        {
            text.push_str("\nEverything was imported as it was.\n");
        } else {
            // Three kinds of note, each with what to do about it.
            let (mut lost, mut todo, mut changed) = (Vec::new(), Vec::new(), Vec::new());
            for sk in &result.skipped {
                let line = format!("• {}: {}\n", sk.item, sk.reason);
                if sk.reason.contains("does not define it") {
                    todo.push(line);
                } else if sk.reason.contains("not imported")
                    || sk.reason.contains("not supported")
                    || sk.reason.contains("no file selected")
                    || sk.reason.contains("neither a request")
                {
                    lost.push(line);
                } else {
                    changed.push(line);
                }
            }
            text.push_str(&format!(
                "  report:    {} entries ({} need action, {} skipped, {} imported with changes)\n",
                result.skipped.len(),
                todo.len(),
                lost.len(),
                changed.len()
            ));
            for (title, help, lines) in [
                (
                    "Needs action",
                    "variables the requests use but nothing defines: set them in an environment (F4, Ctrl+W V)",
                    &todo,
                ),
                (
                    "Skipped",
                    "not imported: Sendito has no equivalent; recreate by hand in that request if you need it",
                    &lost,
                ),
                (
                    "Imported with changes",
                    "works as imported; check the request if the change matters to you",
                    &changed,
                ),
            ] {
                if lines.is_empty() {
                    continue;
                }
                text.push_str(&format!("\n{title} ({}) — {help}:\n\n", lines.len()));
                for l in lines {
                    text.push_str(l);
                }
            }
        }
        self.show_report(&format!("{} import", format.label()), &text);
        let notes = result.skipped.len();
        if let Some(Overlay::Report(r)) = &mut self.overlay {
            r.close_status = Some(format!(
                "imported {} requests into {}/{}",
                rep.requests,
                rep.folder,
                if notes == 0 {
                    String::new()
                } else {
                    format!(" · {notes} entries to review (:report shows them again)")
                }
            ));
        }
        Ok(())
    }

    pub fn show_report(&mut self, title: &str, text: &str) {
        self.last_report = Some((title.to_string(), text.to_string()));
        self.overlay = Some(Overlay::Report(Report {
            title: title.into(),
            viewer: Viewer::new(Doc::text(text)),
            copy_text: text.to_string(),
            copied: false,
            close_status: None,
        }));
    }

    /// The active HTTP request with its folders applied, the scope an export
    /// renders it with, and a warning naming the secrets that did not load
    /// (they stay `{{placeholders}}` in the output). `what` names the format.
    fn export_input(
        &mut self,
        include_secrets: bool,
        what: &str,
    ) -> Option<(String, HttpRequest, Scope, String)> {
        let t = self.tab()?;
        if t.folder.is_some() {
            self.set_status(
                format!("{what} export is for HTTP requests"),
                StatusKind::Warn,
            );
            return None;
        }
        let name = t.name.clone();
        let path = t.path.clone().or_else(|| t.origin_path.clone());
        let model = t.model.clone();
        let chain = match runner::folder_chain(&self.project.root, path.as_deref()) {
            Ok(chain) => chain,
            Err(e) => {
                self.set_status(e, StatusKind::Error);
                return None;
            }
        };
        let (mut scope, warning) = self.export_scope(include_secrets);
        let effective = runner::with_folders(&model, &chain, &mut scope);
        if matches!(effective, Request::Mqtt(_)) {
            self.set_status(
                format!("{what} export is not available for MQTT requests"),
                StatusKind::Warn,
            );
            return None;
        }
        let Some(h) = sendito_import::http_export_request(&effective, &scope) else {
            self.set_status(
                format!("{what} export is for HTTP requests"),
                StatusKind::Warn,
            );
            return None;
        };
        Some((name, h, scope, warning))
    }

    fn export_scope(&self, include_secrets: bool) -> (Scope, String) {
        if !include_secrets {
            return (self.names.clone(), String::new());
        }
        let (scope, errors) = runner::build_scope(
            &self.project.envs,
            self.active_env.as_deref(),
            &self.runtime,
        );
        let warning = if errors.is_empty() {
            String::new()
        } else {
            format!(
                "NOT LOADED (left as typed, or a value from another layer): {}\n\n",
                errors.join("; ")
            )
        };
        (scope, warning)
    }

    /// Copies an export to the terminal clipboard and shows it with its
    /// notes. `what` names the export in the status line (`curl command`).
    fn copy_export(&mut self, title: &str, what: &str, text: String, notes: String, warning: &str) {
        self.clipboard.set(text.clone(), false);
        self.overlay = Some(Overlay::Report(Report {
            title: title.into(),
            viewer: Viewer::new(Doc::text(&format!(
                "{text}\n\n── notes (not part of the {what}) ──\n{warning}{notes}\n"
            ))),
            copy_text: text,
            copied: true,
            close_status: Some(format!("the {what} was sent to the terminal clipboard")),
        }));
        if warning.is_empty() {
            self.set_status(
                format!("{what} sent to the terminal clipboard"),
                StatusKind::Ok,
            );
        } else {
            self.set_status(
                format!("{what} sent to the terminal clipboard; some secrets did not load"),
                StatusKind::Warn,
            );
        }
    }

    /// The clipboard note of an export; `bang` is the command that includes
    /// the secret values (`:curl!`).
    fn clipboard_note(include_secrets: bool, bang: &str) -> String {
        if include_secrets {
            "Sent to the clipboard through the terminal (OSC 52). Some terminals block it: if pasting \
             gives nothing, select the text above. It CONTAINS SECRET VALUES."
                .into()
        } else {
            format!(
                "Copied to the clipboard through the terminal (OSC 52). Some terminals block it: if pasting \
                 gives nothing, select the text above. Secret variables stay \
                 {{{{placeholders}}}}; {bang} includes their values."
            )
        }
    }

    pub fn export_curl(&mut self, include_secrets: bool) {
        let sse = self.tab().is_some_and(|t| t.ed.kind == Kind::Sse);
        let Some((_, h, scope, warning)) = self.export_input(include_secrets, "curl") else {
            return;
        };
        let mut cmd = sendito_import::curl::export(&h, &scope, include_secrets, &self.project.root);
        if sse {
            cmd = cmd.replacen("curl", "curl -N", 1);
        }
        // Options that come from the request's settings, explained.
        let mut flags = Vec::new();
        if cmd.contains("\n  -L") || cmd.ends_with(" -L") {
            flags.push("-L follows redirects (Options: follow redirects is on)");
        }
        if cmd.contains("\n  -k") {
            flags.push("-k skips TLS verification (Options: verify TLS is off)");
        }
        if cmd.contains("--max-time") {
            flags.push("--max-time is the request timeout (Options)");
        }
        let flags = if flags.is_empty() {
            String::new()
        } else {
            format!("{}\n\n", flags.join("; "))
        };
        let notes = flags + &Self::clipboard_note(include_secrets, ":curl!");
        self.copy_export("Copy as curl", "curl command", cmd, notes, &warning);
    }

    /// Copies the active request as an .http file (JetBrains HTTP client /
    /// VS Code REST Client).
    pub(super) fn export_http_file(&mut self, include_secrets: bool) {
        let Some((name, h, scope, warning)) = self.export_input(include_secrets, ".http") else {
            return;
        };
        let file = sendito_import::http_file::export(
            &name,
            &h,
            &scope,
            include_secrets,
            &self.project.root,
        );
        let notes = Self::clipboard_note(include_secrets, ":httpfile!");
        self.copy_export("Copy as .http", ".http file", file, notes, &warning);
    }

    /// `:code <lang>`: copies code that sends the active HTTP request with
    /// another tool.
    pub(super) fn export_code(&mut self, arg: &str, include_secrets: bool) {
        use sendito_import::codegen::Lang;
        let Some(lang) = Lang::parse(arg) else {
            return self.set_status(
                format!("usage: :code <language>  ({})", Lang::names()),
                StatusKind::Error,
            );
        };
        if self.tab().is_some_and(|t| t.ed.kind == Kind::Sse) {
            return self.set_status("code export is for HTTP requests", StatusKind::Warn);
        }
        let Some((_, h, scope, warning)) = self.export_input(include_secrets, "code") else {
            return;
        };
        let code =
            sendito_import::codegen::export(lang, &h, &scope, include_secrets, &self.project.root);
        let notes = format!(
            "The tool can add headers of its own, such as a User-Agent.\n\n{}",
            Self::clipboard_note(include_secrets, &format!(":code! {}", lang.as_str()))
        );
        let title = format!("Copy as {} code", lang.as_str());
        self.copy_export(&title, "code", code, notes, &warning);
    }

    pub(super) fn format_body(&mut self) {
        let Some(t) = self.tab_mut() else { return };
        let ed = if matches!(t.ed.kind, Kind::Grpc | Kind::Mcp | Kind::Ws) {
            &mut t.ed.message
        } else {
            if t.ed.body_kind == BodyKind::GraphQL && t.ed.graphql_section != 1 {
                self.set_status("select Variables to format JSON", StatusKind::Info);
                return;
            }
            match t.ed.text_mut(Field::BodyText) {
                Some(ed) => ed,
                None => return,
            }
        };
        match serde_json::from_str::<serde_json::Value>(&ed.text()) {
            Ok(_) => {
                let (pretty, _) = crate::viewer::pretty_json(ed.text().as_bytes());
                ed.replace_all(&pretty);
                self.changed();
            }
            Err(e) => self.set_status(format!("not valid JSON: {e}"), StatusKind::Warn),
        }
    }

    /// The text of the response tab on screen, in its view (pretty or raw).
    pub(super) fn shown_response_text(&self) -> Option<String> {
        let t = self.tab()?;
        let r = t.response.as_ref()?;
        let v = match t.resp_tab {
            0 => &r.body,
            1 => &r.headers,
            2 => &r.timing,
            _ => &r.messages,
        };
        Some(match v.mode {
            ViewMode::Pretty if v.doc.pretty.is_some() => v
                .doc
                .pretty
                .as_ref()
                .map(|p| p.text.clone())
                .unwrap_or_default(),
            _ => v.doc.raw.text.clone(),
        })
    }

    pub(super) fn copy_body(&mut self) {
        let Some(text) = self.shown_response_text() else {
            return;
        };
        if text.len() > 1_000_000 {
            self.set_status(
                format!(
                    "body is {} — too large for the terminal clipboard; save it with s",
                    human_size(text.len() as u64)
                ),
                StatusKind::Warn,
            );
            return;
        }
        let n = text.len();
        self.clipboard.set(text, false);
        self.set_status(
            format!("copied {} to the clipboard", human_size(n as u64)),
            StatusKind::Ok,
        );
    }

    /// The jq path of the JSON value at the body cursor (pretty view).
    pub(super) fn body_path(&mut self) -> Option<String> {
        let t = self.tabs.get_mut(self.active)?;
        if t.resp_tab != 0 {
            return None;
        }
        t.response.as_mut()?.body.json_path()
    }

    /// Narrows the jq filter to the JSON value at the cursor.
    pub(super) fn filter_here(&mut self) {
        let Some(path) = self.body_path() else {
            self.set_status(
                "no JSON value at the cursor (Pretty view)",
                StatusKind::Info,
            );
            return;
        };
        let current = self
            .tab()
            .and_then(|t| t.response.as_ref())
            .and_then(|r| r.jq.clone());
        // A filtered body is the filter's output: the path continues it.
        let expr = match current {
            Some(j) if path == "." => j,
            Some(j) => format!("{j} | {path}"),
            None => path,
        };
        match self.apply_filter(&expr) {
            Ok(_) => self.set_status("jq filter applied", StatusKind::Ok),
            Err(e) => self.set_status(e, StatusKind::Error),
        }
    }

    /// Shows the previous or next message of a stream in the body.
    pub(super) fn step_message(&mut self, step: isize) {
        let Some(t) = self.tabs.get_mut(self.active) else {
            return;
        };
        t.resp_tab = 0;
        let moved = t.response.as_mut().is_some_and(|r| r.select_message(step));
        if !moved {
            self.set_status("no other message", StatusKind::Info);
        }
    }

    pub(super) fn copy_path(&mut self) {
        let Some(path) = self.body_path() else {
            self.set_status(
                "no JSON value at the cursor (Pretty view)",
                StatusKind::Info,
            );
            return;
        };
        self.clipboard.set(path.clone(), false);
        self.set_status(format!("copied {path}"), StatusKind::Ok);
    }

    pub(super) fn prompt_save_body(&mut self) {
        if self.tab().and_then(|t| t.response.as_ref()).is_none() {
            self.set_status("no response yet", StatusKind::Info);
            return;
        }
        let suggestion = self
            .tab()
            .map(|t| format!("{}.response", t.name.replace(' ', "_")))
            .unwrap_or_default();
        let mut input = TextEditor::single(&suggestion);
        input.move_to_end();
        self.overlay = Some(Overlay::Prompt(Prompt::new(
            "Save response body",
            "File",
            input,
            vec![],
            PromptAction::SaveBody,
        )));
    }

    pub fn save_body(&mut self, path: &str) -> Result<String, String> {
        let t = self.tab().ok_or("no tab")?;
        let r = t.response.as_ref().ok_or("no response")?;
        let p =
            sendito_store::env::expand(&std::env::current_dir().unwrap_or_default(), path.trim());
        // The jq filter is display-only: save the body as received.
        let bytes = r.jq_src.as_deref().unwrap_or(r.body.doc.bytes());
        std::fs::write(&p, bytes).map_err(|e| format!("{}: {e}", p.display()))?;
        Ok(format!(
            "saved {} to {}",
            human_size(bytes.len() as u64),
            p.display()
        ))
    }
}

/// Completes the path typed so far; `resolve` turns a typed folder into the
/// folder to read. Returns the completed text, or `None` when nothing (or
/// more than one name without a longer common start) matches. A hidden name
/// matches only a prefix that starts with `.`.
pub(crate) fn complete_path(
    text: &str,
    resolve: impl Fn(&str) -> std::path::PathBuf,
) -> Option<String> {
    let t = text.trim();
    if t.is_empty() || t.contains('\n') {
        return None;
    }
    let (dir, prefix) = match t.rsplit_once('/') {
        Some((d, p)) => (format!("{d}/"), p),
        None => (String::new(), t),
    };
    let base = resolve(if dir.is_empty() { "." } else { &dir });
    let names: Vec<(String, bool)> = std::fs::read_dir(&base)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            (name.starts_with(prefix) && (prefix.starts_with('.') || !name.starts_with('.')))
                .then(|| (name, e.path().is_dir()))
        })
        .collect();
    match names.as_slice() {
        [] => None,
        [(name, is_dir)] => Some(format!("{dir}{name}{}", if *is_dir { "/" } else { "" })),
        _ => {
            let first = &names[0].0;
            let common = names.iter().fold(first.len(), |n, (s, _)| {
                first
                    .chars()
                    .zip(s.chars())
                    .take_while(|(a, b)| a == b)
                    .count()
                    .min(n)
            });
            let longer: String = first.chars().take(common).collect();
            (longer.len() > prefix.len()).then(|| format!("{dir}{longer}"))
        }
    }
}
