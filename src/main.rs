//! `sendito` — terminal API client for HTTP and gRPC.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use sendito_core::Request;
use sendito_ui::runner::{self, GrpcUpdate, Project, RunCtx};

#[derive(Parser)]
#[command(
    name = "sendito",
    version,
    about = "Terminal API client for HTTP and gRPC",
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
    /// Send one request without the UI and print the response.
    ///
    /// The body goes to stdout; status, timing and assertion results go to stderr.
    /// Exit codes: 0 success, 1 failed assertion, 2 request failed, 3 usage error.
    Run {
        /// Request file, relative to the collection or the current directory (`.toml` optional).
        request: PathBuf,
        /// Environment to use.
        #[arg(long, short)]
        env: Option<String>,
        /// Print response headers to stderr.
        #[arg(long, short = 'i')]
        include: bool,
        /// Send even if some variables are unresolved.
        #[arg(long)]
        allow_unresolved: bool,
        /// Do not record the request in history.
        #[arg(long)]
        no_history: bool,
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
    /// Import a Postman v2.1 collection into the collection.
    Import {
        /// Postman collection JSON file.
        file: PathBuf,
        /// Folder (relative to the collection) to import into.
        #[arg(long, default_value = "")]
        into: String,
    },
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
        }) => rt.block_on(run(&request, env, include, allow_unresolved, no_history)),
        Some(Cmd::Curl {
            request,
            env,
            include_secrets,
        }) => curl(&request, env, include_secrets),
        Some(Cmd::Import { file, into }) => import(&file, &into),
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

fn load(request: &Path, env: &Option<String>) -> Result<(Project, String, Request), (String, u8)> {
    let (root, rel) = locate(request).map_err(|e| (e, 3))?;
    let project = Project::open(&root, runner::default_keychain());
    for w in &project.warnings {
        eprintln!("warning: {w}");
    }
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
    let req = project.coll.load(&rel).map_err(|e| (e.to_string(), 3))?;
    Ok((project, rel, req))
}

async fn run(
    request: &Path,
    env: Option<String>,
    include: bool,
    allow_unresolved: bool,
    no_history: bool,
) -> ExitCode {
    let (project, rel, req) = match load(request, &env) {
        Ok(v) => v,
        Err((e, code)) => {
            eprintln!("sendito: {e}");
            return ExitCode::from(code);
        }
    };
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
    let envs = project.envs.clone();
    let env2 = env.clone();
    let (scope, errors) = tokio::task::spawn_blocking(move || {
        runner::build_scope(&envs, env2.as_deref(), &Default::default())
    })
    .await
    .unwrap_or_default();
    for e in &errors {
        eprintln!("warning: {e}");
    }
    let ctx = RunCtx {
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
        name: sendito_store::collection::name_of(&rel).to_string(),
        path: Some(rel.clone()),
        env: env.clone(),
        schemas: Default::default(),
        grpc_deadline_ms: project.config.grpc.deadline_ms,
        mask_echoed_secrets: false,
    };
    let mut out = std::io::stdout().lock();
    match req {
        Request::Http(h) => {
            let o = runner::exec_http(&ctx, &h, scope, errors).await;
            for n in &o.resolved.notes {
                eprintln!("note: {n}");
            }
            let code = match &o.response {
                Ok(r) => {
                    eprintln!(
                        "{} {} {}  ({} ms, {})",
                        r.version,
                        r.status,
                        r.reason,
                        r.timing.total.as_millis(),
                        sendito_ui::app::human_size(o.body_size)
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
    }
}

fn print_scripts(s: &runner::ScriptResults) -> usize {
    for e in &s.extracts {
        match &e.value {
            Ok(v) => eprintln!(
                "set {} = {}",
                e.var,
                if v.secret {
                    sendito_ui::MASK.to_string()
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

fn import(file: &Path, into: &str) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = sendito_store::init::find_root(&cwd).unwrap_or(cwd);
    let project = Project::open(&root, runner::default_keychain());
    let json = match std::fs::read_to_string(file) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("sendito: {}: {e}", file.display());
            return ExitCode::from(3);
        }
    };
    let result = match sendito_import::postman::import(&json) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("sendito: {e}");
            return ExitCode::from(3);
        }
    };
    match sendito_import::write(&result, &project.coll, Some(&project.envs), into) {
        Ok(rep) => {
            println!(
                "imported {} requests in {} folders into {}/ ({} collection variables)",
                rep.requests, rep.folders, rep.folder, rep.variables
            );
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
