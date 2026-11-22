# Sendito file formats

A Sendito collection is a plain directory that you can keep in git. Every saved
request is one TOML file; every directory is a folder in the sidebar tree.

```
my-api/
├── .gitignore                    # sendito init adds the local files below
├── .sendito/
│   ├── config.toml               # project settings (versioned)
│   ├── collection.toml           # collection-level variables (versioned)
│   ├── environments/
│   │   ├── local.toml            # selectable environments (versioned)
│   │   └── staging.toml
│   ├── secrets.toml              # local secret values      (gitignored)
│   ├── cookies.toml              # per-environment cookie jar (gitignored)
│   ├── cookies.lock              # cookie jar lock (gitignored)
│   ├── session.toml              # open tabs, drafts, layout (gitignored)
│   └── history.jsonl             # sent requests, masked    (gitignored)
├── Users/
│   ├── list users.toml           # a request: the file name is its name
│   └── create user.toml
└── health.toml
```

* The collection root is the directory that contains `.sendito/`. `sendito` with
  no argument searches upward from the current directory.
* Every `*.toml` file under the root whose first keys include `kind = "http"` or
  `kind = "grpc"` or `kind = "mcp"` is a request. Other `.toml` files (e.g. `Cargo.toml`) are ignored.
* Hidden entries (starting with `.`) and the directories listed in
  `[collection] ignore` are skipped. Folders that only contain unrelated files are
  hidden; empty folders are shown (they are folders you created).
* Items are sorted folders first, then by name (case-insensitive, `req 2` before `req 10`).
* Names cannot be empty, start with `.`, or contain `/`, `\` or control characters.

## Canonical form

Sendito writes files in one canonical layout: keys in the order shown below,
defaults omitted, one blank line between tables, `'''` literal strings for
multi-line text (or `"""` with escapes when the text contains `'''`, a carriage
return, a control character or ends with `'`). Saving a request that has not
changed never rewrites its file, so re-saving produces byte-identical files and
hand-written formatting survives until the request is actually edited.

## Variables

Any string field may contain `{{name}}` placeholders (spaces inside the braces
are allowed: `{{ name }}`). Names use letters, digits, `_`, `-` and `.`.

Lookup order, last wins:

1. collection variables (`.sendito/collection.toml`)
2. the active environment (`.sendito/environments/<name>.toml`)
3. runtime variables set by scripts (pre-request `set`, post-response `extract`)

A variable's value may itself contain `{{other}}` placeholders (resolved up to
8 levels; cycles stay unresolved). Built-in dynamic variables, generated once per
send:

| Name | Value |
|---|---|
| `{{$uuid}}` | random UUID v4 |
| `{{$timestamp}}` | Unix time, seconds |
| `{{$timestampMs}}` | Unix time, milliseconds |
| `{{$isoTimestamp}}` | current UTC time, RFC 3339 |
| `{{$randomInt}}` | random integer 0–1000 |

Unresolved placeholders are highlighted in red in the editor. Sending a request
with unresolved placeholders asks for confirmation; if you confirm they are sent
literally. `sendito run` refuses unless `--allow-unresolved` is given.

## HTTP request

```toml
kind = "http"
method = "POST"                 # any token; default GET
url = "{{base_url}}/users"      # no scheme → http://

[options]                       # whole table optional
timeout_ms = 5000               # default: [http] timeout_ms from config (30000)
follow_redirects = false        # default true (up to 10 hops)
verify_tls = false              # default true
cookies = true                  # optional: defaults to [http] cookie_jar (false)

[[params]]                      # query parameters, appended to the URL
name = "q"
value = "café"
enabled = false                 # default true; disabled rows are kept but not sent

[[headers]]
name = "X-Request-Id"
value = "{{$uuid}}"
enabled = true

[auth]                          # optional; one of:
type = "bearer"                 # none | basic | bearer | apikey | oauth (HTTP only)
token = "{{token}}"
# type = "basic"   → username, password   (Authorization: Basic base64(user:pass))
# type = "apikey"  → key, value, in = "header" | "query"
# type = "oauth"   → name = "identity" from [oauth.<identity>]

[body]                          # optional; one of:
type = "json"
text = '''
{"name": "Ana"}
'''
# type = "raw"        → content_type, text
# type = "form"       → [[body.fields]] name, value, enabled
# type = "multipart"  → [[body.parts]] name, value | file, filename, content_type, enabled
# type = "file"       → path, content_type

[[scripts.pre]]                 # set a runtime variable before sending
var = "ts"
value = "{{$timestamp}}"

[[scripts.extract]]             # after the response: store a value
var = "token"
from = "jsonpath"               # jsonpath | header | status
expr = "$.access_token"         # JSONPath, header name; omitted for status
secret = true                   # mask the value in the UI and history

[[scripts.assert]]              # after the response: check a value
from = "status"                 # jsonpath | header | status
op = "eq"                       # eq ne contains not_contains exists not_exists lt le gt ge matches
value = "200"                   # omitted for exists / not_exists; may use {{variables}}
```

