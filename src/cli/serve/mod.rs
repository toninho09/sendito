//! `sendito serve`: an MCP server on stdin/stdout for AI agents.
//!
//! Messages are NDJSON (one JSON-RPC message per line), as the MCP stdio
//! transport says. Only the writer task writes to stdout; logs go to stderr.
//! Each `tools/call` runs in its own task, so a long call does not block a
//! `ping` or a `notifications/cancelled`.

mod tools;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use sendito_store::env::Keychain;

use sendito_mcp::protocol;
use sendito_pipeline::suite::Cancel;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use crate::*;

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
/// MCP: the resource does not exist.
const RESOURCE_NOT_FOUND: i64 = -32002;

/// The request file format, for the agent to read before it writes TOML.
const FORMAT_URI: &str = "sendito://format";
const FORMAT_MD: &str = include_str!("../../../FORMAT.md");

const INSTRUCTIONS: &str = "Sendito sends the API requests of one collection \
(HTTP, GraphQL, gRPC, WebSocket, SSE, MQTT and MCP tools). Paths are relative \
to the collection root and use '/'. Read the resource sendito://format before \
you write a request as TOML. This server does not show secret variable \
values: use the {{name}} placeholders.";

/// The `sendito serve` flags.
pub(crate) struct ServeFlags {
    /// The environment when a call names none.
    pub(crate) env: Option<String>,
    /// The only environments a call can see and use; empty allows every one.
    pub(crate) allow_env: Vec<String>,
    /// Leave out the tools that write files.
    pub(crate) read_only: bool,
    /// Leave out the tools that send requests.
    pub(crate) no_send: bool,
    pub(crate) no_history: bool,
    pub(crate) no_cookies: bool,
}

impl ServeFlags {
    /// A call can use `env`: `--allow-env` names it, or the flag is not given.
    pub(crate) fn allows(&self, env: &str) -> bool {
        self.allow_env.is_empty() || self.allow_env.iter().any(|e| e == env)
    }

    /// Every environment that `--env` and `--allow-env` name exists, and
    /// `--allow-env` holds `--env`. `envs` are the collection's environments.
    fn check_envs(&self, envs: &[String]) -> Result<(), String> {
        if let Some(env) = self
            .allow_env
            .iter()
            .chain(&self.env)
            .find(|e| !envs.contains(e))
        {
            return Err(format!("environment \"{env}\" does not exist"));
        }
        match &self.env {
            Some(env) if !self.allows(env) => Err(format!(
                "--env {env} is not in --allow-env ({})",
                self.allow_env.join(", ")
            )),
            _ => Ok(()),
        }
    }
}

/// What every call reads.
struct Server {
    root: PathBuf,
    keychain: Arc<dyn Keychain>,
    /// The global config file; `None` reads none.
    global: Option<PathBuf>,
    flags: ServeFlags,
    /// The open collection and the [`Stamp`] of its config files.
    project: Mutex<(Stamp, Arc<Project>)>,
}

/// When the project and the global config files last changed (`None`: no file).
type Stamp = [Option<SystemTime>; 2];

/// What the read loop does with one message.
#[derive(Debug, PartialEq)]
enum Step {
    /// Write this message.
    Reply(Value),
    /// Nothing to write (a notification, or a response from the client).
    Quiet,
    /// Stop the call with this id (its JSON text).
    Cancel(String),
    /// Run a tool in its own task.
    Call {
        id: Value,
        name: String,
        args: Value,
        /// The client's `_meta.progressToken`, when it sent one.
        progress: Option<Value>,
    },
}

/// Where a tool reports its progress: the token the client gave and the
/// writer task.
pub(super) struct Progress {
    token: Value,
    tx: mpsc::UnboundedSender<Value>,
}

impl Progress {
    /// Sends `notifications/progress`: `done` of `total`, with `message`.
    pub(super) fn send(&self, done: usize, total: usize, message: &str) {
        let mut msg = protocol::notification("notifications/progress");
        msg["params"] = json!({
            "progressToken": self.token,
            "progress": done,
            "total": total,
            "message": message,
        });
        let _ = self.tx.send(msg);
    }
}

/// The calls in flight, by the JSON text of their id.
type Calls = Mutex<HashMap<String, Cancel>>;

