# Sendito

A terminal API client for **HTTP**, **gRPC**, **WebSocket**, **SSE**, **MQTT** and **MCP tools**. It
runs locally in your terminal,
works with the mouse, conventional keys or vim keys, and keeps your requests as
plain TOML files you can version in git.

[![CI](https://github.com/toninho09/sendito/actions/workflows/ci.yml/badge.svg)](https://github.com/toninho09/sendito/actions/workflows/ci.yml)
![Rust](https://img.shields.io/badge/rust-1.89%2B-orange)
![Platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20macOS-blue)
![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-green)

**[Features](docs/FEATURES.md)** · **[CLI reference](docs/CLI.md)** · **[File formats](FORMAT.md)** · **[Key map](KEYMAP.md)** · **[Limitations](docs/DEBT.md)**

![Sendito: a POST request with a JSON body and the response](docs/media/hero.png)

* **Correct on the wire.** Sendito sends what the request says: no hidden
  `User-Agent` or `Referer` headers, explicit rules for query and form encoding
  ([FORMAT.md](FORMAT.md)), and the headers you define in your order. Two small
  differences come from the HTTP library and are listed in
  [docs/DEBT.md](docs/DEBT.md): HTTP/1 header names are sent in Title-Case, and
  `Accept: */*` is added when you define no `Accept`.
  The headers added are framing (`Host`, `Content-Length`), auth, and a
  `Content-Type` from the body type when you define none (JSON → `application/json`,
  form → `application/x-www-form-urlencoded`, file → its content type or
  `application/octet-stream`; multipart file parts → guessed from the extension).
  A corpus of cases for every protocol is checked byte for byte in CI.
* **HTTP**: every method, query params, headers, raw / JSON / form / multipart
  (with files) / file bodies, Basic / Bearer / API-key / OAuth 2 / Digest /
  AWS SigV4 / JWT auth, timeout, redirects,
  TLS verification, an optional proxy (`[http] proxy`, including `"env"` for
  `HTTPS_PROXY` and friends), a custom CA and client certificates for mutual
  TLS (`[http.tls]`), response decompression (gzip / deflate / br), watch mode and
  a DNS / connect / TLS / TTFB / download breakdown.
  → [HTTP features](docs/FEATURES.md#http)
* **gRPC**: services from server reflection or `.proto` files (compiled in-process,
  no `protoc`), unary, server-, client- and bidi-streaming calls, proto3 JSON
  messages with generated templates, metadata, TLS, deadlines, cancellation, and a
  Log tab for every message sent and received.
  → [gRPC features](docs/FEATURES.md#grpc)
* **WebSocket**: a session per request — handshake headers, subprotocols, a JSON
  document of text and binary frames sent on connect, frames received live in a
  Log tab, `wss://` with the same TLS material as HTTP, and the close code as the
  assertion status.
  → [WebSocket features](docs/FEATURES.md#websocket)
* **MCP tools**: call tools on stdio or Streamable HTTP servers, discover tools
  and generate initial arguments from their input schema, with bearer or OAuth
  2.1 authentication for HTTP servers.
  → [MCP features](docs/FEATURES.md#mcp)
* **Environments** with secrets from four sources: the env file, a gitignored
  secrets file, the OS keychain, or a file. Secrets are masked everywhere.
  An opt-in cookie jar keeps cookies per environment; `:cookies` manages them.
  Each redirect hop gets the jar's cookies for its own URL.
  → [Variables and secrets](docs/FEATURES.md#variables-and-secrets)
* **Scripts**: set variables before sending, extract values (JSONPath,
  header, status) after, and assert on them; or write the logic in
  [Lua](https://www.lua.org) when rows are not enough.
  → [Scripts](docs/FEATURES.md#scripts)
* **Folder settings** (`folder.toml`): headers, auth, variables and scripts that
  every request below a folder inherits. `s` on a tree folder opens them.
  → [Folder settings](docs/FEATURES.md#folder-settings)
* **Saved example responses**: `:saveexample` keeps a response next to its
  request; `:examples` lists them.
  → [Saved examples](docs/FEATURES.md#saved-examples)
* **History**, **Postman / Bruno / Insomnia / OpenAPI / `.http` import**,
  **curl import/export**, `.http` export, session restore, seven themes plus
  your own, optional Nerd Font icons, and a key map you can change.
  → [Import and export](docs/FEATURES.md#import-and-export) ·
  [Themes](docs/FEATURES.md#themes-and-icons)

## Contents

- [Install](#install)
- [Quick start](#quick-start)
- [See it in action](#see-it-in-action)
- [Development](#development)
- [Documentation](#documentation)
- [License](#license)

Every feature, with its keys and settings, is described in
[docs/FEATURES.md](docs/FEATURES.md).

## Install

Download a release archive for Linux x86_64 or macOS arm64 from the
[releases page](https://github.com/toninho09/sendito/releases), check it and put the
binary on your `PATH`:

```sh
tar xzf sendito-v0.2.0-x86_64-unknown-linux-gnu.tar.gz
sha256sum -c sendito-v0.2.0-x86_64-unknown-linux-gnu.tar.gz.sha256
sudo install sendito /usr/local/bin/
```

Or build from source (the repository pins Rust 1.100.0 in `rust-toolchain.toml`, which rustup installs on its own; the code needs 1.89 or newer — edition-2024 let chains need 1.88 and the jq filter library sets the floor at 1.89 — but nothing older than the pinned version is tested):

```sh
cargo install --path .
```

## Quick start

```sh
sendito init my-api        # creates my-api/.sendito and a "local" environment
cd my-api
sendito                    # opens the collection
```

In the app:

1. Press `n` in the sidebar to create a request (choose HTTP, gRPC, WebSocket or MCP
   and a folder):
   its file is created at once, empty. `Ctrl+N` instead opens a scratch tab that has
   no file until `Ctrl+S` asks for a name and folder.
2. Type the URL, e.g. `{{base_url}}/users`, and press **Ctrl+Enter** or **F5**.
   Sending uses the request as shown, unsaved edits included.
3. `Ctrl+S` saves; `F4` switches environment; `F1` lists every key.

Without the UI:

```sh
sendito run "Users/list users" --env local     # body to stdout, status to stderr
sendito run checks/health --env prod || echo "an assertion failed"
sendito run Users --env local                  # every request under Users, in tree order
sendito run . --env local --report json        # current folder (root = all), JSON summary
sendito run Users --env local --fail-fast      # stop at the first failure
sendito run Users --env local -i               # also print the response headers
sendito run "Users/create user" --no-history --env local
sendito curl "Users/create user" --env local   # print as curl (secrets stay {{placeholders}})
sendito http "Users/create user" --env local   # print as an .http file
sendito curl x --include-secrets --env local   # include secret values
sendito import acme.postman_collection.json    # Postman or Insomnia JSON
sendito import path/to/bruno-collection        # Bruno directory
sendito import openapi.yaml                    # OpenAPI or Swagger document
sendito import requests.http                   # .http / .rest file
sendito import acme.json --into "Imported"     # import into a subfolder
```

`sendito run` exits with 0 on success, 1 when an assertion fails or an MCP tool
returns `isError: true`, 2 when the request fails (network error, gRPC status other
than OK) and 3 on usage errors or unresolved variables. Every flag is documented in
the [CLI reference](docs/CLI.md#sendito-run-request).

Given a folder (or `.` for the current folder; at the collection root, `.` means
everything), `sendito run` sends every request below it in tree order — one
`PASS`/`FAIL`/`ERROR` line each and a summary — and **carries variables
forward**: a value a request extracts is available to the ones after it, so a
login followed by the calls that need its token works as one run.
`--fail-fast` stops at the first failure, `--report json` prints a structured
summary (counts and per-request status) to stdout, and the exit code is 0 (all
passed), 1 (an assertion or tool error), 2 (a request failed) or 3 (usage,
including unresolved variables). Folder runs go to History only when history
is enabled, exactly like single requests.

## See it in action

Short recordings of the main flows, rendered from the app's own frames
(`make media && make render`, before each release). The full-size files are in
[`docs/media/recordings/`](docs/media/recordings).

| Flow | Recording |
|---|---|
| **Create a folder and a request** | ![Create a folder and a request](docs/media/recordings/01-create-folder-and-request.gif) |
| **Send a GET, then a POST with a JSON body** | ![Send a GET, then a POST with a JSON body](docs/media/recordings/02-send-get-and-post-json.gif) |
| **Multipart body with a file part** | ![Multipart body with a file part](docs/media/recordings/03-multipart-upload.gif) |
| **Undefined variable → switch env → send** | ![Undefined variable, switch environment, send](docs/media/recordings/04-switch-env.gif) |
| **gRPC unary call via reflection** | ![gRPC unary call via server reflection](docs/media/recordings/05-grpc-unary-reflection.gif) |
| **gRPC bidi stream: send, half-close** | ![gRPC bidi stream](docs/media/recordings/06-grpc-bidi-stream.gif) |
| **Import a Postman collection** | ![Import a Postman collection](docs/media/recordings/07-postman-import.gif) |
| **Paste curl, export as curl** | ![Paste curl, export as curl](docs/media/recordings/08-curl-paste-and-export.gif) |
| **Resend from History** | ![Resend from History](docs/media/recordings/09-history-resend.gif) |
| **Vim mode editing** | ![Vim mode editing](docs/media/recordings/10-vim-mode-editing.gif) |
| **Drag a request into a folder** | ![Drag a request into a folder](docs/media/recordings/11-mouse-drag-in-tree.gif) |
| **Errors, cancel, delete** | ![Errors, cancel, delete](docs/media/recordings/12-errors-cancel-delete.gif) |
| **An SSE session: receive events, end it** | ![An SSE session](docs/media/recordings/13-sse-session.gif) |
| **An MQTT session: subscribe, publish live, disconnect** | ![An MQTT session](docs/media/recordings/14-mqtt-session.gif) |

## Development

```sh
cargo test --workspace            # unit, integration, corpus, UI flows, snapshots
cargo run -p sendito-testkit --bin echo-servers   # or make servers: HTTP :8787, gRPC :50051, WS :8788, MCP :8789
cargo clippy --all-targets -- -D warnings && cargo fmt --check
```

The `Makefile` wraps the recurring commands with quiet, token-efficient output
(`make help` lists them):

| Target | Does |
|---|---|
| `make build` / `make check` | Build the workspace / type-check only (`P=` for one crate) |
| `make servers` | Echo servers for manual tests (HTTP, gRPC, WS, MCP) |
| `make test` | Full test suite (unit, integration, corpus, UI); `P=` and `F=` narrow it to one crate or test name |
| `make lint` / `make fmt` / `make deny` | Clippy `-D warnings` (`P=` for one crate) / rustfmt / cargo-deny |
| `make ci` | `fmt-check` + `lint` + `deny` + `test`, in order |
| `make docs` | Check that the paths, symbols and `make` targets the documentation cites exist |
| `make corpus` / `make corpus-update` | Run the wire corpus / rewrite its goldens |
| `make keymap` | Regenerate `KEYMAP.md` |
| `make snap` | Accept the insta UI snapshots |
| `make media` / `make render` | Generate the frames / render screenshots and GIFs (before a release) |
| `make bundles` | Build the reviewer bundles |
| `make cov` / `make bench` | Line coverage of every crate but the UI, by the whole test suite / benchmarks |

Crates: `sendito-core` (model, variables, resolution, scripts), `sendito-http`,
`sendito-grpc`, `sendito-ws`, `sendito-mcp` (stdio and Streamable HTTP MCP client, OAuth),
`sendito-store` (files, request format, environments, secrets, config, history,
cookies, session, themes), `sendito-import` (Postman, Insomnia, Bruno, OpenAPI,
`.http`, curl), `sendito-pipeline` (send pipeline and response document model),
`sendito-ui` (terminal UI), `sendito-testkit`
(test servers) and the `sendito` binary. See
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the data flow and the test
layers, and [AGENTS.md](AGENTS.md) for the rules that keep them green.

Dependencies are the official crates.io releases (no vendored or patched crates).
Known limitations and trade-offs are listed in [docs/DEBT.md](docs/DEBT.md).

The correctness corpus lives in `tests/corpus`: every request there is sent to the
echo servers and the exact bytes received are compared with
`tests/corpus/expected`. `UPDATE_CORPUS=1 cargo test --test corpus` rewrites them.

## Documentation

Start at the [documentation index](docs/README.md).

| Document | Contents |
|---|---|
| [docs/FEATURES.md](docs/FEATURES.md) | Every feature, grouped by area |
| [docs/CLI.md](docs/CLI.md) | Every subcommand, flag, exit code, JSON report |
| [FORMAT.md](FORMAT.md) | Request / environment / config schema and wire rules |
| [KEYMAP.md](KEYMAP.md) | Generated key map |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Crates, send pipeline, tests |
| [docs/THEMES.md](docs/THEMES.md) | Theme format and color tokens |
| [docs/DEBT.md](docs/DEBT.md) | Known limitations |
| [docs/media/README.md](docs/media/README.md) | How the screenshots and GIFs are made |
| [docs/reports/](docs/reports/) | Coverage, performance, UX and correctness reports |
| [CHANGELOG.md](CHANGELOG.md) | Release history |

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
