//! `sendito run`: sends one request or a suite and prints the report.

use super::mqtt::run_mqtt;
use super::report::SuiteReport;
use super::sse::run_sse;
use crate::*;
use sendito_pipeline::suite;

/// Routes `run` to one request or a folder: a request file wins over a folder
/// of the same name, and errors from whichever path matched are reported as
/// they are.
pub(crate) async fn run_entry(request: &Path, flags: RunFlags) -> ExitCode {
    // Locate once: the result decides between a single request and a folder.
    if let Ok((root, rel)) = locate(request) {
        return match load_at(root, rel, &flags.env) {
            Ok((project, rel, req)) => run(project, rel, req, &flags).await,
            Err((e, code)) => {
                fail!("{e}");
                ExitCode::from(code)
            }
        };
    }
    if is_folder_arg(request) {
        return match load_suite(request, &flags.env) {
            Ok((project, folder)) => run_suite(project, folder, &flags).await,
            Err((e, code)) => {
                fail!("{e}");
                ExitCode::from(code)
            }
        };
    }
    fail!("request {} not found", request.display());
    ExitCode::from(3)
}

pub(crate) async fn run(project: Project, rel: String, req: Request, flags: &RunFlags) -> ExitCode {
    if flags.data.is_some() {
        if flags.include {
            fail!("-i does not apply to a run with --data");
            return ExitCode::from(3);
        }
        return run_rels(project, rel.clone(), vec![rel], flags).await;
    }
    let RunFlags {
        env,
        include,
        allow_unresolved,
        no_history,
        no_cookies,
        fail_fast,
        report,
        report_files,
        retry,
        repeat,
        delay_ms,
        prompts,
        data: _,
    } = flags.clone();
    if repeat > 1 || delay_ms > 0 {
        warn!("--repeat and --delay apply to folder runs; ignoring them for a single request");
    }
    if fail_fast {
        warn!("--fail-fast applies to folder runs; ignoring it for a single request");
    }
    let session = matches!(req, Request::Sse(_) | Request::Mqtt(_));
    if report != RunReport::Text && !(report == RunReport::Json && session) {
        warn!(
            "--report {} applies to folder runs; ignoring it for a single request",
            report_name(report)
        );
    }
    if !report_files.is_empty() {
        warn!("--report-file applies to folder runs; ignoring it for a single request");
    }
    let (mut scope, errors, failed) = scope_of(&project, &env).await;
    for e in &errors {
        warn!("{e}");
    }
    let req = match suite::in_folders(&project, &rel, &req, &mut scope) {
        Ok((req, note)) => {
            if let Some(note) = note {
                eprintln!("note: {note}");
            }
            req
        }
        Err(e) => {
            fail!("{e}");
            return ExitCode::from(3);
        }
    };
    // Check against the loaded values, as a folder run does: a secret that
    // failed to load (no keychain entry, no secrets.toml) is unresolved.
    let ask = |req: &Request, scope: &Scope| {
        super::ask::answer_prompts(req, scope, &prompts, &mut Default::default(), true)
    };
    if let Err(e) = suite::check_ready(&project, &req, &scope, &failed, ask)
        && !allow_unresolved
    {
        match e {
            suite::NotReady::Ask(e) => fail!("{e} (or pass --allow-unresolved)"),
            suite::NotReady::Unresolved(names) => fail!(
                "unresolved variables: {names} (define them in the environment or pass --allow-unresolved)"
            ),
        }
        return ExitCode::from(3);
    }
    let ctx = ctx_for(
        &project,
        &env,
        &rel,
        no_history,
        no_cookies,
        Default::default(),
    );
    let (attempts, interval) = suite::attempts(&req, retry);
    let mut attempt = 1;
    loop {
        let last = attempt == attempts;
        // Only the last attempt writes to stdout at once; an earlier one
        // keeps its output and writes it only when it passes.
        let mut buf = Vec::new();
        let once = Once {
            project: &project,
            ctx: &ctx,
            include,
            report,
            no_cookies,
        };
        let code = if last {
            once.send(
                req.clone(),
                scope.clone(),
                errors.clone(),
                &mut std::io::stdout().lock(),
            )
            .await
        } else {
            once.send(req.clone(), scope.clone(), errors.clone(), &mut buf)
                .await
        };
        if code == 0 && !last {
            let _ = std::io::stdout().write_all(&buf);
        }
        if code == 0 || code == 3 || last {
            return ExitCode::from(code);
        }
        eprintln!(
            "sendito: attempt {attempt}/{attempts} did not pass; retrying in {}",
            sendito_ui::units::format_ms(interval)
        );
        tokio::time::sleep(std::time::Duration::from_millis(interval)).await;
        attempt += 1;
    }
}