/// Records the call `key` and gives its [`Cancel`]; `None` when a call with
/// the same id is still running (the id must be unique).
fn register(calls: &Calls, key: &str) -> Option<Cancel> {
    let mut calls = lock(calls);
    if calls.contains_key(key) {
        return None;
    }
    let cancel = Cancel::default();
    calls.insert(key.to_string(), cancel.clone());
    Some(cancel)
}

/// Serves the collection at `path` (or the one that holds the working
/// directory) until stdin closes.
pub(crate) async fn serve(path: Option<PathBuf>, flags: ServeFlags) -> ExitCode {
    let start = path.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));
    let Some(root) = sendito_store::init::find_root(&start) else {
        fail!(
            "no collection at {} (no .sendito directory found upwards); run sendito init first",
            start.display()
        );
        return ExitCode::from(3);
    };
    let server = Server::open(
        root.clone(),
        runner::default_keychain(),
        sendito_store::config::global_path(),
        flags,
    );
    if let Err(e) = server.flags.check_envs(&server.project().envs.list()) {
        fail!("{e}");
        return ExitCode::from(3);
    }
    eprintln!("sendito: serving {} on stdin/stdout", root.display());
    let server = Arc::new(server);
    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        let mut out = tokio::io::stdout();
        while let Some(msg) = rx.recv().await {
            let mut line = msg.to_string();
            line.push('\n');
            if out.write_all(line.as_bytes()).await.is_err() || out.flush().await.is_err() {
                break;
            }
        }
    });
    let calls: Arc<Calls> = Arc::default();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(e) => {
                fail!("cannot read stdin: {e}");
                break;
            }
        };
        match server.step(&line) {
            Step::Reply(msg) => {
                let _ = tx.send(msg);
            }
            Step::Quiet => {}
            Step::Cancel(key) => {
                if let Some(cancel) = lock(&calls).get(&key) {
                    cancel.cancel();
                }
            }
            Step::Call {
                id,
                name,
                args,
                progress,
            } => {
                let key = id.to_string();
                let Some(cancel) = register(&calls, &key) else {
                    let _ = tx.send(protocol::error_response(
                        &id,
                        INVALID_REQUEST,
                        &format!("a call with the id {key} is still running"),
                    ));
                    continue;
                };
                let progress = progress.map(|token| Progress {
                    token,
                    tx: tx.clone(),
                });
                let (server, tx, calls) = (server.clone(), tx.clone(), calls.clone());
                tokio::spawn(async move {
                    let reply =
                        tools::call(&server, &name, &args, &cancel, progress.as_ref()).await;
                    lock(&calls).remove(&key);
                    // A cancelled call gets no response (MCP cancellation).
                    if !cancel.is_cancelled() {
                        let msg = match reply {
                            Ok(result) => protocol::response(&id, result),
                            Err(e) => protocol::error_response(&id, INVALID_PARAMS, &e),
                        };
                        let _ = tx.send(msg);
                    }
                });
            }
        }
    }
    // The client is gone: stop the calls in flight.
    for cancel in lock(&calls).values() {
        cancel.cancel();
    }
    drop(tx);
    let _ = writer.await;
    ExitCode::SUCCESS
}

/// Opens the collection and prints its warnings; `old` gives the
/// `$response` values to keep.
fn open_project(
    root: &Path,
    keychain: &Arc<dyn Keychain>,
    global: &Option<PathBuf>,
    old: Option<&Project>,
) -> Arc<Project> {
    let mut p = Project::open_with(root, keychain.clone(), global.clone());
    for w in &p.warnings {
        warn!("{w}");
    }
    if let Some(old) = old {
        p.responses = old.responses.clone();
    }
    Arc::new(p)
}

/// When the config files of `root` last changed.
fn stamp(root: &Path, global: Option<&Path>) -> Stamp {
    let changed = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    [
        changed(&sendito_store::config::project_path(root)),
        global.and_then(changed),
    ]
}

