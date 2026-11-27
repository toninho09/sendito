//! Sends a gRPC call and loads its schema.

use super::*;

/// Cache of loaded schemas keyed by target and schema source.
pub type SchemaCache = Arc<Mutex<HashMap<String, Schema>>>;

pub fn schema_key(uri: &str, src: &SchemaSource) -> String {
    match src {
        SchemaSource::Reflection => format!("reflect:{uri}"),
        SchemaSource::Files {
            files,
            import_paths,
        } => format!("files:{}|{}", files.join(","), import_paths.join(",")),
    }
}

/// Loads (or fetches from the cache) the schema for a request.
pub async fn load_schema(
    root: &Path,
    resolved: &ResolvedGrpc,
    src: &SchemaSource,
    cache: &SchemaCache,
    refresh: bool,
    tls: &sendito_http::TlsMaterial,
) -> Result<Schema, String> {
    let key = schema_key(&resolved.uri, src);
    if !refresh && let Some(s) = cache.lock().ok().and_then(|c| c.get(&key).cloned()) {
        return Ok(s);
    }
    let schema = match src {
        SchemaSource::Files {
            files,
            import_paths,
        } => {
            let (files, imports, root) = (files.clone(), import_paths.clone(), root.to_path_buf());
            tokio::task::spawn_blocking(move || Schema::from_files(&files, &imports, &root))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?
        }
        SchemaSource::Reflection => {
            // Reflection goes out with the request's metadata and, when the
            // request has a JWT and no explicit header, a token signed now.
            let mut metadata = resolved.metadata.clone();
            if !metadata
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("authorization"))
                // The runner checked the key file (`confine_files`) before.
                && let Some(token) = jwt_token(root, false, resolved.signed.as_ref())
                    .await
                    .map_err(|e| e.to_string())?
            {
                metadata.push(("authorization".into(), format!("Bearer {token}")));
            }
            let ch = sendito_grpc::connect(&conn_settings(resolved, tls))
                .await
                .map_err(|e| e.to_string())?;
            sendito_grpc::schema::from_reflection_with(ch, &metadata)
                .await
                .map_err(|e| e.to_string())?
        }
    };
    if let Ok(mut c) = cache.lock() {
        c.insert(key, schema.clone());
    }
    Ok(schema)
}

pub fn conn_settings(
    r: &ResolvedGrpc,
    tls: &sendito_http::TlsMaterial,
) -> sendito_grpc::ConnSettings {
    sendito_grpc::ConnSettings {
        uri: r.uri.clone(),
        verify_tls: r.verify_tls,
        connect_timeout: Duration::from_secs(10),
        tls: tls.clone(),
    }
}

/// Commands from the UI to a running gRPC call.
#[derive(Debug, Clone)]
pub enum GrpcControl {
    /// JSON for one message, or an array of messages.
    Send(String),
    EndStream,
    Cancel,
}

/// Progress of a gRPC call, forwarded to the UI as it happens.
#[derive(Debug, Clone)]
pub enum GrpcUpdate {
    Started {
        resolved: ResolvedGrpc,
        kind: sendito_grpc::CallKind,
    },
    Event(Event),
    Error(String),
    Done {
        scripts: ScriptResults,
        scope_errors: Vec<String>,
        /// The configuration problem behind the error or the status.
        hint: Option<diagnose::Hint>,
    },
}

/// What scripts and history need from a gRPC call, kept as events arrive so
/// a long stream is not held in memory: the last message, the metadata, the
/// status, the total time and the received bytes.
pub(super) struct GrpcSummary {
    pub(super) body: Vec<u8>,
    pub(super) headers: Vec<(String, String)>,
    pub(super) code: i32,
    pub(super) total_ms: u64,
    pub(super) size: u64,
}

impl GrpcSummary {
    pub(super) fn new() -> Self {
        Self {
            body: Vec::new(),
            headers: Vec::new(),
            code: -1,
            total_ms: 0,
            size: 0,
        }
    }

