# Architecture

How the code is organized and how a request travels through it. For the rules
that changes must respect, read [AGENTS.md](../AGENTS.md); for the formats, read
[FORMAT.md](../FORMAT.md).

## Workspace

One Cargo workspace (edition 2024, resolver 3) with a binary at the root and a
crate per responsibility. `sendito` is the only binary; everything else is a
library. The non-UI crates build and test without a terminal.

```text
.
├── src/main.rs              # the `sendito` binary entry point
├── src/cli/                 # clap arguments and the run/export/auth/import/theme commands
├── crates/
│   ├── core/                # model, template, vars, resolve, scripts
│   ├── http/                # reqwest/rustls client, decoding, proxy, TLS, timing
│   ├── grpc/                # tonic + prost-reflect + protox schema
│   ├── ws/                  # tokio-tungstenite WebSocket sessions
│   ├── mcp/                 # stdio and Streamable HTTP transports, protocol, OAuth
│   ├── store/               # collection, request files, config, env, history, cookies, session, themes
│   ├── import/              # Postman, Insomnia, Bruno, curl, .http
│   ├── pipeline/            # the send pipeline the UI and the CLI share; response documents
│   ├── ui/                  # ratatui app, draw, overlays, keymap, theme registry
│   ├── widgets/             # text editor, table form, list, icons, color themes
│   └── testkit/             # echo servers, MCP OAuth server, proxy, test PKI
├── tests/                   # integration, corpus (exact bytes), TLS, proxy
├── docs/                    # this documentation
└── bench/, scripts/, Makefile
```

