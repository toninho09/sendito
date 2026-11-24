# Changelog

## Unreleased

### MCP sign-in from the app

* **Sign in when the server asks.** When an MCP call or the tool list gets a
  `401` with a Bearer challenge, or the saved sign-in is missing or expired,
  the app asks "Sign in now?". Sign in opens the browser, stores the tokens,
  and runs the call (or the tool list) again. A server without an auth table
  gets `type = "oauth"` written to its config file after the sign-in.
* **`:auth login|logout <server>`** signs in to or out of an MCP server in the
  app. An `[oauth]` identity with the same name wins.
* **Auth and Token columns** in the MCP servers editor (`Ctrl+W S`): none,
  bearer (with its token) or oauth. OAuth fields in the file stay as they are.
  A token without `{{variable}}` gets a warning on save.
* **`sendito mcp login <server> [--env <name>]`** renders `{{variables}}` in
  the server URL and the OAuth settings. Tokens are bound to the rendered URL.
* `sendito run` prints `sign in with sendito mcp login <server>` when an MCP
  server asks for a sign-in.

### Config fields in the interface

* **Settings chooses the file.** A "Save changes to" row writes to the global
  file or to the project file (`.sendito/config.toml`). A change saved to the
  global file while the project file sets the same key gets a warning.
* **Typed settings.** HTTP timeout, gRPC deadline, WebSocket and MCP timeouts,
  largest response body, history entries kept, watch interval, proxy, the TLS
  CA, client certificate and key, collection name and skipped directories.
  Enter opens a dialog; the value is checked before it is saved (a proxy URL,
  a PEM file that exists, a number in range). The watch interval is typed now
  instead of chosen from 5 / 10 / 30 / 60. The cookie jar is an on/off row.
* **Collection name warning.** It is always saved to the project file, and the
  dialog and the status say that keychain secrets must be set again.
* **MCP server details.** `Ctrl+O` in the servers table (`Ctrl+W S`) edits the
  env of a stdio server, or the headers, `verify_tls` and OAuth overrides of an
  HTTP server.
* **`:oauth`** edits the `[oauth.<name>]` identities in a table. A plain client
  secret is saved with a warning.
* **`:keymap`** edits `[keymap.all]`, `[keymap.conventional]` and
  `[keymap.vim]`. Saved keys apply at once.

### Command palette

* **Settings in the palette.** Every settings row is a "Setting: …" entry with
  its current value. Enter opens the settings dialog on that row.
* **Commands without `:`.** The search lists the `:commands` too. A command
  that needs an argument puts `:<command> ` in the input and waits for it.
  Rows that hold the typed text as written come first.
* **Pickers.** `:theme` lists the themes, `:auth login` and `:auth logout`
  list the OAuth identities and the MCP servers with OAuth, and `:mcpedit`
  lists the MCP servers and opens the details editor of the one you choose.
* **Cookie jar.** `:cookies` lists the cookies (values masked). Enter on a
  cookie deletes it after a question; the last row clears all of them.
* **New commands.** `:method`, `:bodytype`, `:authtype` and `:grpcmethod` open
  the picker of the active request. `:rescan` reads the collection from disk
  again. `:newenv` creates an environment.

### Streams and large responses

* **`[ui] stream_messages_kept`** (default 10000, also in Settings): a gRPC or
  WebSocket stream keeps this many messages in the response; the oldest go
  first. The list says "N older dropped" and numbers every message; the
  counts, sizes and timing still cover all of them. The runner no longer
  keeps every event of a session in memory.
* Each stream message costs the same however long the stream is (the size,
  the timing and the live message were recomputed from every event).
* The response viewer draws long and wrapped lines, search matches and the
  JSON path of the cursor without going through the whole document on each
  frame. A growing log wraps only its new lines.
* Undo keeps at most 32 MB of copies per text, in addition to 200 steps.
* `{{variables}}` that reference each other many times render once per name;
  one render stops at 1 MB.
* Extract and assert rules parse the response body once per run.

### Redirects

* **Redirects are followed with the cookie jar enabled.** Each hop gets the
  jar's `Cookie` header for its own URL, and the cookies a redirect response
  sets are stored before the next hop (they were not followed, and a note said
  so).
* **File bodies are sent while they are read** (a request body `< file` and
  multipart file parts), not loaded into memory first. A 307 or 308 redirect
  sends the file again. The bytes on the wire are the same.

### Sidebar

