# Feature map

Where every feature lives in the code. [FEATURES.md](FEATURES.md) says what
Sendito does; [ARCHITECTURE.md](ARCHITECTURE.md) says how the pieces fit; this
document answers the narrow question a contributor asks first: **which file do I
open to change this?**

Paths are relative to the repository root. A `::` suffix names the item inside
the file, so you can jump straight to it.

## How to read this

* Start at the **entry point** column: it is where the behaviour is triggered.
* Then follow the **supporting** column for the pieces that do the work.
* Each section ends with the tests that guard it, because a feature without a
  test is usually a feature that is not really there.
* Cross-cutting behaviour (masking, resolution, configuration) is in
  [Shared code](#shared-code), not repeated per feature.

## Crates at a glance

| Crate | Directory | Owns | Entry point |
|---|---|---|---|
| `sendito` | `src/` | CLI parsing and every headless command | `src/main.rs` |
| `sendito-core` | `crates/core/` | Request model, templates, variables, resolution, scripts | `crates/core/src/lib.rs` |
| `sendito-http` | `crates/http/` | The HTTP client, TLS material, proxy, timing | `crates/http/src/lib.rs::send` |
| `sendito-grpc` | `crates/grpc/` | Dynamic gRPC calls and schema loading | `crates/grpc/src/lib.rs::start_call` |
| `sendito-ws` | `crates/ws/` | WebSocket handshake, frames and close | `crates/ws/src/lib.rs::open` |
| `sendito-mcp` | `crates/mcp/` | JSON-RPC client, transports, MCP OAuth | `crates/mcp/src/lib.rs::call_tool` |
| `sendito-store` | `crates/store/` | Everything on disk: collection, config, env, history, cookies, session, themes | `crates/store/src/lib.rs` |
| `sendito-import` | `crates/import/` | Postman, Insomnia, Bruno, curl, `.http` | `crates/import/src/lib.rs` |
| `sendito-ui` | `crates/ui/` | ratatui app, drawing, editors, key map, and the shared send pipeline | `crates/ui/src/lib.rs::run` |
| `sendito-testkit` | `crates/testkit/` | Echo servers, fake OAuth server, proxy, test PKI | `crates/testkit/src/lib.rs` |

## Request model and file format

The TOML schema is defined once in `core` and serialized once in `store`. The
model carries no serialization logic on purpose.

| What | File |
|---|---|
| `Request`, `HttpRequest`, `GrpcRequest`, `WebSocketRequest`, `McpRequest`, auth kinds, body kinds, rows | `crates/core/src/model.rs` |
| Parse and canonical write of `<name>.toml` | `crates/store/src/request_file.rs` |
| Canonical TOML emitter (stable key order, multi-line strings) | `crates/store/src/tomlfmt.rs` |
| Cheap `kind`/`method` sniff for tree badges | `crates/store/src/request_file.rs::sniff` |
| Schema and wire rules (prose) | [FORMAT.md](../FORMAT.md) |

Tests: `tests/integration.rs` (format stability — load/save every fixture
reproduces the file byte for byte).

## Shared code

Use these before writing a special case; most "new request option" work happens
here.

| Concern | File | Notes |
|---|---|---|
| `{{placeholder}}` parsing, name rules, leading `$` | `crates/core/src/template.rs` | Anything not matching is literal text |
| Variable layers and precedence, recursive expansion, masking, secret taint | `crates/core/src/vars.rs` | `Scope`, `Render`, `VarValue`, `scrub_bytes` |
| Dynamic values (`$uuid`, `$timestamp`, `$randomInt`, …) | `crates/core/src/vars.rs::uuid_v4` and the `$` table | Generated once per send |
| Request → wire conversion for all four kinds | `crates/core/src/resolve.rs` | `resolve_http`, `resolve_grpc`, `resolve_ws`, `resolve_mcp` |
| Query/form percent-encoding whitelists | `crates/core/src/resolve.rs::query_encode`, `form_urlencode` | The whitelist is a documented decision — see FORMAT.md |
| URL normalization (no scheme → `http://`), query append before fragment | `crates/core/src/resolve.rs::normalize_url`, `append_query` | |
| Unresolved variables, masked copy of a request | `crates/core/src/resolve.rs::unresolved_vars`, `mask_request` | |
| Declarative scripts: pre, extract, assert | `crates/core/src/script.rs` | No code execution |
| Completion shape of a message or tool input | `crates/core/src/complete.rs` | Shared by gRPC and MCP editors |
| Sent pipeline (scope → pre → resolve → send → extract/assert → history) | `crates/ui/src/runner.rs` | Used identically by the TUI and `sendito run` |

## HTTP

| What | Entry point | Supporting |
|---|---|---|
| Sending a resolved request | `crates/http/src/lib.rs::send` | `SendOptions`, `HttpResponse` |
| Header and body assembly, multipart boundary, file bodies | `crates/core/src/resolve.rs::resolve_http` | `ResolvedBody`, `ResolvedPart`, `PartData` |
| MIME guess for multipart file parts | `crates/http/src/lib.rs::guess_mime` | Used by resolve and by the request pane |
| Response decoding (gzip, deflate, br) and display size vs wire size | `crates/http/src/lib.rs::Decoded` | `wire_body_len`, `headers_size` |
| Timing breakdown (DNS, connect, TLS, TTFB, download) | `crates/http/src/lib.rs::Timing` | Rendered by `crates/ui/src/app.rs::timing_text` |
| Proxy (`[http] proxy`, URL or `"env"`) | `crates/http/src/lib.rs::ProxySpec` | `apply` builds the reqwest client |
| TLS material, custom CA, mutual TLS | `crates/http/src/lib.rs::tls_config`, `TlsMaterial` | `pair_error` implements the "cert without key warns" rule |
| Redirect handling | `crates/http/src/lib.rs::send` (`SendOptions`) | Loop bound documented in FORMAT.md |
| Cookie jar wiring during a send | `crates/ui/src/runner.rs` (`cookie_setup`) | Storage in `crates/store/src/cookies.rs` |
| jq response filter (display only) | `crates/ui/src/filter.rs::apply` | `crates/ui/src/tab.rs::apply_jq`; jaq with `halt`/`stderr` removed |
| Watch (repeat the active request) | `crates/ui/src/app.rs::toggle_watch` | `send_opts(…, from_watch)`, sparkline via `watch_text` |

Tests: `tests/corpus.rs` (47 HTTP cases under
`tests/corpus/collection/http/`), `tests/integration.rs`, `tests/tls.rs`,
`tests/proxy.rs`.

## Authentication

| What | Entry point | Supporting |
|---|---|---|
| Basic / bearer / API key resolution | `crates/core/src/resolve.rs::resolve_http` | Auth is part of the resolved request |
| HTTP OAuth identity (`[oauth.<name>]`) | `crates/ui/src/runner.rs::http_oauth_provider` | `oauth_settings`, `oauth_token_account` |
| OAuth login/logout for HTTP, CLI | `src/main.rs` (`auth` command) | `crates/mcp/src/oauth.rs::login_http` |
| OAuth login/logout in the UI | `crates/ui/src/app.rs::http_oauth_command` | `OAuthRunInfo` in `crates/ui/src/runner.rs` |
| MCP OAuth 2.1 (discovery, PKCE, registration, refresh) | `crates/mcp/src/oauth.rs::login` | `pkce`, `RefreshingProvider`, `well_known`, `validate_oauth_url` |
| Token storage keys | `crates/ui/src/runner.rs::oauth_token_account`, `mcp_token_account` | Keychain access in `crates/store/src/env.rs` |

Tests: `crates/mcp/tests/oauth.rs`, corpus case `http/47-oauth-http.toml`,
`tests/mcp_cli.rs`.

## gRPC

| What | Entry point | Supporting |
|---|---|---|
| All four call kinds over one code path | `crates/grpc/src/lib.rs::start_call` | `Call::{send, close_send, cancel}` |
| Schema from `.proto` (protox, in-process) or reflection | `crates/grpc/src/schema.rs` | `SchemaSource` (in `crates/core/src/model.rs`); v1 with v1alpha fallback |
| Message template and completion fields (incl. every `oneof` member) | `crates/grpc/src/schema.rs` | Consumed by `:template` and `crates/ui/src/complete.rs` |
| proto3 JSON ↔ protobuf | `crates/grpc/src/lib.rs::DynCodec` | `DynEncoder`, `DynDecoder` |
| Metadata (`-bin` base64, lower-cased keys) | `crates/grpc/src/lib.rs::metadata_map` | `metadata_pairs` for the response side |
| Deadline to `grpc-timeout`, TLS and ALPN | `crates/grpc/src/lib.rs::ConnSettings` | Built by `crates/ui/src/runner.rs::conn_settings` |
| Status-code naming | `crates/grpc/src/lib.rs::code_name` | All 17 codes |
| Streaming session in the UI | `crates/ui/src/runner.rs::{GrpcControl, GrpcUpdate}` | `crates/ui/src/app.rs` handles the events |

Tests: `crates/grpc/tests/calls.rs`, corpus cases under
`tests/corpus/collection/grpc/`.

## WebSocket

| What | Entry point | Supporting |
|---|---|---|
| Session: handshake, frames, close, deadline | `crates/ws/src/lib.rs::open`, `Session` | `Event`, `FrameKind`, `WsError` |
| Message document (strings and `{"binary": …}`) | `crates/ws/src/lib.rs::parse_messages` | Validated in the editor, tolerated as half-typed |
| Proxy tunnelling (`CONNECT`, `https://` proxy over TLS) | `crates/ws/src/lib.rs::WsProxy` | Reuses `sendito_http::ProxySpec` and `tls_config` |
| UI session control (`Ctrl+W d`, `Ctrl+W x`) | `crates/ui/src/runner.rs::{WsControl, WsUpdate}` | Response summary via `ws_summary` |

Tests: unit tests in `crates/ws/src/lib.rs`, corpus cases under
`tests/corpus/collection/ws/`, `tests/integration.rs` (cookie jar).

## MCP

| What | Entry point | Supporting |
|---|---|---|
| Session lifecycle (spawn/connect, initialize, work, drop) | `crates/mcp/src/lib.rs::call_tool`, `list_tools` | One timeout covers the whole call |
| Transport selection | `crates/mcp/src/lib.rs::Transport` | `crates/ui/src/runner.rs::mcp_transport` |
| stdio transport (NDJSON, stderr to the log) | `crates/mcp/src/stdio.rs` | Child killed on drop |
| Streamable HTTP transport (JSON and SSE bodies) | `crates/mcp/src/http.rs` | Session-id and protocol-version headers |
| JSON-RPC messages, protocol constants | `crates/mcp/src/protocol.rs` | `PROTOCOL_VERSION`, `SUPPORTED_VERSIONS` |
| Tool discovery and argument template | `crates/mcp/src/lib.rs::Tool` | `:mcptools`, F2 in the UI |
| Server table editor (`Ctrl+W S`) | `crates/ui/src/app.rs::open_mcp_editor` | `crates/ui/src/overlay.rs::McpEditor`, save via `save_mcp_editor` |

Tests: `crates/mcp/tests/client.rs`, `tests/mcp_cli.rs`, corpus cases under
`tests/corpus/collection/mcp/`.

## Storage and local state

Every file here is documented in
[FORMAT.md § local state](../FORMAT.md#local-state-files).

| What | Entry point | Supporting |
|---|---|---|
| Collection tree, scan, load/save, rename/move/duplicate/delete | `crates/store/src/collection.rs` | Relative `/`-separated paths, natural sort |
| Collection discovery (nearest `.sendito/`), `sendito init`, gitignore entries | `crates/store/src/init.rs` | |
| Layered config (defaults < global < project) | `crates/store/src/config.rs::Config` | `UiConfig`, `HttpConfig`, `GrpcConfig`, `WsConfig`, `OAuthIdentity`, `HistoryConfig` |
| Environments, four secret sources, keychain, secret files | `crates/store/src/env.rs` | `EnvFile`, `VarSource`, `Envs`, `OsKeychain`, `MemoryKeychain` |
| History (`history.jsonl`, masked, trimmed) | `crates/store/src/history.rs::append` | `load`, `clear` |
| Session restore (`session.toml`) | `crates/store/src/session.rs` | Open tabs, drafts, layout, environment |
| Cookie jar (RFC 6265, PSL, `0600`, cross-process lock) | `crates/store/src/cookies.rs` | `request`, `store_response`, `describe`, `delete`, `clear` |
| User themes on disk (`themes/*.toml`, `extends`) | `crates/store/src/themes.rs` | Contrast and `extends` problems are reported, not fatal |

Tests: corpus cookie cases, `tests/integration.rs` (secret hygiene),
`crates/ui/tests/flows.rs` (files land on disk).

## Import and export

| What | Entry point | Supporting |
|---|---|---|
| Format detection and the common import tree | `crates/import/src/lib.rs` | `ImportFormat`, `ImportResult`, `write`, `safe_name` |
| Postman Collection v2.1 | `crates/import/src/postman.rs` | Dynamic-variable aliases, inheritance |
| Insomnia v4/v5 | `crates/import/src/insomnia.rs` | `{{ _.name }}` translation |
| Bruno directory | `crates/import/src/bruno.rs` | `.bru` block parsing |
| curl import | `crates/import/src/curl.rs::import` | Combined-flag splitting, documented flag set |
| curl export | `crates/import/src/curl.rs::export` | Secret placeholders unless `include_secrets` |
| `.http` export | `crates/import/src/http_file.rs::export` | Boundary escalation, `< path` file bodies |
| Import/report dialogs in the UI | `crates/ui/src/overlay.rs::{Import, Report}` | `crates/ui/src/app.rs` opens them |

Tests: `tests/integration.rs` (32-request Postman fixture, curl → request →
curl → request over 20 fixtures), fixtures in `tests/fixtures/`.

## Terminal UI

The app is one large state machine; start at `crates/ui/src/app.rs`, then follow
the module for the surface you are changing.

| What | File |
|---|---|
| App state, input handling, focus, status, mouse hits, command execution | `crates/ui/src/app.rs` |
| Session bootstrap, `RunOptions`, scripted input for benches | `crates/ui/src/lib.rs` |
| Open tab: editors per field, model ↔ widgets, response state, gRPC/MCP/WS session state | `crates/ui/src/tab.rs` |
| Text editor: multi-line, selection, undo, clipboard, `{{var}}` and JSON highlighting, vim subset | `crates/ui/src/editor.rs` |
| Editable tables (params, headers, form, multipart, scripts, metadata) | `crates/ui/src/form.rs` |
| Response viewer: pretty JSON with folding, raw, hex, search, wrapping | `crates/ui/src/viewer.rs` |
| Completion popup logic (schema keys, methods, servers, tools) | `crates/ui/src/complete.rs` |
| Actions, bindings, contexts; the single source for help and KEYMAP.md | `crates/ui/src/keymap.rs` |
| Color tokens, built-in themes, `NO_COLOR` | `crates/ui/src/theme.rs` |
| Icon sets (`unicode`, `nerd`) | `crates/ui/src/icons.rs` |
| Sidebar tree state: flatten, expand, filter, select | `crates/ui/src/tree.rs` |
| Overlays: palette, help, settings, confirm, prompt, picker, env editor, MCP editor, import, report | `crates/ui/src/overlay.rs` |
| List arithmetic: scroll clamping, steps, page steps, wrap-around | `crates/ui/src/list.rs` |

### Drawing

Every clickable region is registered in `app.hits` while drawing.

| Surface | File |
|---|---|
| Frame composition, layout | `crates/ui/src/draw/mod.rs` |
| Top bar, tabs, hint row, status line | `crates/ui/src/draw/chrome.rs` |
| Primitives and components: panes, chips, key caps, buttons, rules, list rows, spinner | `crates/ui/src/draw/kit.rs` |
| Dialog chrome: the shell, popups, the input row, the button footer, the status line | `crates/ui/src/draw/dialog.rs` |
| The editable table every form is made of | `crates/ui/src/draw/table.rs` |
| The configuration table editor (environment and MCP servers) | `crates/ui/src/draw/editor.rs` |
| Request pane: URL strip, sub-tabs, fields, tables | `crates/ui/src/draw/request.rs` |
| Response pane: tabs, borders, scrollbar | `crates/ui/src/draw/response.rs` |
| Sidebar: request tree and history | `crates/ui/src/draw/sidebar.rs` |
| Overlays and dialogs | `crates/ui/src/draw/overlay.rs` |
| Completion popup | `crates/ui/src/draw/complete.rs` |
| Start screen and pixel wordmark | `crates/ui/src/draw/welcome.rs` |

### Adding a key, command or setting

| Change | Files |
|---|---|
| A new key binding | `crates/ui/src/keymap.rs` (table) → `make keymap` → `KEYMAP.md` + the in-app help |
| A new `:command` | `crates/ui/src/overlay.rs::COMMANDS` → dispatch in `crates/ui/src/app.rs` → `KEYMAP.md` via `make keymap` |
| A new setting | `crates/store/src/config.rs` (field) → `crates/ui/src/overlay.rs::SettingId` → `crates/ui/src/app.rs` (apply and persist) → [FEATURES.md](FEATURES.md#settings) |
| A new dialog | `crates/ui/src/overlay.rs::Overlay` → `crates/ui/src/draw/overlay.rs` (build a `draw::dialog::shell` + `draw::dialog::buttons`) → hits in `crates/ui/src/app.rs` → snapshots |

## Headless CLI

`src/main.rs` holds parsing plus the commands; the send path is
`sendito_ui::runner`, so the CLI and the TUI cannot drift.

| Command | Handler |
|---|---|
| `sendito [PATH]` / `--print-keymap` | `src/main.rs::main` → `crates/ui/src/lib.rs::run` |
| `sendito init` | `crates/store/src/init.rs` |
| `sendito run <request>` | `src/main.rs` → `crates/ui/src/runner.rs` |
| `sendito run <folder>` (tree order, variables carried forward, JSON report) | `src/main.rs` (`RunFlags`, folder loop) |
| `sendito curl` / `sendito http` | `crates/import/src/curl.rs::export`, `crates/import/src/http_file.rs::export` |
| `sendito auth login/logout` | `src/main.rs` → `crates/mcp/src/oauth.rs::login_http` |
| `sendito mcp login/logout` | `src/main.rs` → `crates/mcp/src/oauth.rs::login` |
| `sendito theme list/print` | `crates/ui/src/theme.rs`, `crates/store/src/themes.rs` |
| `sendito import` | `crates/import/src/lib.rs::import_path` |

Tests: `tests/integration.rs`, `tests/mcp_cli.rs`, `tests/proxy.rs` (runs the
binary in a child process), `tests/corpus.rs`.

## Test and tooling map

| Layer | Where | Command |
|---|---|---|
| Unit tests | beside the code in every crate | `make test` |
| Wire corpus (exact bytes; 47 HTTP, 15 gRPC, 12 WS, 5 MCP) | `tests/corpus.rs`, `tests/corpus/collection/`, `tests/corpus/expected/` | `make corpus` |
| Corpus golden rewrite | same, `UPDATE_CORPUS=1` | `make corpus-update` |
| Import, format stability, secret hygiene | `tests/integration.rs`, `tests/fixtures/` | `make test` |
| TLS and mutual TLS | `tests/tls.rs` | `make test` |
| Proxy routing | `tests/proxy.rs` | `make test` |
| MCP over the CLI | `tests/mcp_cli.rs` | `make test` |
| gRPC calls | `crates/grpc/tests/calls.rs` | `make test` |
| MCP client and OAuth | `crates/mcp/tests/`, `crates/testkit/src/mcp_auth.rs` | `make test` |
| UI flows (scripted keys and mouse) | `crates/ui/tests/flows.rs`, harness in `crates/ui/tests/common/mod.rs` | `make test` |
| UI snapshots (120×40, 200×60) | `crates/ui/tests/snapshots.rs` | `make test`, accept with `make snap` |
| Screenshots and GIFs | `crates/ui/tests/media.rs`, `docs/media/render.py`, `docs/media/README.md` | `make media && make render` |
| Visual peeking while developing | `crates/ui/tests/peek.rs` | `cargo test -p sendito-ui --test peek -- --ignored --nocapture` |
| Echo servers for manual work | `crates/testkit/src/bin/echo-servers.rs` | `make servers` |
| Coverage and performance | `crates/*/src/` (non-UI), `bench/run.sh` | `make cov`, `make bench` |
| Reviewer bundles | `scripts/critic-bundles.sh` | `make bundles` |
| Key map currency | `crates/ui/src/keymap.rs` (unit test) | `make keymap` |

## Changing a feature

The full checklist is in [ARCHITECTURE.md § changing
behaviour](ARCHITECTURE.md#changing-behaviour). The short version, by change
type:

| You change | Files to touch |
|---|---|
| A request field or wire rule | `crates/core/src/model.rs` → `crates/core/src/resolve.rs` (or the protocol crate) → `crates/store/src/request_file.rs` → `tests/corpus/` + `make corpus-update` → `FORMAT.md` → `scripts/critic-bundles.sh` |
| An HTTP behaviour | `crates/http/src/lib.rs` → `crates/ui/src/runner.rs` (wiring) → `tests/` → `FORMAT.md` if observable |
| A gRPC behaviour | `crates/grpc/src/` → `crates/ui/src/runner.rs` → `crates/grpc/tests/calls.rs` + corpus |
| A WebSocket behaviour | `crates/ws/src/lib.rs` → `crates/ui/src/runner.rs` → `tests/integration.rs` + corpus |
| An MCP behaviour | `crates/mcp/src/` → `crates/ui/src/runner.rs` → `crates/mcp/tests/` + corpus |
| A secret or masking rule | `crates/core/src/vars.rs` (`scrub_bytes`, taint) → every caller's scrub list → `tests/integration.rs` |
| A UI surface | `crates/ui/src/app.rs` + the matching `draw/` module → snapshots → `make media && make render` → [reports/ux-critic.md](reports/ux-critic.md) decisions |
| A key, command or setting | see [Adding a key, command or setting](#adding-a-key-command-or-setting) |

## Keeping this map current

This file is hand-maintained. It goes stale in one predictable way: a file is
renamed or a module moves and the table still points at the old path. When you
add a module, add its row; when you delete one, delete the row. If a link here
404s, the map is wrong, not the code.

The generated documents ([KEYMAP.md](../KEYMAP.md), the media, the corpus
goldens) are listed in [docs/README.md](README.md#generated-files) — never
hand-edit those.