### What goes on the wire

Sendito sends exactly what the request defines, plus what HTTP framing requires:

* **Method and URL**: the method is upper-cased. The URL is used as written
  (characters that are not allowed in a URL, such as spaces or non-ASCII, are
  percent-encoded by the URL parser). Enabled `[[params]]` are appended as
  `name=value` pairs joined by `&`, after any query already in the URL and before
  any `#fragment`. In parameter names and values these characters are
  percent-encoded: controls, non-ASCII, space, `"` `#` `%` `&` `'` `+` `<` `=` `>`
  `[` `\` `]` `^` `` ` `` `{` `|` `}`. Everything else (including `,` `/` `:` `?`
  `@` `!` `$` `(` `)` `*` `;`) is sent literally. An empty value is sent as `name=`.
* **Headers**: enabled `[[headers]]`. Over HTTP/1.1 header names are sent in
  Title-Case (`x-api-key` and `X-API-KEY` both go out as `X-Api-Key`; names are
  case-insensitive); HTTP/2 always sends lowercase names, as the protocol requires.
  Several headers with the same name are sent next to each other, in their file
  order; the order between different names may differ from the file. Values are sent as the bytes
  of the rendered text (UTF-8); line breaks are rejected.
* **Added headers**: `Host` and `Content-Length`/`Transfer-Encoding` (framing), and:
  * `Content-Type` from the body type when no enabled `Content-Type` header is
    defined: `application/json` (json), the body's `content_type` (raw, if not
    empty), `application/x-www-form-urlencoded` (form), the file's `content_type`
    or `application/octet-stream` (file), `multipart/form-data; boundary=…`
    (multipart — this one replaces a user `Content-Type`, because the boundary must
    match; a note is shown).
  * `Authorization` (basic, bearer, OAuth) or the API-key header, unless the request
    already defines a header with that name (the explicit header wins; a note is shown).
  * `Accept: */*`, when no enabled `Accept` header is defined (added by the HTTP
    library; it cannot be turned off, see `docs/DEBT.md`).
  * `Cookie` from the environment's jar, only when enabled (see below).
  * Nothing else: no `User-Agent`, `Accept-Encoding` or `Referer` is added, and
    responses are never requested compressed.
* **Response decoding**: when the response arrives with `Content-Encoding:
  gzip`, `deflate` (zlib, with a raw-stream fallback) or `br`, the body is
  decompressed for display, extraction and assertions; the response
  border shows the encoding and the size it had on the wire. Sendito never
  sends `Accept-Encoding`, so this only decodes what the server chose to send.
  Any other coding (`zstd`, stacked codings) or a stream that does not decode
  cleanly is shown as received. The configured body-size limit applies to both
  received and decoded bytes; a truncated compressed stream is not decoded.
* **Bodies**:
  * json / raw: the rendered text as UTF-8, byte for byte (including line endings).
    A defined but empty body is sent with `Content-Length: 0`. `type = "none"` (or
    no `[body]`) sends no body; POST, PUT and PATCH then still carry
    `Content-Length: 0` (RFC 9110, section 8.6), other methods no `Content-Length`.
    A `Content-Length` or `Transfer-Encoding` header you define yourself is sent
    as written and nothing is added.
  * form: `name=value` pairs joined by `&`, encoded as `application/x-www-form-urlencoded`
    (space → `+`; letters, digits and `*-._` literal; everything else `%XX`).
  * multipart: one part per enabled entry. Text parts have no `Content-Type` unless
    `content_type` is set. File parts send the file's bytes with
    `filename` (default: the file's name) and `content_type` (default: guessed from
    the extension, else `application/octet-stream`).
  * file: the file's bytes.
