# Technical debt and known limitations

What the current release knowingly does not do, or does less well than intended,
and what it would take to fix each item.

## 1. The HTTP library does not let a client control the wire exactly

Sendito promises to send what the request file defines. The official `reqwest` 0.13
and `hyper` 1.x get close, but three behaviours cannot be turned off from outside.
Sendito uses the official crates (no vendored patches) and accepts these
differences. They are cosmetic or semantically equivalent in HTTP, but they are
differences, and the correctness corpus and the documentation state them.

| Behaviour | Official library | What the request file says | Impact |
|---|---|---|---|
| Implicit `Accept: */*` | `reqwest` adds it to every request without an `Accept` header and offers no switch | no `Accept` | One extra header on requests that define no `Accept`. A request that defines `Accept` is unaffected. |
| Header name spelling | `hyper` writes names in Title-Case (`http1_title_case_headers`); the type that would keep the original spelling is private to the crate | `x-api-key`, `X-API-KEY`, … | `X-Api-Key` goes out for every spelling. Header names are case-insensitive (RFC 9110 §5.1), and HTTP/2 lowercases them anyway. |
| Header order | `HeaderMap` groups values by name | `A: 1`, `B: 2`, `A: 3` | Sent as `A: 1`, `A: 3`, `B: 2`. Order between different names is not significant; order within one name is kept. |

Where it shows: `FORMAT.md` (Headers, Added headers), the README, the correctness
critic's bundle README (`scripts/critic-bundles.sh`) and `tests/corpus.rs`, which
allows `accept` as a transport header.

**How to remove the debt**

* Upstream: ask `hyper` to expose the original-case map to clients (a visibility change
  plus a constructor) and `reqwest` for a way to drop the
  default `Accept`. Both were patched locally before 0.1.0, and the patches are small.
* Meanwhile a fork referenced through `[patch.crates-io]` by git, or vendored copies,
  restores exact behaviour. The patches were: remove the default `Accept` in
  `reqwest::async_impl::client`, forward `HeaderCaseMap` and a `HeaderOrder`
  extension from the request to the hyper request, make `HeaderCaseMap` public, and
  add a `HeaderOrder` extension that the HTTP/1 encoder honours. Restoring them also
  restores the byte-exact wire captures that the corpus had before.
* Alternative: build requests on `hyper` directly and rewrite multipart, redirects and
  the TLS layer that `reqwest` provides.

## 2. Verification gaps

* **UX review did not converge.** An independent UX critic reviewed 18 rounds and never
  reported "nothing above minor"; each round found new, mostly contradictory,
  suggestions. The settled decisions and every declined finding are in
  `docs/reports/ux-critic.md`. No usability test with real users has been done.
* **Real-terminal recordings are missing.** `docs/vhs/*.tape` have never been rendered
  (VHS and ttyd were not installed). The GIFs in `docs/media/recordings` are rendered
  from the application's own frames, not from a terminal, and the tape set has 11 of
  the 12 flows.
* **macOS and the OS keychains have only CI behind them.** The `test` matrix
  (Ubuntu and macOS arm64, with the Secret Service and Keychain steps) and the
  release workflow (Linux x86_64 and macOS arm64) went green once, for v0.1.0 on
  2026-11-21. The OS-keychain tests run only with `SENDITO_TEST_OS_KEYCHAIN=1`,
  which CI sets.
* **Minimum Rust version is unverified.** `rust-toolchain.toml` pins 1.100.0,
  the toolchain CI and the release build use. The code needs at least Rust 1.89
  (edition 2024 let chains; the `jaq` jq library sets this floor) and other
  dependencies may need more; nothing between 1.89 and 1.100.0 has been tried.
* **Terminal sizes.** Screenshots, recordings and snapshot tests cover 120×40 and
  200×60. Smaller terminals work (the layout adapts) but are not covered by snapshots.
* **Coverage** is measured for everything but `sendito-ui` and `sendito-testkit`,
  by the whole test suite (`docs/reports/coverage.md`). The CLI's `src/main.rs`
  counts, and the tests reach only the commands they run. `sendito-ui` is covered
  by the flow tests and the snapshot tests, with no line-coverage figure.
* **The visual redesign was checked in the test backend only.** The cursor shape (bar
  while typing, block in vim NORMAL), the window title push/pop (`CSI 22;0t` /
  `CSI 23;0t`), mouse hover, the Braille spinner in fonts without Braille, Nerd Font
  icon alignment, the 256-color path (`TERM=xterm-256color` without `COLORTERM`),
  `NO_COLOR` and the `ansi` theme's underline fallback have not been looked at in a
  real terminal.

## 3. Product limits worth knowing

* No JavaScript scripting: scripts are declarative (set, extract, assert). Postman
  scripts are reported as not imported.
* Folder runs (`sendito run <folder>`) are sequential and share one variable
  scope in tree order; there is no parallelism, no per-request retry and no
  JUnit/TAP writer (the JSON report is the machine format). A request that
  fails does not stop the run unless `--fail-fast` is given, and a request with
  unresolved variables is reported as an error and skipped.