| Crate | Owns |
|---|---|
| `sendito-core` | `Request` / `HttpRequest` / `GrpcRequest` / `WebSocketRequest` / `McpRequest` model, `{{template}}` parsing, variable scope and precedence, placeholder resolution, dynamic values, declarative scripts (pre / extract / assert), secret tainting and scrubbing |
| `sendito-http` | Sending HTTP with reqwest + rustls, header and body assembly, response decompression, proxy, `[http.tls]`, the DNS/connect/TLS/TTFB/download timing breakdown |
| `sendito-grpc` | Schema from reflection or `.proto` (protox), proto3 JSON ↔ protobuf, all four call kinds, metadata, deadlines, the streaming session |
| `sendito-ws` | The WebSocket handshake (headers, subprotocols, `wss://` TLS, HTTP CONNECT proxy and an `https://` proxy's own TLS), the frame loop, the close handshake with its bound and the session deadline |
| `sendito-mcp` | JSON-RPC 2.0, stdio and Streamable HTTP transports, `tools/list` / `tools/call`, OAuth 2.1 with metadata discovery, PKCE and dynamic registration |
| `sendito-store` | The collection tree on disk, canonical request files, layered config, environments and the four secret sources, cookie jar, history, session restore, themes, `init` |
| `sendito-import` | Postman v2.1, Insomnia v4/v5, Bruno, curl (import and export), `.http` export, importing into a folder |
| `sendito-pipeline` | `runner`, the send pipeline that both the UI and the CLI use (scope → pre-request script → resolve → send → extract/assert → history), and the response document the viewer reads (pretty JSON with folding, raw text, hex, search) |
| `sendito-ui` | The ratatui/crossterm app, drawing, overlays, key map and the theme registry. It reaches the protocol crates only through `sendito-pipeline` |
| `sendito-widgets` | Widgets with no app state: the text editor (vim subset), the editable table, list movement, icon sets and color themes |
| `sendito-testkit` | HTTP/gRPC/WebSocket/MCP echo servers that record exact bytes, a fake MCP OAuth server, a forward proxy and a test CA / server / client |
| `sendito` (binary) | Argument parsing, the headless `run`, `curl` / `http` export, OAuth login/logout, `import`, `theme` |

## How a request is sent

The UI and the CLI share one path in `sendito-pipeline::runner`:

```text
file (.toml)
  → store       load the request, collection and environments
  → core        build the variable scope (collection → env → runtime)
  → core        resolve {{placeholders}} in URL, headers, body, metadata, auth
  → core        run pre-request scripts (set runtime variables)
  → http / grpc / mcp            send
  → core        run extract scripts (store values, possibly secret)
  → core        run assertions
  → pipeline     append to history (masked)
  → ui / cli    render the tabs, or print the response
```

Consequences worth knowing:

* **Resolution is shared.** The same `core::resolve` builds the wire request for
  the UI and for `sendito run`, so both produce identical bytes.
* **Scripts are declarative, not a language.** They can only set a variable,
  extract from a JSONPath / header / status, or assert with a fixed operator
  set, which keeps them deterministic and sandbox-free.
* **Secrets stay tainted.** Any value derived from a secret source is marked
  secret, so it is masked when echoed and scrubbed from errors.
* **The wire is defined by the file.** Anything not defined must not appear,
  except the framing/auth/`Content-Type` headers and the library-added
  `Accept: */*` documented in [FORMAT.md](../FORMAT.md) and
  [DEBT.md](DEBT.md).

## Testing

| Layer | Where | Command |
|---|---|---|
| Unit tests | beside the code in each crate | `make test` |
| Wire corpus — exact bytes | `tests/corpus.rs` + `tests/corpus/expected` (49 HTTP, 15 gRPC, 12 WebSocket, 5 MCP) | `make corpus` |
| Rewrite corpus goldens | same | `make corpus-update` |
| Integration (HTTP, TLS, proxy) | `tests/integration.rs`, `tests/tls.rs`, `tests/proxy.rs` | `make test` |
| gRPC calls, MCP client and OAuth | `crates/{grpc,mcp}/tests/` | `make test` |
| WebSocket sessions | `crates/ws/src/lib.rs` (unit tests) and `tests/integration.rs` (cookie jar) | `make test` |
| UI flows (scripted keys and mouse) | `crates/ui/tests/flows.rs` | `make test` |
| UI snapshots (insta, 120×40 and 200×60) | `crates/ui/tests/snapshots.rs` | `make test`, accept with `make snap` |
| Screenshots and GIFs | `crates/ui/tests/media.rs` + `docs/media/render.py` | `make media && make render` |
| Coverage (all but `ui` and `testkit`, ≥ 80 % lines) | cargo-llvm-cov, by the whole test suite | `make cov` |
| Performance | `bench/run.sh` (hyperfine, `script(1)`) | `make bench` |
| Reviewer bundles | `scripts/critic-bundles.sh` | `make bundles` |
| Documentation currency (cited paths, symbols, `make` targets, links) | `tests/docs.rs` | `make docs` |

CI (`.github/workflows/ci.yml`) runs on the toolchain pinned in
`rust-toolchain.toml`: `fmt --check`, clippy with `-D warnings`, `cargo deny`,
the workspace tests on Ubuntu and macOS, the OS-keychain tests and the coverage
gate. Run one heavy job at a time locally: workspace tests, media,
benchmarks and coverage must not overlap.

## Changing behaviour

The repo encodes a change in several places on purpose. For a wire change or a
new request option:

1. the model and resolution in `crates/core` (and the protocol crate if it
   affects the wire);
2. a case in `tests/corpus` and its golden, via `make corpus-update` — review
   the diff;
3. `FORMAT.md` (and `scripts/critic-bundles.sh` rules for the correctness
   reviewer);
4. `docs/FEATURES.md` if it is user-facing;
5. `KEYMAP.md` via `make keymap` if you touched keys or hints (a unit test
   fails when it is stale);
6. `make media && make render` if the UI changed, then update the reference in
   the docs.

For UI changes, keep the settled decisions in
[reports/ux-critic.md](reports/ux-critic.md) before altering dialog keys, hint
rows or status wording. Add a `CHANGELOG.md` entry under the next release.

## Dependency policy

Dependencies are the official crates.io releases — no vendored or patched
crates. When a wire behaviour cannot be controlled through them, it is recorded
in [DEBT.md](DEBT.md) and [FORMAT.md](../FORMAT.md) instead of being patched.
`unsafe` is denied, and clippy denies `unwrap`, `expect`, `dbg!` and `todo!` in
non-test code.

## See also

* [Features](FEATURES.md) — what each crate exposes to the user.
* [CLI](CLI.md) — the binary's commands and flags.
* [AGENTS.md](../AGENTS.md) — where to start, the commands, the corpus, key-map and media rules.
* [reports/](reports/) — coverage, performance and the critic reports.
