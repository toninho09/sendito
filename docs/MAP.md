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
| `sendito` | `src/` | CLI parsing and every headless command, one module per command group in `src/cli/` | `src/main.rs::main` |
| `sendito-core` | `crates/core/` | Request model, templates, variables, resolution, scripts | `crates/core/src/lib.rs` |
| `sendito-http` | `crates/http/` | The HTTP client (`lib.rs`), TLS material (`tls.rs`), proxy (`proxy.rs`), timing (`timing.rs`), body decoding (`body.rs`), redirects (`redirect.rs`) | `crates/http/src/lib.rs::send` |
| `sendito-grpc` | `crates/grpc/` | Dynamic gRPC calls and schema loading | `crates/grpc/src/lib.rs::start_call` |
| `sendito-ws` | `crates/ws/` | WebSocket handshake (`lib.rs`), proxy tunnel (`proxy.rs`), frames and close (`session.rs`) | `crates/ws/src/lib.rs::open` |
| `sendito-mqtt` | `crates/mqtt/` | MQTT v3.1.1 and v5 sessions, subscriptions, publishes, Last Will | `crates/mqtt/src/lib.rs::open` |
| `sendito-mcp` | `crates/mcp/` | JSON-RPC client, transports, MCP OAuth | `crates/mcp/src/lib.rs::call_tool` |
| `sendito-store` | `crates/store/` | Everything on disk: collection, config, env, history, cookies, session, themes | `crates/store/src/lib.rs` |
| `sendito-import` | `crates/import/` | Postman, Insomnia, Bruno, curl, `.http` | `crates/import/src/lib.rs` |
| `sendito-pipeline` | `crates/pipeline/` | The send pipeline the TUI and `sendito run` share (`runner/`: one module per protocol plus project, scope, OAuth and history), the response document model, and the protocol types the TUI reads (`http`, `grpc`, `ws`, `mcp` re-exports) | `crates/pipeline/src/runner/http.rs::exec_http` |
| `sendito-ui` | `crates/ui/` | ratatui app, drawing, overlays, key map, theme registry. Depends on protocols only through `sendito-pipeline` | `crates/ui/src/lib.rs::run` |
| `sendito-widgets` | `crates/widgets/` | Widgets with no app state: text editor, editable table, list movement, Markdown preview, icons, color themes | `crates/widgets/src/lib.rs` |
| `sendito-testkit` | `crates/testkit/` | Echo servers, fake OAuth server, proxy, test PKI | `crates/testkit/src/lib.rs` |

## Request model and file format

The TOML schema is defined once in `core` and serialized once in `store`. The
model carries no serialization logic on purpose.