* **Pinned folders.** While the tree scrolls, the folders of the top row stay
  pinned above the list, at most half its height. A click on a pinned folder
  selects it and scrolls the tree to it. The "Pin folders while scrolling"
  setting (`ui.sticky_folders`, on by default) turns it off.

### Fixes

* **A request with a comment after `kind` no longer disappears** from the tree
  and from `sendito run` (`kind = "http" # main`, or a quoted key).
* **Filtering the tree by some non-Latin names no longer crashes** the app.
* **The 401 retry with OAuth stays inside the request timeout**: the first
  send, the token refresh and the retry share it. The error says why the token
  or the refresh failed.
* **OAuth token, discovery and registration requests use `[http] tls` and
  `[http] proxy`**, like the API requests.
* **Two requests that refresh the same OAuth token at once** spend the refresh
  token once; the second uses the new token.
* **An MCP stdio server that prints a line that is not JSON-RPC** to stdout no
  longer breaks the call; the line shows as a log (`stdout: …`). Lines are
  limited to 100 MB (stdout) and split at 64 KB (stderr logs). When a call
  ends, the server's stdin is closed and it gets 0.5 s to exit before it is
  killed.
* **MCP Streamable HTTP**: a keep-alive before the reply in one chunk no longer
  reads as a closed stream; bodies are limited to 100 MB; an SSE body is
  searched once per byte.
* **`sendito import` outside a collection exits with code 3** and writes
  nothing (it made a collection of the current directory).
* Folder paths with `.` or `..` parts are refused (create, move, import).
* The import report dialog is as tall as its wrapped text.
* A search that stops at 100000 matches says so (`match 1/100000+`), and case
  folding is the same on every line.
* **Saves are durable**: request, environment, config, secrets, session and
  cookie files are written to a unique temporary file, flushed, and renamed;
  the directory is synced. History appends are locked across processes and
  written in one piece, so a trim does not drop another process's entry.
  Secret and config edits are locked across processes too, and `secrets.toml`
  keeps values it does not write.
* **A session file that does not parse is kept** as `session.toml.corrupt`
  (with a warning) instead of being overwritten.
* A config file with a literal OAuth `client_secret` is written owner-only.

## 0.2.0 — 2026-11-24

### Response query bar and body type chips

* **Query bar instead of the jq and search dialogs.** `j` (vim `J`) and `/`
  open a bar on the first row of the response view. The body follows the text
  as you type; the bar shows `2 results`, `match 3/12` or the jq error, and
  the body keeps the last valid result. Enter keeps the text, Esc goes back to
  it, an empty text closes the bar. A body larger than 1 MB is filtered on
  Enter. The `jq: <expr>` label in the bottom border shows only while the bar
  is closed.
* **jq key completion and history.** In the jq bar, Tab completes an object
  key after `.` from the keys in the body; Up/Down walk the expressions used
  in the tab.
* **Pretty view for jq output.** A filter with several outputs shows each
  value formatted, with folding (before: raw text, one value per line).
* **JSON path at the cursor.** The bottom border of the pretty body shows the
  path of the current line (`.items[0].email`). `f` filters the body to that
  value, `Y` copies the path.
* **Stream message list.** gRPC streams and WebSocket sessions with two or
  more received messages show a list next to the body; `[`/`]` or a click
  change the message, and the last one follows new messages (live). Before,
  the body showed only the last message, and each new message removed the jq
  filter.
* **Body type chips.** The HTTP Body tab shows all body types as chips with the
  `Content-Type` the body adds on the right. A click on the `✗ JSON` error in
  the editor's border moves the cursor to it.

### URL card

* **The URL strip is a card.** A border in the colour of the request kind
  (the HTTP method, or gRPC, WebSocket, MCP) holds the endpoint; its title
  names the protocol (`HTTP`, `GRPC`, `WS`, `MCP`), shows `●` while the request is unsaved and `↻ 5 s` while a
  watch runs. A screen with fewer than 16 rows for the panes keeps one row.
* **One button for Send and Cancel.** It shows `▶ Send ^⏎` while nothing
  runs and `■ Cancel ⠋ 1.2s` during a call (a click or Enter cancels). An open
  stream keeps `▶ Send message` and adds a small `■` that stops it. The
  always-visible, dim Cancel button is gone.
* **Variable preview.** Without focus, the endpoint shows what its
  `{{variables}}` become (`→ http://localhost:8080/users`), dim, or
  `→ undefined: name` in red. Secrets and `$dynamic` values stay as written.
