//! The command-line arguments.

use crate::*;

#[derive(Parser)]
#[command(
    name = "sendito",
    version,
    about = "Terminal API client for HTTP, gRPC, WebSocket and MCP tools",
    args_conflicts_with_subcommands = true
)]
pub(crate) struct Cli {
    /// Collection directory (default: nearest `.sendito/` above the current directory, else the current directory).
    pub(crate) path: Option<PathBuf>,
    #[command(subcommand)]
    pub(crate) cmd: Option<Cmd>,
    /// Exit after drawing the first frame (startup benchmark).
    #[arg(long, hide = true)]
    pub(crate) exit_after_first_frame: bool,
    /// Write per-frame render times (µs) to this file.
    #[arg(long, hide = true)]
    pub(crate) frame_log: Option<PathBuf>,
    /// Replay scripted input from this file.
    #[arg(long, hide = true)]
    pub(crate) script: Option<PathBuf>,
    /// Print KEYMAP.md to stdout.
    #[arg(long, hide = true)]
    pub(crate) print_keymap: bool,
}

#[derive(Subcommand)]
pub(crate) enum Cmd {
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
    /// Import a Postman, Insomnia or Bruno collection, an OpenAPI / Swagger
    /// document (JSON or YAML) or a `.http` file.
    Import {
        /// JSON export file, or a Bruno collection directory.
        file: PathBuf,
        /// Folder (relative to the collection) to import into.
        #[arg(long, default_value = "")]
        into: String,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
pub(crate) enum RunReport {
    Text,
    Json,
}

/// The `sendito run` flags, shared by the one-request and folder paths.
#[derive(Clone)]
pub(crate) struct RunFlags {
    pub(crate) env: Option<String>,
    pub(crate) include: bool,
    pub(crate) allow_unresolved: bool,
    pub(crate) no_history: bool,
    pub(crate) no_cookies: bool,
    pub(crate) fail_fast: bool,
    pub(crate) report: RunReport,
}

#[derive(Subcommand)]
pub(crate) enum AuthCmd {
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
pub(crate) enum McpCmd {
    /// Run the OAuth login for a server from `[mcp.servers]` (opens a browser;
    /// the URL is also printed, for remote sessions).
    Login {
        server: String,
        /// Environment whose variables render the URL and OAuth settings.
        #[arg(long, short)]
        env: Option<String>,
    },
    /// Forget the stored tokens of a server.
    Logout { server: String },
}

#[derive(Subcommand)]
pub(crate) enum ThemeCmd {
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
