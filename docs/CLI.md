# Command line reference

Every subcommand, flag and exit code of the `sendito` binary. For the file
formats the flags point at, see [FORMAT.md](../FORMAT.md). For the features
behind each command, see [FEATURES.md](FEATURES.md).

```text
sendito [PATH]                       # open the TUI
sendito <command> [args]             # headless
```

`sendito PATH` opens the TUI for `PATH`. The first word is a subcommand only
when it is one of `init`, `run`, `curl`, `http`, `code`, `har`, `auth`,
`serve`, `mcp`, `theme`, `import`, `completions` and `man`.

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
| `--report <format>` | Folder report on stdout: `text` (default: only the stderr lines), `json`, `junit`, `tap` or `html` |
| `--report-file <format>=<path>` | Also write a folder report to a file (`json`, `junit`, `tap`, `html`); repeat it for several formats |
| `--retry <n>` | Send a request again up to `n` times after an error or a failed assertion; replaces each request's `[options] retry` |
| `--repeat <n>` | Run the folder `n` times in a row with one variable scope (folder runs only; default 1) |
| `--delay <ms>` | Wait between two requests (folder runs only; default 0) |
| `--data <file>` | Run once per row of a data file: CSV with the variable names in the first line, or a JSON array of objects (`.json`). Not with `--repeat` |
| `--prompt <label>=<value>` | The value of a `$prompt` or `$select` with that label; repeat it for several labels. Without it, a terminal asks; with no terminal the request is not sent (exit 3) |

