//! `sendito run`: sends one request or a suite and prints the report.

use super::mqtt::run_mqtt;
use super::report::{Entry, SuiteReport};
use super::sse::run_sse;
use crate::*;

/// Routes `run` to one request or a folder: a request file wins over a folder
/// of the same name, and errors from whichever path matched are reported as
/// they are.
pub(crate) async fn run_entry(request: &Path, flags: RunFlags) -> ExitCode {
    // Locate once: the result decides between a single request and a folder.
    if let Ok((root, rel)) = locate(request) {
        return match load_at(root, rel, &flags.env) {
            Ok((project, rel, req)) => run(project, rel, req, &flags).await,
            Err((e, code)) => {
                eprintln!("sendito: {e}");
                ExitCode::from(code)
            }
        };
    }
    if is_folder_arg(request) {
        return match load_suite(request, &flags.env) {
            Ok((project, folder)) => run_suite(project, folder, &flags).await,
            Err((e, code)) => {
                eprintln!("sendito: {e}");
                ExitCode::from(code)
            }
        };
    }
    eprintln!("sendito: request {} not found", request.display());
    ExitCode::from(3)
}

pub(crate) async fn run(project: Project, rel: String, req: Request, flags: &RunFlags) -> ExitCode {
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
    } = flags.clone();
    if repeat > 1 || delay_ms > 0 {
        eprintln!(
            "warning: --repeat and --delay apply to folder runs; ignoring them for a single request"
        );
    }
    if fail_fast {
        eprintln!("warning: --fail-fast applies to folder runs; ignoring it for a single request");
    }
    let session = matches!(req, Request::Sse(_) | Request::Mqtt(_));
    if report != RunReport::Text && !(report == RunReport::Json && session) {
        eprintln!(
            "warning: --report {} applies to folder runs; ignoring it for a single request",
            report_name(report)
        );
    }
    if !report_files.is_empty() {
        eprintln!(
            "warning: --report-file applies to folder runs; ignoring it for a single request"
        );
    }
    let (mut scope, errors, failed) = scope_of(&project, &env).await;
    for e in &errors {
        eprintln!("warning: {e}");
    }
    let req = match project.coll.chain(&rel) {
        Ok(chain) => {
            if let Some(note) = sendito_core::inherit::ignored_headers_note(&req, &chain) {
                eprintln!("note: {note}");
            }
            runner::with_folders(&req, &chain, &mut scope)
        }
        Err(e) => {
            eprintln!("sendito: {e}");
            return ExitCode::from(3);
        }
    };
    if let Err(e) = super::ask::answer_prompts(&req, &scope, &prompts, &mut Default::default())
        && !allow_unresolved
    {
        eprintln!("sendito: {e} (or pass --allow-unresolved)");
        return ExitCode::from(3);
    }
    // Check against the loaded values, as a folder run does: a secret that
    // failed to load (no keychain entry, no secrets.toml) is unresolved.
    let un = runner::unresolved_send(&req, &scope, &failed, &project.config.mcp.servers);
    if !un.is_empty() && !allow_unresolved {
        eprintln!(
            "sendito: unresolved variables: {} (define them in the environment or pass --allow-unresolved)",
            runner::unresolved_list(&un, &scope)
        );
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
    let attempts = retry.unwrap_or(req.common().retry).saturating_add(1);
    let interval = req.common().retry_interval();
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
                        0
                    }
                    Err(e) => {
                        eprintln!("sendito: request failed: {e}");
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
                                eprintln!("sendito: {e}");
                                code = 2;
                            }
                            GrpcUpdate::Done { scripts, .. } => fails = print_scripts(&scripts),
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
                                eprintln!("sendito: session failed: {e}");
                            }
                            runner::WsUpdate::Done { scripts, .. } => {
                                fails = print_scripts(&scripts)
                            }
                        }
                    }
                };
                tokio::join!(run, print);
                if code != 1000 {
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
                            } => {
                                if let Some(e) = error {
                                    eprintln!("sendito: {e}");
                                    if needs_login {
                                        eprintln!(
                                            "sendito: sign in with `sendito mcp login {}`",
                                            m.server
                                        );
                                    }
                                    code = 2;
                                } else {
                                    if is_error {
                                        // The tool answered isError: the call went
                                        // through, the tool failed (decision: exit 1).
                                        code = 1;
                                        eprintln!("sendito: the tool reported an error");
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

/// One request's result in a folder run.
pub(crate) struct Ran {
    /// `pass`, `fail` (a failed assertion or a tool `isError`) or `error`.
    pub(super) outcome: &'static str,
    /// Status text: `201 Created`, `OK`, or the error message.
    pub(super) label: String,
    pub(super) scripts: runner::ScriptResults,
    pub(super) events: Option<u64>,
    /// MQTT messages received; only MQTT sets it.
    pub(super) messages: Option<u64>,
}

/// Runs every request under `folder` in tree order, chaining variables: a
/// value a request extracts is available to the ones after it.
pub(crate) async fn run_suite(project: Project, folder: String, flags: &RunFlags) -> ExitCode {
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
    } = flags.clone();
    if include {
        eprintln!(
            "sendito: -i prints response headers of a single request; it does not apply to a folder run"
        );
        return ExitCode::from(3);
    }
    let rels = match project.coll.requests_under(&folder) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("sendito: {e}");
            return ExitCode::from(3);
        }
    };
    if rels.is_empty() {
        let what = if folder.is_empty() {
            "the collection".to_string()
        } else {
            format!("\"{folder}\"")
        };
        eprintln!("sendito: no requests under {what}");
        return ExitCode::from(3);
    }
    let (mut scope, scope_errors, failed) = scope_of(&project, &env).await;
    for e in &scope_errors {
        eprintln!("warning: {e}");
    }
    let mcp_info = runner::McpRunInfo::of(&project);
    // One schema cache for the whole run: reflection and `.proto` files are
    // loaded once, not once per gRPC request.
    let schemas: runner::SchemaCache = Default::default();
    let started = Instant::now();
    let mut suite = SuiteReport {
        environment: env.clone(),
        folder: if folder.is_empty() {
            ".".to_string()
        } else {
            folder.clone()
        },
        ..Default::default()
    };
    eprintln!(
        "sendito: running {} request(s) under \"{}\"{}",
        rels.len(),
        suite.folder,
        if repeat > 1 {
            format!(", {repeat} times")
        } else {
            String::new()
        }
    );
    // A folder run asks each `$prompt` label once.
    let mut answers = super::ask::Answers::new();
    // Repeats share one scope: a value extracted in one round is seen by the next.
    let runs = (1..=repeat).flat_map(|round| rels.iter().map(move |rel| (round, rel)));
    for (i, (round, rel)) in runs.enumerate() {
        if i > 0 && delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        }
        // The report names each round: `Users/list.toml#2`.
        let path = if repeat > 1 {
            format!("{rel}#{round}")
        } else {
            rel.clone()
        };
        let path = &path;
        // Each request gets its folders' variables on top of the shared
        // scope; the runtime values it extracts go to the shared scope.
        let mut rscope = scope.clone();
        let loaded = project
            .coll
            .load(rel)
            .and_then(|r| project.coll.chain(rel).map(|chain| (r, chain)));
        let req = match loaded {
            Ok((r, chain)) => {
                if let Some(note) = sendito_core::inherit::ignored_headers_note(&r, &chain) {
                    eprintln!("note: {rel}: {note}");
                }
                runner::with_folders(&r, &chain, &mut rscope)
            }
            Err(e) => {
                // Multi-line parse errors: the first line names the file and
                // position; the full message stays in the report.
                let first = e.to_string().lines().next().unwrap_or_default().to_string();
                eprintln!("ERROR {path}  {first}");
                suite.entries.push(Entry {
                    path: path.clone(),
                    outcome: "error",
                    error: Some(e.to_string()),
                    ..Default::default()
                });
                if fail_fast {
                    break;
                }
                continue;
            }
        };
        let kind = match &req {
            Request::Http(_) => "http",
            Request::Grpc(_) => "grpc",
            Request::Ws(_) => "websocket",
            Request::Sse(_) => "sse",
            Request::Mqtt(_) => "mqtt",
            Request::Mcp(_) => "mcp",
        };
        if let Err(e) = super::ask::answer_prompts(&req, &rscope, &prompts, &mut answers)
            && !allow_unresolved
        {
            eprintln!("ERROR {path}  {e}");
            suite.entries.push(Entry {
                path: path.clone(),
                kind: Some(kind),
                outcome: "error",
                unresolved: Some(true),
                error: Some(e),
                ..Default::default()
            });
            if fail_fast {
                break;
            }
            continue;
        }
        let un = runner::unresolved_send(&req, &rscope, &failed, &project.config.mcp.servers);
        if !un.is_empty() && !allow_unresolved {
            let names = runner::unresolved_list(&un, &rscope);
            eprintln!("ERROR {path}  unresolved variables: {names}");
            suite.entries.push(Entry {
                path: path.clone(),
                kind: Some(kind),
                outcome: "error",
                unresolved: Some(true),
                error: Some(format!("unresolved variables: {names}")),
                ..Default::default()
            });
            if fail_fast {
                break;
            }
            continue;
        }
        let ctx = ctx_for(&project, &env, rel, no_history, no_cookies, schemas.clone());
        let attempts = retry.unwrap_or(req.common().retry).saturating_add(1);
        let interval = req.common().retry_interval();
        let mut attempt = 1;
        let (ran, ms) = loop {
            let t0 = Instant::now();
            let ran = run_one(&ctx, &req, &rscope, &scope_errors, &mcp_info, no_cookies).await;
            let ms = t0.elapsed().as_millis();
            if ran.outcome == "pass" || attempt == attempts {
                break (ran, ms);
            }
            eprintln!(
                "RETRY {path}  ({}, attempt {attempt}/{attempts}; next in {})",
                ran.label,
                sendito_ui::units::format_ms(interval)
            );
            tokio::time::sleep(std::time::Duration::from_millis(interval)).await;
            attempt += 1;
        };
        let mark = match ran.outcome {
            "pass" => "PASS",
            "fail" => "FAIL",
            _ => "ERROR",
        };
        let tries = if attempt > 1 {
            format!(", attempt {attempt}/{attempts}")
        } else {
            String::new()
        };
        eprintln!("{mark:<5} {path}  ({}, {ms} ms{tries})", ran.label);
        for a in ran.scripts.asserts.iter().filter(|a| !a.pass) {
            eprintln!("      ✗ {}  (actual: {})", a.label, a.actual);
        }
        for (var, value) in ran.scripts.updates() {
            scope.set(RUNTIME_LAYER, &var, value);
        }
        let outcome = ran.outcome;
        suite.entries.push(Entry {
            path: path.clone(),
            kind: Some(kind),
            outcome,
            attempts: attempt,
            status: Some(ran.label),
            duration_ms: Some(ms),
            asserts: ran.scripts.asserts,
            unresolved: Some(false),
            error: None,
            events: ran.events,
            messages: ran.messages,
        });
        if fail_fast && outcome != "pass" {
            break;
        }
    }
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
            eprintln!("sendito: cannot write {}: {e}", f.path.display());
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