* The gRPC method and MCP tool chips are as wide as their text (`▾` next to
  it). The file path moved to the bottom border of the Request pane, so the
  top border holds only the sub-tabs. A WebSocket request's empty URL shows
  `ws://host:port/path`.

### Footer and Ctrl+W menu

* **One footer row.** The hint row and the status line are now one row: status
  segments on the left, key hints on the right. The panes get the row back.
  Key names are short (`^S`, `⏎`, `⇧Tab`), and the hints end with
  `^W menu · F1 all keys`. A note replaces the hints for a few seconds.
* **Ctrl+W menu.** After `Ctrl+W`, a box lists every key that can follow, by
  group, built from the keymap (your overrides show). A click on an entry runs
  it. The old fixed line ("h/j/k/l focus · n/p tab …") is gone; it was
  incomplete and also showed for vim prefixes such as `g`.
* **`[ui] hints`** (`full`, `short`, `off`; also in Settings) sets how many
  hints the footer shows. The default is `short`.

### Review fixes

* **The MCP server editor and `:mcpadd` write only the project's servers.**
  They used to copy every server of the merged config, the global file's
  included (with any literal token), into the versioned project file. While
  the project config has an error they write nothing (they used to read no
  servers and then delete them all).
* **A secret that did not load stops the send.** In the UI the send asks first
  (and a watch stops), with its own "Send anyway"; `sendito run` exits `3`. This
  holds also when the collection defines the same name (whose value used to be
  sent instead), when the name is used inside another variable's value, when
  the active environment file cannot be read, and for the MCP server
  configuration a tool call uses (URL, headers, bearer token, command,
  arguments, environment).
* **No secret leaks in output.** Assertion results (`actual` and the label) are
  masked in every protocol, not only HTTP and WebSocket; a gRPC failure message
  (a connect error names the resolved target) and a WebSocket close reason are
  masked before history and the UI see them; the request as sent (Headers tab,
  the WebSocket "connecting to" line, the gRPC "at" line in the UI and in
  `sendito run`) masks a secret in the URL or a header; WebSocket handshake
  `Set-Cookie` values are masked as on HTTP responses; a syntax error in `secrets.toml` names the line and the reason, not
  the line's text. `session.toml` and `history.jsonl` are written `0600`.
* **OAuth tokens stay with their server.** Stored tokens record the MCP server
  URL as configured, or the HTTP token endpoint; another server in the config
  gets no token and a "sign in again" message. A refresh whose token cannot be
  saved still serves the request. The login callback drops idle browser
  connections, and `http://[::1]` counts as loopback.
* **`sendito run`.** The body on a redirected stdout is byte-exact (the line
  break is added only on a terminal); a body cut at `max_body_mb` prints a
  warning; a secret that did not load counts as unresolved (exit `3`) instead of
  sending `{{name}}`; `--no-cookies` also applies to WebSocket requests.
* **curl import and export.** An option's value is never split (`-d -10`) or
  read as the URL (`-D file`, `-c jar`, `--socks5 h`, …; many more options are
  known); `--oauth2-bearer` imports bearer auth. Export quotes the method, and a
  typed text part as `name="value";type=…`, so curl never reads a file for a
  value that starts with `@` or `<`. `.http` export keeps header values on one
  line and warns about a body line that starts with `###`.
* **gRPC.** Loading the schema and connecting are bounded by `[http]
  timeout_ms` and stop on Cancel, so a server that never answers reflection
  cannot hang a run; a call that ends with no status is `UNKNOWN` (2), not OK.
* **WebSocket.** A write that the peer never reads ends at the deadline or the
  close timer instead of blocking the session; `ws://[::1]` works; a deadline
  after your own close no longer sends a second Close frame.
* **MCP.** A request from the server with the same id (such as `ping`) is no
  longer taken as the reply; blank lines on stdio are skipped; a configured
  `Authorization` header wins over the bearer or OAuth token (OAuth is then not
  needed).
* **Requests.** An auth header that a request header replaces is never sent,
  so its variables no longer count as unresolved (HTTP, gRPC, WebSocket); a
  multipart body removes every `Content-Type` row; gRPC and
  WebSocket targets match their scheme in any case (`HTTPS://`).