    pub(super) fn add(&mut self, e: &Event) {
        match e {
            Event::Received { full, size, .. } => {
                self.body = full.as_bytes().to_vec();
                self.size += *size as u64;
            }
            Event::Headers { metadata, .. } => self.headers = metadata.clone(),
            Event::Finished {
                code, metadata, at, ..
            } => {
                self.code = *code;
                self.headers.extend(metadata.iter().cloned());
                self.total_ms = at.as_millis() as u64;
            }
            _ => {}
        }
    }
}

/// Runs a gRPC call. Updates go to `updates`; `control` drives streaming input.
pub async fn exec_grpc(
    ctx: &RunCtx,
    req: &GrpcRequest,
    mut scope: Scope,
    scope_errors: Vec<String>,
    updates: mpsc::UnboundedSender<GrpcUpdate>,
    mut control: mpsc::UnboundedReceiver<Control>,
) {
    let PrePhase {
        req,
        mut results,
        error: pre_error,
    } = prepare(ctx, req, &mut scope).await;
    let req = &req;
    let mut resolved = resolve_grpc(req, &scope);
    if resolved.deadline_ms.is_none() && ctx.grpc_deadline_ms > 0 {
        resolved.deadline_ms = Some(ctx.grpc_deadline_ms);
    }
    let cx = diagnose::Context {
        grpc_deadline_ms: resolved.deadline_ms,
        verify_tls: Some(resolved.verify_tls),
        ..diagnose::Context::of(ctx)
    };
    let template = Request::Grpc(req.clone());
    let target = match mask_request(&template, &scope) {
        Request::Grpc(g) => format!(
            "{}/{}/{}",
            sendito_core::resolve::resolve_grpc(&g, &Scope::new()).uri,
            resolved.service,
            resolved.method
        ),
        _ => String::new(),
    };
    let fail_secrets = scope.secret_values();
    let fail = |msg: String, results: ScriptResults| {
        // A connect error names the resolved target, which can be secret.
        let msg = sendito_core::vars::scrub(&msg, &fail_secrets);
        let entry = history_entry(
            ctx,
            &template,
            target.clone(),
            None,
            0,
            0,
            Some(msg.clone()),
        );
        append_history(ctx, &entry);
        let _ = updates.send(GrpcUpdate::Error(msg.clone()));
        let _ = updates.send(GrpcUpdate::Done {
            scripts: results,
            scope_errors: scope_errors.clone(),
            hint: diagnose::grpc_message(&msg, &cx),
        });
    };
    if let Some(e) = pre_error {
        return fail(e, results);
    }
    if req.auth.kind().http_only() {
        return fail(
            format!(
                "{} authentication is only supported for HTTP requests",
                req.auth.kind().label()
            ),
            results,
        );
    }
    // A JWT is signed now; the token is masked like the other secrets.
    match jwt_token(&ctx.root, ctx.confine_files, resolved.signed.as_ref()).await {
        Ok(Some(token)) => {
            scope.set(SIGNED_LAYER, "jwt", VarValue::secret(token.clone()));
            resolved
                .metadata
                .push(("authorization".into(), format!("Bearer {token}")));
        }
        Ok(None) => {}
        Err(e) => return fail(e, results),
    }
    if resolved.service.is_empty() || resolved.method.is_empty() {
        return fail("choose a service and method first".into(), results);
    }
    if let SchemaSource::Files {
        files,
        import_paths,
    } = &req.schema
    {
        let paths: Vec<PathBuf> = files
            .iter()
            .chain(import_paths)
            .map(|p| sendito_store::env::expand(&ctx.root, p))
            .collect();
        if let Err(e) = ctx.check_files(paths.iter().map(PathBuf::as_path)) {
            return fail(e, results);
        }
    }
    // Loading the schema and connecting are bounded by the [http] timeout,
    // so a server that never answers reflection cannot hang the run; Cancel
    // stops them too. The deadline covers the call only (`grpc-timeout`).
    let deadline = resolved.deadline_ms.map(Duration::from_millis);
    let prepare = async {
        let schema = load_schema(
            &ctx.root,
            &resolved,
            &req.schema,
            &ctx.schemas,
            false,
            &ctx.send.tls,
        )
        .await?;
        let method = schema
            .method(&resolved.service, &resolved.method)
            .map_err(|e| e.to_string())?;
        let kind = sendito_grpc::CallKind::of(&method);
        let msgs = sendito_grpc::schema::parse_messages(
            &method.input(),
            &resolved.message,
            kind.client_streams(),
        )?;
        let metadata = sendito_grpc::metadata_map(&resolved.metadata).map_err(|e| e.to_string())?;
        let channel = sendito_grpc::connect(&conn_settings(&resolved, &ctx.send.tls))
            .await
            .map_err(|e| e.to_string())?;
        Ok::<_, String>((method, kind, msgs, metadata, channel))
    };
    let prep_limit = ctx.send.default_timeout;
    let cancelled = async {
        loop {
            match control.recv().await {
                Some(Control::Grpc(GrpcControl::Cancel)) => return,
                Some(_) => {}
                None => std::future::pending::<()>().await,
            }
        }
    };
    let prepared = tokio::select! {
        r = prepare => r,
        () = tokio::time::sleep(prep_limit) => Err(format!(
            "the schema load or the connection took more than {} ms ([http] timeout_ms)",
            prep_limit.as_millis()
        )),
        () = cancelled => Err("cancelled".into()),
    };
    let (method, kind, msgs, metadata, channel) = match prepared {
        Ok(p) => p,
        Err(e) => return fail(e, results),
    };
    // The UI shows this target ("at …"): a secret in it is masked.
    let mut shown = resolved.clone();
    shown.uri = sendito_core::vars::scrub(&shown.uri, &scope.secret_values());
    let _ = updates.send(GrpcUpdate::Started {
        resolved: shown,
        kind,
    });
    let secrets = scope.secret_values();
    let mut call = sendito_grpc::start_call(channel, &method, metadata, deadline, msgs);
    let mut summary = GrpcSummary::new();
    let mut hint = None;
    let mut control_open = true;
    loop {
        tokio::select! {
            ev = call.events.recv() => {
                let Some(ev) = ev else { break };
                let done = if let Event::Finished { code, message, .. } = &ev {
                    hint = diagnose::grpc(*code, message, &cx);
                    true
                } else {
                    false
                };
                summary.add(&ev);
                let ev = if ctx.mask_echoed_secrets { scrub_event(ev, &secrets) } else { ev };
                let _ = updates.send(GrpcUpdate::Event(ev));
                if done { break; }
            }
            cmd = control.recv(), if control_open => {
                let cmd = cmd.and_then(|c| match c {
                    Control::Grpc(g) => Some(g),
                    Control::Ws(_) | Control::Sse(_) | Control::Mqtt(_) => None,
                });
                match cmd {
                    Some(GrpcControl::Send(json)) => {
                        let rendered = scope.render(&json).text;
                        match sendito_grpc::schema::parse_messages(&method.input(), &rendered, true) {
                            Ok(m) => {
                                if let Err(e) = call.send(m) {
                                    let _ = updates.send(GrpcUpdate::Error(e.to_string()));
                                }
                            }
                            Err(e) => { let _ = updates.send(GrpcUpdate::Error(e)); }
                        }
                    }
                    Some(GrpcControl::EndStream) => call.close_send(),
                    Some(GrpcControl::Cancel) => call.cancel(),
                    // The controller went away (headless runs): finish the request side.
                    None => { control_open = false; call.close_send(); }
                }
            }
        }
    }
    let GrpcSummary {
        body,
        headers,
        code,
        total_ms: total,
        size,
    } = summary;
    // A call that ends with no status is a failure, not OK: report UNKNOWN (2).
    let code = if code < 0 { 2 } else { code };
    let view = ResponseView {
        status: code as u16,
        headers: &headers,
        body: &body,
        timing: &[("total", Duration::from_millis(total))],
    };
    response_phase(&req.common.scripts, &view, &mut scope, &mut results, &[]);
    scrub_asserts(&mut results.asserts, &scope.secret_values());
    let err = if code > 0 {
        Some(sendito_grpc::code_name(code).to_string())
    } else {
        None
    };
    let entry = history_entry(ctx, &template, target, Some(code as i64), total, size, err);
    write_history(ctx, &entry, None).await;
    let _ = updates.send(GrpcUpdate::Done {
        scripts: results,
        scope_errors,
        hint,
    });
}
