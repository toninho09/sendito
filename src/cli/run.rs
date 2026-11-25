//! `sendito run`: sends one request or a suite and prints the report.

use super::mqtt::run_mqtt;
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
    } = flags.clone();
    if fail_fast {
        eprintln!("warning: --fail-fast applies to folder runs; ignoring it for a single request");
    }
    if report == RunReport::Json && !matches!(req, Request::Sse(_) | Request::Mqtt(_)) {
        eprintln!(
            "warning: --report json applies to folder runs; ignoring it for a single request"
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
    // Check against the loaded values, as a folder run does: a secret that
    // failed to load (no keychain entry, no secrets.toml) is unresolved.
    let un = runner::unresolved_send(&req, &scope, &failed, &project.config.mcp.servers);
    if !un.is_empty() && !allow_unresolved {
        eprintln!(
            "sendito: unresolved variables: {} (define them in the environment or pass --allow-unresolved)",
            un.iter()
                .map(|u| format!("{{{{{u}}}}}"))
                .collect::<Vec<_>>()
                .join(", ")
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
    let mut out = std::io::stdout().lock();
    match req {
        Request::Sse(mut s) => {
            if no_cookies {
                s.options.cookies = Some(false);
            }
            let ran = run_sse(
                &ctx,
                &s,
                scope,
                errors,
                report != RunReport::Json,
                include,
                &mut out,
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
            ExitCode::from(match ran.outcome {
                "pass" => 0,
                "fail" => 1,
                _ => 2,
            })
        }
        Request::Mqtt(m) => {
            let ran = run_mqtt(
                &ctx,
                &m,
                scope,
                errors,
                report != RunReport::Json,
                include,
                &mut out,
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
            ExitCode::from(match ran.outcome {
                "pass" => 0,
                "fail" => 1,
                _ => 2,
            })
        }
        Request::Http(mut h) => {
            if no_cookies {
                h.options.cookies = Some(false);
            }
            let o = runner::exec_http(&ctx, &h, scope, errors).await;
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
                ExitCode::from(code)
            } else if fails > 0 {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
        Request::Grpc(g) => {
            let (utx, mut urx) = tokio::sync::mpsc::unbounded_channel();
            let (ctl_tx, ctl_rx) = tokio::sync::mpsc::unbounded_channel();
            drop(ctl_tx);
            // A secret in the target must not reach the log, as for WebSocket.
            let uri_secrets = scope.secret_values();
            let run = runner::exec_grpc(&ctx, &g, scope, errors, utx, ctl_rx);
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
                ExitCode::from(code)
            } else if fails > 0 {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
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
            let run = runner::exec_ws(&ctx, &w, scope, errors, utx, ctl_rx);
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
                        runner::WsUpdate::Event(sendito_ws::Event::Sent { kind, size, .. }) => {
                            eprintln!("sent {} frame ({size} B)", kind.as_str());
                        }
                        runner::WsUpdate::Error(e) => {
                            eprintln!("sendito: session failed: {e}");
                        }
                        runner::WsUpdate::Done { scripts, .. } => fails = print_scripts(&scripts),
                    }
                }
            };
            tokio::join!(run, print);
            if code != 1000 {
                return ExitCode::from(2);
            }
            if fails > 0 {
                return ExitCode::from(1);
            }
            ExitCode::SUCCESS
        }
        Request::Mcp(m) => {
            let (utx, mut urx) = tokio::sync::mpsc::unbounded_channel();
            let info = runner::McpRunInfo::of(&project);
            let run = runner::exec_mcp(&ctx, &m, scope, errors, &info, utx);
            let mut code = 0u8;
            let mut fails = 0;
            let print = async {
                let mut out = std::io::stdout().lock();
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
                ExitCode::from(code)
            } else if fails > 0 {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
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
    let mut entries: Vec<serde_json::Value> = Vec::new();
    let (mut npass, mut nfail, mut nerr, mut nunresolved) = (0u32, 0u32, 0u32, 0u32);
    let folder_label = if folder.is_empty() {
        ".".to_string()
    } else {
        folder.clone()
    };
    eprintln!(
        "sendito: running {} request(s) under \"{folder_label}\"",
        rels.len()
    );
    for rel in &rels {
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
                nerr += 1;
                // Multi-line parse errors: the first line names the file and
                // position; the full message stays in the JSON report.
                let first = e.to_string().lines().next().unwrap_or_default().to_string();
                eprintln!("ERROR {rel}  {first}");
                entries.push(serde_json::json!({
                    "path": rel,
                    "kind": serde_json::Value::Null,
                    "outcome": "error",
                    "status": serde_json::Value::Null,
                    "duration_ms": serde_json::Value::Null,
                    "asserts": 0,
                    "failed_asserts": 0,
                    "error": e.to_string(),
                }));
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
        let un = runner::unresolved_send(&req, &rscope, &failed, &project.config.mcp.servers);
        if !un.is_empty() && !allow_unresolved {
            nunresolved += 1;
            let names = un
                .iter()
                .map(|u| format!("{{{{{u}}}}}"))
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!("ERROR {rel}  unresolved variables: {names}");
            entries.push(serde_json::json!({
                "path": rel,
                "kind": kind,
                "outcome": "error",
                "status": serde_json::Value::Null,
                "duration_ms": serde_json::Value::Null,
                "asserts": 0,
                "failed_asserts": 0,
                "unresolved": true,
                "error": format!("unresolved variables: {names}"),
            }));
            if fail_fast {
                break;
            }
            continue;
        }
        let ctx = ctx_for(&project, &env, rel, no_history, no_cookies, schemas.clone());
        let t0 = Instant::now();
        let ran = run_one(&ctx, &req, &rscope, &scope_errors, &mcp_info, no_cookies).await;
        let ms = t0.elapsed().as_millis();
        let mark = match ran.outcome {
            "pass" => "PASS",
            "fail" => "FAIL",
            _ => "ERROR",
        };
        eprintln!("{mark:<5} {rel}  ({}, {ms} ms)", ran.label);
        for a in ran.scripts.asserts.iter().filter(|a| !a.pass) {
            eprintln!("      ✗ {}  (actual: {})", a.label, a.actual);
        }
        let fails = ran.scripts.failed_asserts();
        entries.push(serde_json::json!({
            "path": rel,
            "kind": kind,
            "outcome": ran.outcome,
            "status": ran.label,
            "duration_ms": ms,
            "asserts": ran.scripts.asserts.len(),
            "failed_asserts": fails,
            "unresolved": false,
            "error": serde_json::Value::Null,
        }));
        if let Some(events) = ran.events
            && let Some(entry) = entries.last_mut()
        {
            entry["events"] = events.into();
        }
        if let Some(messages) = ran.messages
            && let Some(entry) = entries.last_mut()
        {
            entry["messages"] = messages.into();
        }
        for (var, value) in ran.scripts.updates() {
            scope.set(RUNTIME_LAYER, &var, value);
        }
        match ran.outcome {
            "pass" => npass += 1,
            "fail" => nfail += 1,
            _ => nerr += 1,
        }
        if fail_fast && ran.outcome != "pass" {
            break;
        }
    }
    let total_ms = started.elapsed().as_millis();
    eprintln!(
        "{npass} passed, {nfail} failed, {nerr} error(s){}  ({}, {:.1} s)",
        if nunresolved > 0 {
            format!(", {nunresolved} unresolved")
        } else {
            String::new()
        },
        entries.len(),
        total_ms as f64 / 1000.0
    );
    if report == RunReport::Json {
        // `errors` counts every `"outcome": "error"` entry, unresolved
        // variables included (they are also broken out in `unresolved`), so
        // `passed + failed + errors == total`.
        let out = serde_json::json!({
            "environment": env,
            "folder": folder_label,
            "total": entries.len(),
            "passed": npass,
            "failed": nfail,
            "errors": nerr + nunresolved,
            "unresolved": nunresolved,
            "duration_ms": total_ms,
            "requests": entries,
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    }
    if nunresolved > 0 {
        ExitCode::from(3)
    } else if nerr > 0 {
        ExitCode::from(2)
    } else if nfail > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
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
    for a in &s.asserts {
        if a.pass {
            eprintln!("✓ {}", a.label);
        } else {
            eprintln!("✗ {}  (actual: {})", a.label, a.actual);
        }
    }
    s.failed_asserts()
}