* **Paths** in bodies are relative to the collection root; `~/` is expanded.
* **Redirects** (when followed) re-send the request to the `Location`; no `Referer`
  is added.

### Cookie jar

The cookie jar is opt-in via `[options] cookies = true` or `[http] cookie_jar = true`.
The request option overrides the configuration; `sendito run --no-cookies` disables
it for one run. Cookies are isolated by environment, with `_collection` used when
no environment is selected. Multiple matching cookies are sent as a `Cookie` header
appended after any explicit `Cookie` header, with longer paths first.

Cookies are stored in `.sendito/cookies.toml`, using a private file (0600 on Unix),
a cross-process lock and atomic replacement. `sendito init` adds the file to
`.gitignore`; older collections get `.sendito/.gitignore` entries when first
saved, and that file ignores itself so it never shows up as untracked.
In a git worktree, `git` must be available; writes fail if the cookie file is tracked
or not ignored. Session cookies persist
until deleted. `:cookies` lists cookies without values; `:cookies delete <number>`
deletes one, and `:cookies clear` deletes the active environment's jar.

Cookie parsing and matching follow RFC 6265 using the public suffix list; secure
cookies require HTTPS and public-suffix Domain attributes are rejected. To prevent
cookies from being sent to the wrong path or origin, **redirects are not followed
when the jar is enabled**; the response and a note are shown instead. Set-Cookie
values and their echoes are masked in the UI.

## gRPC request

```toml
kind = "grpc"
target = "{{grpc_host}}"         # host:port; https:// (or grpcs://) = TLS, http:// = plaintext
service = "acme.users.v1.Users"  # fully-qualified service name
method = "GetUser"
message = '''
{"id": "42"}
'''

[options]
tls = true                       # TLS when the target has no scheme; default false
verify_tls = false               # default true
deadline_ms = 2000               # sent as grpc-timeout and enforced by the client

[schema]                         # optional; default: server reflection
source = "files"                 # reflection | files
files = ["protos/users.proto"]   # relative to the collection root
import_paths = ["protos"]        # default: each file's directory

[[metadata]]
name = "x-request-id"            # keys are lower-cased; `-bin` keys take base64 values
value = "{{$uuid}}"

[auth]                           # same as HTTP; sent as metadata
type = "bearer"
token = "{{token}}"

[[scripts.assert]]               # status = the gRPC status code, header = response
from = "jsonpath"                #   metadata or trailers, jsonpath = the last message
expr = "$.user.name"
op = "eq"
value = "Ana"
```

* `message` is proto3 JSON (field names or JSON names; unknown fields are an
  error). For client-streaming and bidi methods a JSON **array** sends one message
  per element; in the UI, further sends on an open stream push more messages and
  `Ctrl+W d` ends the request stream.
* Schemas come from gRPC server reflection (v1, falling back to v1alpha) or from
  `.proto` files compiled in-process (no `protoc` needed). Well-known types
  (`google.protobuf.*`) are built in.
* Metadata is sent in file order; values must be printable ASCII unless the key ends
  in `-bin` (then the value is base64 and the decoded bytes are sent). The client
  also sends `content-type: application/grpc`, `te: trailers`,
  `user-agent: sendito/<version> tonic/<version>` and, with a deadline, `grpc-timeout`.

## MCP request

An MCP request calls one tool on a server configured in `[mcp.servers]`:

```toml
kind = "mcp"
server = "remote"
tool = "search"
arguments = '''
{"query":"{{query}}"}
'''

[options]
timeout_ms = 5000

[[scripts.assert]]
from = "jsonpath"
expr = "$.structuredContent.ok"
op = "eq"
value = "true"
```

`arguments` is JSON text and must be an object or empty. When omitted it defaults
to `{}`; an explicitly empty string omits the `arguments` property on the wire.
The text is stored and read as written (a half-typed object is a valid file) and
is rejected only when the call is sent. Server, tool and arguments are rendered
with the active scope. Authentication is configured on the server, not on the
request. The timeout covers the entire session; it defaults to `[mcp] timeout_ms`
(30000 ms). Scripts see the full tool result as JSON; status is 0 for success or
1 for `isError: true`.

