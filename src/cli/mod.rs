//! The `sendito` commands, one module per command group.

/// Prints `sendito: <message>` to stderr and writes it to the debug log.
macro_rules! fail {
    ($($a:tt)*) => {
        $crate::cli::say("ERROR", "sendito: ", format_args!($($a)*))
    };
}

/// Prints `warning: <message>` to stderr and writes it to the debug log.
macro_rules! warn {
    ($($a:tt)*) => {
        $crate::cli::say("WARN", "warning: ", format_args!($($a)*))
    };
}

mod args;
mod ask;
mod auth;
mod convert;
mod meta;
mod mqtt;
mod project;
mod report;
mod run;
mod serve;
mod sse;
mod theme;

pub(crate) use args::*;
pub(crate) use auth::*;
pub(crate) use convert::*;
pub(crate) use meta::*;
pub(crate) use project::*;
pub(crate) use run::*;
pub(crate) use serve::{ServeFlags, serve};
pub(crate) use theme::*;

/// The body of [`fail!`] and [`warn!`].
pub(crate) fn say(level: &str, prefix: &str, msg: std::fmt::Arguments<'_>) {
    let msg = msg.to_string();
    eprintln!("{prefix}{msg}");
    sendito_store::debug_log::log(level, &msg);
}
