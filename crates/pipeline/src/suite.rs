//! A folder run, shared by `sendito run` and the TUI run tab: every request
//! under a folder in tree order, once per data row and round, with retries,
//! a delay, fail-fast and the variables one request extracts for the next.
//! The caller reads the progress as [`Event`]s and can stop the run with
//! [`Cancel`].

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sendito_core::model::{MqttRequest, Request, SseRequest};
use sendito_core::script::{AssertResult, RUNTIME_LAYER};
use sendito_core::vars::{Layer, Scope, VarValue};
use sendito_http::HttpResponse;
use tokio::sync::Notify;

use crate::diagnose::Hint;
use crate::runner::{
    self, Control, GrpcUpdate, HttpOutcome, McpRunInfo, MqttControl, MqttUpdate, Project,
    ResponseCache, RunCtx, SchemaCache, ScriptResults, SseControl, SseUpdate,
};

/// The scope layer of a data row: above the folder variables, below the
/// runtime values.
pub const DATA_LAYER: &str = "data";

/// One row of a data file: variable name → value.
pub type Row = BTreeMap<String, String>;

/// One request's line in a folder run report.
#[derive(Debug, Clone, Default)]
pub struct Entry {
    /// The request path relative to the collection (`Users/list.toml`), with
    /// `#round` for a repeat or `[row]` for a data row.
    pub path: String,
    /// `http`, `grpc`, …; `None` when the file did not load.
    pub kind: Option<&'static str>,
    /// `pass`, `fail` or `error`.
    pub outcome: &'static str,
    /// `200 OK`, `OK`, …; `None` when the request never ran.
    pub status: Option<String>,
    pub duration_ms: Option<u128>,
    /// The assertions and the Lua `test` results, secrets masked.
    pub asserts: Vec<AssertResult>,
    /// `None` for a file that did not load (the JSON report leaves the key out).
    pub unresolved: Option<bool>,
    pub error: Option<String>,
    pub events: Option<u64>,
    pub messages: Option<u64>,
    /// How many times the request was sent (0 when it never ran).
    pub attempts: u32,
    /// The configuration problem behind an `error`, when one is known.
    pub hint: Option<Hint>,
}

impl Entry {
    /// The entry in the JSON report (`docs/CLI.md`, "The JSON report"):
    /// assertion counts, not the assertions. A key with no value is left out
    /// for `unresolved`, `events` and `messages`.
    pub fn json(&self) -> serde_json::Value {
        let mut v = serde_json::json!({
            "path": self.path,
            "kind": self.kind,
            "outcome": self.outcome,
            "status": self.status,
            "duration_ms": self.duration_ms,
            "asserts": self.asserts.len(),
            "failed_asserts": self.asserts.iter().filter(|a| !a.pass).count(),
            "attempts": self.attempts,
        });
        if let Some(u) = self.unresolved {
            v["unresolved"] = u.into();
        }
        v["error"] = self.error.clone().into();
        if let Some(h) = &self.hint {
            v["hint"] = h.fix.clone().into();
        }
        if let Some(n) = self.events {
            v["events"] = n.into();
        }
        if let Some(n) = self.messages {
            v["messages"] = n.into();
        }
        v
    }
}

/// The result of one send in a folder run.
#[derive(Debug, Clone, Default)]
pub struct Ran {
    /// `pass`, `fail` (a failed assertion or a tool `isError`) or `error`.
    pub outcome: &'static str,
    /// Status text: `201 Created`, `OK`, or the error message.
    pub label: String,
    pub scripts: ScriptResults,
    /// SSE events received; only SSE sets it.
    pub events: Option<u64>,
    /// MQTT messages received; only MQTT sets it.
    pub messages: Option<u64>,
    /// What the server sent, one item per message (gRPC, WebSocket, MCP,
    /// SSE, MQTT); only with [`Options::keep_output`].
    pub output: Vec<String>,
    /// The configuration problem behind an `error`, when one is known.
    pub hint: Option<Hint>,
}

impl Ran {
    fn error(label: impl Into<String>) -> Self {
        Self {
            outcome: "error",
            label: label.into(),
            ..Default::default()
        }
    }

    /// `pass` or `fail` from the assertions; `error` when `error` is set.
    fn judged(error: Option<String>, label: String, scripts: ScriptResults) -> Self {
        let outcome = if error.is_some() {
            "error"
        } else if scripts.failed_asserts() > 0 {
            "fail"
        } else {
            "pass"
        };
        Self {
            outcome,
            label: error.unwrap_or(label),
            scripts,
            ..Default::default()
        }
    }
}

