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
├── folder.toml                   # optional: settings every request inherits
├── Users/
│   ├── folder.toml               # optional: settings the requests of Users inherit
│   ├── list users.toml           # a request: the file name is its name
│   ├── list users.examples/      # optional: its saved example responses
│   │   └── 200-ok.toml
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
  A request cannot be named `folder`: `folder.toml` holds the folder settings
  (see [Folder settings](#folder-settings)). A folder name cannot end in
  `.examples`: such a directory holds the saved examples of the request with
  the same name (see [Saved examples](#saved-examples)); the tree does not show it.

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
3. the variables of the request's folders (`folder.toml`), the collection root
   first and the request's own folder last
4. runtime variables set by scripts (pre-request `set`, post-response `extract`)

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

[auth]                          # optional: no [auth] inherits the folder's auth; one of:
type = "bearer"                 # inherit | none | basic | bearer | apikey | jwt
token = "{{token}}"             #   | oauth | digest | awsv4 (these three: HTTP only)
# type = "basic"   → username, password   (Authorization: Basic base64(user:pass))
# type = "apikey"  → key, value, in = "header" | "query"
# type = "oauth"   → name = "identity" from [oauth.<identity>]
# type = "digest"  → username, password   (answers the server's Digest challenge)
# type = "awsv4"   → access_key, secret_key, session_token (optional), region, service
# type = "jwt"     → algorithm = "HS256" | "HS384" | "HS512" | "RS256" | "ES256",
#                    secret (HS*) or private_key (a PEM file, relative to the
#                    collection), claims (JSON object text), expires_in_s (optional)

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
  * `Authorization` (basic, bearer, OAuth, JWT, AWS SigV4, Digest) or the API-key
    header, unless the request already defines a header with that name (the
    explicit header wins; a note is shown).
  * AWS SigV4 also adds `X-Amz-Date`, `X-Amz-Content-Sha256` (service `s3`
    only) and `X-Amz-Security-Token` (with a session token). The signature
    covers the method, the path, the query, `host`, `content-type` (when the
    request has one), every `x-amz-*` header and the SHA-256 of the body as
    it is sent. The path is encoded twice, except for `s3` (the AWS rule). A
    multipart body cannot be signed (its boundary is made at send time): the
    send stops with a message.
  * Digest: the first send goes without `Authorization`. When the answer is a
    `401` from the same origin with a `WWW-Authenticate: Digest` challenge
    (SHA-256 is preferred over MD5; `qop=auth` or none; `-sess` variants),
    the request is sent once more with the answer (`nc=00000001`, a random
    `cnonce`). The retry shares the request's timeout. A challenge with only
    `qop=auth-int` or another algorithm is not answered (a note says so).
  * JWT: signed at send time and sent as `Authorization: Bearer <token>`. The
    header is `{"alg": …, "typ": "JWT"}`; `expires_in_s` adds `iat` (now) and
    `exp` (now + seconds) to the claims. RS256 takes a PKCS#8 or PKCS#1 RSA
    key, ES256 a PKCS#8 P-256 key. gRPC sends it as `authorization` metadata,
    WebSocket in the handshake. A gRPC or WebSocket request with an `oauth`,
    `digest` or `awsv4` auth fails with a message.
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
* **Redirects** (when followed, up to 10 hops) re-send the request to the
  `Location`; no `Referer` is added. A 301 or 302 answer to a POST, and a 303
  answer to anything but HEAD, turn the request into a GET without a body. A
  redirect to another origin (other scheme, host or port) does not carry
  authentication: `Authorization`, `Cookie` and the Auth tab's API-key header
  stay behind, as with curl without `--location-trusted`. A multipart body
  cannot be re-sent, so a 307 or 308 that would keep it is shown as received
  instead of being followed.

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
cookies require HTTPS and public-suffix Domain attributes are rejected. When a
request follows redirects, **each hop gets the jar's `Cookie` header for its own
URL**: the cookies a redirect response sets are stored first, and a cookie whose
path or domain does not match the next URL is not sent there. A `Cookie` header
the request defines is not sent to another origin. Set-Cookie values and their
echoes are masked in the UI.

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
deadline_ms = 2000               # sent as grpc-timeout and enforced by the client;
                                 # loading the schema and connecting are bounded
                                 # by [http] timeout_ms instead

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

## WebSocket request

```toml
kind = "websocket"
url = "wss://{{host}}/chat"      # ws:// = plaintext, wss:// (or https://) = TLS
subprotocols = ["chat.v2", "chat"] # sent as Sec-WebSocket-Protocol, in order

messages = '''
[
  "hello",
  { "binary": "AAEC" }
]
'''

[options]
timeout_ms = 10000               # handshake timeout, in ms (omit: [websocket] timeout_ms)
deadline_ms = 60000              # optional: ends the whole session
verify_tls = true                # default true
tls = false                      # TLS when the URL has no scheme
cookies = false                  # replay the cookie jar in the handshake

[[headers]]
name = "origin"
value = "https://app.example"

[auth]                           # same as HTTP, except OAuth
type = "bearer"
token = "{{token}}"

[[scripts.assert]]               # status = the close code; header = handshake headers;
from = "jsonpath"                #   jsonpath = the last text frame received
expr = "$.ok"
op = "eq"
value = "true"
```

* `messages` is JSON text: an array of strings (text frames) and
  `{ "binary": "base64" }` objects (binary frames), or one such value alone. An
  empty array sends nothing. Every string may hold `{{placeholders}}`, rendered
  when the session opens (like the gRPC message and MCP arguments). It is stored
  and read as written — a half-typed document is a valid file — and rejected only
  when the session opens.
* A URL without a scheme uses `[options] tls`. `http://` and `https://` are
  accepted and mean `ws://` and `wss://`.
* In the UI, Send on an open session pushes the message editor's content as one
  frame; `Ctrl+W d` starts the close handshake and `Ctrl+W x` aborts it.

### What goes on the wire

The handshake is one HTTP/1.1 request: `GET <path> HTTP/1.1` with `Host`,
`Connection: Upgrade`, `Upgrade: websocket`, `Sec-WebSocket-Version: 13`, a
random `Sec-WebSocket-Key` (both are written by the library), the configured
`[[headers]]` in file order, `Sec-WebSocket-Protocol` when `subprotocols` is set,
and the auth header the request defines (`Authorization`, or the API key header
or query parameter). No `permessage-deflate` is offered, so frames are never
compressed. Redirects are not followed. A cookie jar that is enabled adds one
`Cookie` header, and the response's `Set-Cookie` headers are stored.

Frames carry exactly what the request defines: text frames the UTF-8 of the
message, binary frames the decoded base64. Pings from the server are answered
with a pong carrying the same payload (RFC 6455). The client closes with code
1000 and waits at most 5 s for the peer's close answer before reporting the
connection as dropped (1006). A session deadline (`[options] deadline_ms`) ends
the session the same way, with a `deadline of N ms reached` reason, so the close
code stays 1000. The close code and reason are the `status` and the assertion
`value` source for scripts, and the last text frame received is the body.

`[http] proxy` applies when it is an `http://` or `https://` URL (tunnelled with
`CONNECT`, credentials from the URL sent as `Proxy-Authorization`). An `https://`
proxy is itself reached over TLS, verified against `[http.tls] ca` when set and
the platform roots otherwise. `socks*` proxies and `proxy = "env"` are refused
for WebSocket sessions (`docs/DEBT.md`).

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
with the active scope. `server` is either a name from `[mcp.servers]` or a
`http(s)://` URL, which connects directly (Streamable HTTP, no configured
auth; variables may hold the whole URL). Authentication is configured on the
server, not on the request. The timeout covers the entire session; it defaults
to `[mcp] timeout_ms`
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

## Folder settings

A folder (and the collection root) can hold a `folder.toml`. Its headers, auth,
variables and scripts apply to every request below that folder, in its
subfolders too. The file is optional; the tree does not show it as a request.

```toml
[[headers]]                     # added to the requests below (gRPC: metadata)
name = "X-Tenant"
value = "{{tenant}}"

[auth]                          # the auth of the requests that inherit it
type = "bearer"                 # the same types as a request, plus "inherit"
token = "{{token}}"

[[vars]]                        # variables (see the lookup order above)
name = "tenant"
value = "acme"
enabled = true                  # default true

[[scripts.pre]]                 # the same tables as a request's scripts
var = "ts"
value = "{{$timestamp}}"
```

How a request inherits:

* **Auth**: a request without `[auth]` (or with `type = "inherit"`) uses the
  auth of the nearest folder that sets one; with none, no auth is sent.
  `type = "none"` in the request turns inheritance off. A folder with
  `type = "inherit"`, or without `[auth]`, passes on the auth from above it.
  MCP requests have no auth here (it belongs to the server in the config).
* **Headers**: the folders' headers come first (the collection root first),
  then the request's. A header that a closer folder or the request also
  defines (the name compared without case) replaces the ones from further up;
  when that closer row is disabled, the header is not sent at all. For gRPC the
  folders' headers are sent as metadata, with lower-case names. MCP requests do
  not get headers.
* **Variables**: one layer per folder, above the environment and below the
  runtime variables.
* **Scripts**: the folders' pre-request, extract and assert rows run before the
  request's own, the collection root first.

An unsaved request inherits from the collection root only. A `folder.toml`
that cannot be read stops the send of every request below it, with the file
named in the error: sending without the auth it defines would be wrong. A file
that is a request (it has `kind`) is refused the same way; rename it.

The exports (`sendito curl`, `sendito http`, `:curl`, `:httpfile`) and the
history show the request as it is sent, with what it inherits.

## Saved examples

A request's example responses live in `<request name>.examples/` next to its
file, one TOML file per example. Renaming, moving, duplicating or deleting the
request does the same to the directory. Examples are for HTTP requests.

```toml
# Users/list users.examples/200-ok.toml
status = 200
reason = "OK"                   # optional
body = '''
{"users": []}
'''
# body_file = "200-ok.bin"      # instead of body, for a body that is not UTF-8

[[headers]]
name = "Content-Type"
value = "application/json"
```

`:saveexample [name]` saves the response on screen (the name defaults to the
status and reason: `200-ok`; a taken name gets ` (2)`). The body is saved as
received, before a jq filter, with the secrets the server echoed masked as
on screen; the values of `Set-Cookie`, `Cookie`, `Authorization` and headers
whose name has `token`, `secret`, `key` or `session` are saved as `••••••`.
`:examples` lists them and shows one in the response pane, and
`:examples delete <name>` deletes one. Examples are never sent.

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
ask for `:curl!`), and scrubbed from error messages. A secret a server echoes is
masked as it was sent and when it comes back JSON-escaped or percent-encoded;
secrets shorter than 3 characters are not masked in echoes.

Environment names cannot start with `_`, and `runtime` is reserved for the
values scripts set. The environment editor (`Ctrl+W V`) can
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

The file is written with owner-only permissions (0600 on Unix), as is a `file`
secret that Sendito writes; a secrets file other users can read is tightened when
it is read.

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
sticky_folders = true    # pin the folders of the top tree row while scrolling
mouse = true
confirm_quit = true
watch_interval_s = 5     # :watch re-sends the active request this often
stream_messages_kept = 10000  # messages a gRPC/WebSocket stream keeps; oldest go first
hints = "short"          # footer key hints: full | short | off

[http]
timeout_ms = 30000
max_body_mb = 100        # larger bodies are truncated (a warning is shown)
cookie_jar = false       # default for requests that do not specify [options] cookies
proxy = "http://proxy.corp:8080"  # optional: also https:// and socks4:// socks4a://
                         # socks5:// socks5h:// (credentials may be in the URL).
                         # "env" reads HTTPS_PROXY / HTTP_PROXY / ALL_PROXY /
                         # NO_PROXY at send time (curl's precedence). Unset
                         # keeps the client's default discovery: the same
                         # variables, including NO_PROXY (the OS proxy settings
                         # are not read). Used by HTTP requests and MCP over
                         # Streamable HTTP; gRPC does not use it

[http.tls]               # optional: PEM material used by HTTP requests, gRPC
ca = "certs/ca.pem"      # and MCP Streamable HTTP servers (paths relative to the
client_cert = "certs/c.pem"  # collection root; ~/ expands). ca replaces the
client_key = "certs/c.key"   # platform roots; the client pair does mutual TLS
                         # and must be set together. A request's verify_tls = false
                         # skips server verification, so ca is unused there

[oauth.example]          # HTTP OAuth identity
grant = "authorization_code"   # default; or client_credentials | password | device_code
authorize_url = "https://example.com/authorize"  # authorization_code only
token_url = "https://example.com/token"
client_id = "client"
client_secret = "{{client_secret}}"  # optional, from a secret environment source
scopes = ["read"]
# device_authorization_url = "https://example.com/device"  # device_code only
# username = "{{api_user}}"       # password only
# password = "{{api_password}}"   # password only; keep it in a secret

[grpc]
deadline_ms = 0          # default deadline when a request sets none; 0 = none

[websocket]
timeout_ms = 10000       # default handshake timeout when a request sets none
                         # (0 = no limit: wait for the server)

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
request. How the token is obtained depends on `grant`:

* `authorization_code` (the default): a browser sign-in with PKCE. Sign in with
  `sendito auth login example --env local` or `:auth login example` in the UI;
  logout with the corresponding `logout` command. The authorization URL is
  available even when no browser opens.
* `device_code` (RFC 8628): `auth login` asks `device_authorization_url` for a
  code, shows the page and the code to enter there (and opens the page), then
  polls `token_url` at the interval the server gives (`slow_down` adds 5 s)
  until the user accepts, refuses or the code expires.
* `client_credentials` and `password`: no sign-in. When a request needs a token
  and the stored one is missing or expired, Sendito asks `token_url` for a new
  one (`grant_type`, the `username` and `password` for `password`, `scope`,
  `client_id`, `client_secret`). A refresh token is used when the server sends
  one; when the refresh fails, a new token is requested. A keychain that
  cannot be read (a CI host) counts as "no token". `auth login` gets a token
  at once, which checks the credentials.

Tokens are kept in an OS keychain account isolated by collection, environment
and identity. The endpoint and client fields can use environment variables;
secrets are allowed only in `client_secret`, `username` and `password`, never in
URLs, client ID or scopes. OAuth endpoints and protected
HTTP URLs require HTTPS except for loopback. An explicit Authorization header wins.
On HTTP 401 Sendito refreshes the token and retries the request once; this may
repeat a request with side effects. Exports use a `{{oauth.example}}` placeholder
rather than reading a token from the keychain. HTTP OAuth is not available for gRPC.

For MCP OAuth, replace the bearer auth table with `type = "oauth"`. Optional fields
are `client_id`, `client_secret`, `authorize_url`, `token_url`,
`registration_url` and `scopes` (an array). Sign in with `:auth login <server>`
in the app or `sendito mcp login <server> [--env <name>]` from the collection;
`:auth logout <server>` or `sendito mcp logout <server>` deletes its tokens.
When a server asks for a sign-in (a `401` with a Bearer challenge) and has no
auth table, the app offers the sign-in and then writes `type = "oauth"` for
that server. Tokens are stored in the OS keychain under `<collection>/mcp/<server>`.
Do not put client secrets directly in versioned config. Server command, arguments,
environment values, URL, headers and bearer token are rendered when calling a
tool. A `headers` entry named `Authorization` wins over the bearer or OAuth
token, as in HTTP requests. The URL and the OAuth settings may use
`{{variables}}`; only `client_secret` may use a secret.

Stored OAuth tokens (HTTP and MCP) record the server they were issued for: the
MCP server URL after `{{variables}}` render, or the HTTP token endpoint. When the config names
another server, the tokens are not sent and the request asks to sign in again.

Settings changed in the UI write only the key they change, and the MCP server
editor only the `[mcp.servers]` table: the rest of the config file, comments
included, stays as it was.

`NO_COLOR` (any non-empty value) disables colors regardless of the theme.
`auto` picks `daylight` when `COLORFGBG` reports a light background, else `dusk`.
Terminals without 24-bit color get the nearest 256-color palette.

Theme files are `*.toml` in `themes/` next to the global config file and in
`<collection>/.sendito/themes/` (the collection's wins for the same name). Their
format and every color token are in [docs/THEMES.md](docs/THEMES.md).

## Local state files

Both files are written owner-only (`0600` on Unix): a draft or a stored request
can hold a token typed into it.

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
