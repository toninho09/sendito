//! `sendito` — terminal API client for HTTP, gRPC and MCP tools.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::{Parser, Subcommand, ValueEnum};
use sendito_core::Request;
use sendito_core::script::RUNTIME_LAYER;
use sendito_core::vars::Scope;
use sendito_pipeline::human_size;
use sendito_pipeline::runner::{self, GrpcUpdate, Project, RunCtx};

#[derive(Parser)]
#[command(
    name = "sendito",
    version,
    about = "Terminal API client for HTTP, gRPC, WebSocket and MCP tools",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    /// Collection directory (default: nearest `.sendito/` above the current directory, else the current directory).
    path: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// Exit after drawing the first frame (startup benchmark).
    #[arg(long, hide = true)]
    exit_after_first_frame: bool,
    /// Write per-frame render times (µs) to this file.
    #[arg(long, hide = true)]
    frame_log: Option<PathBuf>,
    /// Replay scripted input from this file.
    #[arg(long, hide = true)]
    script: Option<PathBuf>,
    /// Print KEYMAP.md to stdout.
    #[arg(long, hide = true)]
    print_keymap: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a collection in PATH (default: current directory).
    Init { path: Option<PathBuf> },
    /// Send one request, or every request in a folder, without the UI.
    ///
    /// One request: the body goes to stdout; status, timing and assertion
    /// results go to stderr. A folder (or `.` for the current folder — the
    /// whole collection at its root) runs every request below it in tree order
    /// — one line per request and a summary; variables a request extracts are
    /// available to the next ones.
    /// Exit codes: 0 success, 1 failed assertion or MCP tool error, 2 request
    /// failed, 3 usage error (or unresolved variables).
    Run {
        /// Request file, or a folder to run recursively (`.` = current folder).
        request: PathBuf,
        /// Environment to use.
        #[arg(long, short)]
        env: Option<String>,
        /// Print response headers to stderr (one request only).
        #[arg(long, short = 'i')]
        include: bool,
        /// Send even if some variables are unresolved.
        #[arg(long)]
        allow_unresolved: bool,
        /// Do not record the requests in history.
        #[arg(long)]
        no_history: bool,
        /// Do not use the collection's cookie jar for this run.
        #[arg(long)]
        no_cookies: bool,
        /// Stop at the first failed or errored request (folder runs only).
        #[arg(long)]
        fail_fast: bool,
        /// Folder report format: `text` (stderr lines) or `json` (stdout).
        #[arg(long, value_enum, default_value_t = RunReport::Text)]
        report: RunReport,
    },
    /// Print a request as a curl command (secret variables stay as placeholders).
    Curl {
        request: PathBuf,
        #[arg(long, short)]
        env: Option<String>,
        /// Include secret values in the output.
        #[arg(long)]
        include_secrets: bool,
    },
    /// Print a request as an .http file (JetBrains HTTP client / VS Code REST
    /// Client). Secret variables stay as {{placeholders}}.
    Http {
        request: PathBuf,
        #[arg(long, short)]
        env: Option<String>,
        /// Include secret values in the output.
        #[arg(long)]
        include_secrets: bool,
    },
    /// Sign in or out of a named HTTP OAuth identity.
    Auth {
        #[command(subcommand)]
        cmd: AuthCmd,
    },
    /// Sign an HTTP MCP server in (OAuth) or out.
    Mcp {
        #[command(subcommand)]
        cmd: McpCmd,
    },
    /// List color themes or print one as a theme file to start your own.
    Theme {
        #[command(subcommand)]
        cmd: ThemeCmd,
    },
    /// Import a Postman, Insomnia, or Bruno collection.
    Import {
        /// JSON export file, or a Bruno collection directory.
        file: PathBuf,
        /// Folder (relative to the collection) to import into.
        #[arg(long, default_value = "")]
        into: String,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
enum RunReport {
    Text,
    Json,
}

/// The `sendito run` flags, shared by the one-request and folder paths.
#[derive(Clone)]
struct RunFlags {
    env: Option<String>,
    include: bool,
    allow_unresolved: bool,
    no_history: bool,
    no_cookies: bool,
    fail_fast: bool,
    report: RunReport,
}

#[derive(Subcommand)]
enum AuthCmd {
    /// Sign in with authorization code + PKCE, storing tokens in the OS keychain.
    Login {
        name: String,
        #[arg(long, short)]
        env: Option<String>,
    },
    /// Remove tokens for an identity and environment.
    Logout {
        name: String,
        #[arg(long, short)]
        env: Option<String>,
    },
}

#[derive(Subcommand)]
enum McpCmd {
    /// Run the OAuth login for a server from `[mcp.servers]` (opens a browser;
    /// the URL is also printed, for remote sessions).
    Login { server: String },
    /// Forget the stored tokens of a server.
    Logout { server: String },
}

#[derive(Subcommand)]
enum ThemeCmd {
    /// List the built-in themes and your own (with any problems found).
    List {
        /// Collection whose `.sendito/themes` is searched too.
        #[arg(long)]
        collection: Option<PathBuf>,
    },
    /// Print a theme with every color token, as a file you can edit.
    Print {
        name: String,
        #[arg(long)]
        collection: Option<PathBuf>,
    },
}

fn theme_registry(collection: Option<&Path>) -> sendito_ui::theme::Registry {
    let root = find_collection(collection);
    let dirs = sendito_store::themes::dirs(sendito_store::config::global_path().as_deref(), &root);
    let (files, warnings) = sendito_store::themes::discover(&dirs);
    sendito_ui::theme::Registry::from_files(files, warnings)
}

fn theme(cmd: ThemeCmd) -> ExitCode {
    match cmd {
        ThemeCmd::List { collection } => {
            let reg = theme_registry(collection.as_deref());
            for name in reg.names() {
                let path = reg
                    .path(&name)
                    .map(|p| format!("  {}", p.display()))
                    .unwrap_or_else(|| "  (built in)".into());
                println!("{name:<16}{path}");
                for n in reg.notes(&name) {
                    println!("{:<16}  ! {n}", "");
                }
            }
            for w in &reg.warnings {
                eprintln!("sendito: {w}");
            }
            ExitCode::SUCCESS
        }
        ThemeCmd::Print { name, collection } => {
            let reg = theme_registry(collection.as_deref());
            if !reg.is_known(&name) {
                eprintln!(
                    "sendito: no theme named \"{name}\" (available: {})",
                    reg.names().join(", ")
                );
                return ExitCode::from(3);
            }
            print!("{}", reg.theme_full(&name).to_toml());
            ExitCode::SUCCESS
        }
    }
}

fn find_collection(path: Option<&Path>) -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match path {
        Some(p) => p.to_path_buf(),
        None => sendito_store::init::find_root(&cwd).unwrap_or(cwd),
    }
}

