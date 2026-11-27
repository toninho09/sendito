//! The run tab: runs every request of a folder with the shared suite engine
//! (`sendito_pipeline::suite`), as `sendito run <folder>` does, and shows one
//! row per request. The run has its own runtime values: it does not change
//! the runtime of the other tabs.

use sendito_pipeline::suite;

use super::ingest::http_response;
use crate::tab::{RunRow, RunView};

use super::*;

/// What `$prompt` and `$select` answer in a run: a run cannot ask.
const RUN_CANNOT_ASK: &str =
    "$prompt and $select ask for a value in a request tab: a run tab cannot ask";

impl App {
    /// `R` in the tree: runs the selected folder (for a request, its folder).
    pub(super) fn tree_run_folder(&mut self) {
        let rel = match self.tree.selected_row() {
            Some(r) if r.is_folder() => r.rel.clone(),
            Some(r) => parent_rel(&r.rel).to_string(),
            None => String::new(),
        };
        self.open_run(&rel, None);
    }

    /// `:run [folder] [--data file]`: without a folder, the folder of the
    /// active request (the tree's selection without a tab); `.` is the
    /// collection root.
    pub(super) fn run_folder_command(&mut self, arg: &str) {
        let mut words = arg.split_whitespace();
        let mut folder: Option<String> = None;
        let mut data = None;
        while let Some(w) = words.next() {
            if w == "--data" {
                match words.next() {
                    Some(f) => data = Some(f.to_string()),
                    None => {
                        self.set_status(
                            "--data needs a file: :run [folder] --data <file>",
                            StatusKind::Error,
                        );
                        return;
                    }
                }
            } else if folder.is_none() {
                folder = Some(w.trim_matches('/').to_string());
            }
        }
        let folder = match folder.as_deref() {
            Some(".") => String::new(),
            Some(f) => f.to_string(),
            None => match self.tab() {
                Some(t) if t.run.is_some() => {
                    t.run.as_ref().map(|v| v.folder.clone()).unwrap_or_default()
                }
                Some(t) if t.folder.is_some() => t.folder.clone().unwrap_or_default(),
                Some(t) => parent_rel(Self::inherit_path(t).unwrap_or("")).to_string(),
                None => match self.tree.selected_row() {
                    Some(r) if r.is_folder() => r.rel.clone(),
                    Some(r) => parent_rel(&r.rel).to_string(),
                    None => String::new(),
                },
            },
        };
        self.open_run(&folder, data.as_deref());
    }

    /// Opens a run tab for `folder` and starts the run. `data` is a data
    /// file, relative to the collection root.
    pub(super) fn open_run(&mut self, folder: &str, data: Option<&str>) {
        self.complete = None;
        self.clear_transient_status();
        let what = if folder.is_empty() {
            "the collection".to_string()
        } else {
            format!("\"{folder}\"")
        };
        let rels = match self.project.coll.requests_under(folder) {
            Ok(v) if v.is_empty() => {
                self.set_status(format!("no requests under {what}"), StatusKind::Error);
                return;
            }
            Ok(v) => v,
            Err(e) => {
                self.set_status(format!("cannot run {what}: {e}"), StatusKind::Error);
                return;
            }
        };
        let rows = match data {
            Some(f) => match suite::load_data(&self.project.root.join(f)) {
                Ok(rows) => rows,
                Err(e) => {
                    self.set_status(e, StatusKind::Error);
                    return;
                }
            },
            None => Vec::new(),
        };
        let name = if folder.is_empty() {
            format!("Run {}", self.project.name)
        } else {
            format!("Run {}", name_of(folder))
        };
        let id = self.alloc_id();
        let view = RunView::new(folder, data.map(str::to_string), rels, rows);
        self.tabs.push(Tab::new_run(id, name, view));
        self.active = self.tabs.len() - 1;
        self.focus = Focus::ReqTabs;
        self.start_run(id);
    }