/// What one send of a single request needs besides the request.
struct Once<'a> {
    project: &'a Project,
    ctx: &'a RunCtx,
    include: bool,
    report: RunReport,
    no_cookies: bool,
}

impl Once<'_> {
    /// Sends the request once and prints its output; returns the exit code.
    async fn send(
        &self,
        req: Request,
        scope: Scope,
        errors: Vec<String>,
        out: &mut dyn Write,
    ) -> u8 {
        let Once {
            project,
            ctx,
            include,
            report,
            no_cookies,
        } = *self;
        match req {
            Request::Sse(mut s) => {
                if no_cookies {
                    s.options.cookies = Some(false);
                }
                let ran = run_sse(
                    ctx,
                    &s,
                    scope,
                    errors,
                    report != RunReport::Json,
                    include,
                    out,
                )
                .await;
                if report == RunReport::Json {
                    let _ = writeln!(
                        out,
                        "{}",
                        serde_json::json!({"kind":"sse", "outcome":ran.outcome, "events":ran.events, "status":ran.label, "failed_asserts":ran.scripts.failed_asserts()})
                    );
                }
                print_scripts(&ran.scripts);
                match ran.outcome {
                    "pass" => 0,
                    "fail" => 1,
                    _ => 2,
                }
            }
            Request::Mqtt(m) => {
                let ran = run_mqtt(
                    ctx,
                    &m,
                    scope,
                    errors,
                    report != RunReport::Json,
                    include,
                    out,
                )
                .await;
                if report == RunReport::Json {
                    let _ = writeln!(
                        out,
                        "{}",
                        serde_json::json!({"kind":"mqtt", "outcome":ran.outcome, "messages":ran.messages, "status":ran.label, "failed_asserts":ran.scripts.failed_asserts()})
                    );
                }
                print_scripts(&ran.scripts);
                match ran.outcome {
                    "pass" => 0,
                    "fail" => 1,
                    _ => 2,
                }
            }
            Request::Http(mut h) => {
                if no_cookies {
                    h.options.cookies = Some(false);
                }
                let o = runner::exec_http(ctx, &h, scope, errors).await;
                for n in &o.resolved.notes {
                    eprintln!("note: {n}");
                }
                let code = match &o.response {
                    Ok(r) => {
                        let decoded = r
                            .decoded
                            .as_ref()
                            .map(|d| format!(", {} {} decoded", d.encoding, human_size(d.raw_len)))
                            .unwrap_or_default();
                        eprintln!(
                            "{} {} {}  ({} ms, {}{})",
                            r.version,
                            r.status,
                            r.reason,
                            r.timing.total.as_millis(),
                            human_size(o.body_size),
                            decoded
                        );
                        if include {
                            for (k, v) in &r.headers {
                                eprintln!("{k}: {v}");
                            }
                            eprintln!();
                        }
                        if r.truncated {
                            eprintln!(
                                "sendito: warning: the body is cut at the [http] max_body_mb limit"
                            );
                        }
                        let _ = out.write_all(o.doc.bytes());
                        // Keep redirected output byte-exact; end the line only on a terminal.
                        if std::io::IsTerminal::is_terminal(&std::io::stdout())
                            && !o.doc.bytes().ends_with(b"\n")
                            && !o.doc.is_empty()
                        {
                            let _ = out.write_all(b"\n");
                        }
                        if let Some(h) = &o.hint {
                            fail!("hint: {}", h.fix);
                        }
                        0
                    }
                    Err(e) => {
                        fail!("request failed: {e}");
                        if let Some(h) = &o.hint {
                            fail!("hint: {}", h.fix);
                        }
                        if matches!(
                            o.resolved.body,
                            sendito_core::resolve::ResolvedBody::Invalid(_)
                        ) {
                            3
                        } else {
                            2
                        }
                    }
                };
                let fails = print_scripts(&o.scripts);
                if code != 0 {
                    code
                } else if fails > 0 {
                    1
                } else {
                    0
                }
            }
            Request::Grpc(g) => {
                let (utx, mut urx) = tokio::sync::mpsc::unbounded_channel();
                let (ctl_tx, ctl_rx) = tokio::sync::mpsc::unbounded_channel();
                drop(ctl_tx);
                // A secret in the target must not reach the log, as for WebSocket.
                let uri_secrets = scope.secret_values();
                let run = runner::exec_grpc(ctx, &g, scope, errors, utx, ctl_rx);
                let mut code = 0u8;
                let mut fails = 0;
                let print = async {
                    while let Some(u) = urx.recv().await {
                        match u {
                            GrpcUpdate::Started { resolved, kind } => eprintln!(
                                "{} /{}/{} ({})",
                                sendito_core::vars::scrub(&resolved.uri, &uri_secrets),
                                resolved.service,
                                resolved.method,
                                kind.label()
                            ),
                            GrpcUpdate::Event(sendito_grpc::Event::Received { json, .. }) => {
                                let _ = writeln!(out, "{json}");
                            }
                            GrpcUpdate::Event(sendito_grpc::Event::Finished {
                                code: c,
                                message,
                                at,
                                ..
                            }) => {
                                eprintln!(
                                    "status: {} ({c}){}  ({} ms)",
                                    sendito_grpc::code_name(c),
                                    if message.is_empty() {
                                        String::new()
                                    } else {
                                        format!(": {message}")
                                    },
                                    at.as_millis()
                                );
                                if c != 0 {
                                    code = 2;
                                }
                            }
                            GrpcUpdate::Event(_) => {}
                            GrpcUpdate::Error(e) => {
                                fail!("{e}");
                                code = 2;
                            }
                            GrpcUpdate::Done { scripts, hint, .. } => {
                                // The cause of a failed call, with the setting to change.
                                if code != 0
                                    && let Some(h) = &hint
                                {
                                    fail!("hint: {}", h.fix);
                                }
                                fails = print_scripts(&scripts)
                            }
                        }
                    }
                };
                tokio::join!(run, print);
                if code != 0 {
                    code
                } else if fails > 0 {
                    1
                } else {
                    0
                }
            }
            Request::Ws(mut w) => {
                if no_cookies {
                    w.options.cookies = Some(false);
                }
                let (utx, mut urx) = tokio::sync::mpsc::unbounded_channel();
                let (ctl_tx, ctl_rx) = tokio::sync::mpsc::unbounded_channel::<runner::Control>();
                drop(ctl_tx);
                // Secrets in the resolved URL (an API key in the query) must not
                // reach the log; `scope` itself moves into the session.
                let url_secrets = scope.secret_values();
                let run = runner::exec_ws(ctx, &w, scope, errors, utx, ctl_rx);
                let mut code = 1006i64;
                let mut fails = 0;
                let mut failed = false;
                let print = async {
                    while let Some(u) = urx.recv().await {
                        match u {
                            runner::WsUpdate::Started { resolved } => {
                                eprintln!(
                                    "connecting to {}",
                                    sendito_core::vars::scrub(&resolved.url, &url_secrets)
                                );
                            }
                            runner::WsUpdate::Event(sendito_ws::Event::Handshake {
                                status,
                                ref headers,
                                ..
                            }) => {
                                eprintln!("handshake {status}");
                                if include {
                                    for (k, v) in headers {
                                        eprintln!("{k}: {v}");
                                    }
                                    eprintln!();
                                }
                            }
                            runner::WsUpdate::Event(sendito_ws::Event::Received {
                                kind,
                                preview,
                                ..
                            }) => match kind {
                                sendito_ws::FrameKind::Text => {
                                    let _ = out.write_all(preview.as_bytes());
                                    if !preview.ends_with('\n') {
                                        let _ = out.write_all(b"\n");
                                    }
                                }
                                _ => eprintln!("{} frame: {preview}", kind.as_str()),
                            },
                            runner::WsUpdate::Event(sendito_ws::Event::Finished {
                                code: c,
                                reason,
                                ..
                            }) => {
                                code = c as i64;
                                eprintln!(
                                    "close {c}{}",
                                    if reason.is_empty() {
                                        String::new()
                                    } else {
                                        format!(": {reason}")
                                    }
                                );
                            }
                            runner::WsUpdate::Event(sendito_ws::Event::Sent {
                                kind, size, ..
                            }) => {
                                eprintln!("sent {} frame ({size} B)", kind.as_str());
                            }
                            runner::WsUpdate::Error(e) => {
                                failed = true;
                                fail!("session failed: {e}");
                            }
                            runner::WsUpdate::Done { scripts, hint, .. } => {
                                if failed && let Some(h) = &hint {
                                    fail!("hint: {}", h.fix);
                                }
                                fails = print_scripts(&scripts)
                            }
                        }
                    }
                };
                tokio::join!(run, print);
                if code != 1000 || failed {
                    return 2;
                }
                if fails > 0 {
                    return 1;
                }
                0
            }
            Request::Mcp(m) => {
                let (utx, mut urx) = tokio::sync::mpsc::unbounded_channel();
                let info = runner::McpRunInfo::of(project);
                let run = runner::exec_mcp(ctx, &m, scope, errors, &info, utx);
                let mut code = 0u8;
                let mut fails = 0;
                let print = async {
                    while let Some(u) = urx.recv().await {
                        match u {
                            runner::McpUpdate::Event(_) => {}
                            runner::McpUpdate::Done {
                                error,
                                is_error,
                                result,
                                scripts,
                                // Already printed once when the scope was built.
                                scope_errors: _,
                                needs_login,
                                hint,
                            } => {
                                if let Some(e) = error {
                                    fail!("{e}");
                                    if let Some(h) = &hint {
                                        fail!("hint: {}", h.fix);
                                    }
                                    if needs_login {
                                        fail!("sign in with `sendito mcp login {}`", m.server);
                                    }
                                    code = 2;
                                } else {
                                    if is_error {
                                        // The tool answered isError: the call went
                                        // through, the tool failed (decision: exit 1).
                                        code = 1;
                                        fail!("the tool reported an error");
                                    }
                                    if let Some(b) = result {
                                        let _ = out.write_all(&b);
                                        if !b.ends_with(b"\n") {
                                            let _ = out.write_all(b"\n");
                                        }
                                    }
                                    fails = print_scripts(&scripts);
                                }
                            }
                        }
                    }
                };
                tokio::join!(run, print);
                if code != 0 {
                    code
                } else if fails > 0 {
                    1
                } else {
                    0
                }
            }
        }
    }
}

