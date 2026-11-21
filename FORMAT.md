# Sendito file formats

A Sendito collection is a plain directory that you can keep in git. Every saved
request is one TOML file; every directory is a folder in the sidebar tree.

```
my-api/
├── .gitignore                    # sendito init adds the three local files below
├── .sendito/
│   ├── config.toml               # project settings (versioned)
│   ├── collection.toml           # collection-level variables (versioned)
│   ├── environments/
│   │   ├── local.toml            # selectable environments (versioned)
│   │   └── staging.toml
│   ├── secrets.toml              # local secret values      (gitignored)
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
  `kind = "grpc"` is a request. Other `.toml` files (e.g. `Cargo.toml`) are ignored.
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

[[params]]                      # query parameters, appended to the URL
name = "q"
value = "café"
enabled = false                 # default true; disabled rows are kept but not sent

[[headers]]
name = "X-Request-Id"
value = "{{$uuid}}"
enabled = true

[auth]                          # optional; one of:
type = "bearer"                 # none | basic | bearer | apikey
token = "{{token}}"
# type = "basic"   → username, password   (Authorization: Basic base64(user:pass))
# type = "apikey"  → key, value, in = "header" | "query"

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
  * `Authorization` (basic, bearer) or the API-key header, unless the request
    already defines a header with that name (the explicit header wins; a note is shown).
  * `Accept: */*`, when no enabled `Accept` header is defined (added by the HTTP
    library; it cannot be turned off, see `docs/DEBT.md`).
  * Nothing else: no `User-Agent`, `Accept-Encoding` or `Referer` is added, and
    responses are shown as received (no decompression).
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
theme = "auto"           # auto | ansi | dusk | daylight | contrast
sidebar = "left"         # left | right
mouse = true
confirm_quit = true

[http]
timeout_ms = 30000
max_body_mb = 100        # larger bodies are truncated (a warning is shown)

[grpc]
deadline_ms = 0          # default deadline when a request sets none; 0 = none

[history]
enabled = true
max_entries = 10000

[keymap.all]             # see KEYMAP.md
send = ["f9", "ctrl+w e"]
```

`NO_COLOR` (any non-empty value) disables colors regardless of the theme.
`auto` picks `daylight` when `COLORFGBG` reports a light background, else `dusk`.
Terminals without 24-bit color get the nearest 256-color palette.

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