* Postman import covers v2.1 collections; OAuth 2 and Digest auth are not imported
  (reported), and neither are saved example responses. Bruno and Insomnia imports
  cover supported HTTP requests and environments, not arbitrary scripts or template
  engines; skipped features and secret environments are reported.
* gRPC supports unary and the three streaming kinds over plaintext or TLS; there is
  no gRPC-Web.
* MCP supports tools only; resources and prompts are not implemented. Each list
  or call opens a separate session. The legacy HTTP+SSE transport and the
  server-initiated Streamable HTTP GET stream are not supported. Image and other
  non-text tool content is shown as JSON, not decoded into a media viewer.
* An MCP `arguments` value that is not a JSON object is kept as file text and only
  rejected when the call is sent (so a half-typed object can be saved and
  reopened); the editor shows `✓ JSON` / `✗ JSON` while typing.
* MCP OAuth login uses configured URLs and OAuth settings literally, without
  resolving environment placeholders. The local callback requires a browser able
  to reach the machine running Sendito.
* The jq response filter runs synchronously on the UI thread with no output
  or time cap: a pathological expression (`repeat(.)`, `[range(1e12)]`) freezes
  the UI and grows memory until the expression finishes or the process is
  killed. `halt`, `halt_error` and `stderr` are removed from the builtin set,
  `debug` logs nowhere (no logger is initialized) and `input`/`inputs` yield
  nothing (no further inputs). Removing the freeze needs a step budget or
  a cancellable worker thread, neither of which jaq's `run` API offers.
* **Echo masking is textual.** A secret is masked in responses when it appears
  as sent, JSON-escaped or percent-encoded (form and query, either hex case).
  Other encodings — base64, HTML entities, `\uXXXX` escapes of non-ASCII
  characters, hashes — are not recognised, and a secret shorter than 3
  characters is never masked in what a server sends back. Closing the gap needs
  decoding the response by content type before matching.
* HTTP/3 is not supported.
* gRPC connections do not use `[http] proxy`: the tonic connector has no proxy
  support yet. HTTP requests, MCP servers over Streamable HTTP and WebSocket
  sessions do.
* `[http.tls]` is read when a connection is TLS: a plain `http://` request
  ignores the PEM paths entirely, so a missing file only fails (with its name)
  on an `https://` request or gRPC channel. The PEM bytes are kept and read
  again only when a file's size or time changes; the TLS configuration is
  still built for each request.
* Each HTTP send uses a new client, so connections are not reused (a folder
  run or a watch does a TCP and TLS handshake per request). This keeps the
  DNS / connect / TLS timing of every send; reuse would need the timing marks
  per request instead of per client.
* Response bodies with a `Content-Encoding` other than `gzip`, `deflate` and
  `br` (e.g. `zstd`, stacked codings) are shown as received; `gzip`, `deflate`
  and `br` are decompressed automatically. A truncated body is never decompressed.
* Windows is not a target of this release.
* **WebSocket proxies are HTTP(S) CONNECT only.** A `socks4`/`socks4a`/`socks5`/
  `socks5h` proxy URL and `proxy = "env"` (which cannot be resolved without
  reading the environment per request) are refused with a message instead of
  being silently ignored. Fixing it means implementing the SOCKS handshake and
  the `NO_PROXY` matching that `reqwest` provides for HTTP.
* **WebSocket handshake headers are written by `tungstenite`.** The library
  writes `Host`, `Connection`, `Upgrade`, `Sec-WebSocket-Version` and
  `Sec-WebSocket-Key` first and then the configured headers; a configured header
  keeps the spelling used by the library for `Sec-WebSocket-Protocol` and
  `Origin` and the original spelling for the rest. Header order is the file
  order after those five. The `Sec-WebSocket-Key` is random per session, so it
  is masked in the corpus captures.
* **No WebSocket extensions are negotiated.** `permessage-deflate` is not
  offered, so a server cannot compress frames and Sendito never has to
  decompress them; a server that requires the extension must be reached
  without it.
* **`curl` cannot import a WebSocket session.** `curl ws://…` is refused with a
  message pointing at `:newws`, because an upgrade header pair does not
  describe a session. Postman, Insomnia and Bruno exports that contain
  WebSocket requests import only their HTTP requests.
* The `auto` theme picks dark or light from `COLORFGBG` only, which many terminals do
  not set (it then picks `dusk`). Asking the terminal for its background with OSC 11
  (`ESC ] 11 ; ? BEL`) would be more reliable, but needs a raw-mode query with a
  timeout before the first frame, and terminals that do not answer (or multiplexers
  that swallow the reply) must not delay start-up. Set `theme` explicitly meanwhile.

## 4. Housekeeping

* `docs/PLAN.md` is the original specification and verification plan; it states a
  minimum terminal size of 120×40 and the 40-case corpus, which has since grown to
  49 HTTP, 15 gRPC, 12 WebSocket and 5 MCP cases.