/// Cancels a run on Ctrl+C while it is alive.
pub(super) struct CtrlC {
    pub(super) cancel: suite::Cancel,
    task: tokio::task::JoinHandle<()>,
}

impl CtrlC {
    pub(super) fn new() -> Self {
        let cancel = suite::Cancel::default();
        let c = cancel.clone();
        let task = tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                c.cancel();
            }
        });
        Self { cancel, task }
    }
}

impl Drop for CtrlC {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Runs every request under `folder` in tree order (see [`run_rels`]).
pub(crate) async fn run_suite(project: Project, folder: String, flags: &RunFlags) -> ExitCode {
    if flags.include {
        fail!("-i prints response headers of a single request; it does not apply to a folder run");
        return ExitCode::from(3);
    }
    let rels = match project.coll.requests_under(&folder) {
        Ok(v) => v,
        Err(e) => {
            fail!("{e}");
            return ExitCode::from(3);
        }
    };
    if rels.is_empty() {
        let what = if folder.is_empty() {
            "the collection".to_string()
        } else {
            format!("\"{folder}\"")
        };
        fail!("no requests under {what}");
        return ExitCode::from(3);
    }
    let label = if folder.is_empty() {
        ".".to_string()
    } else {
        folder
    };
    run_rels(project, label, rels, flags).await
}

/// Runs `rels` with the shared suite engine, chaining variables: a value a
/// request extracts is available to the ones after it. Prints one line per
/// request and the summary, then writes the reports.
async fn run_rels(
    project: Project,
    label: String,
    rels: Vec<String>,
    flags: &RunFlags,
) -> ExitCode {
    let RunFlags {
        env,
        allow_unresolved,
        no_history,
        no_cookies,
        fail_fast,
        report,
        report_files,
        retry,
        repeat,
        delay_ms,
        prompts,
        data,
        ..
    } = flags.clone();
    let data = match &data {
        Some(_) if repeat > 1 => {
            fail!("--data and --repeat cannot be used together");
            return ExitCode::from(3);
        }
        Some(path) => match suite::load_data(path) {
            Ok(rows) => rows,
            Err(e) => {
                fail!("{e}");
                return ExitCode::from(3);
            }
        },
        None => Vec::new(),
    };
    let (scope, scope_errors, failed) = scope_of(&project, &env).await;
    for e in &scope_errors {
        warn!("{e}");
    }
    let started = Instant::now();
    let mut suite = SuiteReport {
        environment: env.clone(),
        folder: label,
        ..Default::default()
    };
    eprintln!(
        "sendito: running {} request(s) under \"{}\"{}",
        rels.len(),
        suite.folder,
        if repeat > 1 {
            format!(", {repeat} times")
        } else if !data.is_empty() {
            format!(", once per data row ({})", data.len())
        } else {
            String::new()
        }
    );
    let opts = suite::Options {
        env,
        allow_unresolved,
        no_history,
        no_cookies,
        fail_fast,
        retry,
        repeat,
        delay_ms,
        mask_echoed_secrets: false,
        data,
        ..Default::default()
    };
    let plan = suite::Plan {
        project: &project,
        scope,
        scope_errors: &scope_errors,
        failed: &failed,
    };
    // A folder run asks each `$prompt` label once.
    let mut answers = super::ask::Answers::new();
    let ask = |req: &Request, scope: &Scope| {
        super::ask::answer_prompts(req, scope, &prompts, &mut answers, true)
    };
    // The attempt count of the request that is retried, for its final line.
    let mut attempts_of = 0;
    let on = |ev: suite::Event| match ev {
        suite::Event::Note(n) => eprintln!("note: {n}"),
        suite::Event::Log(l) => eprintln!("{l}"),
        suite::Event::Retry {
            path,
            label,
            attempt,
            attempts,
            interval_ms,
        } => {
            attempts_of = attempts;
            eprintln!(
                "RETRY {path}  ({label}, attempt {attempt}/{attempts}; next in {})",
                sendito_ui::units::format_ms(interval_ms)
            )
        }
        suite::Event::Finished(sent) => {
            print_entry(&sent.entry, attempts_of);
            suite.entries.push(sent.entry);
        }
    };
    let ctrl_c = CtrlC::new();
    suite::run(plan, &rels, &opts, ask, on, &ctrl_c.cancel).await;
    drop(ctrl_c);
    suite.duration_ms = started.elapsed().as_millis();
    let nunresolved = suite.unresolved();
    eprintln!(
        "{} passed, {} failed, {} error(s){}  ({}, {:.1} s)",
        suite.passed(),
        suite.failed(),
        suite.errors() - nunresolved,
        if nunresolved > 0 {
            format!(", {nunresolved} unresolved")
        } else {
            String::new()
        },
        suite.entries.len(),
        suite.duration_ms as f64 / 1000.0
    );
    if let Some(text) = report_text(&suite, report) {
        println!("{}", text.trim_end());
    }
    let mut write_failed = false;
    for f in &report_files {
        let text = report_text(&suite, f.format).unwrap_or_default();
        if let Err(e) = std::fs::write(&f.path, text) {
            fail!("cannot write {}: {e}", f.path.display());
            write_failed = true;
        }
    }
    if nunresolved > 0 || write_failed {
        ExitCode::from(3)
    } else if suite.errors() > 0 {
        ExitCode::from(2)
    } else if suite.failed() > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// The stderr line of one finished request, and its failed assertions.
/// `attempts` is the attempt count when the request was retried.
fn print_entry(e: &suite::Entry, attempts: u32) {
    if e.attempts == 0 {
        // Multi-line parse errors: the first line names the file and
        // position; the full message stays in the report.
        let error = e.error.as_deref().unwrap_or_default();
        let first = error.lines().next().unwrap_or_default();
        eprintln!("ERROR {}  {first}", e.path);
        return;
    }
    let mark = match e.outcome {
        "pass" => "PASS",
        "fail" => "FAIL",
        _ => "ERROR",
    };
    let tries = if e.attempts > 1 {
        format!(", attempt {}/{attempts}", e.attempts)
    } else {
        String::new()
    };
    eprintln!(
        "{mark:<5} {}  ({}, {} ms{tries})",
        e.path,
        e.status.as_deref().unwrap_or_default(),
        e.duration_ms.unwrap_or_default()
    );
    if e.outcome != "pass"
        && let Some(h) = &e.hint
    {
        eprintln!("      hint: {}", h.fix);
    }
    for a in e.asserts.iter().filter(|a| !a.pass) {
        eprintln!("      ✗ {}  (actual: {})", a.label, a.actual);
    }
}

/// The report in `format`; `None` for `text`, which is only the stderr lines.
fn report_text(suite: &SuiteReport, format: RunReport) -> Option<String> {
    match format {
        RunReport::Text => None,
        RunReport::Json => Some(suite.json()),
        RunReport::Junit => Some(suite.junit()),
        RunReport::Tap => Some(suite.tap()),
        RunReport::Html => Some(suite.html()),
    }
}

fn report_name(format: RunReport) -> &'static str {
    match format {
        RunReport::Text => "text",
        RunReport::Json => "json",
        RunReport::Junit => "junit",
        RunReport::Tap => "tap",
        RunReport::Html => "html",
    }
}

pub(crate) fn print_scripts(s: &runner::ScriptResults) -> usize {
    for l in &s.logs {
        eprintln!("log: {l}");
    }
    for e in &s.extracts {
        match &e.value {
            Ok(v) => eprintln!(
                "set {} = {}",
                e.var,
                if v.secret {
                    sendito_core::vars::MASK.to_string()
                } else {
                    v.value.clone()
                }
            ),
            Err(err) => eprintln!("extract {}: {err}", e.var),
        }
    }
    for (var, v) in &s.post {
        eprintln!(
            "set {var} = {}",
            if v.secret {
                sendito_core::vars::MASK.to_string()
            } else {
                v.value.clone()
            }
        );
    }
    for a in &s.asserts {
        if a.pass {
            eprintln!("✓ {}", a.label);
        } else {
            eprintln!("✗ {}  (actual: {})", a.label, a.actual);
        }
    }
    s.failed_asserts()
}