* **Files.** `sendito init` appends to a `.gitignore` that is not UTF-8 instead
  of replacing it; one bad line in `history.jsonl` no longer hides (and, at the
  next trim, deletes) the entries after it; a folder symlink that leads back up
  the tree is not followed, and a folder duplicate copies links as links; a
  setting changed in the UI keeps the comments of the config file; an
  environment cannot be named `runtime` (an import renames it).
* **Terminal UI.**
  * No crash on a non-ASCII method in a request file.
  * The environment editor keeps unsaved edits on `Ctrl+N` and on an outside
    click of the "New environment" prompt, and never writes over an environment
    file that cannot be read.
  * `:wq` on an unnamed request leaves the name prompt up.
  * Saving the response body saves the bytes received, not the jq output; a jq
    filter with endless output (`repeat(1)`) stops instead of freezing the UI.
  * The history list keeps its selection when new entries arrive, and saving an
    entry saves the one you chose.
  * A curl import that fails keeps the dialog and the pasted text.
  * `Ctrl+W x` cancels the request in view before background jobs.
  * The MCP server editor refuses two servers with one name.
  * New tabs get the lowest free "untitled N" name.
  * The Mouse setting turns capture on or off at once.
  * Closing the last tab shows the hidden sidebar so focus has a place.
  * Search and wrap keep the cursor on its line in the response viewer.
  * Control characters in a field are drawn as spaces, never sent to the
    terminal; a tab takes one column.
  * A paste into the environment or MCP editor counts as an edit (Esc asks);
    the environment editor refuses two variables with one name.
  * Watch repeats keep the response view: the last response stays until the
    next one arrives, and the view mode, wrap, jq filter and position carry
    over.
  * Completing inside an existing JSON key replaces the whole key and keeps
    its value.
  * `:curl!` and `:httpfile!` warn when a secret did not load.
  * Opening a WebSocket request focuses its URL, not the static badge.
  * Format JSON (`Ctrl+W f`) works on WebSocket messages; the MCP Args tab
    shows its "set" marker; picking another MCP tool replaces the previous
    tool's argument template.

### Hardening

* **Redirects to another origin no longer carry authentication.** A followed
  redirect that changes the scheme, host or port leaves `Authorization`,
  `Proxy-Authorization`, `Cookie`, `Cookie2`, `WWW-Authenticate` and the headers
  the Auth tab added behind; one that turns the request into a body-less `GET`
  also drops the payload headers. Same-origin redirects resend everything, and
  307/308 keep the method and body as before. Corpus case 48 (HTTP) checks the
  bytes; `FORMAT.md`, `docs/FEATURES.md` and the correctness reviewer's rules
  say so.
* **Secret files are private.** `.sendito/secrets.toml` and a secret that reads
  a file are written `0600` on Unix, and tightened to `0600` when a looser one
  is read.
* **Echo masking covers encoded copies.** A secret is masked in what a server
  sends back when it appears as sent, JSON-escaped, or percent-encoded as a form
  field or a query component (either hex case). `docs/DEBT.md` lists the
  encodings still not recognised.
* **The terminal is restored when the UI panics.** The panic hook leaves the
  alternate screen, raw mode, bracketed paste, the window title and mouse
  tracking before the message is printed.
* **New crate `sendito-pipeline`.** `runner` (the send pipeline the TUI and
  `sendito run` share) and the response document model move out of
  `sendito-ui`; `sendito_ui::runner` stays as a re-export. Line coverage now
  measures every crate but the UI and the testkit, by the whole test suite, so
  the pipeline and the CLI count (`make cov`, CI, `docs/reports/coverage.md`).
* **Toolchain and CI.** `rust-toolchain.toml` pins Rust 1.100.0 for CI, the
  release build and local runs; GitHub Actions are pinned to commit SHAs; the
  release workflow lints and tests before it builds; `cargo deny` warns on
  duplicate dependency versions (16 today); `cookie` is 0.18.3
  (RUSTSEC-2026-0594; Sendito does not fork or use private cookies). The theme
  flow test no longer assumes a true-color terminal.
* **Narrower commands and a documentation check.** `make check`, `make lint` and
  `make test` take `P=<package>` to run on one crate, and `make test` takes
  `F=<name>` to run the tests whose name contains it (it fails when none does).
  `make docs` runs `tests/docs.rs`, which fails when a path, a `file::symbol`,
  a `make` target or a link cited by `AGENTS.md`, `README.md`, `FORMAT.md` or
  `docs/` no longer exists. `AGENTS.md` sends each kind of change to the files
  to open and the section to read.