    /// Starts (or starts again) the run of the run tab `id` with the active
    /// environment and empty runtime values.
    pub(super) fn start_run(&mut self, id: u64) {
        let project = self.project.clone();
        let env = self.active_env.clone();
        let tx = self.tx.clone();
        let rt = self.rt.clone();
        let Some(t) = self.tab_by_id(id) else { return };
        let Some(view) = t.run.as_mut() else { return };
        if t.running.is_some() {
            return;
        }
        view.restart();
        t.response = None;
        let rels = view.rels.clone();
        let cancel = view.cancel.clone();
        let opts = suite::Options {
            env: env.clone(),
            mask_echoed_secrets: true,
            data: view.data.clone(),
            ..Default::default()
        };
        let task = rt.spawn(async move {
            let envs = project.envs.clone();
            let active = env.clone();
            let (scope, errors, failed) = tokio::task::spawn_blocking(move || {
                runner::build_scope_checked(&envs, active.as_deref(), &Default::default())
            })
            .await
            .unwrap_or_default();
            let plan = suite::Plan {
                project: &project,
                rels: &rels,
                scope,
                scope_errors: &errors,
                failed: &failed,
            };
            let ask = |req: &Request, scope: &Scope| {
                let asks = sendito_core::resolve::pre_send_calls(req, scope)
                    .into_iter()
                    .any(|(_, p)| !matches!(p, sendito_core::funcs::PreSend::Response { .. }));
                if asks {
                    Err(RUN_CANNOT_ASK.to_string())
                } else {
                    Ok(())
                }
            };
            let events = tx.clone();
            let on = move |event| {
                let _ = events.send(AppMsg::Run {
                    tab: id,
                    event: Box::new(event),
                });
            };
            suite::run(plan, &opts, ask, on, &cancel).await;
            let _ = tx.send(AppMsg::RunDone { tab: id });
        });
        t.running = Some(Running {
            task,
            control: None,
            started: Instant::now(),
            streaming_input: false,
        });
        self.session_dirty = true;
    }

    /// One event of the run of tab `id`.
    pub(super) fn on_run(&mut self, id: u64, event: suite::Event) {
        let Some(t) = self.tab_by_id(id) else { return };
        let Some(view) = t.run.as_mut() else { return };
        match event {
            suite::Event::Note(n) => view.pending_log.push(format!("note: {n}")),
            suite::Event::Log(l) => view.pending_log.push(l),
            suite::Event::Retry {
                label,
                attempt,
                attempts,
                interval_ms,
                ..
            } => view.pending_log.push(format!(
                "attempt {attempt}/{attempts} did not pass ({label}); next in {}",
                crate::units::format_ms(interval_ms)
            )),
            suite::Event::Finished { entry, response } => {
                let log = std::mem::take(&mut view.pending_log);
                let resp = row_response(&entry, response, &log);
                // The selection follows the new rows while it is on the last one.
                let follow = view.rows.is_empty() || view.sel + 1 == view.rows.len();
                view.rows.push(RunRow {
                    entry,
                    response: Some(resp),
                });
                if follow {
                    let last = view.rows.len() - 1;
                    select_row(t, last);
                }
            }
        }
    }

    /// The run of tab `id` ended (finished, failed fast or cancelled).
    pub(super) fn on_run_done(&mut self, id: u64) {
        let Some(t) = self.tab_by_id(id) else { return };
        let Some(view) = t.run.as_mut() else { return };
        t.running = None;
        view.took = Some(view.started.elapsed());
        let (pass, fail, error) = (view.count("pass"), view.count("fail"), view.count("error"));
        let stopped = if view.cancel.is_cancelled() {
            " (stopped)"
        } else {
            ""
        };
        let kind = if fail + error > 0 {
            StatusKind::Error
        } else {
            StatusKind::Ok
        };
        let msg = format!("run done{stopped}: {pass} passed, {fail} failed, {error} error(s)");
        if self.tabs.get(self.active).is_some_and(|t| t.id == id) {
            self.set_status(msg, kind);
        }
    }

