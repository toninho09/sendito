# Command line reference

Every subcommand, flag and exit code of the `sendito` binary. For the file
formats the flags point at, see [FORMAT.md](../FORMAT.md). For the features
behind each command, see [FEATURES.md](FEATURES.md).

```text
sendito [PATH]                       # open the TUI
sendito <command> [args]             # headless
```

`args_conflicts_with_subcommands = true`: `sendito PATH` opens the TUI for
`PATH`; the first word is only read as a subcommand when it is one
(`init`, `run`, `curl`, `http`, `auth`, `mcp`, `theme`, `import`).

`PATH` defaults to the nearest directory above the current one that contains
`.sendito/`, else the current directory.

## Environment variables

| Variable | Effect |
|---|---|
| `XDG_CONFIG_HOME` | Base for the global config (`<base>/sendito/config.toml`); defaults to `~/.config` |
| `NO_COLOR` | Set (any value) disables color, whatever the theme |
| `COLORFGBG` | Read by the `auto` theme to pick `daylight` or `dusk` |
| `SENDITO_KEYCHAIN=memory` | Use an in-memory keychain instead of the OS keychain |
| `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY` | Used when `[http] proxy = "env"`, and as the client's default discovery when `proxy` is unset (curl precedence) |

Colors and themes are described in [THEMES.md](THEMES.md).

---

## `sendito [PATH]`

Opens the interactive UI for the collection at `PATH`.

| Argument | Meaning |
|---|---|
| `PATH` | Collection directory. Default: nearest `.sendito/` above the current directory, else the current directory |

The UI has its own keys; see [KEYMAP.md](../KEYMAP.md) and the in-app help
(`F1`).

---

## `sendito init [PATH]`

Creates a collection: `.sendito/`, `collection.toml`, an empty `local`
environment and the `.gitignore` entries for the local state files.

| Argument | Meaning |
|---|---|
| `PATH` | Directory to create the collection in. Default: the current directory |