fn main() -> ExitCode {
    let started = std::time::Instant::now();
    let cli = Cli::parse();
    if cli.print_keymap {
        print!("{}", sendito_ui::keymap::keymap_markdown());
        return ExitCode::SUCCESS;
    }
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("sendito: cannot start runtime: {e}");
            return ExitCode::from(3);
        }
    };
    match cli.cmd {
        Some(Cmd::Init { path }) => init(path),
        Some(Cmd::Run {
            request,
            env,
            include,
            allow_unresolved,
            no_history,
            no_cookies,
            fail_fast,
            report,
        }) => {
            let flags = RunFlags {
                env,
                include,
                allow_unresolved,
                no_history,
                no_cookies,
                fail_fast,
                report,
            };
            rt.block_on(run_entry(&request, flags))
        }
        Some(Cmd::Curl {
            request,
            env,
            include_secrets,
        }) => curl(&request, env, include_secrets),
        Some(Cmd::Http {
            request,
            env,
            include_secrets,
        }) => http_export(&request, env, include_secrets),
        Some(Cmd::Import { file, into }) => import(&file, &into),
        Some(Cmd::Mcp { cmd }) => rt.block_on(mcp(cmd)),
        Some(Cmd::Auth { cmd }) => rt.block_on(http_auth(cmd)),
        Some(Cmd::Theme { cmd }) => theme(cmd),
        None => {
            let root = find_collection(cli.path.as_deref());
            if !root.is_dir() {
                eprintln!("sendito: {} is not a directory", root.display());
                return ExitCode::from(3);
            }
            let project = Project::open(&root, runner::default_keychain());
            let opts = sendito_ui::RunOptions {
                root,
                exit_after_first_frame: cli.exit_after_first_frame,
                frame_log: cli.frame_log,
                script: cli.script,
                started: Some(started),
            };
            match sendito_ui::run(project, opts, &rt) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("sendito: {e}");
                    ExitCode::from(3)
                }
            }
        }
    }
}