| What | File |
|---|---|
| `Request`, `HttpRequest`, `GrpcRequest`, `WebSocketRequest`, `SseRequest`, `MqttRequest`, `McpRequest`, auth kinds, body kinds, rows | `crates/core/src/model.rs` |
| Fields every request kind has (scripts, description): one place in the model and one read in the parser | `crates/core/src/model.rs::RequestCommon`, `crates/store/src/request_file.rs::common` |
| Parse and canonical write of `<name>.toml` | `crates/store/src/request_file.rs` |
| Folder settings: parse and write `folder.toml`, the chain of a request | `crates/store/src/folder.rs`, `crates/store/src/collection.rs::chain` |
| Saved examples: `<name>.examples/<example>.toml` | `crates/store/src/examples.rs` |
| Saved examples in the UI (`:saveexample`, `:examples`) | `crates/ui/src/app/examples.rs` |
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
| Dynamic values (`$uuid`, `$timestamp`, `$randomInt`, …) and `$processEnv.NAME` | `crates/core/src/vars.rs::uuid_v4`, the `$` table and `crates/core/src/vars.rs::is_builtin` | Generated once per send; `$processEnv` values are secrets |
| Request → wire conversion for all four kinds | `crates/core/src/resolve.rs` | `resolve_http`, `resolve_grpc`, `resolve_ws`, `resolve_mcp` |
| Query/form percent-encoding whitelists | `crates/core/src/resolve.rs::query_encode`, `form_urlencode` | The whitelist is a documented decision — see FORMAT.md |
| URL normalization (no scheme → `http://`), query append before fragment | `crates/core/src/resolve.rs::normalize_url`, `append_query` | |
| Unresolved variables, masked copy of a request | `crates/core/src/resolve.rs::unresolved_vars`, `mask_request` | |
| Declarative scripts: pre, extract, assert | `crates/core/src/script.rs` | No code execution |
| Lua scripts: `[scripts.lua] pre` and `post`, sandbox and API | `crates/core/src/lua_script.rs` | Run by `crates/pipeline/src/runner/mod.rs::pre_phase` and `response_phase` (extracts, assertions, post scripts) in every runner |
| Script sources (`jsonpath`, `header`, `status`, `time`, `size`, `hash`, `xpath`, `regex`, `cookie`) | `crates/core/src/script.rs::read`, `crates/core/src/xpath.rs` | Each runner fills `ResponseView`, with `timing` |
| Folder inheritance (auth, headers, variables, scripts) | `crates/core/src/inherit.rs::apply` | `var_layers`; applied once per send by `crates/pipeline/src/runner/scope.rs::with_folders` |
| Completion shape of a message or tool input | `crates/core/src/complete.rs` | Shared by gRPC and MCP editors |
| A sample value from a JSON Schema (MCP argument template, OpenAPI bodies) | `crates/core/src/json_schema.rs` | `sample`, `object_template` |
| Sent pipeline (scope → pre → resolve → send → extract/assert → history) | `crates/pipeline/src/runner/` | Used identically by the TUI and `sendito run` |

## HTTP

| What | Entry point | Supporting |
|---|---|---|
| Sending a resolved request | `crates/http/src/lib.rs::send` | `SendOptions`, `HttpResponse` |
| SSE transport and parser | `crates/http/src/sse.rs::open`, `crates/http/src/sse.rs::Parser` | `crates/http/src/lib.rs::open_stream`; `crates/http/tests/sse.rs` |
| GraphQL completion | `crates/core/src/graphql.rs::graphql_context` | `crates/ui/src/complete.rs::graphql_candidates`; `crates/ui/src/app/completion.rs`; `crates/ui/tests/graphql_schema.rs` |
| GraphQL introspection | `crates/http/src/graphql.rs::GraphqlSchema` | `crates/pipeline/src/runner/graphql.rs::fetch_schema`; `crates/ui/src/app/schema.rs::App::load_graphql_schema`; `crates/ui/tests/graphql_schema.rs` |
| GraphQL body assembly | `crates/core/src/model.rs::graphql_document_json` | `crates/core/src/resolve.rs::resolve_http`; editor sections in `crates/ui/src/tab/editors.rs` |
| Header and body assembly, multipart boundary, file bodies | `crates/core/src/resolve.rs::resolve_http` | `ResolvedBody`, `ResolvedPart`, `PartData` |
| MIME guess for multipart file parts | `crates/http/src/body.rs::guess_mime` | Used by resolve and by the request pane |
| Response decoding (gzip, deflate, br) and display size vs wire size | `crates/http/src/lib.rs::Decoded` | `wire_body_len`, `headers_size` |
| Timing breakdown (DNS, connect, TLS, TTFB, download) | `crates/http/src/timing.rs::Timing` | Rendered by `crates/ui/src/app/text.rs::timing_text` |
| Proxy (`[http] proxy`, URL or `"env"`) | `crates/http/src/proxy.rs::ProxySpec` | `apply` builds the reqwest client |
| TLS material, custom CA, mutual TLS | `crates/http/src/tls.rs::tls_config`, `TlsMaterial` | `pair_error` implements the "cert without key warns" rule |
| Redirect handling | `crates/http/src/redirect.rs::follow_redirects`, `hop_headers`, `RedirectCookies` | Auth headers stay behind on another origin; the jar sets each hop's Cookie (FORMAT.md) |
| Cookie jar wiring during a send | `crates/pipeline/src/runner/http.rs::exec_http` (`jar_request`, `jar_store`, `JarHops`) | Storage in `crates/store/src/cookies.rs` |
| jq response filter (display only) | `crates/ui/src/filter.rs::apply` | `crates/ui/src/tab/response.rs::apply_jq`; jaq with `halt`/`stderr` removed |
| Response query bar (jq and search) | `crates/ui/src/tab/response.rs::QueryBar` | `crates/ui/src/app/query.rs::query_key`, `crates/ui/src/draw/response.rs::draw_query_bar`; key completion `crates/ui/src/complete.rs::jq_keys` |
| Stream message list (gRPC, WebSocket) | `crates/ui/src/tab/response.rs::stream_messages` | `crates/ui/src/tab/response.rs::set_message`, `crates/ui/src/draw/response.rs::draw_stream_split`; `[` / `]` in `crates/ui/src/app/import_export.rs::step_message` |
| Body format detection and pretty forms (XML, HTML, YAML, CSV/TSV) | `crates/pipeline/src/pretty/mod.rs::detect` | `crates/pipeline/src/pretty/markup.rs::pretty_markup`, `crates/pipeline/src/pretty/yaml.rs::yaml_folds`, `crates/pipeline/src/pretty/table.rs::pretty_table`; chosen in `crates/pipeline/src/doc.rs::from_bytes` |
| JSON path at the cursor | `crates/pipeline/src/doc.rs::json_path` | `crates/ui/src/viewer.rs::json_path`; `f` / `Y` in `crates/ui/src/app/import_export.rs::filter_here` |
| Watch (repeat the active request) | `crates/ui/src/app/mod.rs::toggle_watch` | `send_opts(…, from_watch)`, sparkline via `watch_text` |