    /// Keys of the active run tab while the request pane has the focus:
    /// Up and Down choose a row, Enter goes to its response. Returns `false`
    /// for a key it does not use.
    pub(super) fn run_key(&mut self, key: KeyEvent) -> bool {
        if !self.focus.in_request() || !self.pending.is_empty() {
            return false;
        }
        let page = self.areas.request.height.saturating_sub(4).max(1) as isize;
        let Some(t) = self.tab_mut() else {
            return false;
        };
        let Some(view) = t.run.as_ref() else {
            return false;
        };
        let n = view.rows.len() as isize;
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        let to = match key.code {
            KeyCode::Up | KeyCode::Char('k') if plain => view.sel as isize - 1,
            KeyCode::Down | KeyCode::Char('j') if plain => view.sel as isize + 1,
            KeyCode::PageUp => view.sel as isize - page,
            KeyCode::PageDown => view.sel as isize + page,
            KeyCode::Home | KeyCode::Char('g') if plain => 0,
            KeyCode::End | KeyCode::Char('G') if plain => n - 1,
            KeyCode::Enter if plain => {
                self.focus = Focus::RespBody;
                return true;
            }
            _ => return false,
        };
        if n > 0 {
            select_row(t, to.clamp(0, n - 1) as usize);
        }
        self.focus = Focus::ReqTabs;
        true
    }

    /// An action on the active run tab: Send runs again, Cancel stops the
    /// run, and the actions that need a request say so. Returns `false` for
    /// the other actions.
    pub(super) fn run_action(&mut self, a: Action) -> bool {
        let Some(t) = self.tab() else { return false };
        if t.run.is_none() {
            return false;
        }
        let (id, running) = (t.id, t.running.is_some());
        match a {
            Action::Send if !matches!(self.focus, Focus::Sidebar | Focus::Filter) => {
                if running {
                    let key = self.keymap.hint(Action::Cancel, self.vim);
                    self.set_status(
                        format!("the run is still going ({key} stops it)"),
                        StatusKind::Info,
                    );
                } else {
                    self.start_run(id);
                }
            }
            Action::Cancel if running => {
                if let Some(v) = self.tab().and_then(|t| t.run.as_ref()) {
                    v.cancel.cancel();
                }
                self.set_status("stopping the run", StatusKind::Info);
            }
            Action::Save
            | Action::WatchToggle
            | Action::EndStream
            | Action::ExportCurl
            | Action::FormatBody
            | Action::ExternalEdit
            | Action::MoveToSecret => {
                self.set_status("a run tab has no request to edit or send", StatusKind::Info);
            }
            _ => return false,
        }
        true
    }
}

/// Shows row `i` of the run tab `t` in its response pane. The pane gives
/// back the response of the row it showed.
fn select_row(t: &mut Tab, i: usize) {
    let Some(view) = t.run.as_mut() else { return };
    if let Some(j) = view.shown.take()
        && let Some(row) = view.rows.get_mut(j)
    {
        row.response = t.response.take();
    }
    view.sel = i;
    t.response = view.rows.get_mut(i).and_then(|r| r.response.take());
    view.shown = Some(i);
    t.resp_tab = 0;
}

/// The response pane of one row: the HTTP exchange, else the status and
/// the assertions. `log` goes first in Messages.
fn row_response(
    entry: &suite::Entry,
    response: Option<Box<HttpOutcome>>,
    log: &[String],
) -> Response {
    let mut resp = match response {
        Some(mut o) => http_response(&mut o, None),
        None => {
            let mut resp = Response::new();
            let status = entry.status.clone().unwrap_or_else(|| "Error".into());
            resp.summary = Summary {
                status: status.clone(),
                ok: entry.outcome == "pass",
                time_ms: entry.duration_ms.map(|ms| ms as f64),
                error: entry.error.clone(),
                ..Default::default()
            };
            let mut body = format!("{}  {}\n", entry.outcome.to_uppercase(), entry.path);
            match &entry.error {
                Some(e) => body.push_str(&format!("\n{e}\n")),
                None => body.push_str(&format!("\n{status}\n")),
            }
            for a in &entry.asserts {
                let mark = if a.pass { "✓" } else { "✗" };
                body.push_str(&format!("{mark} {}  (actual: {})\n", a.label, a.actual));
            }
            resp.set_body_wrapped(body);
            resp
        }
    };
    if !log.is_empty() {
        let mut text = log.join("\n");
        text.push_str("\n\n");
        text.push_str(&String::from_utf8_lossy(resp.messages.doc.bytes()));
        resp.messages = crate::tab::wrapped(Doc::text(&text));
    }
    resp
}