Creates no requests. Files and ignore rules are listed in
[FORMAT.md](../FORMAT.md#project-and-global-config).

---

## `sendito run <request>`

Sends a request without the UI, or runs every request in a folder.

| Flag | Meaning |
|---|---|
| `-e`, `--env <name>` | Environment to resolve variables with |
| `-i`, `--include` | Also print the response headers to stderr (one request only; rejected for folder runs) |
| `--allow-unresolved` | Send even when some variables are unresolved |
| `--no-history` | Do not append the request to history |
| `--no-cookies` | Do not use the collection's cookie jar for this run |
| `--fail-fast` | Stop at the first failed or errored request (folder runs only) |
| `--report <text\|json>` | Folder report format; `text` (default) writes lines to stderr, `json` writes the summary to stdout |

One request: the response body goes to **stdout**, and the status, timing and
assertion results go to **stderr**. A failure is signalled by the exit code (see
below), so `sendito run x > body.txt` is safe: redirected output holds the body
bytes unchanged (a line break is added only when stdout is a terminal). A body
cut at `[http] max_body_mb` prints a warning on stderr. A variable whose secret
did not load (no keychain entry, no `secrets.toml` value) counts as unresolved,
also when the collection defines the same name; an active environment file that
cannot be read makes every collection variable unresolved.

For a WebSocket request (`kind = "websocket"`) the session opens, the message
document is sent, the text frames received are printed on **stdout** (one per
line), and the handshake status, the frames sent, the close code and the
assertions go to **stderr**. `--include` also prints the handshake response
headers. Without a `deadline_ms` the run closes the session as soon as the
opening messages are out; with one, it reads frames until the deadline ends the
session (a close with `1000` and a `deadline …` reason). Either way the close
handshake is bounded (5 s), so the process cannot hang on a peer that never
answers. A session that does not close with code 1000 exits `2`.

A folder (or `.` for the current folder — the whole collection at its root) runs
every request below it in tree order, one `PASS`/`FAIL`/`ERROR` line each plus a
summary. **Variables a request extracts are carried forward** to the requests
after it, so a login followed by its dependent calls runs as one command.

Folder runs honour `[history]` the same way single requests do.

### The JSON report

With `--report json` the summary is printed to stdout:

```json
{
  "environment": "local",
  "folder": "Users",
  "total": 3,
  "passed": 2,
  "failed": 1,
  "errors": 0,
  "unresolved": 0,
  "duration_ms": 412,
  "requests": [
    {
      "name": "Users/list users",
      "kind": "http",
      "outcome": "pass",
      "status": "200 OK",
      "duration_ms": 87,
      "asserts": 2,
      "failed_asserts": 0,
      "unresolved": false,
      "error": null
    }
  ]
}
```

`passed + failed + errors == total`. A request skipped because of unresolved
variables counts in `errors` and also in `unresolved`. `kind` is `http`, `grpc`
or `mcp`. The exact per-request fields depend on the protocol; a request that
never ran has `status` and `duration_ms` set to `null` and an `error` message.

---

## `sendito curl <request>`

Prints the request as a `curl` command.

| Flag | Meaning |
|---|---|
| `-e`, `--env <name>` | Environment to resolve variables with |
| `--include-secrets` | Print secret values instead of `{{placeholders}}` |

Secret variables stay `{{placeholder}}` unless `--include-secrets` is given.
The same conversion is available in the UI as `Ctrl+W u` / `:curl` (`:curl!`
includes secrets).

---

## `sendito http <request>`

Prints the request as an `.http` file (JetBrains HTTP client / VS Code REST
Client).

| Flag | Meaning |
|---|---|
| `-e`, `--env <name>` | Environment to resolve variables with |
| `--include-secrets` | Print secret values instead of `{{placeholders}}` |

Each request becomes an `### <name>` block with its method, URL, headers and
body. Directives that VS Code REST Client understands (JetBrains ignores them
as comments) are emitted when they differ from the editor's default:
`# @timeout <ms>` and `# @no-redirect`. A file body is written as `< path`, and
a multipart body with an explicit boundary. With Basic auth and unresolved
credentials the export uses
`Authorization: Basic {{sendito_basic_credentials}}`, which must be set to
base64(username:password). In the UI: `:httpfile` (`:httpfile!` includes
secrets).

---

## `sendito auth login <identity>` / `logout`

Signs in or out of a named HTTP OAuth identity (authorization code + PKCE).
Tokens are stored in the OS keychain per collection, environment and identity.

| Subcommand | Flags |
|---|---|
| `auth login <identity>` | `-e`, `--env <name>` |
| `auth logout <identity>` | `-e`, `--env <name>` |

The identity is a `[oauth.<name>]` table in the config. OAuth endpoints and
protected URLs must use HTTPS except on loopback. A `401` at send time triggers
one token refresh and retry. In the UI: `:auth login <identity>`.

---

## `sendito mcp login <server>` / `logout`

Runs (or forgets) the OAuth login for an MCP server from `[mcp.servers]`.

| Subcommand | Arguments |
|---|---|
| `mcp login <server> [--env <name>]` | server name; the environment renders `{{variables}}` in the URL and OAuth settings |
| `mcp logout <server>` | server name |

The authorization URL is printed and a browser is opened when available; the
redirect must reach Sendito's local loopback listener. Tokens are stored in the
OS keychain under `<collection>/mcp/<server>`. Use it before calling a server
whose auth table has `type = "oauth"`. In the app, `:auth login <server>` does the
same, and a server that asks for a sign-in offers it.

---

## `sendito theme list` / `print`

| Subcommand | Flags | Output |
|---|---|---|
| `theme list` | `--collection <path>` | Every built-in and user theme with its file and any problems |
| `theme print <name>` | `--collection <path>` | The theme as a complete TOML file with every color token |

`--collection` also searches `<collection>/.sendito/themes/` for that collection.
`theme print` exits 3 when the name is unknown. Both are described in
[THEMES.md](THEMES.md).

---

## `sendito import <file-or-directory>`

Imports a Postman Collection v2.1 JSON, an Insomnia v4/v5 JSON export, or a
Bruno collection directory.

| Flag | Meaning |
|---|---|
| `--into <folder>` | Folder (relative to the collection root) to import into. Default: the root |

A report of imported, changed and skipped items is printed. The same import is
available in the UI (`Ctrl+W i` / `:import`). What is imported is listed in
[FEATURES.md](FEATURES.md#import-and-export) and the gaps in
[DEBT.md](DEBT.md).

---

## Exit codes

| Code | Meaning |
|---:|---|
| `0` | Success: every assertion passed (and, for a folder run, every request succeeded) |
| `1` | A failed assertion, or an MCP tool returned `isError: true` |
| `2` | The request failed: network error, a gRPC status other than `OK`, or a WebSocket close other than `1000` |
| `3` | Usage error, including unresolved variables |

For a folder run the exit code is the worst of the four over all requests:
`3` if any request had unresolved variables or usage failed, else `2` if any
request failed, else `1` if any assertion or tool failed, else `0`.

---

## Internal flags (hidden)

These are hidden from `--help` and used by the benchmarks, the key-map
generation and the tests. They are documented for contributors; they are not a
stable interface.

| Flag | Use |
|---|---|
| `--exit-after-first-frame` | Quit after drawing the first frame (cold-start benchmark) |
| `--frame-log <file>` | Write per-frame render times (µs) to a file |
| `--script <file>` | Replay scripted input from a file |
| `--print-keymap` | Print [KEYMAP.md](../KEYMAP.md) to stdout (`make keymap`) |

The TUI path is used by `make bench`, `make keymap` and the UI test harness.

---

## Configuration tables these commands read

`run`, `curl`, `http`, `auth` and `mcp` all read the config; the tables are
documented in [FORMAT.md](../FORMAT.md):

* `[http]` — `timeout_ms`, `max_body_mb`, `cookie_jar`, `proxy`, `tls`;
* `[http.tls]` — `ca`, `client_cert`, `client_key`;
* `[oauth.<identity>]` — `authorize_url`, `token_url`, `client_id`,
  `client_secret`, `scopes`;
* `[grpc]` — `deadline_ms`;
* `[mcp]`, `[mcp.servers.<name>]`, `[mcp.servers.<name>.auth]`;
* `[history]` — `enabled`, `max_entries`;
* `[ui]` — `theme`, `icons`, `vim_mode`, `sidebar`, `mouse`, `confirm_quit`,
  `watch_interval_s`;
* `[keymap.all]`, `[keymap.conventional]`, `[keymap.vim]`.