Tests: `tests/corpus.rs` (57 HTTP cases under
`tests/corpus/collection/http/`), `tests/integration.rs`, `tests/tls.rs`,
`tests/proxy.rs`.

## Authentication

| What | Entry point | Supporting |
|---|---|---|
| Basic / bearer / API key resolution | `crates/core/src/resolve.rs::resolve_http` | Auth is part of the resolved request |
| Digest, AWS SigV4 and JWT (computed at send time) | `crates/http/src/auth/` (`digest.rs`, `sigv4.rs`, `jwt.rs`) | Fields rendered into `crates/core/src/resolve.rs::SignedAuth`; applied by `crates/pipeline/src/runner/http.rs::sign_before_send` and `retry_401`, `jwt_token` for gRPC and WebSocket |
| HTTP OAuth identity (`[oauth.<name>]`) | `crates/pipeline/src/runner/http.rs::http_oauth_provider` | `crates/pipeline/src/runner/oauth.rs::oauth_settings`, `oauth_token_account` |
| OAuth login/logout for HTTP, CLI | `src/cli/auth.rs::http_auth` | `crates/mcp/src/oauth/flow.rs::login_http` |
| OAuth grants: client credentials, password, device code | `crates/mcp/src/oauth/flow.rs::fetch_token`, `device_login` | A new token on expiry: `crates/mcp/src/oauth/provider.rs::RefreshingProvider`; grant in `crates/store/src/config.rs::OAuthGrant` |
| OAuth login/logout in the UI | `crates/ui/src/app/commands.rs::http_oauth_command` | `OAuthRunInfo` in `crates/pipeline/src/runner/oauth.rs` |
| MCP OAuth 2.1 (discovery, PKCE, registration, refresh) | `crates/mcp/src/oauth/flow.rs::login` | `crates/mcp/src/oauth/flow.rs::pkce`, `crates/mcp/src/oauth/provider.rs::RefreshingProvider`, `crates/mcp/src/oauth/discovery.rs::well_known`, `validate_oauth_url` |
| MCP sign-in in the app (`:auth login <server>`, "Sign in now?" after `McpError::NeedsLogin`) | `crates/ui/src/app/mcp.rs` (`offer_mcp_login`, `mcp_oauth_command`, `after_mcp_login`) | `crates/pipeline/src/runner/oauth.rs::mcp_login`, `crates/store/src/config.rs::set_mcp_server_auth` |
| Token storage keys | `crates/pipeline/src/runner/oauth.rs::oauth_token_account`, `crates/pipeline/src/runner/mcp.rs::mcp_token_account` | Keychain access in `crates/store/src/env.rs` |

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
| Deadline to `grpc-timeout`, TLS and ALPN | `crates/grpc/src/lib.rs::ConnSettings` | Built by `crates/pipeline/src/runner/grpc.rs::conn_settings` |
| Status-code naming | `crates/grpc/src/lib.rs::code_name` | All 17 codes |
| Streaming session in the UI | `crates/pipeline/src/runner/::{GrpcControl, GrpcUpdate}` | `crates/ui/src/app/ingest.rs` handles the events |

