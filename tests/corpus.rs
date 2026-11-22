//! Correctness corpus: every request in `tests/corpus/collection` is sent through
//! the same pipeline as the app against the echo servers. The exact bytes the
//! HTTP and MCP servers received (and the messages the gRPC server decoded) are
//! compared with the reviewed captures in `tests/corpus/expected`.
//!
//! Secret values are random per run: they never appear in tracked files. In the
//! captures they are shown as `<secret:NAME>`; the raw value is checked in memory.
//!
//! `UPDATE_CORPUS=1 cargo test --test corpus` rewrites the expected captures.
//! Captures for review are written to `target/corpus/`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sendito_core::Request;
use sendito_store::env::{Keychain, MemoryKeychain};
use sendito_testkit::{grpc_echo, http_echo, mcp_echo};
use sendito_ui::runner::{self, GrpcUpdate, Project, RunCtx};

/// `unwrap` for test helpers (the workspace denies `unwrap_used`).
pub trait Must<T> {
    fn must(self) -> T;
}

impl<T, E: std::fmt::Debug> Must<T> for Result<T, E> {
    #[track_caller]
    fn must(self) -> T {
        match self {
            Ok(v) => v,
            Err(e) => panic!("{e:?}"),
        }
    }
}

impl<T> Must<T> for Option<T> {
    #[track_caller]
    fn must(self) -> T {
        match self {
            Some(v) => v,
            None => panic!("unexpected None"),
        }
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).must();
    for e in std::fs::read_dir(from).must().flatten() {
        let (s, d) = (e.path(), to.join(e.file_name()));
        if s.is_dir() {
            copy_dir(&s, &d);
        } else {
            std::fs::copy(&s, &d).must();
        }
    }
}

fn random_secret(prefix: &str) -> String {
    format!("{prefix}-{:032x}", fastrand::u128(..))
}

fn adler32(b: &[u8]) -> u32 {
    let (mut a, mut s) = (1u32, 0u32);
    for x in b {
        a = (a + *x as u32) % 65521;
        s = (s + a) % 65521;
    }
    (s << 16) | a
}

struct Env {
    dir: tempfile::TempDir,
    secrets: BTreeMap<String, String>,
    http: http_echo::HttpEcho,
    grpc: grpc_echo::GrpcEcho,
    mcp: mcp_echo::McpEcho,
    keychain: Arc<MemoryKeychain>,
}

async fn setup() -> Env {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/collection");
    let dir = tempfile::tempdir().must();
    copy_dir(&src, dir.path());
    let http = http_echo::start().await.must();
    let grpc = grpc_echo::start().await.must();
    let mcp = mcp_echo::start().await.must();
    let env_path = dir.path().join(".sendito/environments/corpus.toml");
    let text = std::fs::read_to_string(&env_path)
        .must()
        .replace("http://127.0.0.1:0", &http.url())
        .replace(
            "host_port = \"127.0.0.1:0\"",
            &format!("host_port = \"127.0.0.1:{}\"", http.addr.port()),
        )
        .replace(
            "grpc_target = \"127.0.0.1:0\"",
            &format!("grpc_target = \"{}\"", grpc.target()),
        );
    let text = format!("{text}\nmcp_url = \"{}\"\n", mcp.url());
    std::fs::write(&env_path, text).must();
    let mut secrets = BTreeMap::new();
    secrets.insert("local_token".to_string(), random_secret("lt"));
    secrets.insert(
        "basic_password".to_string(),
        format!("p@ss:wörd-{}", &random_secret("bp")[3..15]),
    );
    secrets.insert("keychain_key".to_string(), random_secret("kc"));
    secrets.insert("file_secret".to_string(), random_secret("fs"));
    std::fs::write(
        dir.path().join(".sendito/secrets.toml"),
        format!(
            "[corpus]\nlocal_token = \"{}\"\nbasic_password = \"{}\"\n",
            secrets["local_token"], secrets["basic_password"]
        ),
    )
    .must();
    std::fs::create_dir_all(dir.path().join("secrets")).must();
    std::fs::write(
        dir.path().join("secrets/token.txt"),
        format!("{}\n", secrets["file_secret"]),
    )
    .must();
    let keychain = Arc::new(MemoryKeychain::default());
    keychain
        .set("corpus/corpus/keychain_key", &secrets["keychain_key"])
        .must();
    let large: Vec<u8> = (0..1_048_576u32)
        .map(|i| (i.wrapping_mul(2654435761) >> 24) as u8)
        .collect();
    std::fs::write(dir.path().join("files/large.bin"), large).must();
    Env {
        dir,
        secrets,
        http,
        grpc,
        mcp,
        keychain,
    }
}

