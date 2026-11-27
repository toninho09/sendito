//! `sendito` — terminal API client for HTTP, gRPC and MCP tools.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::{Parser, Subcommand, ValueEnum};
use sendito_core::vars::Scope;
use sendito_core::{HttpRequest, Request};
use sendito_pipeline::human_size;
use sendito_pipeline::runner::{self, GrpcUpdate, Project, RunCtx};

mod cli;
use cli::*;

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
            report_files,
            retry,
            repeat,
            delay_ms,
            prompts,
            data,
        }) => {
            let flags = RunFlags {
                env,
                include,
                allow_unresolved,
                no_history,
                no_cookies,
                fail_fast,
                report,
                report_files,
                retry,
                repeat,
                delay_ms,
                prompts,
                data,
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
        Some(Cmd::Code {
            request,
            lang,
            env,
            include_secrets,
        }) => code(&request, lang, env, include_secrets),
        Some(Cmd::Har {
            request,
            env,
            include_secrets,
        }) => har(&request, env, include_secrets),
        Some(Cmd::Import { file, into }) => import(&file, &into),
        Some(Cmd::Mcp { cmd }) => rt.block_on(mcp(cmd)),
        Some(Cmd::Auth { cmd }) => rt.block_on(http_auth(cmd)),
        Some(Cmd::Theme { cmd }) => theme(cmd),
        Some(Cmd::Completions { shell }) => completions(shell),
        Some(Cmd::Man) => man(),
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