Tests: `crates/grpc/tests/calls.rs`, corpus cases under
`tests/corpus/collection/grpc/`.

## SSE

| Feature | Main code | Wiring and tests |
|---|---|---|
| Saved request and resolution | `crates/core/src/model.rs::SseRequest`, `crates/core/src/resolve.rs::resolve_sse` | `crates/store/src/request_file.rs`; `tests/sse.rs` |
| Authenticated live session | `crates/pipeline/src/runner/sse.rs::exec_sse` | Shared HTTP auth in `crates/pipeline/src/runner/http.rs`; `tests/sse_pipeline.rs` |
| Headless output and report | `src/cli/sse.rs::run_sse` | `src/cli/run.rs`; `tests/sse.rs` |
| Live UI response | `crates/ui/src/app/sse.rs::App::on_sse` | `crates/ui/src/tab/response.rs`; `crates/ui/tests/sse.rs` |
| Echo routes | `crates/testkit/src/sse_echo.rs` | `crates/testkit/src/http_echo.rs`; `tests/corpus/sse.rs` |
| Screens and recordings | `crates/ui/tests/common/sse.rs` | `crates/ui/tests/snapshots.rs`, `crates/ui/tests/media.rs` |

## MQTT

| Feature | Main code | Wiring and tests |
|---|---|---|
| Client sessions (v3.1.1, v5, QoS 0 to 2, Last Will, TLS) | `crates/mqtt/src/lib.rs::open`, `crates/mqtt/src/run.rs` | `crates/mqtt/tests/client.rs` |
| Saved request and resolution | `crates/core/src/model.rs::MqttRequest`, `crates/core/src/resolve.rs::resolve_mqtt` | `crates/core/src/resolve.rs::mqtt_target`; `crates/store/src/request_file.rs`; `tests/mqtt.rs` |
| Folder headers note | `crates/core/src/inherit.rs::ignored_headers_note` | `src/cli/run.rs`, `crates/ui/src/app/send.rs` |
| Live session | `crates/pipeline/src/runner/mqtt.rs::exec_mqtt` | `MqttControl`, `MqttUpdate`; `tests/mqtt_pipeline.rs` |
| Headless output and report | `src/cli/mqtt.rs::run_mqtt` | `src/cli/run.rs`; `tests/mqtt.rs` |
| Editor | `crates/ui/src/tab/editors.rs::Editors` (`Kind::Mqtt`, `MQTT_TABS`) | `crates/ui/src/tab/fields.rs` |
| Live UI response and live publish | `crates/ui/src/app/mqtt.rs::App::on_mqtt` | `crates/ui/src/app/mqtt.rs::App::mqtt_publish_selected`; `crates/ui/src/tab/response.rs::MqttMsg`; `crates/ui/tests/mqtt.rs` |
| Test broker (TCP and TLS) | `crates/testkit/src/mqtt_broker.rs::MqttBroker` | `tests/corpus/mqtt.rs` |
| Screens and recordings | `crates/ui/tests/common/mqtt.rs` | `crates/ui/tests/snapshots.rs`, `crates/ui/tests/media.rs` |

## WebSocket

| What | Entry point | Supporting |
|---|---|---|
| Session: handshake, frames, close, deadline | `crates/ws/src/lib.rs::open`, `crates/ws/src/session.rs::Session` | `Event`, `FrameKind`, `WsError` |
| Message document (strings and `{"binary": …}`) | `crates/ws/src/lib.rs::parse_messages` | Validated in the editor, tolerated as half-typed |
| Proxy tunnelling (`CONNECT`, `https://` proxy over TLS) | `crates/ws/src/proxy.rs::WsProxy` | Reuses `sendito_http::ProxySpec` and `tls_config` |
| UI session control (`Ctrl+W d`, `Ctrl+W x`) | `crates/pipeline/src/runner/::{WsControl, WsUpdate}` | Response summary via `ws_summary` |