/// Stops a run: the request in flight ends (a session closes as on its
/// cancel control) and no other request starts. Clones share the state.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<(AtomicBool, Notify)>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.0.store(true, Ordering::SeqCst);
        self.0.1.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.0.load(Ordering::SeqCst)
    }

    /// Completes when [`Cancel::cancel`] is called (at once if it was).
    pub async fn cancelled(&self) {
        loop {
            let notified = self.0.1.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

/// How a folder run sends its requests.
#[derive(Debug, Clone)]
pub struct Options {
    pub env: Option<String>,
    /// Send a request that has unresolved variables.
    pub allow_unresolved: bool,
    pub no_history: bool,
    pub no_cookies: bool,
    /// Stop at the first request that does not pass.
    pub fail_fast: bool,
    /// Retries of a request that does not pass; `None`: the request's `retry`.
    pub retry: Option<u32>,
    /// Rounds of the whole folder.
    pub repeat: u32,
    /// The wait between two requests.
    pub delay_ms: u64,
    /// Mask secret values the server echoes back (the TUI).
    pub mask_echoed_secrets: bool,
    /// The data rows; empty: no data file. Each row runs the folder once.
    pub data: Vec<Row>,
    /// Keep what the server sends in [`Event::Finished`] (`output`).
    pub keep_output: bool,
    /// Read only the files of the collection (`RunCtx::confine_files`).
    pub confine_files: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            env: None,
            allow_unresolved: false,
            no_history: false,
            no_cookies: false,
            fail_fast: false,
            retry: None,
            repeat: 1,
            delay_ms: 0,
            mask_echoed_secrets: false,
            data: Vec::new(),
            keep_output: false,
            confine_files: false,
        }
    }
}

/// The progress of a folder run, in order.
#[derive(Debug)]
pub enum Event {
    /// A note about one request (`Users/a.toml: …`).
    Note(String),
    /// A progress line of a session (`SSE connected: 200`).
    Log(String),
    /// A send did not pass and is sent again after `interval_ms`.
    Retry {
        path: String,
        label: String,
        attempt: u32,
        attempts: u32,
        interval_ms: u64,
    },
    /// One request is done. Boxed: the entry is large, the other variants
    /// are not.
    Finished(Box<Sent>),
}

/// One finished request.
#[derive(Debug)]
pub struct Sent {
    pub entry: Entry,
    /// The HTTP exchange, when there is one.
    pub response: Option<Box<HttpOutcome>>,
    /// What the server sent (see [`Ran::output`]).
    pub output: Vec<String>,
}

/// What a run starts from: the loaded scope and what did not load.
pub struct Plan<'a> {
    pub project: &'a Project,
    pub scope: Scope,
    pub scope_errors: &'a [String],
    /// The names whose value did not load (see `build_scope_checked`).
    pub failed: &'a [String],
}

/// Runs the requests `rels` (in run order) from `plan` with `opts`. `ask`
/// gives the `$prompt` and `$select` values of a request (an error makes the
/// request unresolved); `on` gets every [`Event`].
pub async fn run<A, F>(
    plan: Plan<'_>,
    rels: &[String],
    opts: &Options,
    mut ask: A,
    mut on: F,
    cancel: &Cancel,
) where
    A: FnMut(&Request, &Scope) -> Result<(), String>,
    F: FnMut(Event),
{
    let Plan {
        project,
        scope: base,
        scope_errors,
        failed,
    } = plan;
    // The MCP sessions stay open for the whole run.
    let mcp_info = run_mcp_info(project);
    // One schema cache for the whole run: reflection and `.proto` files are
    // loaded once, not once per gRPC request.
    let schemas = SchemaCache::default();
    let rows: Vec<Option<(usize, &Row)>> = if opts.data.is_empty() {
        vec![None]
    } else {
        opts.data
            .iter()
            .enumerate()
            .map(|(i, r)| Some((i + 1, r)))
            .collect()
    };
    let mut sent = 0usize;
    'run: for row in rows {
        // A data row starts from the loaded scope and its own responses: the
        // values of one row never reach the next.
        let mut scope = base.clone();
        let responses = match row {
            Some(_) => ResponseCache::default(),
            None => project.responses.clone(),
        };
        for round in 1..=opts.repeat.max(1) {
            for rel in rels {
                if cancel.is_cancelled() {
                    break 'run;
                }
                if sent > 0 && opts.delay_ms > 0 {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(opts.delay_ms)) => {}
                        _ = cancel.cancelled() => break 'run,
                    }
                }
                sent += 1;
                let path = match row {
                    Some((n, _)) => format!("{rel}[{n}]"),
                    None if opts.repeat > 1 => format!("{rel}#{round}"),
                    None => rel.clone(),
                };
                let mut ctx = run_ctx(
                    project,
                    opts,
                    sendito_store::collection::name_of(rel).to_string(),
                    Some(rel.clone()),
                );
                ctx.schemas = schemas.clone();
                ctx.responses = responses.clone();
                ctx.read_history = row.is_none();
                let one = One {
                    project,
                    scope_errors,
                    failed,
                    opts,
                    ctx: &ctx,
                    mcp_info: &mcp_info,
                    cancel,
                };
                let sent = one
                    .send(
                        rel,
                        path,
                        row.map(|(_, r)| r),
                        &mut scope,
                        &mut ask,
                        &mut on,
                    )
                    .await;
                let stop = opts.fail_fast && sent.entry.outcome != "pass";
                on(Event::Finished(Box::new(sent)));
                if stop {
                    break 'run;
                }
            }
        }
    }
}