/// Locks `m`; a panic in another call does not stop the server.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Server {
    fn open(
        root: PathBuf,
        keychain: Arc<dyn Keychain>,
        global: Option<PathBuf>,
        flags: ServeFlags,
    ) -> Self {
        let stamp = stamp(&root, global.as_deref());
        let project = open_project(&root, &keychain, &global, None);
        Self {
            root,
            keychain,
            global,
            flags,
            project: Mutex::new((stamp, project)),
        }
    }

    /// The collection for a call: opened again when a config file changed
    /// since the last call. The `$response` values of the session stay.
    fn project(&self) -> Arc<Project> {
        let now = stamp(&self.root, self.global.as_deref());
        let mut cur = lock(&self.project);
        if cur.0 != now {
            eprintln!("sendito: the config changed; reading it again");
            let project = open_project(&self.root, &self.keychain, &self.global, Some(&cur.1));
            *cur = (now, project);
        }
        cur.1.clone()
    }

    /// What to do with one line from stdin.
    fn step(&self, line: &str) -> Step {
        if line.trim().is_empty() {
            return Step::Quiet;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(msg) => self.dispatch(&msg),
            Err(e) => Step::Reply(protocol::error_response(
                &Value::Null,
                PARSE_ERROR,
                &format!("not JSON: {e}"),
            )),
        }
    }

    fn dispatch(&self, msg: &Value) -> Step {
        if !msg.is_object() {
            return Step::Reply(protocol::error_response(
                &Value::Null,
                INVALID_REQUEST,
                "a message must be one JSON object (batches are not supported)",
            ));
        }
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            // A response from the client: this server sends no requests.
            return Step::Quiet;
        };
        let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
        let Some(id) = msg.get("id").cloned() else {
            if method == "notifications/cancelled"
                && let Some(id) = params.get("requestId")
            {
                return Step::Cancel(id.to_string());
            }
            return Step::Quiet;
        };
        let result = match method {
            "initialize" => Ok(initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(tools::list(&self.flags)),
            "tools/call" => match params.get("name").and_then(Value::as_str) {
                Some(name) => {
                    let args = params
                        .get("arguments")
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    let progress = params
                        .get("_meta")
                        .and_then(|m| m.get("progressToken"))
                        .cloned();
                    return Step::Call {
                        id,
                        name: name.to_string(),
                        args,
                        progress,
                    };
                }
                None => Err((INVALID_PARAMS, "tools/call needs a tool name".to_string())),
            },
            "resources/list" => Ok(json!({ "resources": [{
                "uri": FORMAT_URI,
                "name": "format",
                "title": "Sendito request file format",
                "description": "FORMAT.md: how a request, a folder and an environment are written as TOML.",
                "mimeType": "text/markdown",
            }]})),
            "resources/templates/list" => Ok(json!({ "resourceTemplates": [] })),
            "resources/read" => match params.get("uri").and_then(Value::as_str) {
                Some(FORMAT_URI) => Ok(json!({ "contents": [{
                    "uri": FORMAT_URI,
                    "mimeType": "text/markdown",
                    "text": FORMAT_MD,
                }]})),
                Some(uri) => Err((RESOURCE_NOT_FOUND, format!("no resource {uri}"))),
                None => Err((INVALID_PARAMS, "resources/read needs a uri".to_string())),
            },
            _ => Err((
                METHOD_NOT_FOUND,
                format!("method {method:?} is not supported"),
            )),
        };
        Step::Reply(match result {
            Ok(result) => protocol::response(&id, result),
            Err((code, message)) => protocol::error_response(&id, code, &message),
        })
    }
}

