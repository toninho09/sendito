//! `sendito auth` and `sendito mcp`: sign-in and token commands.

use crate::*;

/// `sendito auth login|logout <name>`: OAuth for HTTP requests.
pub(crate) async fn http_auth(cmd: AuthCmd) -> ExitCode {
    let root = match collection_root() {
        Ok(root) => root,
        Err(code) => return code,
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
    let settings = match runner::oauth_settings(identity, &scope, project.oauth_network()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sendito: {e}");
            return ExitCode::from(3);
        }
    };
    let note = |n: String| eprintln!("{n}");
    match sendito_mcp::oauth::login_http(
        &settings,
        std::time::Duration::from_secs(300),
        &runner::open_browser,
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

pub(crate) async fn mcp(cmd: McpCmd) -> ExitCode {
    let root = match collection_root() {
        Ok(root) => root,
        Err(code) => return code,
    };
    let project = Project::open(&root, runner::default_keychain());
    for w in &project.warnings {
        eprintln!("warning: {w}");
    }
    let info = runner::McpRunInfo::of(&project);
    match cmd {
        McpCmd::Logout { server } => match runner::mcp_logout(&info, &server).await {
            Ok(()) => {
                println!("forgot the tokens of {server:?}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("sendito: {e}");
                ExitCode::from(2)
            }
        },
        McpCmd::Login { server, env } => {
            if let Some(e) = env.as_deref()
                && !project.envs.list().iter().any(|v| v == e)
            {
                eprintln!("sendito: environment not found");
                return ExitCode::from(3);
            }
            let (scope, errors) =
                runner::build_scope(&project.envs, env.as_deref(), &Default::default());
            if !errors.is_empty() {
                eprintln!("sendito: cannot resolve environment secrets");
                return ExitCode::from(2);
            }
            let note = |n: String| eprintln!("{n}");
            match runner::mcp_login(&info, &server, scope, &runner::open_browser, &note).await {
                Ok(()) => {
                    println!("signed in to {server:?} (tokens in the OS keychain)");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("sendito: {e}");
                    ExitCode::from(2)
                }
            }
        }
    }
}