### What goes on the wire

Each list or call opens a separate session. Sendito sends compact JSON-RPC 2.0:
`initialize` (ID 1, proposed protocol version `2025-06-18`, empty capabilities
and client name/version), `notifications/initialized` (no ID), then `tools/list`
or `tools/call` (ID 2). List pagination sends additional requests with increasing
IDs and the returned cursor. Tool arguments are parsed and serialized as JSON,
not sent byte-for-byte as written. The client recognizes `2024-11-05`,
`2025-03-26` and `2025-06-18`; other versions produce a warning.

* **stdio:** one UTF-8 JSON object and newline per message on the child process's
  stdin/stdout. The child inherits the process environment with configured `env`
  entries added; stderr is reported in the log.
* **Streamable HTTP:** a POST per message with `Content-Type: application/json`
  and `Accept: application/json, text/event-stream`, plus configured headers and
  HTTP framing headers. After initialization, a returned `Mcp-Session-Id` and
  the negotiated `Mcp-Protocol-Version` are sent on subsequent POSTs. JSON and
  SSE response bodies are accepted. Bearer or OAuth authentication adds an
  `Authorization` header. OAuth retries once after a 401 and token refresh.
  MCP and OAuth HTTP requests do not follow redirects and advertise `h2` and
  `http/1.1` via ALPN, as the HTTP client does. OAuth endpoints and the protected
  resource URL require HTTPS except for loopback HTTP. Authorization, token
  exchange and refresh send the MCP server URL as the `resource` parameter.

The client supports tools only; it does not consume a server-initiated GET stream
or the legacy HTTP+SSE transport. See `docs/DEBT.md` for current limits.

## Environment files

`.sendito/environments/<name>.toml` and `.sendito/collection.toml`:

```toml
[variables]
base_url = "http://localhost:8080"                    # plain: stored in this file
token = { source = "local" }                          # .sendito/secrets.toml
api_key = { source = "keychain" }                     # OS keychain
signing_key = { source = "file", path = "~/.acme/key" } # contents of a file
admin_pw = { source = "local", key = "admin" }        # explicit key in secrets.toml
deploy = { source = "keychain", key = "acme/deploy" } # explicit keychain account
```

Each variable has exactly one source; sources can be mixed in one environment.

| Source | Where the value lives | Notes |
|---|---|---|
| plain string | the env file itself | versioned with the collection; not a secret |
| `local` | `.sendito/secrets.toml`, section `[<env>]` (`[_collection]` for collection variables), key = `key` or the variable name | gitignored by `sendito init` |
| `keychain` | OS keychain, service `sendito`, account = `key` or `<collection name>/<env>/<variable>` | Linux: Secret Service (GNOME Keyring, KWallet); macOS: Keychain |
| `file` | the file's contents, one trailing newline removed | path relative to the collection root, `~/` expanded |

Values from `local`, `keychain` and `file` are secrets, and so is any value built
from one (for example a header `Bearer {{token}}` or a pre-request variable that
references a secret). Secrets are masked (`••••••`) in the UI, kept as
`{{placeholders}}` in `history.jsonl` and in exported curl commands (unless you
ask for `:curl!`), and scrubbed from error messages.

Environment names cannot start with `_`. The environment editor (`Ctrl+W V`) can
set secret values; they are written to their source (secrets file or keychain),
never to the env file.

### `.sendito/secrets.toml`

```toml
# Local secret values. Keep this file out of version control.

[local]
token = "eyJhbGciOi..."

[_collection]
shared = "..."
```

## Project and global config

Precedence: built-in defaults < global `$XDG_CONFIG_HOME/sendito/config.toml`
(default `~/.config/sendito/config.toml`, also on macOS) < project
`<collection>/.sendito/config.toml`. A file that cannot be read is skipped with a
warning in the status line.