/// Sends `req`, a request that no file holds, as a run of one request:
/// no folder settings, no data rows and no rounds. `name` is its name in the
/// history and its path in the report.
pub async fn run_request<A, F>(
    plan: Plan<'_>,
    req: &Request,
    name: &str,
    opts: &Options,
    mut ask: A,
    mut on: F,
    cancel: &Cancel,
) -> Sent
where
    A: FnMut(&Request, &Scope) -> Result<(), String>,
    F: FnMut(Event),
{
    let Plan {
        project,
        mut scope,
        scope_errors,
        failed,
    } = plan;
    let mcp_info = run_mcp_info(project);
    let ctx = run_ctx(project, opts, name.to_string(), None);
    let one = One {
        project,
        scope_errors,
        failed,
        opts,
        ctx: &ctx,
        mcp_info: &mcp_info,
        cancel,
    };
    let rscope = scope.clone();
    one.send_loaded(req, name.to_string(), rscope, &mut scope, &mut ask, &mut on)
        .await
}

/// `req`, the request at `rel`, with the settings of the folders above it;
/// their variables go to `scope`. The note names the folder headers that the
/// request does not get (see `ignored_headers_note`).
pub fn in_folders(
    project: &Project,
    rel: &str,
    req: &Request,
    scope: &mut Scope,
) -> sendito_store::Result<(Request, Option<String>)> {
    let chain = project.coll.chain(rel)?;
    let note = sendito_core::inherit::ignored_headers_note(req, &chain);
    Ok((runner::with_folders(req, &chain, scope), note))
}

/// Why a request is not sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotReady {
    /// A `$prompt` or `$select` has no value: the reason `ask` gave.
    Ask(String),
    /// The placeholders that do not resolve, as a list of names.
    Unresolved(String),
}

/// Checks that `req` can be sent with `scope`: `ask` gives the values of its
/// `$prompt` and `$select` calls, then every placeholder must resolve.
/// `failed` names the values that did not load (see `build_scope_checked`).
pub fn check_ready<A>(
    project: &Project,
    req: &Request,
    scope: &Scope,
    failed: &[String],
    ask: A,
) -> Result<(), NotReady>
where
    A: FnOnce(&Request, &Scope) -> Result<(), String>,
{
    ask(req, scope).map_err(NotReady::Ask)?;
    let un = runner::unresolved_send(req, scope, failed, &project.config.mcp.servers);
    if un.is_empty() {
        Ok(())
    } else {
        Err(NotReady::Unresolved(runner::unresolved_list(&un, scope)))
    }
}

/// How many times `req` is sent at most (`retry`, else its `[options]
/// retry`, plus one) and the wait before each retry, in milliseconds.
pub fn attempts(req: &Request, retry: Option<u32>) -> (u32, u64) {
    let common = req.common();
    let attempts = retry.unwrap_or(common.retry).saturating_add(1);
    (attempts, common.retry_interval())
}

/// The MCP sessions of a run: kept open until the run ends.
fn run_mcp_info(project: &Project) -> McpRunInfo {
    let mut mcp_info = McpRunInfo::of(project);
    mcp_info.pool = Some(sendito_mcp::Pool::default());
    mcp_info
}