/// The `initialize` result: the client's protocol version when Sendito
/// speaks it, else Sendito's latest.
fn initialize(params: &Value) -> Value {
    let version = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .filter(|v| protocol::SUPPORTED_VERSIONS.contains(v))
        .unwrap_or(protocol::PROTOCOL_VERSION);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {}, "resources": {} },
        "serverInfo": { "name": "sendito", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn server(read_only: bool) -> (tempfile::TempDir, Server) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".sendito")).unwrap();
        let flags = ServeFlags {
            env: None,
            allow_env: Vec::new(),
            read_only,
            no_send: false,
            no_history: true,
            no_cookies: true,
        };
        let keychain = Arc::new(sendito_store::env::MemoryKeychain::default());
        let s = Server::open(dir.path().to_path_buf(), keychain, None, flags);
        (dir, s)
    }

    fn reply(s: &Server, line: &str) -> Value {
        match s.step(line) {
            Step::Reply(v) => v,
            other => panic!("not a reply: {other:?}"),
        }
    }

    #[test]
    fn initialize_answers_a_known_version_and_else_the_latest() {
        let (_d, s) = server(false);
        let v = reply(
            &s,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#,
        );
        assert_eq!(v["id"], 1);
        assert_eq!(v["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(v["result"]["serverInfo"]["name"], "sendito");
        let v = reply(
            &s,
            r#"{"jsonrpc":"2.0","id":"a","method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
        );
        assert_eq!(v["id"], "a");
        assert_eq!(v["result"]["protocolVersion"], protocol::PROTOCOL_VERSION);
    }

    #[test]
    fn the_last_handshake_version_is_answered() {
        let (_d, s) = server(false);
        let v = reply(
            &s,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}"#,
        );
        assert_eq!(v["result"]["protocolVersion"], "2025-11-25");
    }

    #[test]
    fn env_flags_name_environments_that_exist() {
        let (_d, mut s) = server(false);
        let envs = ["dev".to_string(), "prod".to_string()];
        assert!(s.flags.check_envs(&envs).is_ok());
        s.flags.env = Some("dev".into());
        s.flags.allow_env = vec!["dev".into(), "nope".into()];
        let e = s.flags.check_envs(&envs).unwrap_err();
        assert!(e.contains("\"nope\" does not exist"), "{e}");
        s.flags.allow_env = vec!["prod".into()];
        let e = s.flags.check_envs(&envs).unwrap_err();
        assert!(e.contains("--env dev is not in --allow-env"), "{e}");
        s.flags.env = Some("gone".into());
        assert!(s.flags.check_envs(&envs).is_err());
        s.flags.env = None;
        assert!(s.flags.check_envs(&envs).is_ok());
        assert!(s.flags.allows("prod") && !s.flags.allows("dev"));
    }

    #[test]
    fn a_changed_config_is_read_again() {
        let (dir, s) = server(false);
        let before = s.project();
        assert_ne!(before.config.http.timeout_ms, 1234);
        std::fs::write(
            dir.path().join(".sendito/config.toml"),
            "[http]\ntimeout_ms = 1234\n",
        )
        .unwrap();
        let after = s.project();
        assert_eq!(after.config.http.timeout_ms, 1234);
        // No change: the same collection is kept.
        assert!(Arc::ptr_eq(&after, &s.project()));
    }

    #[test]
    fn errors_follow_json_rpc() {
        let (_d, s) = server(false);
        assert_eq!(reply(&s, "{not json")["error"]["code"], PARSE_ERROR);
        assert_eq!(reply(&s, "[]")["error"]["code"], INVALID_REQUEST);
        let v = reply(&s, r#"{"jsonrpc":"2.0","id":3,"method":"nope"}"#);
        assert_eq!(v["error"]["code"], METHOD_NOT_FOUND);
        let v = reply(
            &s,
            r#"{"jsonrpc":"2.0","id":4,"method":"resources/read","params":{"uri":"x://y"}}"#,
        );
        assert_eq!(v["error"]["code"], RESOURCE_NOT_FOUND);
    }

    #[test]
    fn notifications_and_calls_are_not_answered_in_the_loop() {
        let (_d, s) = server(false);
        assert_eq!(
            s.step(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
            Step::Quiet
        );
        assert_eq!(
            s.step(
                r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":7}}"#
            ),
            Step::Cancel("7".into())
        );
        assert_eq!(
            s.step(r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"list_requests"}}"#),
            Step::Call {
                id: json!(5),
                name: "list_requests".into(),
                args: json!({}),
                progress: None,
            }
        );
        assert_eq!(
            s.step(r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"run_folder","_meta":{"progressToken":"p1"}}}"#),
            Step::Call {
                id: json!(6),
                name: "run_folder".into(),
                args: json!({}),
                progress: Some(json!("p1")),
            }
        );
    }

    #[test]
    fn an_id_in_flight_cannot_start_a_second_call() {
        let calls = Calls::default();
        let first = register(&calls, "1").unwrap();
        assert!(register(&calls, "1").is_none());
        assert!(register(&calls, "\"1\"").is_some());
        lock(&calls).remove("1");
        assert!(register(&calls, "1").is_some());
        assert!(!first.is_cancelled());
    }

    #[test]
    fn the_format_resource_is_format_md() {
        let (_d, s) = server(false);
        let v = reply(
            &s,
            r#"{"jsonrpc":"2.0","id":1,"method":"resources/read","params":{"uri":"sendito://format"}}"#,
        );
        let text = v["result"]["contents"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("[http]") || text.contains("method"),
            "{}",
            &text[..80]
        );
    }
}