### WebSocket

* **WebSocket requests** (`kind = "websocket"`): a session per request, like a
  gRPC call. The handshake carries the request's headers, subprotocols, auth and
  (optionally) the cookie jar; `wss://` uses `[http.tls]`; an `http(s)://`
  `[http] proxy` is tunnelled with `CONNECT` (an `https://` proxy over its own
  TLS). A JSON document of strings and `{ "binary": "base64" }` objects (with
  `{{variables}}` rendered) is sent on connect, and on an open session Send
  pushes one more frame while `Ctrl+W d` starts the close handshake. Frames
  arrive live in the Log tab; the Handshake and Timing tabs report the upgrade
  response and the frame counters, the last text frame is the Body, and scripts
  see the close code as `status`. New crate `sendito-ws` (tokio-tungstenite), a
  WebSocket echo server in `sendito-testkit`, 12 corpus cases checked byte for
  byte, and `sendito run` support (text frames on stdout, exit `2` unless the
  session closes with `1000`). `curl ws://…` is refused with a pointer to
  `:newws`; SOCKS proxies and `proxy = "env"` are refused for sessions
  (`docs/DEBT.md`).

### jq response filter

* **jq filtering of the response body**: `j` (conventional) / `J` (vim) or
  `:jq [expr]` filters the JSON body with a real jq expression (the `jaq`
  library): paths, pipes, `select`, `map`, slicing, arithmetic, regex — for
  HTTP, gRPC messages and MCP results alike. The filter is display-only:
  assertions, extracts, history and re-sends always see the full body, an
  empty expression restores it, and a new response clears it. The body
  border shows `jq: <expr>` while a filter is active; several outputs print
  one JSON value per line as raw text (no pretty view). `halt`, `halt_error`
  and `stderr` are removed
  from the builtin set (they would exit the process or write to the
  terminal). The new dependency raises the minimum Rust version to 1.89
  (README and DEBT updated). The wire is unchanged.

### Completion

* **Autocomplete for gRPC and MCP.** The message editor (`Message` for gRPC, `Args`
  for MCP) suggests the object keys at the cursor — from the proto descriptors or
  from the tool's `inputSchema` — aware of the nesting level and skipping keys that
  are already present; the gRPC method field (now editable, `Service/Method`) and the
  MCP server and tool fields complete their names too. It opens on `Ctrl+Space` or by
  itself at a key position, filters as you type, inserts with `Enter`/`Tab` (inside
  the new value) and closes with `Esc` (or when the cursor or the focus moves
  away, or a dialog opens). A schema or tool list that is not loaded yet is fetched in the
  background; a load that fails is reported in the popup and the status line and
  is only retried when asked. `KEYMAP.md` gains the `complete` action; the wire is
  unchanged.

### Documentation

* **Rewritten documentation.** The README is now a showcase with the hero image,
  a gallery of the twelve flow recordings (previously never referenced) and
  screenshots in each feature section. New reference pages cover the whole
  surface: `docs/FEATURES.md` (every feature), `docs/CLI.md` (every subcommand,
  flag, exit code and the JSON report), `docs/ARCHITECTURE.md` (crates, the send
  pipeline, the test layers) and `docs/README.md` (the index). `docs/media/README.md`
  documents the screenshot/GIF pipeline. The previously undocumented `-i`,
  `--no-history`, `--no-cookies`, `--allow-unresolved`, `--include-secrets`,
  `--into`, `theme --collection` and the hidden benchmark flags are now written
  down, and stale claims were corrected (the non-existent `vendor/` license
  note, the coverage figure, the crate descriptions, the settings table and the
  VHS note).

### Folder runs

* **`sendito run <folder>`** (or `.` for the current folder, which at the
  collection root is the whole collection) sends every request below it in
  tree order and prints one `PASS`/`FAIL`/`ERROR` line per request plus a
  summary. Variables flow forward: a value extracted by one
  request is available to the next, so a login and its dependent calls can run
  as one command in CI. `--fail-fast`, `--report json` (structured summary on
  stdout) and aggregated exit codes (0/1/2/3) are new; sending a single request
  is unchanged. `docs/DEBT.md` notes the limits (no parallelism, no JUnit).

### Custom CA and mutual TLS