fn init(path: Option<PathBuf>) -> ExitCode {
    let dir = path.unwrap_or_else(|| PathBuf::from("."));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("sendito: {}: {e}", dir.display());
        return ExitCode::from(3);
    }
    match sendito_store::init::init(&dir) {
        Ok(rep) => {
            for p in &rep.created {
                println!("created {}", p.display());
            }
            for g in &rep.gitignore_added {
                println!("added {g} to .gitignore");
            }
            if rep.created.is_empty() && rep.gitignore_added.is_empty() {
                println!("{} is already a Sendito collection", dir.display());
            } else {
                println!("\nOpen it with: sendito {}", dir.display());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("sendito: {e}");
            ExitCode::from(3)
        }
    }
}

/// Resolves a request argument to (collection root, relative path).
fn locate(request: &Path) -> Result<(PathBuf, String), String> {
    let with_ext = |p: &Path| {
        if p.extension().is_some_and(|e| e == "toml") {
            p.to_path_buf()
        } else {
            p.with_extension("toml")
        }
    };
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let candidates = [with_ext(request), request.to_path_buf()];
    for c in &candidates {
        let abs = if c.is_absolute() {
            c.clone()
        } else {
            cwd.join(c)
        };
        if abs.is_file() {
            let dir = abs.parent().unwrap_or(&cwd).to_path_buf();
            let root = sendito_store::init::find_root(&dir)
                .unwrap_or_else(|| sendito_store::init::find_root(&cwd).unwrap_or(dir.clone()));
            let abs = abs.canonicalize().unwrap_or(abs);
            let root = root.canonicalize().unwrap_or(root);
            let rel = abs.strip_prefix(&root).map_err(|_| {
                format!(
                    "{} is outside the collection {}",
                    abs.display(),
                    root.display()
                )
            })?;
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            return Ok((root, rel));
        }
    }
    // Relative to the collection root.
    if let Some(root) = sendito_store::init::find_root(&cwd) {
        for c in &candidates {
            if root.join(c).is_file() {
                let rel = c
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                return Ok((root, rel));
            }
        }
    }
    Err(format!("request {} not found", request.display()))
}

fn check_env(project: &Project, env: &Option<String>) -> Result<(), (String, u8)> {
    if let Some(e) = env
        && !project.envs.list().contains(e)
    {
        return Err((
            format!(
                "no environment named \"{e}\" (available: {})",
                project.envs.list().join(", ")
            ),
            3,
        ));
    }
    Ok(())
}

fn load(request: &Path, env: &Option<String>) -> Result<(Project, String, Request), (String, u8)> {
    let (root, rel) = locate(request).map_err(|e| (e, 3))?;
    load_at(root, rel, env)
}

/// Opens the collection of a located request and loads it.
fn load_at(
    root: PathBuf,
    rel: String,
    env: &Option<String>,
) -> Result<(Project, String, Request), (String, u8)> {
    let project = Project::open(&root, runner::default_keychain());
    for w in &project.warnings {
        eprintln!("warning: {w}");
    }
    check_env(&project, env)?;
    let req = project.coll.load(&rel).map_err(|e| (e.to_string(), 3))?;
    Ok((project, rel, req))
}

