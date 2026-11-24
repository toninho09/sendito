//! Import and export: collections, curl and `.http` files.

use crate::*;

pub(crate) fn curl(request: &Path, env: Option<String>, include_secrets: bool) -> ExitCode {
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

pub(crate) fn http_export(request: &Path, env: Option<String>, include_secrets: bool) -> ExitCode {
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