* **`[http.tls]`**: `ca` replaces the platform roots with your own PEM file
  (private CAs), and `client_cert` + `client_key` present a client certificate
  for mutual TLS — used by HTTP requests, gRPC connections and MCP servers
  over Streamable HTTP. Paths are relative to the collection root (`~/`
  expands); a `client_cert` without `client_key` (or the reverse) warns at
  startup. Nothing changes for collections without the table, and a request's
  `verify_tls = false` still skips server verification entirely.

### Proxy

* **HTTP proxy (`[http] proxy`)**: HTTP requests and MCP servers over
  Streamable HTTP can go through a proxy — a URL such as
  `http://proxy.corp:8080` (also `https://`, `socks4://`, `socks4a://`,
  `socks5://`, `socks5h://`, credentials allowed in the URL) or `"env"` to
  read `HTTPS_PROXY` / `HTTP_PROXY` / `ALL_PROXY` / `NO_PROXY` at send time
  with curl's precedence. Unset keeps the client's default discovery: the same
  variables, `NO_PROXY` included (the OS proxy settings are not read).
  Nothing changes on the wire. gRPC does not use the proxy
  (docs/DEBT.md). A failed `-x`/`--proxy` curl import now points at the setting
  instead of ignoring it silently.

### Response decoding and watch

* **Automatic response decompression**: responses with `Content-Encoding: gzip`,
  `deflate` (zlib and raw) or `br` are decoded for display, extraction,
  assertions. Sendito still never sends `Accept-Encoding` — nothing
  changes on the wire; the body is decoded only when the server chose to
  compress. The response border shows the encoding and the wire size
  (`45.6 KB ← gzip 1.2 KB`), and `sendito run` notes it on stderr. Other codings
  or a corrupt stream are shown as received.
* **Watch (`Ctrl+W m`, `:watch [s]`)**: re-sends the active HTTP request every
  few seconds (`ui.watch_interval_s`, default 5, changeable in Settings), even
  when another tab is active. The status line shows `↻ 5 s`; the Timing tab
  gains a sparkline with one mark per send (`✗` for errors and status 400+).
  Cancelling, closing the tab, switching environment or leaving variables
  unresolved stops it. Watch repeats are not written to History.
* **`.http` export (`:httpfile`, `sendito http <request> --env <env>`)**: the
  request as a JetBrains HTTP client / VS Code REST Client file — method, URL,
  headers, JSON/raw bodies inline, file bodies as `< path`, multipart written
  with an explicit boundary. `# @timeout` and `# @no-redirect` directives carry
  the request options. Secret variables stay as `{{placeholders}}` unless
  `:httpfile!` / `--include-secrets`.
* **Cookie jar**: opt in per request or collection; cookies are isolated per
  environment, matched with public-suffix protection, and stored in a private,
  gitignored file. `:cookies` lists them without values; `:cookies delete <number>`
  or `:cookies clear` removes them. Redirects are not followed while enabled.
* **HTTP OAuth 2**: named identities use authorization code and PKCE, with tokens
  in the keychain isolated per environment. `sendito auth login|logout` and
  `:auth login|logout` manage them; 401 triggers one refresh and retry.
* **Bruno and Insomnia import**: folders, supported HTTP requests and selectable
  environments can be imported from a Bruno directory or an Insomnia v4/v5 JSON
  export. Unsupported data and secret environments are reported. Imported
  environments whose name is taken get a unique name, and existing collection
  variables are kept rather than overwritten; both are stated in the report.

### MCP tools

* **MCP requests**: a new `kind = "mcp"` request calls one tool on a server from
  `[mcp.servers]` in the collection config, over a child process (stdio) or
  Streamable HTTP. `sendito-mcp` speaks JSON-RPC 2.0 (`initialize`,
  `notifications/initialized`, `tools/list`, `tools/call`), accepts JSON and SSE
  responses, and follows list cursors.
* **Discovery**: F2 on the server field picks a configured server; F2 on the tool
  field lists the server's tools and, when the arguments are still empty or `{}`,
  fills a template from the tool's input schema.
* **Auth**: a static bearer (with `{{variables}}`) or OAuth 2.1 with PKCE,
  discovery and dynamic client registration. `sendito mcp login <server>` /
  `sendito mcp logout <server>` manage tokens in the OS keychain; a `401` refreshes
  and retries once.
* **CLI**: `sendito run` calls the tool and prints the pretty JSON result; exit 0
  on success, 1 on `isError`, 2 when the call fails. `sendito mcp login|logout`.
