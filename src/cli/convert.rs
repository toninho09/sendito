//! Import and export: collections, curl and `.http` files.

use crate::*;

/// The HTTP request to export, with its folders applied, and the scope to
/// render it with (secret values only with `include_secrets`).
fn export_input(
    request: &Path,
    env: &Option<String>,
    include_secrets: bool,
    what: &str,
) -> Result<(Project, String, HttpRequest, Scope), ExitCode> {
    let (project, rel, req) = load(request, env).map_err(|(e, code)| {
        eprintln!("sendito: {e}");
        ExitCode::from(code)
    })?;
    let mut scope = if include_secrets {
        runner::build_scope(&project.envs, env.as_deref(), &Default::default()).0
    } else {
        runner::names_scope(&project.envs, env.as_deref(), &Default::default())
    };
    let chain = project.coll.chain(&rel).map_err(|e| {
        eprintln!("sendito: {e}");
        ExitCode::from(3)
    })?;
    let Request::Http(h) = runner::with_folders(&req, &chain, &mut scope) else {
        eprintln!("sendito: {what} export is only available for HTTP requests");
        return Err(ExitCode::from(3));
    };
    Ok((project, rel, h, scope))
}

pub(crate) fn curl(request: &Path, env: Option<String>, include_secrets: bool) -> ExitCode {
    let (project, _, h, scope) = match export_input(request, &env, include_secrets, "curl") {
        Ok(v) => v,
        Err(code) => return code,
    };
    println!(
        "{}",
        sendito_import::curl::export(&h, &scope, include_secrets, &project.root)
    );
    ExitCode::SUCCESS
}

pub(crate) fn http_export(request: &Path, env: Option<String>, include_secrets: bool) -> ExitCode {
    let (project, rel, h, scope) = match export_input(request, &env, include_secrets, ".http") {
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

pub(crate) fn import(file: &Path, into: &str) -> ExitCode {
    let root = match collection_root() {
        Ok(root) => root,
        Err(code) => return code,
    };
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