Tests: unit tests in `crates/ws/src/lib.rs`, corpus cases under
`tests/corpus/collection/ws/`, `tests/integration.rs` (cookie jar).

## MCP

| What | Entry point | Supporting |
|---|---|---|
| Session lifecycle (spawn/connect, initialize, work, drop) | `crates/mcp/src/lib.rs::call_tool`, `list_tools` | One timeout covers the whole call |
| Transport selection | `crates/mcp/src/lib.rs::Transport` | `crates/pipeline/src/runner/mcp.rs::mcp_transport` |
| stdio transport (NDJSON, stderr to the log) | `crates/mcp/src/stdio.rs` | Child killed on drop |
| Streamable HTTP transport (JSON and SSE bodies) | `crates/mcp/src/http.rs` | Session-id and protocol-version headers |
| JSON-RPC messages, protocol constants | `crates/mcp/src/protocol.rs` | `PROTOCOL_VERSION`, `SUPPORTED_VERSIONS` |
| Tool discovery and argument template | `crates/mcp/src/lib.rs::Tool` | `:mcptools`, F2 in the UI |
| Servers editor (`Ctrl+W S`, `:mcpedit`): list and form | `crates/ui/src/records.rs::RecordEditor` (`open_mcp_editor`, `open_mcp_server_editor`, `save_records`) | Fields per kind in `crates/ui/src/records.rs::Record`; drawing in `crates/ui/src/draw/overlay/records.rs` |
| Connection test (`Ctrl+T` in the servers editor) | `crates/ui/src/app/mcp.rs::start_mcp_test` | Answer in `AppMsg::McpTest` (`crates/ui/src/app/ingest.rs`) |

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
| Session restore (`session.toml`) | `crates/store/src/session.rs` | Open tabs, drafts, pinned tabs, layout, environment, favorites |
| Cookie jar (RFC 6265, PSL, `0600`, cross-process lock) | `crates/store/src/cookies.rs` | `request`, `store_response`, `describe`, `delete`, `clear` |
| Durable writes and cross-process locks | `crates/store/src/fsio.rs` | `write_atomic`, `write_private`, `with_lock`, `with_dir_lock` |
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
| `.http` import | `crates/import/src/http_file.rs::import`, `import_path` | `http-client.env.json` environments |
| OpenAPI 3.x and Swagger 2.0 import | `crates/import/src/openapi.rs::import` | YAML read by `crates/import/src/lib.rs::import_path`; bodies from `crates/core/src/json_schema.rs::sample` |
| Import/report dialogs in the UI | `crates/ui/src/overlay/mod.rs::{Import, Report}` | `crates/ui/src/app/import_export.rs` opens them |

Tests: `tests/integration.rs` (32-request Postman fixture, curl → request →
curl → request over 20 fixtures), fixtures in `tests/fixtures/`.

## Terminal UI

The app is one large state machine; start at `crates/ui/src/app/mod.rs`, then follow
the module for the surface you are changing.