/// The context of one send of a run.
fn run_ctx(project: &Project, opts: &Options, name: String, path: Option<String>) -> RunCtx {
    let mut ctx = RunCtx::of(project, name, path);
    if opts.no_history {
        ctx.history = None;
    }
    ctx.cookie_jar &= !opts.no_cookies;
    ctx.env = opts.env.clone();
    ctx.mask_echoed_secrets = opts.mask_echoed_secrets;
    ctx.confine_files = opts.confine_files;
    ctx
}

/// One request of a run and what it reads.
struct One<'a> {
    project: &'a Project,
    scope_errors: &'a [String],
    failed: &'a [String],
    opts: &'a Options,
    ctx: &'a RunCtx,
    mcp_info: &'a McpRunInfo,
    cancel: &'a Cancel,
}

impl One<'_> {
    /// Loads, checks and sends `rel` with its retries. The runtime values it
    /// sets go to `scope`.
    async fn send<A, F>(
        &self,
        rel: &str,
        path: String,
        row: Option<&Row>,
        scope: &mut Scope,
        ask: &mut A,
        on: &mut F,
    ) -> Sent
    where
        A: FnMut(&Request, &Scope) -> Result<(), String>,
        F: FnMut(Event),
    {
        let project = self.project;
        // The request gets its folders' variables (and the data row) on top
        // of the shared scope; the runtime values it sets go to the shared scope.
        let mut rscope = scope.clone();
        let loaded = project
            .coll
            .load(rel)
            .and_then(|r| in_folders(project, rel, &r, &mut rscope));
        let req = match loaded {
            Ok((r, note)) => {
                if let Some(note) = note {
                    on(Event::Note(format!("{rel}: {note}")));
                }
                r
            }
            Err(e) => {
                let entry = Entry {
                    path,
                    outcome: "error",
                    error: Some(e.to_string()),
                    ..Default::default()
                };
                return Sent {
                    entry,
                    response: None,
                    output: Vec::new(),
                };
            }
        };
        if let Some(row) = row {
            let vars = row
                .iter()
                .map(|(k, v)| (k.clone(), VarValue::plain(v.clone())))
                .collect();
            rscope.insert_below(
                RUNTIME_LAYER,
                vec![Layer {
                    name: DATA_LAYER.into(),
                    vars,
                }],
            );
        }
        self.send_loaded(&req, path, rscope, scope, ask, on).await
    }

    /// Checks and sends `req`, already loaded, with its retries. `rscope` is
    /// the scope of the request; the runtime values it sets go to `scope`.
    async fn send_loaded<A, F>(
        &self,
        req: &Request,
        path: String,
        rscope: Scope,
        scope: &mut Scope,
        ask: &mut A,
        on: &mut F,
    ) -> Sent
    where
        A: FnMut(&Request, &Scope) -> Result<(), String>,
        F: FnMut(Event),
    {
        let project = self.project;
        let kind = kind_of(req);
        let unresolved = |error: String| Sent {
            entry: Entry {
                path: path.clone(),
                kind: Some(kind),
                outcome: "error",
                unresolved: Some(true),
                error: Some(error),
                hint: Some(crate::diagnose::unresolved()),
                ..Default::default()
            },
            response: None,
            output: Vec::new(),
        };
        if let Err(e) = check_ready(project, req, &rscope, self.failed, &mut *ask)
            && !self.opts.allow_unresolved
        {
            return unresolved(match e {
                NotReady::Ask(e) => e,
                NotReady::Unresolved(names) => format!("unresolved variables: {names}"),
            });
        }
        let (attempts, interval) = attempts(req, self.opts.retry);
        let mut attempt = 1;
        let (ran, response, ms) = loop {
            let t0 = Instant::now();
            let (ran, response) = self.once(req, rscope.clone(), on).await;
            let ms = t0.elapsed().as_millis();
            if ran.outcome == "pass" || attempt == attempts || self.cancel.is_cancelled() {
                break (ran, response, ms);
            }
            on(Event::Retry {
                path: path.clone(),
                label: ran.label.clone(),
                attempt,
                attempts,
                interval_ms: interval,
            });
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(interval)) => {}
                _ = self.cancel.cancelled() => break (ran, response, ms),
            }
            attempt += 1;
        };
        for (var, value) in ran.scripts.updates() {
            scope.set(RUNTIME_LAYER, &var, value);
        }
        let entry = Entry {
            path,
            kind: Some(kind),
            outcome: ran.outcome,
            attempts: attempt,
            status: Some(ran.label),
            duration_ms: Some(ms),
            asserts: ran.scripts.asserts,
            unresolved: Some(false),
            error: None,
            events: ran.events,
            messages: ran.messages,
            hint: ran.hint,
        };
        Sent {
            entry,
            response,
            output: ran.output,
        }
    }

    /// One send. A cancel ends a session as its cancel control does and
    /// drops any other request in flight.
    async fn once<F: FnMut(Event)>(
        &self,
        req: &Request,
        scope: Scope,
        on: &mut F,
    ) -> (Ran, Option<Box<HttpOutcome>>) {
        let (ctx, errors, cancel) = (self.ctx, self.scope_errors.to_vec(), self.cancel);
        let keep = self.opts.keep_output;
        let mut output = Vec::new();
        let mut log = |live: Live<'_>| {
            match live {
                Live::Line(l) => on(Event::Log(l)),
                Live::Data(d) if keep => output.push(d.to_string()),
                Live::Message { payload, .. } if keep => output.push(payload_text(payload)),
                _ => {}
            }
            true
        };
        match req {
            Request::Sse(s) => {
                let mut s = s.clone();
                if self.opts.no_cookies {
                    s.options.cookies = Some(false);
                }
                let ran = run_sse(ctx, &s, scope, errors, cancel, &mut log).await;
                (Ran { output, ..ran }, None)
            }
            Request::Mqtt(m) => {
                let ran = run_mqtt(ctx, m, scope, errors, cancel, &mut log).await;
                (Ran { output, ..ran }, None)
            }
            _ => {
                let opts = SendOne {
                    mcp_info: self.mcp_info,
                    no_cookies: self.opts.no_cookies,
                    keep,
                };
                tokio::select! {
                    r = send_one(ctx, req, scope, errors, opts) => r,
                    _ = cancel.cancelled() => (Ran::error("cancelled"), None),
                }
            }
        }
    }
}

