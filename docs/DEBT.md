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
* **Coverage** is measured for the non-UI crates only (91.77%); `sendito-ui` is covered
  by the 13 flow tests and the snapshot tests, with no line-coverage figure.

## 3. Product limits worth knowing

* No JavaScript scripting: scripts are declarative (set, extract, assert). Postman
  scripts are reported as not imported.
* Postman import covers v2.1 collections; OAuth 2 and Digest auth are not imported
  (reported), and neither are saved example responses.
* gRPC supports unary and the three streaming kinds over plaintext or TLS; there is
  no gRPC-Web.
* HTTP/3 and WebSocket are not supported.
* Response bodies are shown as received; there is no automatic decompression.
* Windows is not a target of this release.

## 4. Housekeeping

* `docs/PLAN.md` is the original specification and verification plan; it states a
  minimum terminal size of 120×40 and the 40-case corpus, which has grown to 44 HTTP
  cases.
