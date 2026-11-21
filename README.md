# Sendito

A terminal API client for **HTTP** and **gRPC**. It runs locally in your terminal,
works with the mouse, conventional keys or vim keys, and keeps your requests as
plain TOML files you can version in git.

```
 GET list users × │ POST create user ● × │ +                    env: local ▾  F1 all keys
╭  Requests  History ────╮╭ Request · Users/list users.toml ──────────────────────╮
│▾ Users/                ││ GET    ▾ {{base_url}}/users                      Send │
│  POST  create user     ││ Params (1) │ Headers (1) │ Body │ Auth•  │ Scripts │ … │
│  DEL   delete user     ││    Name               Value                           │
│  GET   list users      ││[x] page              │1                              ✕│
│▸ Orders/               ││ +  add name          │                                │
│  GRPC  echo unary      │╰───────────────────────────────────────────────────────╯
│  GET   health          │╭ Response ─────────────────────── 200 OK  14 ms 219 B ─╮
│                        ││ Body │ Headers │ Timing │ Results │  Pretty  Raw  Hex  │
│                        ││▾ {                                                    │
│                        ││▾   "items": [                                         │
╰────────────────────────╯╰───────────────────────────────────────────────────────╯
 Ctrl+Enter/F5 send  Ctrl+S save  Ctrl+P commands  F1 help
 env: local │ 200 OK │ 14 ms │ 219 B
```

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
  A corpus of 44 HTTP and 15 gRPC cases is checked byte for byte in CI.
* **HTTP**: every method, query params, headers, raw / JSON / form / multipart
  (with files) / file bodies, Basic / Bearer / API-key auth, timeout, redirects,
  TLS verification, and a DNS / connect / TLS / TTFB / download breakdown.
* **gRPC**: services from server reflection or `.proto` files (compiled in-process,
  no `protoc`), unary, server-, client- and bidi-streaming calls, JSON messages
  with generated templates, metadata, TLS, deadlines, cancellation.
* **Environments** with secrets from four sources: the env file, a gitignored
  secrets file, the OS keychain, or a file. Secrets are masked everywhere.
* **Scripts without code**: set variables before sending, extract values (JSONPath,
  header, status) after, and assert on them.
* **History**, **Postman v2.1 import**, **curl import/export**, session restore,
  themes, and a key map you can change.

## Install