/// The `kind` name of a request in the report.
pub fn kind_of(req: &Request) -> &'static str {
    match req {
        Request::Http(_) => "http",
        Request::Grpc(_) => "grpc",
        Request::Ws(_) => "websocket",
        Request::Sse(_) => "sse",
        Request::Mqtt(_) => "mqtt",
        Request::Mcp(_) => "mcp",
    }
}

/// An MQTT payload as text: UTF-8 when it is, else base64.
fn payload_text(payload: &[u8]) -> String {
    match std::str::from_utf8(payload) {
        Ok(s) => s.to_string(),
        Err(_) => {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.encode(payload)
        }
    }
}

/// How [`send_one`] sends.
struct SendOne<'a> {
    mcp_info: &'a McpRunInfo,
    no_cookies: bool,
    /// Keep what the server sends ([`Ran::output`]).
    keep: bool,
}

/// Sends one HTTP, gRPC, WebSocket or MCP request and classifies the outcome.
async fn send_one(
    ctx: &RunCtx,
    req: &Request,
    scope: Scope,
    errors: Vec<String>,
    opts: SendOne<'_>,
) -> (Ran, Option<Box<HttpOutcome>>) {
    let SendOne {
        mcp_info,
        no_cookies,
        keep,
    } = opts;
    let mut output = Vec::new();
    match req {
        Request::Http(h) => {
            let mut h = h.clone();
            if no_cookies {
                h.options.cookies = Some(false);
            }
            let o = runner::exec_http(ctx, &h, scope, errors).await;
            let ran = match &o.response {
                Ok(r) => Ran::judged(
                    None,
                    format!("{} {}", r.status, r.reason),
                    o.scripts.clone(),
                ),
                Err(e) => Ran {
                    scripts: o.scripts.clone(),
                    hint: o.hint.clone(),
                    ..Ran::error(e.clone())
                },
            };
            (ran, Some(Box::new(o)))
        }
        Request::Grpc(g) => {
            let (utx, mut urx) = tokio::sync::mpsc::unbounded_channel();
            let (ctl_tx, ctl_rx) = tokio::sync::mpsc::unbounded_channel();
            drop(ctl_tx);
            let run = runner::exec_grpc(ctx, g, scope, errors, utx, ctl_rx);
            let mut error = None;
            let mut label = String::from("OK");
            let mut scripts = ScriptResults::default();
            let mut hint = None;
            // Drive the call and collect its updates together: the channel is
            // only fed while `run` is polled.
            let collect = async {
                while let Some(u) = urx.recv().await {
                    match u {
                        GrpcUpdate::Event(sendito_grpc::Event::Received { json, .. }) if keep => {
                            output.push(json);
                        }
                        GrpcUpdate::Event(sendito_grpc::Event::Finished {
                            code: c,
                            message,
                            ..
                        }) => {
                            label = if c == 0 {
                                "OK".into()
                            } else if message.is_empty() {
                                format!("{} ({c})", sendito_grpc::code_name(c))
                            } else {
                                format!("{}: {message}", sendito_grpc::code_name(c))
                            };
                            if c != 0 {
                                error = Some(label.clone());
                            }
                        }
                        GrpcUpdate::Error(e) => error = Some(e),
                        GrpcUpdate::Done {
                            scripts: s,
                            hint: h,
                            ..
                        } => (scripts, hint) = (s, h),
                        _ => {}
                    }
                }
            };
            tokio::join!(run, collect);
            let ran = Ran::judged(error, label, scripts);
            (
                Ran {
                    output,
                    hint,
                    ..ran
                },
                None,
            )
        }
        Request::Ws(w) => {
            let mut w = w.clone();
            if no_cookies {
                w.options.cookies = Some(false);
            }
            let (utx, mut urx) = tokio::sync::mpsc::unbounded_channel();
            let (ctl_tx, ctl_rx) = tokio::sync::mpsc::unbounded_channel::<Control>();
            drop(ctl_tx);
            let run = runner::exec_ws(ctx, &w, scope, errors, utx, ctl_rx);
            let mut code = 1006u16;
            let mut label = String::from("close 1006");
            let mut error: Option<String> = None;
            let mut scripts = ScriptResults::default();
            let mut hint = None;
            let collect = async {
                while let Some(u) = urx.recv().await {
                    match u {
                        runner::WsUpdate::Event(sendito_ws::Event::Received {
                            preview, ..
                        }) if keep => {
                            output.push(preview);
                        }
                        runner::WsUpdate::Event(sendito_ws::Event::Finished {
                            code: c,
                            reason,
                            ..
                        }) => {
                            code = c;
                            label = if reason.is_empty() {
                                format!("close {c}")
                            } else {
                                format!("close {c}: {reason}")
                            };
                        }
                        runner::WsUpdate::Error(e) => error = Some(e),
                        runner::WsUpdate::Done {
                            scripts: s,
                            hint: h,
                            ..
                        } => (scripts, hint) = (s, h),
                        _ => {}
                    }
                }
            };
            tokio::join!(run, collect);
            if error.is_none() && code != 1000 {
                error = Some(label.clone());
            }
            let ran = Ran::judged(error, label, scripts);
            (
                Ran {
                    output,
                    hint,
                    ..ran
                },
                None,
            )
        }
        Request::Mcp(m) => {
            let (utx, mut urx) = tokio::sync::mpsc::unbounded_channel();
            let run = runner::exec_mcp(ctx, m, scope, errors, mcp_info, utx);
            let mut error = None;
            let mut is_error = false;
            let mut scripts = ScriptResults::default();
            let mut hint = None;
            // Same as gRPC: the future must be polled while the updates are
            // read, or the channel never produces anything.
            let collect = async {
                while let Some(u) = urx.recv().await {
                    if let runner::McpUpdate::Done {
                        error: e,
                        is_error: i,
                        result,
                        scripts: s,
                        hint: h,
                        ..
                    } = u
                    {
                        (error, is_error, scripts, hint) = (e, i, s, h);
                        if keep && let Some(r) = result {
                            output.push(String::from_utf8_lossy(&r).into_owned());
                        }
                    }
                }
            };
            tokio::join!(run, collect);
            if is_error && error.is_none() {
                // The tool answered isError: the call went through, the tool
                // failed (decision: a fail, not an error).
                return (
                    Ran {
                        outcome: "fail",
                        label: "the tool reported an error".into(),
                        scripts,
                        output,
                        ..Default::default()
                    },
                    None,
                );
            }
            let ran = Ran::judged(error, "ok".into(), scripts);
            (
                Ran {
                    output,
                    hint,
                    ..ran
                },
                None,
            )
        }
        Request::Sse(_) | Request::Mqtt(_) => (Ran::error("not a one-shot request"), None),
    }
}