| What | File |
|---|---|
| App state, focus, status, session, tick | `crates/ui/src/app/mod.rs` |
| Terminal events and key routing | `crates/ui/src/app/keys.rs` |
| Keymap actions: focus, tabs, close, save | `crates/ui/src/app/actions.rs` |
| Completion wiring | `crates/ui/src/app/completion.rs` |
| Send, cancel, run context | `crates/ui/src/app/send.rs` |
| Runner results (HTTP, gRPC, WebSocket, MCP) and `AppMsg` | `crates/ui/src/app/ingest.rs` |
| MCP sign-in, discovery, server editor | `crates/ui/src/app/mcp.rs` |
| gRPC schema and method picker | `crates/ui/src/app/schema.rs` |
| Environment picker and editor | `crates/ui/src/app/env.rs` |
| History panel | `crates/ui/src/app/history.rs` |
| Tree operations (new, rename, duplicate, delete, move) | `crates/ui/src/app/tree_ops.rs` |
| Folder settings: the cache, the folder tab (open, save, restore), what a tab inherits | `crates/ui/src/app/folders.rs` |
| Pickers, palette, help, `:command` execution | `crates/ui/src/app/commands.rs` |
| Response query bar (find, filter, jq) | `crates/ui/src/app/query.rs` |
| Import, export, body actions | `crates/ui/src/app/import_export.rs` |
| Settings and themes | `crates/ui/src/app/settings.rs`; the dialog's state (filter, in-place edit) in `crates/ui/src/overlay/settings.rs::SettingsView` |
| Name prompt preview (the file a name creates, a name taken) | `crates/ui/src/app/dialogs.rs::prompt_preview` |
| Import dialog preview and path completion | `crates/ui/src/app/import_export.rs` (`import_preview`, `complete_import_path`) |
| Hint row | `crates/ui/src/app/hints.rs` |
| Report, timing, header and help text | `crates/ui/src/app/text.rs` |
| Overlay keys, dialog accept actions | `crates/ui/src/app/overlay_keys.rs`, `crates/ui/src/app/dialogs.rs` |
| Mouse hits, wheel, drag, clicks | `crates/ui/src/app/mouse.rs` |
| Session bootstrap, `RunOptions`, scripted input for benches | `crates/ui/src/lib.rs` |
| Open tab: editors per field, model ↔ widgets, response state, gRPC/MCP/WS session state | `crates/ui/src/tab/` (`fields.rs` field list, `editors.rs` model ↔ widgets, `response.rs` response state) |
| Text editor: multi-line, selection, undo, clipboard, `{{var}}` and JSON highlighting, vim subset | `crates/widgets/src/editor/` (`conventional.rs`, `vim.rs`, `render.rs`) |
| Editable tables (params, headers, form, multipart, scripts, metadata) | `crates/widgets/src/form.rs` |
| Response viewer: pretty JSON, XML, HTML, YAML and CSV with folding, raw, hex, search, wrapping | `crates/ui/src/viewer.rs` (widget, colors per format in `pretty_spans`), `crates/pipeline/src/doc.rs` (document model) |
| Completion popup logic (schema keys, methods, servers, tools) | `crates/ui/src/complete.rs` |
| Actions, bindings, contexts; the single source for help and KEYMAP.md | `crates/ui/src/keymap.rs` |
| Color tokens, built-in themes, `NO_COLOR` | `crates/widgets/src/theme.rs` |
| Markdown preview of a description (Docs tab) | `crates/widgets/src/markdown.rs` |
| Theme registry: user theme files, `extends`, notes | `crates/ui/src/theme.rs::Registry` |
| Icon sets (`unicode`, `nerd`) | `crates/widgets/src/icons.rs` |
| Sidebar tree state: flatten, expand, filter, select | `crates/ui/src/tree.rs` |
| Durations as the interface shows and reads them (`30 s`, `500 ms`) | `crates/ui/src/units.rs` |
| Field labels, help lines, credential fields | `crates/ui/src/tab/fields.rs` (`Field::label`, `Field::help`, `Field::is_secret`) |
| Overlays: palette, help, settings, confirm, prompt, picker, env editor, import, report | `crates/ui/src/overlay/` (`palette.rs`, `settings.rs`, `env_editor.rs`; `mcp_editor.rs` holds the MCP kinds and the stdio command-line parsing) |
| Record editors (list and form): MCP servers (`Ctrl+W S`), OAuth identities (`:oauth`) | `crates/ui/src/records.rs` |
| Key bindings editor (`:keymap`): grouped actions, key recording, clash warnings | `crates/ui/src/keymap_editor.rs` (drawing in `crates/ui/src/draw/overlay/keymap.rs`) |
| List arithmetic: scroll clamping, steps, page steps, wrap-around | `crates/widgets/src/list.rs` |

### Drawing

Every clickable region is registered in `app.hits` while drawing.