```toml
[collection]
name = "acme"            # used in keychain account names; default: directory name
ignore = ["target", "node_modules", "vendor", "dist", "build"]

[ui]
vim_mode = false
theme = "auto"           # auto | dusk | daylight | ember | linen | contrast | ansi,
                         # or a theme file of yours (docs/THEMES.md)
icons = "unicode"        # unicode | nerd (Nerd Font icons)
sidebar = "left"         # left | right
mouse = true
confirm_quit = true
watch_interval_s = 5     # :watch re-sends the active request this often

[http]
timeout_ms = 30000
max_body_mb = 100        # larger bodies are truncated (a warning is shown)
cookie_jar = false       # default for requests that do not specify [options] cookies

[oauth.example]          # HTTP OAuth identity; authorization code + PKCE
authorize_url = "https://example.com/authorize"
token_url = "https://example.com/token"
client_id = "client"
client_secret = "{{client_secret}}"  # optional, from a secret environment source
scopes = ["read"]

[grpc]
deadline_ms = 0          # default deadline when a request sets none; 0 = none

[mcp]
timeout_ms = 30000

[mcp.servers.local]
command = "my-mcp-server"
args = ["--stdio"]
env = { ROOT = "{{root}}" }

[mcp.servers.remote]
url = "https://example.com/mcp"
verify_tls = true
headers = { X-Client = "sendito" }

[mcp.servers.remote.auth]
type = "bearer"          # or "oauth"
token = "{{token}}"

[history]
enabled = true
max_entries = 10000

[keymap.all]             # see KEYMAP.md
send = ["f9", "ctrl+w e"]
```

For HTTP OAuth use `[auth] type = "oauth"` and `name = "example"` in an HTTP
request. Sign in with `sendito auth login example --env local` or
`:auth login example` in the UI; logout with the corresponding `logout` command.
The authorization URL is available even when no browser opens. Tokens are kept in
an OS keychain account isolated by collection, environment and identity. The
endpoint and client fields can use environment variables; secrets are allowed only
in `client_secret`, never in URLs, client ID or scopes. OAuth endpoints and protected
HTTP URLs require HTTPS except for loopback. An explicit Authorization header wins.
On HTTP 401 Sendito refreshes the token and retries the request once; this may
repeat a request with side effects. Exports use a `{{oauth.example}}` placeholder
rather than reading a token from the keychain. HTTP OAuth is not available for gRPC.

For MCP OAuth, replace the bearer auth table with `type = "oauth"`. Optional fields
are `client_id`, `client_secret`, `authorize_url`, `token_url`,
`registration_url` and `scopes` (an array). Run `sendito mcp login <server>`
from the collection to authorize; `sendito mcp logout <server>` deletes its
tokens. Tokens are stored in the OS keychain under `<collection>/mcp/<server>`.
Do not put client secrets directly in versioned config. Server command, arguments,
environment values, URL, headers and bearer token are rendered when calling a
tool. OAuth login currently uses the configured URL and OAuth settings literally.

`NO_COLOR` (any non-empty value) disables colors regardless of the theme.
`auto` picks `daylight` when `COLORFGBG` reports a light background, else `dusk`.
Terminals without 24-bit color get the nearest 256-color palette.

Theme files are `*.toml` in `themes/` next to the global config file and in
`<collection>/.sendito/themes/` (the collection's wins for the same name). Their
format and every color token are in [docs/THEMES.md](docs/THEMES.md).

## Local state files

* `session.toml` — open tabs (path, or the unsaved request as a draft in request
  format), active tab, layout, expanded folders, active environment and non-secret
  runtime variables. Restored on start.
* `history.jsonl` — one JSON object per sent request:

  ```json
  {"ts":"2026-11-21T10:15:30Z","name":"list users","path":"Users/list users.toml",
   "env":"local","badge":"GET","target":"http://localhost:8080/users",
   "template":"{{base_url}}/users",
   "request":"kind = \"http\"\n...","status":200,"duration_ms":14,"size":291}
  ```

  `target` is the URL (gRPC: target/service/method) as it was sent, with secret
  values masked; `template` is the same as written. `request` is the request as
  written, in this file format, with its `{{variables}}` unsubstituted:
  opening or resending an entry resolves it again with the active environment,
  and saving it to the tree keeps the variables. Secret values are never
  written. Response bodies are not stored. The file is trimmed to `max_entries` (newest kept).
