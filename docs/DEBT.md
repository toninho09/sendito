# Technical debt and known limitations

What v0.1.0 knowingly does not do, or does less well than intended, and what it would
take to fix each item.

## 1. The HTTP library does not let a client control the wire exactly

Sendito promises to send what the request file defines. The official `reqwest` 0.13
and `hyper` 1.x get close, but three behaviours cannot be turned off from outside.
Version 0.1.0 uses the official crates (no vendored patches) and accepts these
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
* **Only Linux was run locally.** The CI matrix includes macOS arm64 and the OS keychain
  tests, but none of that has run yet; the release workflow has not run either.
* **Minimum Rust version is unverified.** The code needs at least Rust 1.88
  (edition 2024 let chains); dependencies may need more. Only the current stable
  toolchain has been used.
* **Terminal sizes.** Screenshots, recordings and snapshot tests cover 120×40 and
  200×60. Smaller terminals work (the layout adapts) but are not covered by snapshots.
* **Coverage** is measured for the non-UI crates only (90.72%); `sendito-ui` is covered
  by the 15 flow tests and the snapshot tests, with no line-coverage figure.
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
* HTTP/3 and WebSocket are not supported.
* gRPC connections do not use `[http] proxy`: the tonic connector has no proxy
  support yet. HTTP requests and MCP servers over Streamable HTTP do.
* `[http.tls]` is read when a connection is TLS: a plain `http://` request
  ignores the PEM paths entirely, so a missing file only fails (with its name)
  on an `https://` request or gRPC channel. The same material is loaded once
  per request; a folder run re-reads it for every TLS request.
* OAuth endpoints (HTTP `[oauth]` identities and MCP server OAuth) do not use
  `[http.tls]` or `[http] proxy` yet: authorize/token requests always verify
  with the platform roots, send no client certificate and ignore the
  configured proxy.
* HTTP requests with the cookie jar enabled do not follow redirects, so an
  automatically forwarded Cookie header cannot escape its path or origin scope.
* Response bodies with a `Content-Encoding` other than `gzip`, `deflate` and
  `br` (e.g. `zstd`, stacked codings) are shown as received; `gzip`, `deflate`
  and `br` are decompressed automatically. A truncated body is never decompressed.
* Windows is not a target of this release.
* The `auto` theme picks dark or light from `COLORFGBG` only, which many terminals do
  not set (it then picks `dusk`). Asking the terminal for its background with OSC 11
  (`ESC ] 11 ; ? BEL`) would be more reliable, but needs a raw-mode query with a
  timeout before the first frame, and terminals that do not answer (or multiplexers
  that swallow the reply) must not delay start-up. Set `theme` explicitly meanwhile.

## 4. Housekeeping

* `docs/PLAN.md` is the original specification and verification plan; it states a
  minimum terminal size of 120×40 and the 40-case corpus, which has since grown to
  47 HTTP, 15 gRPC and 5 MCP cases.
