# Changelog

## Unreleased — documentation

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

## Unreleased — folder runs

* **`sendito run <folder>`** (or `.` for the current folder, which at the
  collection root is the whole collection) sends every request below it in
  tree order and prints one `PASS`/`FAIL`/`ERROR` line per request plus a
  summary. Variables flow forward: a value extracted by one
  request is available to the next, so a login and its dependent calls can run
  as one command in CI. `--fail-fast`, `--report json` (structured summary on
  stdout) and aggregated exit codes (0/1/2/3) are new; sending a single request
  is unchanged. `docs/DEBT.md` notes the limits (no parallelism, no JUnit).

## Unreleased — custom CA and mutual TLS

* **`[http.tls]`**: `ca` replaces the platform roots with your own PEM file
  (private CAs), and `client_cert` + `client_key` present a client certificate
  for mutual TLS — used by HTTP requests, gRPC connections and MCP servers
  over Streamable HTTP. Paths are relative to the collection root (`~/`
  expands); a `client_cert` without `client_key` (or the reverse) warns at
  startup. Nothing changes for collections without the table, and a request's
  `verify_tls = false` still skips server verification entirely.

## Unreleased — proxy

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

## Unreleased — response decoding and watch

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

## Unreleased — MCP tools

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

## 0.2.0 — visual redesign

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