Download a release archive for Linux x86_64 or macOS arm64 from the
[releases page](https://github.com/toninho09/sendito/releases), check it and put the
binary on your `PATH`:

```sh
tar xzf sendito-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
sha256sum -c sendito-v0.1.0-x86_64-unknown-linux-gnu.tar.gz.sha256
sudo install sendito /usr/local/bin/
```

Or build from source (a recent stable Rust; the code needs 1.88 or newer for edition-2024 let chains, and CI builds with the current stable):

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

1. Press `n` in the sidebar to create a request (choose HTTP or gRPC and a folder):
   its file is created at once, empty. `Ctrl+N` instead opens a scratch tab that has
   no file until `Ctrl+S` asks for a name and folder.
2. Type the URL, e.g. `{{base_url}}/users`, and press **Ctrl+Enter** or **F5**.
   Sending uses the request as shown, unsaved edits included.
3. `Ctrl+S` saves; `F4` switches environment; `F1` lists every key.

Without the UI:

```sh
sendito run "Users/list users" --env local     # body to stdout, status to stderr
sendito run checks/health --env prod || echo "an assertion failed"
sendito curl "Users/create user" --env local   # print as curl (secrets stay {{placeholders}})
sendito import acme.postman_collection.json    # Postman v2.1
```

`sendito run` exits with 0 on success, 1 when an assertion fails, 2 when the
request fails (network error, gRPC status other than OK) and 3 on usage errors.

## The screen

* **Tab bar** (top): open requests (`●` = unsaved changes, `↻` = running), `+`
  for a new one, the environment selector and the help button.
* **Sidebar**: **Requests** (the collection tree) or **History**. Method badges show the type;
  requests that are open in a tab are in italics. `/` filters. The sidebar can be
  on the right and resized.
* **Request**: method / URL (or target / method for gRPC) and **Send**, then the
  tabs **Params · Headers · Body · Auth · Scripts · Options** (gRPC: **Message ·
  Metadata · Auth · Scripts · Options**). A tab label shows its number of rows,
  `Headers (2)`, or `•` when Body or Auth is set.
* **Response**: status, time and size in the border, then **Body** (pretty JSON
  with folding, raw, hex, search) · **Headers** (response and the request that was
  sent, secrets masked) · **Timing** · **Results** (assertions, extracted
  variables, notes). For gRPC the tabs are **Body · Metadata · Timing ·
  Log**, where Log lists every message sent and received (legend pinned on top: `→` sent,
  `←` received, `■` final status, `+s` seconds since the call started) plus the
  results.
  gRPC messages are shown as proto3 JSON: fields holding their default value
  are not on the wire and are not shown (assertions still see them, so
  `$.count == 0` works).
* **Hint row**: the most useful keys for what is focused. **Status line**:
  environment, unsaved count, last status, time, size, messages, vim mode.

Panels resize with the mouse (drag the borders) or `Ctrl+W <`/`>` and `Ctrl+W +`/`-`;
`Ctrl+W z` maximizes the focused pane, `Ctrl+B` hides the sidebar.

## Keys

Conventional keys by default; vim mode in Settings (`Ctrl+W ,`). The most used:

| Key | Action |
|---|---|
| `Ctrl+Enter` / `F5` / `Ctrl+W e` | send (on an open client/bidi stream: send the next message; the sent text stays selected so you can type the next one over it) |
| `Ctrl+W x` | cancel the running request or stream |
| `Ctrl+W d` | end the request stream (client/bidi gRPC) |
| `Ctrl+S` | save (asks for a name and folder the first time) |
| `Ctrl+P` | command palette: actions, requests, environments and `:commands` |
| `Tab` / `Shift+Tab` | next / previous field or pane |
| `Ctrl+W h/j/k/l` | focus left / down / up / right |
| `Ctrl+W n` / `Ctrl+W p` | next / previous tab · `Ctrl+W c` close · `Ctrl+N` new HTTP · `Ctrl+W g` new gRPC |
| `Alt+1`…`Alt+6`, `Ctrl+W ]`/`[` | switch the tabs of the focused pane |
| `F4` | environment · `Ctrl+W V` edit environments |
| `F7` | history · `Ctrl+W i` import · `Ctrl+W u` copy as curl |
| `F1` | help (the full key map) · `Ctrl+W q` quit |

In the sidebar: `n` new request, `N` new folder, `r` rename, `d` duplicate,
`m` move, `Delete` delete, `/` filter. In the response: `p`/`r`/`x` pretty / raw /
hex, `/` search with `n`/`N`, `Enter` fold, `-`/`+` fold / unfold all, `w` wrap,
`y` copy, `s` save the body. Tables: `Enter` next cell, `Ctrl+T` enable/disable a
row, `Ctrl+D` delete it.

Copying (`y`, `Ctrl+W u`) goes through the terminal's clipboard support (OSC 52);
inside tmux enable it with `set -g set-clipboard on`.

Ctrl+Enter needs a terminal that reports it (kitty, Alacritty, WezTerm, foot,
Ghostty, iTerm2 with CSI u); `F5` and `Ctrl+W e` work everywhere.

[KEYMAP.md](KEYMAP.md) lists every action and is generated from the same table as
the in-app help. Keys can be changed per action and mode in the config:

```toml
[keymap.all]
send = ["f9", "ctrl+w e"]
[keymap.vim]
tree_delete = "D"
```

### Mouse

Click to focus, select rows, switch and close tabs (middle-click closes too) and
press buttons; double-click opens a request; drag a tree item onto a folder to
move it; drag pane borders to resize; the wheel scrolls the pane under the pointer.

### Vim mode

Editors are modal (normal / insert / visual / visual-line) with `hjkl`, `w b e`,
`0 ^ $`, `gg G`, `x dd D cc C yy p P u Ctrl+R`, `v V` then `d y c`, and `:` opens
the palette in command mode (`:w`, `:send`, `:env dev`, `:q`). The tree and the
response viewer use `hjkl`, `gg`/`G` and `/`. The status line shows the mode.

## Environments and secrets

Variables are written `{{name}}` anywhere: URL, params, headers, bodies, gRPC
messages and metadata, auth fields. Collection variables apply to every
environment; the active environment overrides them; scripts override both.
Undefined variables are shown in red and named in the status line, and sending
asks before going ahead.

```toml
# .sendito/environments/local.toml
[variables]
base_url = "http://localhost:8080"          # plain text, versioned
token = { source = "local" }                # .sendito/secrets.toml (gitignored;
                                            # "secrets" in the environment editor)
api_key = { source = "keychain" }           # OS keychain
client_key = { source = "file", path = "~/.acme/key" }
```

Set secret values in the environment editor (`Ctrl+W V`), which writes them to
the right place and shows whether each one is set. Secret values are never
shown, never written to history and never included in curl exports unless you
run `:curl!`. When a server echoes a secret back (in a body, a header or gRPC
metadata), the app shows `••••••` in its place; `sendito run` writes the body
to stdout unchanged.

History keeps each request as written, with its `{{variables}}`: opening or
resending an entry resolves it again with the active environment.

## Scripts (no code)

The **Scripts** tab has three sections:

* **Pre-request**: set variables, e.g. `request_id = req-{{$uuid}}`.
* **Extract**: after the response, store a JSONPath result, a header or the status
  in a variable (optionally secret), e.g. `token ← $.access_token`.
* **Assertions**: check status, a header or a JSONPath value with
  `== != contains !contains exists !exists < <= > >= matches`. Results appear in
  the **Results** tab (**Log** for gRPC) and decide the exit code of
  `sendito run`.

## Import and export

* `Ctrl+W i` (or `:import path`) imports a Postman Collection v2.1 (a relative
  path is looked up in the working directory, then the collection folder): folders,
  requests, bodies, auth (including inherited auth) and collection variables. A
  report lists everything that was not imported as-is (scripts, saved examples,
  unsupported auth types, path variables replaced by their value, …).
* Paste a `curl …` command into the URL field or the import dialog to turn it into
  a request. `Ctrl+W u` (or `:curl`) copies the request as curl.

## Configuration

Global `~/.config/sendito/config.toml` (respects `XDG_CONFIG_HOME`), overridden by
the collection's `.sendito/config.toml`. Settings changed in the app are saved to
the global file. Themes: `auto`, `ansi`, `dusk`, `daylight`, `contrast`;
`NO_COLOR=1` turns colors off. All keys are described in [FORMAT.md](FORMAT.md).

The Settings dialog (`Ctrl+W ,`) changes, at once:

| Setting | What it does |
|---|---|
| Vim mode | vim keys in editors and lists (normal / insert modes) |
| Theme | colors; `auto` follows the terminal background |
| Sidebar position | Requests / History sidebar on the left or right |
| Mouse support | clicks, wheel scrolling, dragging tree items and pane borders |
| Confirm quit with unsaved changes | ask before quitting while tabs have unsaved changes |
| Record history | keep sent requests in History (`F7`) to reopen or resend |

## Development

```sh
cargo test --workspace            # unit, integration, corpus, UI flows, snapshots
cargo run -p sendito-testkit --bin echo-servers   # HTTP echo :8787, gRPC echo :50051
cargo clippy --all-targets -- -D warnings && cargo fmt --check
```

Crates: `sendito-core` (model, variables, resolution, scripts), `sendito-http`,
`sendito-grpc`, `sendito-store` (files, environments, secrets, config, history),
`sendito-import` (Postman, curl), `sendito-ui` (terminal UI and the send
pipeline), `sendito-testkit` (test servers) and the `sendito` binary.

Dependencies are the official crates.io releases (no vendored or patched crates).
Known limitations and trade-offs are listed in [docs/DEBT.md](docs/DEBT.md).

The correctness corpus lives in `tests/corpus`: every request there is sent to the
echo servers and the exact bytes received are compared with
`tests/corpus/expected`. `UPDATE_CORPUS=1 cargo test --test corpus` rewrites them.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. The vendored `reqwest` and `hyper` keep
their own licenses in `vendor/`.
