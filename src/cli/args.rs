//! The command-line arguments.

use crate::*;

#[derive(Parser)]
#[command(
    name = "sendito",
    version,
    about = "Terminal API client for HTTP, gRPC, WebSocket, SSE and MCP tools",
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
        /// Folder report on stdout: `text` (stderr lines only), `json`,
        /// `junit`, `tap` or `html`.
        #[arg(long, value_enum, default_value_t = RunReport::Text)]
        report: RunReport,
        /// Also write a folder report to a file: `FORMAT=PATH`, for example
        /// `junit=results.xml`. Repeat it for several formats.
        #[arg(long = "report-file", value_name = "FORMAT=PATH", value_parser = parse_report_file)]
        report_files: Vec<ReportFile>,
        /// Send a request again up to N times after an error or a failed
        /// assertion. Replaces each request's `[options] retry`.
        #[arg(long, value_name = "N")]
        retry: Option<u32>,
        /// Run the folder N times in a row (folder runs only).
        #[arg(long, value_name = "N", default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
        repeat: u32,
        /// Wait this many milliseconds between two requests (folder runs only).
        #[arg(long = "delay", value_name = "MS", default_value_t = 0)]
        delay_ms: u64,
        /// The value of a `$prompt` or `$select`: `LABEL=VALUE`. Repeat it
        /// for several labels. Without it, a terminal asks; else the
        /// placeholder is unresolved.
        #[arg(long = "prompt", value_name = "LABEL=VALUE", value_parser = parse_prompt)]
        prompts: Vec<(String, String)>,
        /// Run once per row of this data file: CSV with the variable names
        /// in the first line, or a JSON array of objects (`.json`).
        #[arg(long, value_name = "FILE")]
        data: Option<PathBuf>,
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
    /// Print code that sends an HTTP request with another tool: python
    /// (requests), fetch (Node.js), go (net/http), httpie or wget. Secret
    /// variables stay as {{placeholders}}.
    Code {
        request: PathBuf,
        /// The tool: python, fetch, go, httpie or wget.
        #[arg(long, short, value_parser = parse_lang)]
        lang: sendito_import::codegen::Lang,
        #[arg(long, short)]
        env: Option<String>,
        /// Include secret values in the output.
        #[arg(long)]
        include_secrets: bool,
    },
    /// Print a HAR 1.2 document (no responses) of a request, or of every HTTP
    /// request in a folder (`.` = current folder). Secret variables stay as
    /// {{placeholders}}.
    Har {
        /// Request file, or a folder to export recursively.
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
    /// document (JSON or YAML), a `.http` file or a HAR file.
    Import {
        /// JSON export or HAR file, or a Bruno collection directory.
        file: PathBuf,
        /// Folder (relative to the collection) to import into.
        #[arg(long, default_value = "")]
        into: String,
    },
    /// Print a shell completion script (bash, zsh, fish, elvish, powershell).
    Completions { shell: clap_complete::Shell },
    /// Print the manual page (roff) to stdout.
    #[command(hide = true)]
    Man,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
pub(crate) enum RunReport {
    Text,
    Json,
    Junit,
    Tap,
    Html,
}

/// A `--report-file FORMAT=PATH` value.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct ReportFile {
    pub(crate) format: RunReport,
    pub(crate) path: PathBuf,
}

fn parse_lang(s: &str) -> Result<sendito_import::codegen::Lang, String> {
    sendito_import::codegen::Lang::parse(s).ok_or_else(|| {
        format!(
            "unknown language {s:?} ({})",
            sendito_import::codegen::Lang::names()
        )
    })
}

fn parse_prompt(s: &str) -> Result<(String, String), String> {
    let (label, value) = s
        .split_once('=')
        .ok_or("expected LABEL=VALUE, for example OTP=123456")?;
    if label.trim().is_empty() {
        return Err("the LABEL before = is empty".into());
    }
    Ok((label.trim().to_string(), value.to_string()))
}

fn parse_report_file(s: &str) -> Result<ReportFile, String> {
    let (format, path) = s
        .split_once('=')
        .ok_or("expected FORMAT=PATH, for example junit=results.xml")?;
    let format = RunReport::from_str(format, true)
        .ok()
        .filter(|f| *f != RunReport::Text)
        .ok_or_else(|| format!("unknown report format {format:?} (json, junit, tap, html)"))?;
    if path.is_empty() {
        return Err("the PATH after = is empty".into());
    }
    Ok(ReportFile {
        format,
        path: PathBuf::from(path),
    })
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
    pub(crate) report_files: Vec<ReportFile>,
    pub(crate) retry: Option<u32>,
    pub(crate) repeat: u32,
    pub(crate) delay_ms: u64,
    /// `--prompt LABEL=VALUE` answers.
    pub(crate) prompts: Vec<(String, String)>,
    /// `--data FILE`: one run per row.
    pub(crate) data: Option<PathBuf>,
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