/// Locates a folder to run: a directory in the current directory (or an
/// absolute one), or one relative to the collection root. `""` is the whole
/// collection.
fn load_suite(request: &Path, env: &Option<String>) -> Result<(Project, String), (String, u8)> {
    let cwd = std::env::current_dir().map_err(|e| (e.to_string(), 3))?;
    // The relative path of `dir` inside `root`, `/`-separated; `None` when
    // `root` is not an ancestor of `dir`.
    let rel_of = |dir: &Path, root: &Path| -> Option<String> {
        let d: Vec<_> = dir.components().collect();
        let r: Vec<_> = root.components().collect();
        (d.len() >= r.len() && d[..r.len()] == r[..]).then(|| {
            d[r.len()..]
                .iter()
                .filter_map(|c| c.as_os_str().to_str().map(str::to_string))
                .collect::<Vec<_>>()
                .join("/")
        })
    };
    // A directory next to the current one: its collection is the nearest
    // `.sendito` at or above it (the directory itself may be the collection).
    let mut found = None;
    let dir = cwd.join(request);
    // Canonicalize before looking for the root: with a lexical `..`,
    // `find_root` would walk ancestors of the written path (which may contain
    // the current collection) instead of ancestors of the real one.
    if dir.is_dir()
        && let Ok(dir) = dir.canonicalize()
        && let Some(root) = sendito_store::init::find_root(&dir)
        && let Some(rel) = rel_of(&dir, &root)
    {
        found = Some((root, rel));
    } else if !request.is_absolute()
        && !cwd.join(request).exists()
        && let Some(root) = sendito_store::init::find_root(&cwd)
        && root.join(request).is_dir()
    {
        // A folder of the collection addressed from anywhere inside it. Only
        // when the path does not exist next to the current directory: an
        // absolute or `../` path that exists outside the collection must not
        // be reinterpreted as collection-relative.
        found = Some((
            root,
            request
                .components()
                .filter_map(|c| match c {
                    std::path::Component::Normal(s) => s.to_str().map(str::to_string),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("/"),
        ));
    }
    let Some((root, rel)) = found else {
        return Err(if cwd.join(request).is_dir() {
            (
                format!("{} is not inside a Sendito collection", request.display()),
                3,
            )
        } else {
            (format!("folder {} not found", request.display()), 3)
        });
    };
    let project = Project::open(&root, runner::default_keychain());
    for w in &project.warnings {
        eprintln!("warning: {w}");
    }
    check_env(&project, env)?;
    Ok((project, rel))
}

fn ctx_for(
    project: &Project,
    env: &Option<String>,
    rel: &str,
    no_history: bool,
    no_cookies: bool,
    schemas: runner::SchemaCache,
) -> RunCtx {
    RunCtx {
        root: project.root.clone(),
        send: project.send_options(),
        history: if no_history || !project.config.history.enabled {
            None
        } else {
            Some((
                sendito_store::history::history_path(&project.root),
                project.config.history.max_entries,
            ))
        },
        name: sendito_store::collection::name_of(rel).to_string(),
        path: Some(rel.to_string()),
        env: env.clone(),
        schemas,
        grpc_deadline_ms: project.config.grpc.deadline_ms,
        ws_timeout_ms: project.config.websocket.timeout_ms,
        mask_echoed_secrets: false,
        cookie_jar: project.config.http.cookie_jar && !no_cookies,
        oauth: Some(runner::OAuthRunInfo::of(project)),
    }
}

async fn scope_of(project: &Project, env: &Option<String>) -> (Scope, Vec<String>) {
    let envs = project.envs.clone();
    let e2 = env.clone();
    tokio::task::spawn_blocking(move || {
        runner::build_scope(&envs, e2.as_deref(), &Default::default())
    })
    .await
    .unwrap_or_default()
}

/// True when the argument names a directory (a folder run). Checked before
/// loading so a folder's own errors (unknown environment, missing folder) are
/// reported instead of "request not found".
fn is_folder_arg(request: &Path) -> bool {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if cwd.join(request).is_dir() {
        return true;
    }
    sendito_store::init::find_root(&cwd).is_some_and(|root| root.join(request).is_dir())
}

/// Routes `run` to one request or a folder: a request file wins over a folder
/// of the same name, and errors from whichever path matched are reported as
/// they are.
async fn run_entry(request: &Path, flags: RunFlags) -> ExitCode {
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

async fn run(project: Project, rel: String, req: Request, flags: &RunFlags) -> ExitCode {
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
    if report == RunReport::Json {
        eprintln!(
            "warning: --report json applies to folder runs; ignoring it for a single request"
        );
    }
    let names = runner::names_scope(&project.envs, env.as_deref(), &Default::default());
    let un = runner::unresolved(&req, &names);
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
    let (scope, errors) = scope_of(&project, &env).await;
    for e in &errors {
        eprintln!("warning: {e}");
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
                    let _ = out.write_all(o.doc.bytes());
                    if !o.doc.bytes().ends_with(b"\n") && !o.doc.is_empty() {
                        let _ = out.write_all(b"\n");
                    }
                    0
                }
                Err(e) => {
                    eprintln!("sendito: request failed: {e}");
                    2
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
            let run = runner::exec_grpc(&ctx, &g, scope, errors, utx, ctl_rx);
            let mut code = 0u8;
            let mut fails = 0;
            let print = async {
                while let Some(u) = urx.recv().await {
                    match u {
                        GrpcUpdate::Started { resolved, kind } => eprintln!(
                            "{} /{}/{} ({})",
                            resolved.uri,
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
        Request::Ws(w) => {
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
                        } => {
                            if let Some(e) = error {
                                eprintln!("sendito: {e}");
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
struct Ran {
    /// `pass`, `fail` (a failed assertion or a tool `isError`) or `error`.
    outcome: &'static str,
    /// Status text: `201 Created`, `OK`, or the error message.
    label: String,
    scripts: runner::ScriptResults,
}

/// Runs every request under `folder` in tree order, chaining variables: a
/// value a request extracts is available to the ones after it.
async fn run_suite(project: Project, folder: String, flags: &RunFlags) -> ExitCode {
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
    let (mut scope, scope_errors) = scope_of(&project, &env).await;
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
        let req = match project.coll.load(rel) {
            Ok(r) => r,
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
            Request::Mcp(_) => "mcp",
        };
        let un = runner::unresolved(&req, &scope);
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
        let ran = run_one(&ctx, &req, &scope, &scope_errors, &mcp_info, no_cookies).await;
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
async fn run_one(
    ctx: &RunCtx,
    req: &Request,
    scope: &Scope,
    scope_errors: &[String],
    mcp_info: &runner::McpRunInfo,
    no_cookies: bool,
) -> Ran {
    match req {
        Request::Http(h) => {
            let mut h = h.clone();
            if no_cookies {
                h.options.cookies = Some(false);
            }
            let o = runner::exec_http(ctx, &h, scope.clone(), scope_errors.to_vec()).await;
            let failed_asserts = o.scripts.failed_asserts();
            match &o.response {
                Ok(r) => Ran {
                    outcome: if failed_asserts > 0 { "fail" } else { "pass" },
                    label: format!("{} {}", r.status, r.reason),
                    scripts: o.scripts,
                },
                Err(e) => Ran {
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
                    outcome: "error",
                    label,
                    scripts,
                }
            } else {
                let fails = scripts.failed_asserts();
                Ran {
                    outcome: if fails > 0 { "fail" } else { "pass" },
                    label,
                    scripts,
                }
            }
        }
        Request::Ws(w) => {
            let (utx, mut urx) = tokio::sync::mpsc::unbounded_channel();
            let (ctl_tx, ctl_rx) = tokio::sync::mpsc::unbounded_channel::<runner::Control>();
            drop(ctl_tx);
            let run = runner::exec_ws(ctx, w, scope.clone(), scope_errors.to_vec(), utx, ctl_rx);
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
                    outcome: "error",
                    label: e,
                    scripts,
                }
            } else if code != 1000 {
                Ran {
                    outcome: "error",
                    label,
                    scripts,
                }
            } else {
                let failed_asserts = scripts.failed_asserts();
                Ran {
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
                    outcome: "error",
                    label,
                    scripts,
                }
            } else {
                let fails = scripts.failed_asserts();
                Ran {
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

fn print_scripts(s: &runner::ScriptResults) -> usize {
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

fn curl(request: &Path, env: Option<String>, include_secrets: bool) -> ExitCode {
    let (project, _, req) = match load(request, &env) {
        Ok(v) => v,
        Err((e, code)) => {
            eprintln!("sendito: {e}");
            return ExitCode::from(code);
        }
    };
    let Request::Http(h) = req else {
        eprintln!("sendito: curl export is only available for HTTP requests");
        return ExitCode::from(3);
    };
    let scope = if include_secrets {
        runner::build_scope(&project.envs, env.as_deref(), &Default::default()).0
    } else {
        runner::names_scope(&project.envs, env.as_deref(), &Default::default())
    };
    println!(
        "{}",
        sendito_import::curl::export(&h, &scope, include_secrets, &project.root)
    );
    ExitCode::SUCCESS
}

fn http_export(request: &Path, env: Option<String>, include_secrets: bool) -> ExitCode {
    let (project, rel, req) = match load(request, &env) {
        Ok(v) => v,
        Err((e, code)) => {
            eprintln!("sendito: {e}");
            return ExitCode::from(code);
        }
    };
    let Request::Http(h) = req else {
        eprintln!("sendito: .http export is only available for HTTP requests");
        return ExitCode::from(3);
    };
    let scope = if include_secrets {
        runner::build_scope(&project.envs, env.as_deref(), &Default::default()).0
    } else {
        runner::names_scope(&project.envs, env.as_deref(), &Default::default())
    };
    let name = sendito_store::collection::name_of(&rel).to_string();
    print!(
        "{}",
        sendito_import::http_file::export(&name, &h, &scope, include_secrets, &project.root)
    );
    ExitCode::SUCCESS
}

fn import(file: &Path, into: &str) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = sendito_store::init::find_root(&cwd).unwrap_or(cwd);
    let project = Project::open(&root, runner::default_keychain());
    let (format, result) = match sendito_import::import_path(file) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("sendito: {e}");
            return ExitCode::from(3);
        }
    };
    match sendito_import::write(&result, &project.coll, Some(&project.envs), into) {
        Ok(rep) => {
            println!(
                "imported {} requests in {} folders into {}/ ({} collection variables, {} environments; {})",
                rep.requests,
                rep.folders,
                rep.folder,
                rep.variables,
                rep.environments,
                format.label()
            );
            for name in &rep.variable_conflicts {
                println!("  collection variable {name:?} already exists; existing value kept");
            }
            for (original, renamed) in &rep.renamed_environments {
                println!("  environment {original:?} imported as {renamed:?}");
            }
            if !result.skipped.is_empty() {
                println!("\nnot imported or changed ({}):", result.skipped.len());
                for s in &result.skipped {
                    println!("  {}: {}", s.item, s.reason);
                }
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("sendito: {e}");
            ExitCode::from(3)
        }
    }
}

/// `sendito auth login|logout <name>`: OAuth for HTTP requests.
async fn http_auth(cmd: AuthCmd) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let Some(root) = sendito_store::init::find_root(&cwd) else {
        eprintln!("sendito: no collection here (no .sendito directory found upwards)");
        return ExitCode::from(3);
    };
    let project = Project::open(&root, runner::default_keychain());
    let (name, env, login) = match cmd {
        AuthCmd::Login { name, env } => (name, env, true),
        AuthCmd::Logout { name, env } => (name, env, false),
    };
    if let Some(e) = env.as_deref()
        && !project.envs.list().iter().any(|v| v == e)
    {
        eprintln!("sendito: environment not found");
        return ExitCode::from(3);
    }
    let account = runner::oauth_token_account(&project.name, env.as_deref(), &name);
    let keychain = project.envs.keychain.clone();
    if !login {
        return match keychain.delete(&account) {
            Ok(()) => {
                println!("forgot HTTP OAuth tokens for {name:?}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("sendito: {e}");
                ExitCode::from(2)
            }
        };
    }
    let Some(identity) = project.config.oauth.get(&name) else {
        eprintln!("sendito: OAuth identity {name:?} is not defined in [oauth]");
        return ExitCode::from(3);
    };
    let (scope, errors) = runner::build_scope(&project.envs, env.as_deref(), &Default::default());
    if !errors.is_empty() {
        eprintln!("sendito: cannot resolve environment secrets");
        return ExitCode::from(2);
    }
    let settings = match runner::oauth_settings(identity, &scope) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sendito: {e}");
            return ExitCode::from(3);
        }
    };
    let opener = |url: &str| -> bool {
        let program = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        std::process::Command::new(program).arg(url).spawn().is_ok()
    };
    let note = |n: String| eprintln!("{n}");
    match sendito_mcp::oauth::login_http(
        &settings,
        std::time::Duration::from_secs(300),
        &opener,
        &note,
    )
    .await
    {
        Ok(tokens) => match keychain.set(&account, &tokens.to_json()) {
            Ok(()) => {
                println!("signed in to {name:?} (tokens in the OS keychain)");
                ExitCode::SUCCESS
            }
            Err(_) => {
                eprintln!("sendito: cannot store OAuth tokens in the keychain");
                ExitCode::from(2)
            }
        },
        Err(_) => {
            eprintln!(
                "sendito: HTTP OAuth login failed; check the endpoints and authorization response"
            );
            ExitCode::from(2)
        }
    }
}

async fn mcp(cmd: McpCmd) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let Some(root) = sendito_store::init::find_root(&cwd) else {
        eprintln!("sendito: no collection here (no .sendito directory found upwards)");
        return ExitCode::from(3);
    };
    let project = Project::open(&root, runner::default_keychain());
    for w in &project.warnings {
        eprintln!("warning: {w}");
    }
    match cmd {
        McpCmd::Logout { server } => {
            let account = runner::mcp_token_account(&project.name, &server);
            match project.envs.keychain.delete(&account) {
                Ok(()) => {
                    println!("forgot the tokens of {server:?}");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("sendito: {e}");
                    ExitCode::from(2)
                }
            }
        }
        McpCmd::Login { server } => {
            let Some(sendito_store::config::McpServer::Http(h)) =
                project.config.mcp.servers.get(&server)
            else {
                eprintln!("sendito: server {server:?} is not an HTTP entry of [mcp.servers]");
                return ExitCode::from(3);
            };
            let settings = match &h.auth {
                Some(sendito_store::config::McpServerAuth::Oauth {
                    client_id,
                    client_secret,
                    authorize_url,
                    token_url,
                    registration_url,
                    scopes,
                }) => sendito_mcp::oauth::OAuthSettings {
                    client_id: client_id.clone(),
                    client_secret: client_secret.clone(),
                    authorize_url: authorize_url.clone(),
                    token_url: token_url.clone(),
                    registration_url: registration_url.clone(),
                    scopes: scopes.clone(),
                },
                _ => {
                    eprintln!(
                        "sendito: server {server:?} has no [mcp.servers.{server}.auth] type = \"oauth\""
                    );
                    return ExitCode::from(3);
                }
            };
            let url = h.url.clone();
            let account = runner::mcp_token_account(&project.name, &server);
            let keychain = project.envs.keychain.clone();
            let opener = |url: &str| -> bool {
                let program = if cfg!(target_os = "macos") {
                    "open"
                } else {
                    "xdg-open"
                };
                std::process::Command::new(program).arg(url).spawn().is_ok()
            };
            let note = |n: String| eprintln!("{n}");
            match sendito_mcp::oauth::login(
                &url,
                &settings,
                std::time::Duration::from_secs(300),
                &opener,
                &note,
            )
            .await
            {
                Ok(tokens) => match keychain.set(&account, &tokens.to_json()) {
                    Ok(()) => {
                        println!("signed in to {server:?} (tokens in the OS keychain)");
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("sendito: cannot store the tokens: {e}");
                        ExitCode::from(2)
                    }
                },
                Err(e) => {
                    eprintln!("sendito: login failed: {e}");
                    ExitCode::from(2)
                }
            }
        }
    }
}
