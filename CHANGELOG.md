# Changelog

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