| Surface | File |
|---|---|
| Frame composition, layout | `crates/ui/src/draw/mod.rs` |
| Top bar, tabs, the footer (status segments, message, key hints) | `crates/ui/src/draw/chrome.rs` |
| The Ctrl+W menu (which-key box built from the keymap) | `crates/ui/src/draw/whichkey.rs` |
| Primitives and components: panes, chips, key caps, buttons, rules, list rows, spinner | `crates/ui/src/draw/kit.rs` |
| Dialog chrome: the shell, popups, the input row, the button footer, the status line | `crates/ui/src/draw/dialog.rs` |
| The editable table every form is made of | `crates/ui/src/draw/table.rs` |
| The configuration table editor (environment, MCP servers and the config editors) | `crates/ui/src/draw/editor.rs` |
| Request pane: URL strip, sub-tabs, fields, tables | `crates/ui/src/draw/request/` (`url.rs`, `fields.rs`) |
| Response pane: tabs, borders, scrollbar | `crates/ui/src/draw/response.rs` |
| Sidebar: request tree and history | `crates/ui/src/draw/sidebar.rs` |
| Overlays and dialogs | `crates/ui/src/draw/overlay/` (`palette.rs`, `settings.rs`, `dialogs.rs`, `editors.rs`, `keymap.rs`, `records.rs`, `report.rs`) |
| Completion popup | `crates/ui/src/draw/complete.rs` |
| Start screen and pixel wordmark | `crates/ui/src/draw/welcome.rs` |

### Adding a key, command or setting

