//! Import and export: collections, curl and `.http` files.

use crate::*;

/// The HTTP request to export, with its folders applied, and the scope to
/// render it with (secret values only with `include_secrets`).
fn export_input(
    request: &Path,
    env: &Option<String>,
    include_secrets: bool,
    what: &str,
) -> Result<(Project, String, HttpRequest, Scope, bool), ExitCode> {
    let (project, rel, req) = load(request, env).map_err(|(e, code)| {
        fail!("{e}");
        ExitCode::from(code)
    })?;
    let mut scope = if include_secrets {
        runner::build_scope(&project.envs, env.as_deref(), &Default::default()).0
    } else {
        runner::names_scope(&project.envs, env.as_deref(), &Default::default())
    };
    let chain = project.coll.chain(&rel).map_err(|e| {
        fail!("{e}");
        ExitCode::from(3)
    })?;
    let effective = runner::with_folders(&req, &chain, &mut scope);
    let sse = matches!(effective, Request::Sse(_));
    // Code for other tools does not read an event stream.
    let http = if sse && what == "code" {
        None
    } else {
        sendito_import::http_export_request(&effective, &scope)
    };
    let Some(h) = http else {
        let kinds = if what == "code" {
            "HTTP requests"
        } else {
            "HTTP and SSE requests"
        };
        fail!(
            "{what} export is only available for {kinds}, not {}",
            effective.badge()
        );
        return Err(ExitCode::from(3));
    };
    Ok((project, rel, h, scope, sse))
}

pub(crate) fn curl(request: &Path, env: Option<String>, include_secrets: bool) -> ExitCode {
    let (project, _, h, scope, sse) = match export_input(request, &env, include_secrets, "curl") {
        Ok(v) => v,
        Err(code) => return code,
    };
    let mut command = sendito_import::curl::export(&h, &scope, include_secrets, &project.root);
    if sse {
        command = command.replacen("curl", "curl -N", 1);
    }
    println!("{command}");
    ExitCode::SUCCESS
}

pub(crate) fn http_export(request: &Path, env: Option<String>, include_secrets: bool) -> ExitCode {
    let (project, rel, h, scope, _) = match export_input(request, &env, include_secrets, ".http") {
        Ok(v) => v,
        Err(code) => return code,
    };
    let name = sendito_store::collection::name_of(&rel).to_string();
    print!(
        "{}",
        sendito_import::http_file::export(&name, &h, &scope, include_secrets, &project.root)
    );
    ExitCode::SUCCESS
}

pub(crate) fn code(
    request: &Path,
    lang: sendito_import::codegen::Lang,
    env: Option<String>,
    include_secrets: bool,
) -> ExitCode {
    let (project, _, h, scope, _) = match export_input(request, &env, include_secrets, "code") {
        Ok(v) => v,
        Err(code) => return code,
    };
    print!(
        "{}",
        sendito_import::codegen::export(lang, &h, &scope, include_secrets, &project.root)
    );
    ExitCode::SUCCESS
}

/// `sendito har`: one request, or every HTTP request below a folder.
pub(crate) fn har(request: &Path, env: Option<String>, include_secrets: bool) -> ExitCode {
    let fail = |(e, code): (String, u8)| {
        fail!("{e}");
        ExitCode::from(code)
    };
    // A request file wins over a folder of the same name, as for `run`.
    let (project, rels) = if let Ok((root, rel)) = locate(request) {
        match load_at(root, rel, &env) {
            Ok((project, rel, _)) => (project, vec![rel]),
            Err(e) => return fail(e),
        }
    } else if is_folder_arg(request) {
        match load_suite(request, &env).and_then(|(project, folder)| {
            let rels = project
                .coll
                .requests_under(&folder)
                .map_err(|e| (e.to_string(), 3))?;
            Ok((project, rels))
        }) {
            Ok(v) => v,
            Err(e) => return fail(e),
        }
    } else {
        fail!("request {} not found", request.display());
        return ExitCode::from(3);
    };
    let scope = if include_secrets {
        runner::build_scope(&project.envs, env.as_deref(), &Default::default()).0
    } else {
        runner::names_scope(&project.envs, env.as_deref(), &Default::default())
    };
    let mut items: Vec<sendito_import::har::ExportItem> = Vec::new();
    for rel in &rels {
        let loaded = project
            .coll
            .load(rel)
            .and_then(|r| project.coll.chain(rel).map(|chain| (r, chain)));
        let (req, chain) = match loaded {
            Ok(v) => v,
            Err(e) => return fail((e.to_string(), 3)),
        };
        let mut rscope = scope.clone();
        match runner::with_folders(&req, &chain, &mut rscope) {
            Request::Http(h) => {
                let name = sendito_store::collection::name_of(rel).to_string();
                items.push((name, h, rscope));
            }
            other => eprintln!(
                "note: {rel}: a {} request is not in a HAR file",
                other.badge()
            ),
        }
    }
    if items.is_empty() {
        fail!("no HTTP requests to export");
        return ExitCode::from(3);
    }
    print!(
        "{}",
        sendito_import::har::export(&items, include_secrets, &project.root)
    );
    ExitCode::SUCCESS
}

pub(crate) fn import(file: &Path, into: &str) -> ExitCode {
    let root = match collection_root() {
        Ok(root) => root,
        Err(code) => return code,
    };
    let project = Project::open(&root, runner::default_keychain());
    let (format, result) = match sendito_import::import_path(file) {
        Ok(r) => r,
        Err(e) => {
            fail!("{e}");
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
            fail!("{e}");
            ExitCode::from(3)
        }
    }
}