/// What an SSE or MQTT session reports while it runs, for the caller to
/// print or drop.
pub enum Live<'a> {
    /// A progress line (`SSE connected: 200`, `MQTT subscribed a/b (QoS 1)`).
    Line(String),
    /// The SSE response head, before the first event.
    Head(&'a HttpResponse),
    /// The data of one SSE event.
    Data(&'a str),
    /// One MQTT message.
    Message {
        topic: &'a str,
        qos: u8,
        retain: bool,
        duplicate: bool,
        payload: &'a [u8],
    },
}

/// Runs one SSE session. `on` gets the progress; when it returns `false`
/// (the output failed), the session is cancelled and the result is an error.
pub async fn run_sse<F>(
    ctx: &RunCtx,
    req: &SseRequest,
    scope: Scope,
    errors: Vec<String>,
    cancel: &Cancel,
    on: &mut F,
) -> Ran
where
    F: FnMut(Live<'_>) -> bool,
{
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let (control, controls) = tokio::sync::mpsc::unbounded_channel();
    let run = runner::exec_sse(ctx, req, scope, errors, tx, controls);
    let mut result = Ran {
        events: Some(0),
        ..Ran::error("SSE session did not finish")
    };
    let collect = async {
        let mut stopped = false;
        let stop = |stopped: &mut bool| {
            *stopped = true;
            let _ = control.send(Control::Sse(SseControl::Cancel));
        };
        loop {
            tokio::select! {
                _ = cancel.cancelled(), if !stopped => stop(&mut stopped),
                update = rx.recv() => match update {
                    Some(SseUpdate::Started { response, .. }) => {
                        let ok = on(Live::Line(format!("SSE connected: {}", response.status)))
                            && on(Live::Head(&response));
                        if !ok {
                            stop(&mut stopped);
                        }
                    }
                    Some(SseUpdate::Event { event, .. }) => {
                        if !on(Live::Data(&event.data)) {
                            stop(&mut stopped);
                        }
                    }
                    Some(SseUpdate::Done { events, reason, error, scripts, hint, .. }) => {
                        on(Live::Line(format!("SSE {reason}: {events} events")));
                        if let Some(e) = &error {
                            on(Live::Line(format!("sendito: {e}")));
                        }
                        if let Some(h) = &hint && error.is_some() {
                            on(Live::Line(format!("sendito: hint: {}", h.fix)));
                        }
                        let error = error.or_else(|| stopped.then(|| reason.clone()));
                        result = Ran {
                            events: Some(events),
                            hint,
                            ..Ran::judged(error, reason, scripts)
                        };
                    }
                    None => break,
                }
            }
        }
    };
    tokio::join!(run, collect);
    result
}

/// Runs one MQTT session, as [`run_sse`] does.
pub async fn run_mqtt<F>(
    ctx: &RunCtx,
    req: &MqttRequest,
    scope: Scope,
    errors: Vec<String>,
    cancel: &Cancel,
    on: &mut F,
) -> Ran
where
    F: FnMut(Live<'_>) -> bool,
{
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let (control, controls) = tokio::sync::mpsc::unbounded_channel();
    let run = runner::exec_mqtt(ctx, req, scope, errors, tx, controls);
    let mut result = Ran {
        messages: Some(0),
        ..Ran::error("MQTT session did not finish")
    };
    let collect = async {
        let mut stopped = false;
        let stop = |stopped: &mut bool| {
            *stopped = true;
            let _ = control.send(Control::Mqtt(MqttControl::Cancel));
        };
        loop {
            tokio::select! {
                _ = cancel.cancelled(), if !stopped => stop(&mut stopped),
                update = rx.recv() => match update {
                    Some(MqttUpdate::Started { resolved }) => {
                        on(Live::Line(format!(
                            "MQTT connecting to {} as {}",
                            resolved.url, resolved.client_id
                        )));
                    }
                    Some(MqttUpdate::Connected { session_present, .. }) => {
                        let present = if session_present { "yes" } else { "no" };
                        on(Live::Line(format!("MQTT connected (session present: {present})")));
                    }
                    Some(MqttUpdate::Subscribed { topic, qos, .. }) => {
                        on(Live::Line(format!("MQTT subscribed {topic} (QoS {qos})")));
                    }
                    Some(MqttUpdate::SubscribeRefused { topic, .. }) => {
                        on(Live::Line(format!("MQTT subscription refused {topic}")));
                    }
                    Some(MqttUpdate::Published { topic, qos, .. }) => {
                        on(Live::Line(format!("MQTT published {topic} (QoS {qos})")));
                    }
                    Some(MqttUpdate::PublishFailed { error }) => {
                        on(Live::Line(format!("sendito: {error}")));
                    }
                    Some(MqttUpdate::Message { topic, qos, retain, duplicate, payload, .. }) => {
                        let message = Live::Message {
                            topic: &topic,
                            qos,
                            retain,
                            duplicate,
                            payload: &payload,
                        };
                        if !stopped && !on(message) {
                            stop(&mut stopped);
                        }
                    }
                    Some(MqttUpdate::Done { messages, reason, error, scripts, hint, .. }) => {
                        on(Live::Line(format!("MQTT {reason}: {messages} messages")));
                        if let Some(e) = &error {
                            on(Live::Line(format!("sendito: {e}")));
                        }
                        if let Some(h) = &hint && error.is_some() {
                            on(Live::Line(format!("sendito: hint: {}", h.fix)));
                        }
                        let error = error.or_else(|| stopped.then(|| reason.clone()));
                        result = Ran {
                            messages: Some(messages),
                            hint,
                            ..Ran::judged(error, reason, scripts)
                        };
                    }
                    None => break,
                }
            }
        }
    };
    tokio::join!(run, collect);
    result
}

/// The rows of a data file: a JSON array of objects (`.json`), else CSV
/// with the variable names in the first line. A JSON value that is not a
/// string becomes its JSON text.
pub fn load_data(path: &Path) -> Result<Vec<Row>, String> {
    let shown = path.display();
    let text = std::fs::read_to_string(path).map_err(|e| format!("{shown}: {e}"))?;
    let json = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("json"));
    let rows = if json {
        json_rows(&text)
    } else {
        csv_rows(&text)
    }
    .map_err(|e| format!("{shown}: {e}"))?;
    if rows.is_empty() {
        return Err(format!("{shown}: the data file has no rows"));
    }
    Ok(rows)
}

fn json_rows(text: &str) -> Result<Vec<Row>, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let serde_json::Value::Array(items) = v else {
        return Err("a JSON data file is an array of objects".into());
    };
    items
        .into_iter()
        .enumerate()
        .map(|(i, item)| match item {
            serde_json::Value::Object(o) => Ok(o
                .into_iter()
                .map(|(k, v)| {
                    let text = match v {
                        serde_json::Value::String(s) => s,
                        other => other.to_string(),
                    };
                    (k, text)
                })
                .collect()),
            _ => Err(format!("row {} is not an object", i + 1)),
        })
        .collect()
}

fn csv_rows(text: &str) -> Result<Vec<Row>, String> {
    let mut r = csv::ReaderBuilder::new()
        .trim(csv::Trim::Headers)
        .from_reader(text.as_bytes());
    let names = r.headers().map_err(|e| e.to_string())?.clone();
    r.records()
        .map(|rec| {
            let rec = rec.map_err(|e| e.to_string())?;
            Ok(names
                .iter()
                .zip(rec.iter())
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(name: &str, text: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        (dir, path)
    }

    #[test]
    fn data_files_give_one_row_per_record() {
        let (_d, p) = write("u.csv", "user, pass\nana,\"a,1\"\nbob,b2\n");
        let rows = load_data(&p).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["user"], "ana");
        assert_eq!(rows[0]["pass"], "a,1");
        let (_d, p) = write("u.json", r#"[{"id": 7, "name": "x", "tags": ["a"]}]"#);
        let rows = load_data(&p).unwrap();
        assert_eq!(rows[0]["id"], "7");
        assert_eq!(rows[0]["name"], "x");
        assert_eq!(rows[0]["tags"], r#"["a"]"#);
    }

    #[test]
    fn bad_data_files_say_why() {
        let (_d, p) = write("e.csv", "user\n");
        assert!(load_data(&p).unwrap_err().contains("no rows"));
        let (_d, p) = write("e.json", r#"{"a": 1}"#);
        assert!(load_data(&p).unwrap_err().contains("array of objects"));
        let (_d, p) = write("e.json", r#"[1]"#);
        assert!(
            load_data(&p)
                .unwrap_err()
                .contains("row 1 is not an object")
        );
        let (_d, p) = write("e.csv", "a,b\n1,2,3\n");
        assert!(load_data(&p).is_err());
    }

    #[tokio::test]
    async fn cancel_wakes_a_waiter_and_stays_set() {
        let c = Cancel::default();
        let c2 = c.clone();
        let wait = tokio::spawn(async move { c2.cancelled().await });
        tokio::task::yield_now().await;
        c.cancel();
        wait.await.unwrap();
        assert!(c.is_cancelled());
        c.cancelled().await;
    }
}