| Change | Files |
|---|---|
| A new key binding | `crates/ui/src/keymap.rs` (table) → `make keymap` → `KEYMAP.md` + the in-app help |
| A new `:command` | `crates/ui/src/overlay/palette.rs::COMMANDS` → dispatch in `crates/ui/src/app/commands.rs::run_command` → `KEYMAP.md` via `make keymap` |
| A new setting | `crates/store/src/config.rs` (field) → `crates/ui/src/overlay/settings.rs::SettingId` (`key`, `typed`, `opens`, `heading`) and `SETTINGS` → `crates/ui/src/app/settings.rs` (`setting_value`, `change_setting` or `set_typed_setting`, `setting_note`) → [FEATURES.md](FEATURES.md#settings) |
| A new dialog | `crates/ui/src/overlay/mod.rs::Overlay` → `crates/ui/src/draw/overlay/` (build a `draw::dialog::shell` + `draw::dialog::buttons`) → hits in `crates/ui/src/app/mouse.rs` → snapshots |

## Headless CLI

`src/main.rs` holds `main`; `src/cli/` holds the arguments (`args.rs`) and the commands; the send path is
`sendito_pipeline::runner`, so the CLI and the TUI cannot drift.

| Command | Handler |
|---|---|
| `sendito [PATH]` / `--print-keymap` | `src/main.rs::main` → `crates/ui/src/lib.rs::run` |
| `sendito init` | `crates/store/src/init.rs` |
| `sendito run <request>` | `src/cli/run.rs::run_one` → `crates/pipeline/src/runner/` |
| `sendito run <folder>` (tree order, variables carried forward, JSON report) | `src/cli/run.rs::run_suite` (`RunFlags` in `src/cli/args.rs`) |
| `sendito curl` / `sendito http` | `crates/import/src/curl.rs::export`, `crates/import/src/http_file.rs::export` |
| `sendito auth login/logout` | `src/cli/auth.rs::http_auth` → `crates/mcp/src/oauth/flow.rs::login_http` |
| `sendito mcp login/logout` | `src/cli/auth.rs::mcp` → `crates/pipeline/src/runner/oauth.rs::mcp_login` → `crates/mcp/src/oauth/flow.rs::login` |
| `sendito theme list/print` | `src/cli/theme.rs::theme`, `crates/ui/src/theme.rs`, `crates/store/src/themes.rs` |
| `sendito import` | `crates/import/src/lib.rs::import_path` |

Tests: `tests/integration.rs`, `tests/mcp_cli.rs`, `tests/proxy.rs` (runs the
binary in a child process), `tests/corpus.rs`.

## Test and tooling map

| Layer | Where | Command |
|---|---|---|
| Unit tests | beside the code in every crate | `make test`; `P=<package>` and `F=<name>` narrow it |
| Wire corpus (exact bytes; 57 HTTP, 15 gRPC, 12 WS, 13 SSE, 13 MQTT, 5 MCP) | `tests/corpus.rs`, `tests/corpus/collection/`, `tests/corpus/expected/` | `make corpus` |
| Corpus golden rewrite | same, `UPDATE_CORPUS=1` | `make corpus-update` |
| Import, format stability, secret hygiene | `tests/integration.rs`, `tests/fixtures/` | `make test` |
| TLS and mutual TLS | `tests/tls.rs` | `make test` |
| Proxy routing | `tests/proxy.rs` | `make test` |
| MCP over the CLI | `tests/mcp_cli.rs` | `make test` |
| gRPC calls | `crates/grpc/tests/calls.rs` | `make test` |
| MCP client and OAuth | `crates/mcp/tests/`, `crates/testkit/src/mcp_auth.rs` | `make test` |
| UI flows (scripted keys and mouse) | `crates/ui/tests/flows.rs`, harness in `crates/ui/tests/common/mod.rs` | `make test` |
| UI snapshots (one pane or dialog; the layout at 120×40 and 200×60) | `crates/ui/tests/snapshots.rs` | `make test`, accept with `make snap` |
| Screenshots and GIFs | `crates/ui/tests/media.rs`, `docs/media/render.py`, `docs/media/README.md` | `make media && make render` |
| Visual peeking while developing | `crates/ui/tests/peek.rs` | `cargo test -p sendito-ui --test peek -- --ignored --nocapture` |
| Echo servers for manual work | `crates/testkit/src/bin/echo-servers.rs` | `make servers` |
| Coverage and performance | `crates/*/src/` (non-UI), `bench/run.sh` | `make cov`, `make bench` |
| Reviewer bundles | `scripts/critic-bundles.sh` | `make bundles` |
| Key map currency | `crates/ui/src/keymap.rs` (unit test) | `make keymap` |
| Documentation currency (every cited path, symbol, `make` target and link) | `tests/docs.rs` | `make docs` |

## Changing a feature

The full checklist is in [ARCHITECTURE.md § changing
behaviour](ARCHITECTURE.md#changing-behaviour). The short version, by change
type:

| You change | Files to touch |
|---|---|
| A request field or wire rule | `crates/core/src/model.rs` → `crates/core/src/resolve.rs` (or the protocol crate) → `crates/store/src/request_file.rs` → `tests/corpus/` + `make corpus-update` → `FORMAT.md` → `scripts/critic-bundles.sh` |
| An HTTP behaviour | `crates/http/src/` → `crates/pipeline/src/runner/http.rs` (wiring) → `tests/` → `FORMAT.md` if observable |
| A gRPC behaviour | `crates/grpc/src/` → `crates/pipeline/src/runner/grpc.rs` → `crates/grpc/tests/calls.rs` + corpus |
| A WebSocket behaviour | `crates/ws/src/` → `crates/pipeline/src/runner/ws.rs` → `tests/integration.rs` + corpus |
| An MCP behaviour | `crates/mcp/src/` → `crates/pipeline/src/runner/mcp.rs` → `crates/mcp/tests/` + corpus |
| A secret or masking rule | `crates/core/src/vars.rs` (`scrub_bytes`, taint) → every caller's scrub list → `tests/integration.rs` |
| A UI surface | `crates/ui/src/app/` + the matching `draw/` module → snapshots → [reports/ux-critic.md](reports/ux-critic.md) decisions |
| A key, command or setting | see [Adding a key, command or setting](#adding-a-key-command-or-setting) |

## Keeping this map current

This file is hand-maintained. It goes stale in one predictable way: a file is
renamed or a module moves and the table still points at the old path. When you
add a module, add its row; when you delete one, delete the row. If a link here
404s, the map is wrong, not the code. `tests/docs.rs` checks every path and
symbol this map cites: run `make docs` after a move or a rename and fix the
lines it lists.

The generated documents ([KEYMAP.md](../KEYMAP.md), the media, the corpus
goldens) are listed in [docs/README.md](README.md#generated-files) — never
hand-edit those.
