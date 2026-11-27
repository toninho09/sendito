//! Output about the command line itself: shell completions and the man page.

use clap::CommandFactory;

use crate::*;

/// Writes the completion script of `shell` to stdout.
pub(crate) fn completions(shell: clap_complete::Shell) -> ExitCode {
    let mut cmd = Cli::command();
    clap_complete::generate(shell, &mut cmd, "sendito", &mut std::io::stdout());
    ExitCode::SUCCESS
}

/// Writes the man page (roff) to stdout.
pub(crate) fn man() -> ExitCode {
    let page = clap_mangen::Man::new(Cli::command());
    match page.render(&mut std::io::stdout()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            fail!("{e}");
            ExitCode::from(2)
        }
    }
}