One request: the response body goes to **stdout**, and the status, timing,
assertion and test results go to **stderr**, with the Lua script output as
`log:` lines. A failure is signalled by the exit code (see
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

**Retry, repeat and delay.** A request with `[options] retry`
([FORMAT.md](../FORMAT.md#retry)) or a run with `--retry` sends a request
again when it does not pass, after `retry_interval_ms`. A folder run prints a
`RETRY` line for each attempt that did not pass, and the final line says
`attempt 3/3`; the reports give `attempts`. A single request prints the
response of the last attempt only, or of the first attempt that passes.
`--repeat 3` runs the folder three times and names each round in the
report (`Users/list.toml#2`); `--fail-fast` stops all rounds.

**Data files.** `--data users.csv` runs the folder once per row. The first
line of a CSV file gives the variable names; a `.json` file is an array of
objects, and a value that is not a string becomes its JSON text. A row's
values are variables above the folder variables and below the runtime values.
Each row starts with a clean runtime and its own `$response` values, and does
not read the history: a login in row 1 does not serve row 2. The report names
each row (`Users/login.toml[2]`). `--data` also runs a single request once per
row, with the folder output. It cannot be used with `--repeat` (exit 3).
Ctrl+C stops a folder run: the request in flight ends, and the summary and
the reports are written.

### Report formats

`--report` writes one report to stdout; `--report-file` writes one to a file
and can repeat, so one run can give a JUnit file to the CI and an HTML page to
people:

```sh
sendito run . --env ci --report-file junit=results.xml --report-file html=report.html
```

| Format | Content |
|---|---|
| `json` | The summary below |
| `junit` | JUnit XML: one `<testsuite>` named after the folder, one `<testcase>` per request (`classname` is the request's folder, `name` its path), `<failure>` for a failed assertion or test, `<error>` for a request that failed or did not run |
| `tap` | TAP version 13: one test point per request; a request that did not pass gets a YAML block with its outcome, status and problems |
| `html` | One page with no script and no external file: a summary line and a table, with the problems below each request that did not pass |

Assertion results in every format have their secrets masked. A file that
cannot be written prints an error and makes the exit code `3`; the other
reports are still written. A single request ignores `--report-file` and every
`--report` except `json` for SSE and MQTT sessions, with a warning.

### SSE sessions

For `kind = "sse"`, stdout contains each event's data, one line per data line.
Connection and end messages go to stderr. `-i` includes opening response headers.
The client exits successfully on server EOF, maximum event count or session deadline.
Invalid Content-Type, status, compression and transport errors return exit code 2.
A failed assertion returns 1; unresolved variables return 3. Ctrl+C cancels the session.

`--report json` also works for a single SSE request. Its result contains `kind`,
`outcome`, `events`, `status` and `failed_asserts`. Data does not precede the JSON.
Folder report entries for SSE contain `events`; variables extracted from the last
event are available to the next request. `--no-cookies` also applies to SSE.

Set `[sse] timeout_ms = 30000` in config for the default opening timeout.
Use request `[options] deadline_ms` or `max_events` to bound a live session.

### MQTT sessions

For `kind = "mqtt"`, stdout contains each received payload as one block: the
payload bytes as received, then a line break when the payload does not end with
one. `-i` adds a `# <topic> qos=<n>[ retain][ dup]` line before each payload.
Connection, subscription, publish and end messages go to stderr, and a folder
that sets headers prints a note (MQTT has no headers).

The session ends successfully at the request's `[options] deadline_ms`. Without
a deadline it runs until Ctrl+C, which cancels it and returns exit code 2 (as
for SSE). A refused CONNACK, a CONNACK timeout, a TLS error, a transport error
or an invalid request (URL, auth kind, base64 payload) returns exit code 2 with
a short message on stderr. A failed assertion returns 1; unresolved variables
return 3.

`--report json` also works for a single MQTT request. Its result contains
`kind`, `outcome`, `messages`, `status` and `failed_asserts`. Folder report
entries for MQTT contain `messages`; variables extracted from the last payload
are available to the next request.

Set `[mqtt] timeout_ms = 30000` in config for the default CONNACK timeout
(0 = no limit). `sendito curl` and `sendito http` refuse MQTT requests with exit
code 3.

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
      "path": "Users/list users.toml",
      "kind": "http",
      "outcome": "pass",
      "status": "200 OK",
      "duration_ms": 87,
      "asserts": 2,
      "failed_asserts": 0,
      "attempts": 1,
      "unresolved": false,
      "error": null
    }
  ]
}
```

`passed + failed + errors == total`. `attempts` is how many times the request
was sent (`0` when it never ran). A request skipped because of unresolved
variables counts in `errors` and also in `unresolved`. `kind` is `http`, `grpc`,
`websocket`, `sse`, `mqtt` or `mcp`. The exact per-request fields depend on the protocol; a request that
never ran has `status` and `duration_ms` set to `null` and an `error` message.
`hint` is present when a setting causes the failure (or a `401` after an OAuth
token): the sentence with the setting to change.

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

## `sendito har <request-or-folder>`

Prints a HAR 1.2 document of one HTTP request, or of every HTTP request below
a folder (`.` for the current folder), with the folder settings applied.

| Flag | Meaning |
|---|---|
| `-e`, `--env <name>` | Environment to resolve variables with |
| `--include-secrets` | Print secret values instead of `{{placeholders}}` |

Each entry holds the request (method, URL, query, headers, body) and an empty
response with status `0`; its `comment` is the request name, which `sendito
import` reads back as the name. Basic auth becomes an `Authorization` header
(`Basic {{sendito_basic_credentials}}` while a credential is a placeholder);
Digest and AWS Signature V4 are left out with a note in the request
`comment`. A request of another kind is left out with a note on stderr.

---

## `sendito code <request> --lang <language>`

Prints code that sends an HTTP request with another tool.

| Flag | Meaning |
|---|---|
| `-l`, `--lang <language>` | `python` (requests), `fetch` (Node.js 18 or later), `go` (net/http), `httpie` or `wget` |
| `-e`, `--env <name>` | Environment to resolve variables with |
| `--include-secrets` | Print secret values instead of `{{placeholders}}` |

The code sends the method, URL, query, headers, body, timeout, redirect and
TLS options of the request, with its folders applied. Basic auth uses the
tool's own option; Digest auth too where the tool has one (`requests`,
HTTPie, wget). A part of the request that the tool cannot express becomes a
comment at the top: AWS Signature V4, Digest in `fetch` and Go, a multipart
body in wget, TLS verification in `fetch`. The tool can add headers of its
own (a `User-Agent`, for example), so its bytes on the wire can differ from
Sendito's. SSE requests are refused. In the UI: `:code <language>`
(`:code! <language>` includes secrets).

---

## `sendito auth login <identity>` / `logout`

Signs in or out of a named HTTP OAuth identity. Tokens are stored in the OS
keychain per collection, environment and identity.

* `authorization_code` (default grant): opens the browser for a sign-in with
  PKCE and waits for the redirect on a loopback port.
* `device_code`: prints the page and the code to enter there (and opens the
  page), then waits until the sign-in on the other device ends.
* `client_credentials` and `password`: need no sign-in; `login` gets a token
  at once to check the credentials and prints the token endpoint's error code
  when they are refused.

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

## `sendito serve [PATH]`

Serves a collection to an AI agent (Claude Code, Cursor and other MCP
clients) as an MCP server on stdin and stdout. The agent starts the process.
`PATH` defaults to the collection that holds the current directory; a
directory without `.sendito/` above it is an error (exit `3`).

| Flag | Effect |
|---|---|
| `-e, --env <name>` | The environment when a tool call names none |
| `--allow-env <names>` | The only environments the agent can see and send with, separated by commas (or the flag given again). `list_environments` shows only these, and a call that names another one gets an error. Each name must exist, and `--env` must be one of them (else exit `3`). Default: every environment |
| `--read-only` | Offer only the tools that do not change files. The tools that send stay |
| `--no-send` | Offer no tool that sends a request (`run_request`, `run_folder`, `send_request`) |
| `--no-history` | Do not record the sends in the history |
| `--no-cookies` | Do not read or write the cookie jar |

```sh
claude mcp add sendito -- sendito serve ~/api-collection --env local
```

The setup for Claude Desktop and Cursor, and examples, are in the guide
[Let an AI agent use your collection](GUIDES.md#let-an-ai-agent-use-your-collection).

The tools:

| Tool | Does |
|---|---|
| `list_requests` | The requests (path, method or kind, description) and folders, of the collection or of one `folder`, down to `depth` levels |
| `read_request` | The TOML of a request and of the `folder.toml` files that apply to it |
| `list_environments` | The environments and their variables; a secret shows its name and source, never its value |
| `preview_request` | An HTTP or SSE request (`path` or `toml`, `env`) as a curl command, with its folders applied and secrets as `{{placeholders}}`; nothing is sent |
| `import_curl` | The request TOML of a curl `command`, with notes on the options that were ignored; nothing is saved |
| `run_request` | Sends a saved request (`path`, `env`, `prompts`, `retry`, `max_bytes`) |
| `run_folder` | Sends every request under `folder` in tree order (`env`, `prompts`, `retry`, `fail_fast`, `data`); the result is the [JSON report](#the-json-report). With `failures`, it also holds the response of each request that does not pass (cut at `max_bytes`) |
| `send_request` | Sends a request given as TOML (`toml`); nothing is saved |
| `write_request` | Creates or replaces a request file from TOML; missing folders are created |
| `create_folder`, `move`, `delete` | Change the tree; `move` changes nothing when the new path is taken; `delete` removes a folder with its contents |

Each tool has a `title` and an `outputSchema` for its `structuredContent`.
A `run_folder` call with `_meta.progressToken` gets a
`notifications/progress` after each request. A call whose `id` is the id of
a call in flight gets the error `-32600`.

`run_request` and `send_request` return the `outcome` (`pass`, `fail`,
`error`), the `status`, the assertions and the response: `http` (status and
headers) and `body` for HTTP, `messages` for gRPC, WebSocket, SSE, MQTT and MCP.
The body and the messages are cut at `max_bytes` (default 65536) and
`truncated` says so; a body that is not UTF-8 is base64 (`body_encoding`).
Secret values the server echoes back are masked. This masking prevents
accidents, not an agent that wants a secret: a request that the agent writes
can encode a secret, or send it to any host. Use `--allow-env` and
`--no-send` to limit what the agent can use. `prompts` gives the values of
`$prompt` and `$select` (label → value); a label with no value makes the
request an `error` that names it.

The resource `sendito://format` is [FORMAT.md](../FORMAT.md), for the agent to
read before it writes TOML. Paths are relative to the collection root; a path
that leaves the collection or names a hidden file (`.sendito/`, `.env`) is
refused. A tool sees the collection as the tree does: a `.toml` file that is
not a request (`folder.toml`, `Cargo.toml`) and a directory without requests
(`src/` of a code repository) cannot be read, replaced, moved or deleted, and
a tool cannot create a request or a folder inside such a directory. A sent
request reads only the files of the collection: a file body, a multipart
file, a JWT key or a `.proto` file outside the collection, under a hidden
directory, or behind a link that leads out, makes the send an `error`. A change to `.sendito/config.toml` or to the global config is read
before the next tool call; environments are read at each call. Stdout carries
only the protocol; warnings go to stderr. The server stops when stdin closes.

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

Imports a Postman Collection v2.1 JSON, an Insomnia v4/v5 JSON export, a
Bruno collection directory, an OpenAPI 3.x or Swagger 2.0 document (JSON, or
YAML with a `.yaml`/`.yml` extension), a `.http`/`.rest` file (with the
environments of the `http-client.env.json` next to it), or a HAR file.

| Flag | Meaning |
|---|---|
| `--into <folder>` | Folder (relative to the collection root) to import into. Default: the root |

The command must run inside a collection (`sendito init` makes one); outside
one it exits with code 3 and writes nothing.

A report of imported, changed and skipped items is printed. The same import is
available in the UI (`Ctrl+W i` / `:import`). What is imported is listed in
[FEATURES.md](FEATURES.md#import-and-export) and the gaps in
[DEBT.md](DEBT.md).

---

## `sendito completions <shell>`

Prints a completion script for `bash`, `zsh`, `fish`, `elvish` or
`powershell` to stdout. Load it in the shell, for example:

```sh
sendito completions zsh > ~/.zfunc/_sendito      # zsh (fpath+=~/.zfunc)
sendito completions bash > ~/.local/share/bash-completion/completions/sendito
sendito completions fish > ~/.config/fish/completions/sendito.fish
```

## `sendito man`

Prints the manual page (roff) to stdout. The command is not listed in
`--help`. Install the page with `sendito man > ~/.local/share/man/man1/sendito.1`.

---

## Exit codes

| Code | Meaning |
|---:|---|
| `0` | Success: every assertion passed (and, for a folder run, every request succeeded) |
| `1` | A failed assertion or Lua `test`, an error in a Lua post script, or an MCP tool returned `isError: true` |
| `2` | The request failed: an error in a Lua pre script (nothing is sent), network error, a gRPC status other than `OK`, a WebSocket close other than `1000`, an SSE transport/response validation error, or an MQTT session error (refused CONNACK, CONNACK timeout, transport error) |
| `3` | Usage error, including unresolved variables |

For a folder run the exit code is the worst of the four over all requests:
`3` if any request had unresolved variables or usage failed, else `2` if any
request failed, else `1` if any assertion or tool failed, else `0`.

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
* `[history]` — `enabled`, `max_entries`, `max_body_kb`;
* `[ui]` — `theme`, `icons`, `vim_mode`, `sidebar`, `sticky_folders`, `mouse`,
  `confirm_quit`, `watch_interval_s`;
* `[keymap.all]`, `[keymap.conventional]`, `[keymap.vim]`.