/// Executes one request of a suite run and classifies the outcome.
pub(crate) async fn run_one(
    ctx: &RunCtx,
    req: &Request,
    scope: &Scope,
    scope_errors: &[String],
    mcp_info: &runner::McpRunInfo,
    no_cookies: bool,
) -> Ran {
    match req {
        Request::Sse(s) => {
            let mut s = s.clone();
            if no_cookies {
                s.options.cookies = Some(false);
            }
            run_sse(
                ctx,
                &s,
                scope.clone(),
                scope_errors.to_vec(),
                false,
                false,
                &mut std::io::sink(),
            )
            .await
        }
        Request::Mqtt(m) => {
            run_mqtt(
                ctx,
                m,
                scope.clone(),
                scope_errors.to_vec(),
                false,
                false,
                &mut std::io::sink(),
            )
            .await
        }
        Request::Http(h) => {
            let mut h = h.clone();
            if no_cookies {
                h.options.cookies = Some(false);
            }
            let o = runner::exec_http(ctx, &h, scope.clone(), scope_errors.to_vec()).await;
            let failed_asserts = o.scripts.failed_asserts();
            match &o.response {
                Ok(r) => Ran {
                    events: None,
                    messages: None,
                    outcome: if failed_asserts > 0 { "fail" } else { "pass" },
                    label: format!("{} {}", r.status, r.reason),
                    scripts: o.scripts,
                },
                Err(e) => Ran {
                    events: None,
                    messages: None,
                    outcome: "error",
                    label: e.clone(),
                    scripts: o.scripts,
                },
            }
        }
        Request::Grpc(g) => {
            let (utx, mut urx) = tokio::sync::mpsc::unbounded_channel();
            let (ctl_tx, ctl_rx) = tokio::sync::mpsc::unbounded_channel();
            drop(ctl_tx);
            let run = runner::exec_grpc(ctx, g, scope.clone(), scope_errors.to_vec(), utx, ctl_rx);
            let mut code = 0i32;
            let mut label = String::from("OK");
            let mut scripts = runner::ScriptResults::default();
            // Drive the call and collect its updates together: the channel is
            // only fed while `run` is polled.
            let collect = async {
                while let Some(u) = urx.recv().await {
                    match u {
                        GrpcUpdate::Event(sendito_grpc::Event::Finished {
                            code: c,
                            message,
                            ..
                        }) => {
                            code = c;
                            label = if c == 0 {
                                "OK".into()
                            } else if message.is_empty() {
                                format!("{} ({c})", sendito_grpc::code_name(c))
                            } else {
                                format!("{}: {message}", sendito_grpc::code_name(c))
                            };
                        }
                        GrpcUpdate::Error(e) => {
                            code = -1;
                            label = e;
                        }
                        GrpcUpdate::Done { scripts: s, .. } => scripts = s,
                        _ => {}
                    }
                }
            };
            tokio::join!(run, collect);
            if code != 0 {
                Ran {
                    events: None,
                    messages: None,
                    outcome: "error",
                    label,
                    scripts,
                }
            } else {
                let fails = scripts.failed_asserts();
                Ran {
                    events: None,
                    messages: None,
                    outcome: if fails > 0 { "fail" } else { "pass" },
                    label,
                    scripts,
                }
            }
        }
        Request::Ws(w) => {
            let mut w = w.clone();
            if no_cookies {
                w.options.cookies = Some(false);
            }
            let (utx, mut urx) = tokio::sync::mpsc::unbounded_channel();
            let (ctl_tx, ctl_rx) = tokio::sync::mpsc::unbounded_channel::<runner::Control>();
            drop(ctl_tx);
            let run = runner::exec_ws(ctx, &w, scope.clone(), scope_errors.to_vec(), utx, ctl_rx);
            let mut code = 1006i64;
            let mut label = String::from("close 1006");
            let mut error: Option<String> = None;
            let mut scripts = runner::ScriptResults::default();
            let collect = async {
                while let Some(u) = urx.recv().await {
                    match u {
                        runner::WsUpdate::Event(sendito_ws::Event::Finished {
                            code: c,
                            reason,
                            ..
                        }) => {
                            code = c as i64;
                            label = if reason.is_empty() {
                                format!("close {c}")
                            } else {
                                format!("close {c}: {reason}")
                            };
                        }
                        runner::WsUpdate::Error(e) => {
                            error = Some(e.clone());
                            label = e;
                        }
                        runner::WsUpdate::Done { scripts: s, .. } => scripts = s,
                        _ => {}
                    }
                }
            };
            tokio::join!(run, collect);
            if let Some(e) = error {
                Ran {
                    events: None,
                    messages: None,
                    outcome: "error",
                    label: e,
                    scripts,
                }
            } else if code != 1000 {
                Ran {
                    events: None,
                    messages: None,
                    outcome: "error",
                    label,
                    scripts,
                }
            } else {
                let failed_asserts = scripts.failed_asserts();
                Ran {
                    events: None,
                    messages: None,
                    outcome: if failed_asserts > 0 { "fail" } else { "pass" },
                    label,
                    scripts,
                }
            }
        }
        Request::Mcp(m) => {
            let (utx, mut urx) = tokio::sync::mpsc::unbounded_channel();
            let run = runner::exec_mcp(ctx, m, scope.clone(), scope_errors.to_vec(), mcp_info, utx);
            let mut code = 0u8;
            let mut label = String::from("ok");
            let mut scripts = runner::ScriptResults::default();
            // Same as gRPC: the future must be polled while the updates are
            // read, or the channel never produces anything.
            let collect = async {
                while let Some(u) = urx.recv().await {
                    if let runner::McpUpdate::Done {
                        error,
                        is_error,
                        scripts: s,
                        ..
                    } = u
                    {
                        scripts = s;
                        if let Some(e) = error {
                            code = 2;
                            label = e;
                        } else if is_error {
                            code = 1;
                            label = "the tool reported an error".into();
                        }
                    }
                }
            };
            tokio::join!(run, collect);
            if code == 2 {
                Ran {
                    events: None,
                    messages: None,
                    outcome: "error",
                    label,
                    scripts,
                }
            } else {
                let fails = scripts.failed_asserts();
                Ran {
                    events: None,
                    messages: None,
                    outcome: if code == 1 || fails > 0 {
                        "fail"
                    } else {
                        "pass"
                    },
                    label,
                    scripts,
                }
            }
        }
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
