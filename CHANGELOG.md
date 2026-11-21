# Changelog

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
