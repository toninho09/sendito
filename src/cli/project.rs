//! Finds, creates and loads a collection and its environment.

use crate::*;

pub(crate) fn find_collection(path: Option<&Path>) -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match path {
        Some(p) => p.to_path_buf(),
        None => sendito_store::init::find_root(&cwd).unwrap_or(cwd),
    }
}

pub(crate) fn init(path: Option<PathBuf>) -> ExitCode {
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
pub(crate) fn locate(request: &Path) -> Result<(PathBuf, String), String> {
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

pub(crate) fn check_env(project: &Project, env: &Option<String>) -> Result<(), (String, u8)> {
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

pub(crate) fn load(
    request: &Path,
    env: &Option<String>,
) -> Result<(Project, String, Request), (String, u8)> {
    let (root, rel) = locate(request).map_err(|e| (e, 3))?;
    load_at(root, rel, env)
}

/// Opens the collection of a located request and loads it.
pub(crate) fn load_at(
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
pub(crate) fn load_suite(
    request: &Path,
    env: &Option<String>,
) -> Result<(Project, String), (String, u8)> {
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

pub(crate) fn ctx_for(
    project: &Project,
    env: &Option<String>,
    rel: &str,
    no_history: bool,
    no_cookies: bool,
    schemas: runner::SchemaCache,
) -> RunCtx {
    let mut ctx = RunCtx::of(
        project,
        sendito_store::collection::name_of(rel).to_string(),
        Some(rel.to_string()),
    );
    if no_history {
        ctx.history = None;
    }
    ctx.cookie_jar &= !no_cookies;
    RunCtx {
        env: env.clone(),
        schemas,
        ..ctx
    }
}

/// The loaded scope, its errors and the names whose value did not load.
pub(crate) async fn scope_of(
    project: &Project,
    env: &Option<String>,
) -> (Scope, Vec<String>, Vec<String>) {
    let envs = project.envs.clone();
    let e2 = env.clone();
    tokio::task::spawn_blocking(move || {
        runner::build_scope_checked(&envs, e2.as_deref(), &Default::default())
    })
    .await
    .unwrap_or_default()
}

/// True when the argument names a directory (a folder run). Checked before
/// loading so a folder's own errors (unknown environment, missing folder) are
/// reported instead of "request not found".
pub(crate) fn is_folder_arg(request: &Path) -> bool {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if cwd.join(request).is_dir() {
        return true;
    }
    sendito_store::init::find_root(&cwd).is_some_and(|root| root.join(request).is_dir())
}

/// The collection that holds the working directory. Without one, prints the
/// error and returns the exit code: commands that write must not make a
/// collection of whatever directory they run in.
pub(crate) fn collection_root() -> Result<PathBuf, ExitCode> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    sendito_store::init::find_root(&cwd).ok_or_else(|| {
        eprintln!(
            "sendito: no collection here (no .sendito directory found upwards); run sendito init first"
        );
        ExitCode::from(3)
    })
}