fn project(e: &Env) -> Project {
    Project::open_with(e.dir.path(), e.keychain.clone(), None)
}

fn ctx(e: &Env, p: &Project, rel: &str) -> RunCtx {
    RunCtx {
        root: e.dir.path().to_path_buf(),
        send: p.send_options(),
        history: Some((e.dir.path().join(".sendito/history.jsonl"), 10_000)),
        name: sendito_store::collection::name_of(rel).to_string(),
        path: Some(rel.to_string()),
        env: Some("corpus".into()),
        schemas: Default::default(),
        grpc_deadline_ms: 0,
        mask_echoed_secrets: false,
    }
}

fn mask_secrets(mut s: String, secrets: &BTreeMap<String, String>) -> String {
    for (k, v) in secrets {
        s = s.replace(v.as_str(), &format!("<secret:{k}>"));
        // Basic auth carries the password inside base64(user:password).
        let b64 = base64_std(&format!("ana:{v}"));
        s = s.replace(&b64, &format!("<base64 of \"ana:\" + secret:{k}>"));
    }
    s
}

fn base64_std(s: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(s.as_bytes())
}

fn normalize_http(
    c: &http_echo::Captured,
    port: u16,
    secrets: &BTreeMap<String, String>,
) -> String {
    let mut head = c.head();
    let body = c.body();
    let boundary = c.header("content-type").and_then(|ct| {
        ct.split("boundary=")
            .nth(1)
            .map(|b| b.trim_matches('"').to_string())
    });
    let mut body_text = if body.len() > 64 * 1024 {
        format!(
            "<{} bytes, adler32={:08x}, first16={}>",
            body.len(),
            adler32(&body),
            hex(&body[..16])
        )
    } else {
        match std::str::from_utf8(&body) {
            Ok(t) => t.to_string(),
            Err(_) => format!("<binary {} bytes: {}>", body.len(), hex(&body)),
        }
    };
    if let Some(b) = boundary {
        head = head.replace(&b, "<boundary>");
        // Multipart: text parts verbatim, binary part contents as hex.
        if std::str::from_utf8(&body).is_err() && body.len() <= 64 * 1024 {
            let sep = format!("--{b}");
            let mut out = String::new();
            let mut rest: &[u8] = &body;
            while let Some(i) = find(rest, sep.as_bytes()) {
                let (part, tail) = rest.split_at(i);
                out.push_str(&render_part(part));
                out.push_str("--<boundary>");
                rest = &tail[sep.len()..];
            }
            out.push_str(&String::from_utf8_lossy(rest));
            body_text = out;
        }
        body_text = body_text.replace(&b, "<boundary>");
    }
    let s = format!("{head}{body_text}").replace(&format!("127.0.0.1:{port}"), "127.0.0.1:<port>");
    mask_secrets(s, secrets)
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

fn render_part(part: &[u8]) -> String {
    match find(part, b"\r\n\r\n") {
        Some(i) if std::str::from_utf8(&part[i + 4..]).is_err() => {
            let content = &part[i + 4..];
            let content = content.strip_suffix(b"\r\n").unwrap_or(content);
            format!(
                "{}\r\n\r\n<binary {} bytes: {}>\r\n",
                String::from_utf8_lossy(&part[..i]),
                content.len(),
                hex(content)
            )
        }
        _ => String::from_utf8_lossy(part).into_owned(),
    }
}

fn hex(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn expected_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/expected")
}

fn report_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("target/corpus")
}

fn check(kind: &str, case: &str, actual: &str, failures: &mut Vec<String>) {
    check_with_capture(kind, case, actual, actual, failures)
}