* **Scripts and history** work as for HTTP and gRPC; secrets echoed by a tool are
  masked in the UI, the body and the history.
* The wire-level corpus gained 5 MCP cases (JSON, SSE, bearer, empty arguments,
  `isError` and `tools/list` pagination) checked byte for byte.

### Visual redesign

The same features in a new layout, with more themes and room for your own.

* **Layout**: the URL has its own strip above both panes (method chip, highlighted
  `{{variables}}`, Send); pane tabs sit in the pane's top border with the request's
  folder path on the right; the bottom border shows `✓ JSON` / `✗ JSON: line, col`,
  `Ln, Col`, the view mode and the position; scrollbars are drawn in the border.
* **Top bar and status line**: open tabs as pills with a spinner while a request
  runs, an environment chip colored by name (prod red, staging/qa/test yellow,
  local/dev green), hints as key caps. The status line drops the `│` separators,
  shows the environment only in the top bar and the last request only while the
  Response pane is hidden; messages appear in a chip with an icon.
* **Themes**: two new built-in themes, `ember` (warm dark) and `linen` (warm light);
  the existing ones refined. **User themes** as TOML files in the config folder or
  the collection (`docs/THEMES.md`), extending another theme. `sendito theme list`,
  `sendito theme print <name>`, and `:theme <name>` / `:theme reload` in the palette.
* **Icons**: `ui.icons = "nerd"` (Settings → Icons) uses Nerd Font icons; the default
  `unicode` set works with any font.
* **Editors and viewers**: line numbers and a current-line highlight in editors,
  JSON validation as you type, line numbers in the response body, tables with
  zebra rows and `[✓]`, a waterfall chart in Timing, centered empty / waiting /
  error states.
* **Sidebar**: tree connectors, the filter match highlighted, request and folder
  counts in the border, history grouped by day with the status in color.
* **Dialogs** open over a dimmed screen with a shadow (`NO_COLOR` still clears it);
  the command palette highlights the matched letters; Help has colored sections and
  two columns on screens 180 columns wide or more; Settings is grouped and shows a
  color sample.
* **Start screen**: a wordmark, what the collection holds, the keys to get going and
  the most recent requests (clickable).
* Hover feedback for the mouse, a bar cursor while typing (block in vim NORMAL), and
  the window title shows the open request and the collection (restored on exit).
* Nothing else changes on the wire: the corpus of 44 HTTP and 15 gRPC requests is
  byte-for-byte the same, apart from the version in the gRPC `user-agent`
  (`sendito/0.2.0`).

## 0.1.0 — first release

A terminal API client for HTTP and gRPC.

* **Requests as files**: TOML request files in a collection folder (format in
  `FORMAT.md`), a request tree with create, rename, move (also by mouse drag), delete,
  and a filter.
* **HTTP**: all methods and custom ones, params, headers, JSON / raw / form /
  multipart / file bodies, Basic / Bearer / API key auth, redirects, TLS options,
  timeout, response viewer (pretty / raw / hex, JSON folding, search, save body) and
  a DNS / connect / TLS / first-byte timing breakdown.
* **gRPC**: schema from server reflection or `.proto` files, message templates, unary
  and streaming calls (several messages per stream), metadata, deadlines, a Log tab.
* **Environments** with plain, local-secret, OS keychain and file sources; secrets are
  masked in the editor, in echoed responses and in history; undefined variables are
  highlighted and confirmed before sending.
* **Scripts without code**: pre-request variables, extracted variables (JSONPath,
  header, status) and assertions, shown in the Results tab.
* **History** (open a copy, resend, save to the tree), **Postman v2.1 import** with a
  report of everything not imported as-is, **curl import and export**.
* **Interface**: command palette, vim mode, mouse support, two themes plus ansi and
  high-contrast, sidebar on either side, pane zoom, session restore, a key map you can
  change (`KEYMAP.md`).
* Built on the official `reqwest` and `hyper` crates. Two wire differences follow
  from that (`Accept: */*` is added when the request defines none; HTTP/1 header
  names are Title-Cased) and are listed with other known limits in `docs/DEBT.md`.
* Linux x86_64 and macOS arm64 builds. Terminal size from 120×40 is what the
  screenshots, recordings and tests cover.
* Verified against a wire-level corpus (44 HTTP and 15 gRPC requests whose exact
  bytes are compared), UI flow and snapshot tests, and two rounds of independent
  review; see `docs/reports/`.