/// Compares `actual` with the golden file and writes `capture` for review.
fn check_with_capture(
    kind: &str,
    case: &str,
    actual: &str,
    capture: &str,
    failures: &mut Vec<String>,
) {
    let path = expected_dir().join(kind).join(format!("{case}.wire"));
    let out = report_dir()
        .join("captures")
        .join(kind)
        .join(format!("{case}.wire"));
    std::fs::create_dir_all(out.parent().must()).must();
    std::fs::write(&out, capture).must();
    if std::env::var("UPDATE_CORPUS").is_ok() {
        std::fs::create_dir_all(path.parent().must()).must();
        std::fs::write(&path, actual).must();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_default();
    if expected != actual {
        failures.push(format!("{kind}/{case}: capture differs from {}\n--- expected\n{expected}\n--- actual\n{actual}", path.display()));
    }
}

fn cases(dir: &Path, sub: &str) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir.join(sub))
        .must()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".toml"))
        .collect();
    v.sort();
    v
}

/// Independent checks derived from the request file (the golden capture covers the rest).
fn oracle_http(
    req: &sendito_core::HttpRequest,
    caps: &[http_echo::Captured],
    scope: &sendito_core::Scope,
    case: &str,
    failures: &mut Vec<String>,
) {
    let Some(first) = caps.first() else {
        failures.push(format!("{case}: server received nothing"));
        return;
    };
    if first.method() != req.method {
        failures.push(format!(
            "{case}: method {} != {}",
            first.method(),
            req.method
        ));
    }
    for h in req.headers.iter().filter(|h| h.enabled) {
        let name = scope.render(&h.name).text;
        let value = scope.render(&h.value).text;
        let got = first.header_bytes(&name);
        if !got.iter().any(|g| *g == value.as_bytes()) {
            failures.push(format!(
                "{case}: header {name}: {value:?} missing (got {:?})",
                got.iter()
                    .map(|g| String::from_utf8_lossy(g).into_owned())
                    .collect::<Vec<_>>()
            ));
        }
    }
    for h in req.headers.iter().filter(|h| !h.enabled) {
        if first.header(&h.name).is_some() {
            failures.push(format!("{case}: disabled header {} was sent", h.name));
        }
    }
    let target = first.target();
    for p in req.params.iter() {
        let enc = format!(
            "{}=",
            sendito_core::resolve::query_encode(&scope.render(&p.name).text)
        );
        if p.enabled != target.contains(&enc) {
            failures.push(format!(
                "{case}: param {} enabled={} but target is {target}",
                p.name, p.enabled
            ));
        }
    }
    match &req.body {
        sendito_core::Body::Json { text } | sendito_core::Body::Raw { text, .. } => {
            let want = scope.render(text).text;
            if first.body() != want.as_bytes() {
                failures.push(format!(
                    "{case}: body differs: {:?} vs {:?}",
                    String::from_utf8_lossy(&first.body()),
                    want
                ));
            }
        }
        sendito_core::Body::None if req.method != "HEAD" && !first.body().is_empty() => {
            failures.push(format!("{case}: unexpected body"));
        }
        _ => {}
    }
    // Nothing the request did not define: allowed extra headers are transport ones.
    let defined: Vec<String> = req
        .headers
        .iter()
        .filter(|h| h.enabled)
        .map(|h| scope.render(&h.name).text.to_ascii_lowercase())
        .collect();
    for (n, _) in first.headers() {
        let n = n.to_ascii_lowercase();
        let derived = matches!(
            n.as_str(),
            "host"
                | "content-length"
                | "transfer-encoding"
                | "content-type"
                | "authorization"
                // reqwest adds `Accept: */*` to every request (see docs/DEBT.md).
                | "accept"
        ) || (matches!(&req.auth, sendito_core::Auth::ApiKey { key, location: sendito_core::ApiKeyLocation::Header, .. } if key.eq_ignore_ascii_case(&n)));
        if !derived && !defined.contains(&n) {
            failures.push(format!("{case}: header {n} was sent but not defined"));
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_corpus() {
    let e = setup().await;
    let p = project(&e);
    let root = e.dir.path().to_path_buf();
    let mut failures = Vec::new();
    let names = cases(&root, "http");
    assert!(names.len() >= 40, "only {} HTTP cases", names.len());
    let mut passed = 0;
    for file in &names {
        let case = file.trim_end_matches(".toml");
        let rel = format!("http/{file}");
        let Request::Http(req) = p.coll.load(&rel).must() else {
            panic!("{rel} is not HTTP")
        };
        let (scope, errors) = runner::build_scope(&p.envs, Some("corpus"), &Default::default());
        assert!(errors.is_empty(), "{case}: scope errors {errors:?}");
        let before = failures.len();
        e.http.take();
        let o = runner::exec_http(&ctx(&e, &p, &rel), &req, scope.clone(), vec![]).await;
        if let Err(err) = &o.response {
            failures.push(format!("{case}: request failed: {err}"));
            continue;
        }
        let caps = e.http.take();
        // Pre-request script variables are part of the request definition.
        let mut oscope = scope.clone();
        sendito_core::script::run_pre(&req.scripts.pre, &mut oscope);
        oracle_http(&req, &caps, &oscope, case, &mut failures);
        let mut dump = String::new();
        for (i, c) in caps.iter().enumerate() {
            if caps.len() > 1 {
                dump.push_str(&format!("### request {} of {}\n", i + 1, caps.len()));
            }
            dump.push_str(&normalize_http(c, e.http.addr.port(), &e.secrets));
            if !dump.ends_with('\n') {
                dump.push('\n');
            }
        }
        if let Ok(r) = &o.response {
            dump.push_str(&format!("### client saw: {} {}\n", r.status, r.reason));
        }
        for n in &o.resolved.notes {
            dump.push_str(&format!("### note: {n}\n"));
        }
        check("http", case, &dump, &mut failures);
        // Secrets never reach the exported curl command.
        let curl = sendito_import::curl::export(&req, &scope, false, &root);
        for v in e.secrets.values() {
            if curl.contains(v.as_str()) {
                failures.push(format!("{case}: secret leaked into curl export"));
            }
        }
        if failures.len() == before {
            passed += 1;
        }
    }
    let history = std::fs::read_to_string(root.join(".sendito/history.jsonl")).must();
    for (k, v) in &e.secrets {
        assert!(
            !history.contains(v.as_str()),
            "secret {k} leaked into history"
        );
    }
    write_summary("http", names.len(), passed, &failures);
    assert!(
        failures.is_empty(),
        "{} of {} HTTP cases failed:\n\n{}",
        names.len() - passed,
        names.len(),
        failures.join("\n\n")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grpc_corpus() {
    let e = setup().await;
    let p = project(&e);
    let root = e.dir.path().to_path_buf();
    let names = cases(&root, "grpc");
    assert!(names.len() >= 15, "only {} gRPC cases", names.len());
    let mut failures = Vec::new();
    let mut passed = 0;
    for file in &names {
        let case = file.trim_end_matches(".toml");
        let rel = format!("grpc/{file}");
        let Request::Grpc(req) = p.coll.load(&rel).must() else {
            panic!("{rel} is not gRPC")
        };
        let (scope, _) = runner::build_scope(&p.envs, Some("corpus"), &Default::default());
        e.grpc.take();
        let (utx, mut urx) = tokio::sync::mpsc::unbounded_channel();
        let (ctl_tx, ctl_rx) = tokio::sync::mpsc::unbounded_channel();
        drop(ctl_tx);
        runner::exec_grpc(&ctx(&e, &p, &rel), &req, scope, vec![], utx, ctl_rx).await;
        let mut status = String::new();
        let mut received = 0;
        let mut errors = Vec::new();
        while let Ok(u) = urx.try_recv() {
            match u {
                GrpcUpdate::Event(sendito_grpc::Event::Finished { code, message, .. }) => {
                    status = format!(
                        "{} ({code}){}",
                        sendito_grpc::code_name(code),
                        if message.is_empty() {
                            String::new()
                        } else {
                            format!(": {message}")
                        }
                    )
                }
                GrpcUpdate::Event(sendito_grpc::Event::Received { .. }) => received += 1,
                GrpcUpdate::Error(err) => errors.push(err),
                _ => {}
            }
        }
        let caps = e.grpc.take();
        let before = failures.len();
        if !errors.is_empty() {
            failures.push(format!("{case}: {errors:?}"));
        }
        let mut dump = String::new();
        for c in &caps {
            dump.push_str(&format!("method: /sendito.test.v1.Echo/{}\n", c.method));
            dump.push_str("metadata received:\n");
            for (k, v) in &c.metadata {
                dump.push_str(&format!("  {k}: {v}\n"));
                // Binary values travel base64-encoded; show the bytes as well.
                if k.ends_with("-bin") {
                    use base64::Engine;
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(v.trim())
                        .or_else(|_| {
                            base64::engine::general_purpose::STANDARD_NO_PAD.decode(v.trim())
                        })
                        .unwrap_or_default();
                    let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
                    dump.push_str(&format!("    (binary value; bytes: {})\n", hex.join(" ")));
                }
            }
            dump.push_str(&format!("messages received: {}\n", c.messages.len()));
            for (m, raw) in c.messages.iter().zip(&c.raw) {
                // Protobuf does not define the order of map entries on the wire;
                // show them sorted so the capture is stable.
                dump.push_str(&format!(
                    "  json: {}\n  protobuf (base64): {raw}\n",
                    sorted_json(m)
                ));
            }
        }
        dump.push_str(&format!(
            "client: {received} response message(s), status {status}\n"
        ));
        let dump = mask_secrets(dump, &e.secrets);
        // Independent check: the server decoded exactly the messages in the file.
        if let Some(c) = caps.first() {
            let sent: serde_json::Value =
                serde_json::from_str(&req.message).unwrap_or(serde_json::Value::Null);
            let sent: Vec<serde_json::Value> = match sent {
                serde_json::Value::Array(a) => a,
                v => vec![v],
            };
            if c.messages.len() != sent.len() {
                failures.push(format!(
                    "{case}: server received {} messages, file has {}",
                    c.messages.len(),
                    sent.len()
                ));
            }
        } else {
            failures.push(format!("{case}: server recorded no call"));
        }
        // Map entries (also inside google.protobuf.Struct) have no defined order
        // on the wire, so for messages with maps the golden compares the decoded
        // message; the review capture keeps the exact bytes.
        let golden = if case.contains("maps") || case.contains("well-known") {
            dump.lines()
                .map(|l| if l.trim_start().starts_with("protobuf (base64):") { "  protobuf (base64): <not compared: contains map entries, whose wire order is unspecified>".to_string() } else { l.to_string() })
                .collect::<Vec<_>>()
                .join("\n")
                + "\n"
        } else {
            dump.clone()
        };
        check_with_capture("grpc", case, &golden, &dump, &mut failures);
        if failures.len() == before {
            passed += 1;
        }
    }
    let history = std::fs::read_to_string(root.join(".sendito/history.jsonl")).must();
    for (k, v) in &e.secrets {
        assert!(
            !history.contains(v.as_str()),
            "secret {k} leaked into history"
        );
    }
    write_summary("grpc", names.len(), passed, &failures);
    assert!(
        failures.is_empty(),
        "{} of {} gRPC cases failed:\n\n{}",
        names.len() - passed,
        names.len(),
        failures.join("\n\n")
    );
}

fn normalize_mcp(caps: &[mcp_echo::Captured], url: &str) -> String {
    let sid = caps
        .get(1)
        .and_then(|c| c.header("mcp-session-id"))
        .unwrap_or_default();
    let mut dump = String::new();
    for (i, c) in caps.iter().enumerate() {
        dump.push_str(&format!("### request {}\n{}\r\n", i + 1, c.request_line));
        for (name, value) in &c.headers {
            dump.push_str(&format!("{name}: {value}\r\n"));
        }
        dump.push_str("\r\n");
        dump.push_str(&c.body);
        dump.push('\n');
    }
    let host = url.trim_start_matches("http://");
    let dump = dump.replace(host, "127.0.0.1:<port>");
    if sid.is_empty() {
        dump
    } else {
        dump.replace(&sid, "<session>")
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_list_corpus() {
    let echo = mcp_echo::start().await.must();
    let transport = sendito_mcp::Transport::Http {
        url: format!("{}/mcp?paginate=1", echo.url()),
        headers: Vec::new(),
        verify_tls: true,
        auth: None,
    };
    let (tx, _) = tokio::sync::mpsc::unbounded_channel();
    let tools = sendito_mcp::list_tools(&transport, &sendito_mcp::Options::default(), tx)
        .await
        .must();
    assert_eq!(
        tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        ["add", "greet"]
    );
    let caps = echo.captured();
    assert_eq!(caps.len(), 4);
    let mut failures = Vec::new();
    check(
        "mcp",
        "list",
        &normalize_mcp(&caps, &echo.url()),
        &mut failures,
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_corpus() {
    let e = setup().await;
    let p = project(&e);
    let names = cases(e.dir.path(), "mcp");
    assert!(names.len() >= 3);
    let mut failures = Vec::new();
    let mut passed = 0;
    for file in &names {
        let case = file.trim_end_matches(".toml");
        let rel = format!("mcp/{file}");
        let Request::Mcp(req) = p.coll.load(&rel).must() else {
            panic!("{rel} is not MCP");
        };
        let (scope, errors) = runner::build_scope(&p.envs, Some("corpus"), &Default::default());
        assert!(errors.is_empty(), "{errors:?}");
        let start = e.mcp.captured().len();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        runner::exec_mcp(
            &ctx(&e, &p, &rel),
            &req,
            scope,
            vec![],
            &runner::McpRunInfo::of(&p),
            tx,
        )
        .await;
        let mut outcome = None;
        while let Ok(update) = rx.try_recv() {
            if let runner::McpUpdate::Done {
                error, is_error, ..
            } = update
            {
                outcome = Some((error, is_error));
            }
        }
        let before = failures.len();
        let (error, is_error) = outcome.must();
        if error.is_some() || is_error != (case == "tool-error") {
            failures.push(format!("{case}: error={error:?}, is_error={is_error}"));
        }
        let caps = e.mcp.captured();
        let caps = &caps[start..];
        if caps.len() != 3 {
            failures.push(format!("{case}: expected three POSTs, got {}", caps.len()));
        }
        if case == "bearer" {
            let expected = format!("Bearer {}", e.secrets["local_token"]);
            if caps
                .iter()
                .any(|c| c.header("authorization").as_deref() != Some(expected.as_str()))
            {
                failures.push(format!("{case}: bearer header missing or incorrect"));
            }
        }
        let dump = normalize_mcp(caps, &e.mcp.url());
        check("mcp", case, &mask_secrets(dump, &e.secrets), &mut failures);
        if failures.len() == before {
            passed += 1;
        }
    }
    let history = std::fs::read_to_string(e.dir.path().join(".sendito/history.jsonl")).must();
    for (name, value) in &e.secrets {
        assert!(
            !history.contains(value.as_str()),
            "secret {name} leaked into MCP history"
        );
    }
    write_summary("mcp", names.len(), passed, &failures);
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// JSON with object keys sorted (map fields have no defined order).
fn sorted_json(s: &str) -> String {
    fn sort(v: serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(m) => {
                let mut entries: Vec<(String, serde_json::Value)> =
                    m.into_iter().map(|(k, v)| (k, sort(v))).collect();
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                serde_json::Value::Object(entries.into_iter().collect())
            }
            serde_json::Value::Array(a) => {
                serde_json::Value::Array(a.into_iter().map(sort).collect())
            }
            other => other,
        }
    }
    serde_json::from_str::<serde_json::Value>(s)
        .map(|v| sort(v).to_string())
        .unwrap_or_else(|_| s.to_string())
}

fn write_summary(kind: &str, total: usize, passed: usize, failures: &[String]) {
    let dir = report_dir();
    std::fs::create_dir_all(&dir).must();
    std::fs::write(
        dir.join(format!("{kind}-summary.txt")),
        format!("{kind}: {passed}/{total} passed\n{}", failures.join("\n\n")),
    )
    .must();
}
