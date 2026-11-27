#!/usr/bin/env bash
# Assembles the two critic bundles (see VERIFICATION in docs/PLAN.md) into $1.
# Run the tests and the media generator first:
#   cargo test --workspace && cargo test -p sendito-ui --test media -- --ignored && python3 docs/media/render.py
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=${1:?usage: critic-bundles.sh OUTDIR}
C="$OUT/correctness"; U="$OUT/ux"
rm -rf "$C" "$U"; mkdir -p "$C"/{http,grpc,ws,sse,mqtt,mcp,environment,files,import} "$U"/{recordings,screenshots}

# --- correctness: wire captures next to the request files and import inputs
for kind in http grpc ws sse mqtt mcp; do
  for f in tests/corpus/collection/$kind/*.toml; do
    n=$(basename "$f" .toml)
    cp "$f" "$C/$kind/$n.toml"
    cp "target/corpus/captures/$kind/$n.wire" "$C/$kind/$n.wire"
  done
done
cp target/corpus/captures/mcp/list.wire "$C/mcp/list.wire"
cp target/corpus/captures/mcp/session-reuse.wire "$C/mcp/session-reuse.wire"
cp tests/corpus/collection/.sendito/config.toml "$C/environment/config.toml"
cp tests/corpus/collection/.sendito/collection.toml "$C/environment/collection.toml"
cp tests/corpus/collection/.sendito/environments/corpus.toml "$C/environment/corpus.toml"
cp tests/corpus/collection/files/data.json tests/corpus/collection/files/notes.txt "$C/files/"
xxd -p tests/corpus/collection/files/pixel.png | tr -d '\n' | sed 's/../& /g' > "$C/files/pixel.png.hex"
xxd -p tests/corpus/collection/files/binary.bin | tr -d '\n' | sed 's/../& /g' > "$C/files/binary.bin.hex"
echo "1048576 bytes; byte i = ((i * 2654435761) mod 2^32) >> 24" > "$C/files/large.bin.txt"
mkdir -p "$C/proto" && cp -r tests/corpus/collection/proto/sendito "$C/proto/"
mkdir -p "$C/import/postman" "$C/import/curl"
cp tests/fixtures/acme.postman_collection.json "$C/import/postman/input.json"
cp -r target/corpus/import/postman/output "$C/import/postman/output"
cp target/corpus/import/postman/collection-variables.toml target/corpus/import/postman/report.txt "$C/import/postman/"
for f in tests/fixtures/curl/*.sh; do
  n=$(basename "$f" .sh)
  cp "$f" "$C/import/curl/$n.sh"
  cp "target/corpus/import/curl/$n.toml" "target/corpus/import/curl/$n.export.sh" "$C/import/curl/"
done
cat > "$C/README.txt" <<'TXT'
Layout of this bundle

http/NAME.toml   a saved HTTP request.
http/NAME.wire   the bytes the HTTP/1.1 echo server received when NAME.toml was sent:
                 the request head verbatim, then the body. The server port appears as
                 <port>, a multipart boundary as <boundary>. Bodies over 64 KiB appear as
                 their length, Adler-32 and first 16 bytes; binary bodies and binary
                 multipart parts appear as hex. Lines starting with "###" are not part of
                 the request: "### request i of n" separates requests (redirects),
                 "### client saw:" is the response status the client received and
                 "### note:" is a note the application showed to the user.
grpc/NAME.toml   a saved gRPC request.
grpc/NAME.wire   what the gRPC echo server received for the call: the method path,
                 the request metadata, and each request message decoded with the schema
                 in proto/ (JSON, proto field names, keys sorted, default values omitted)
                 followed by the exact protobuf bytes of the message in base64. The last
                 line is what the client observed.
sse/NAME.toml    a saved SSE request.
sse/NAME.wire    the GET opening request(s), parsed event data/type/id/retry and
                 the end reason or validation error. Ports and secrets are masked.
mqtt/NAME.toml   a saved MQTT request.
mqtt/NAME.wire   what the MQTT test broker saw: the protocol and client ID, then one
                 line per packet: "<" received by the broker, ">" sent by it, the
                 packet name and its fields (payloads as text, or "hex .." when they
                 are not printable UTF-8; passwords are not shown). "< WILL" means the
                 broker published the Last Will after an ungraceful close. Lines
                 starting with "###" are what the client received ("### message"),
                 the end reason and any error. {{mqtt_url}} and {{mqtt_tls_url}} are
                 the local broker URLs.
ws/NAME.toml     a saved WebSocket request.
ws/NAME.wire     the handshake head the WebSocket echo server received verbatim (the
                 random Sec-WebSocket-Key and the generated x-request-id appear as
                 <sec-websocket-key> and <x-request-id>; the port as <port>), then one
                 line per frame: "<" received by the server, ">" sent by it, the frame
                 kind, the payload length and the payload (text as text, binary as
                 base64). The first line is the close code the client ended with.
mcp/NAME.toml    a saved MCP request: a tool call, a resource read (op = "resource") or a
                 prompt get (op = "prompt"); server definitions are in
                 environment/config.toml.
mcp/NAME.wire    the HTTP request lines, headers and compact JSON-RPC bodies received
                 by the echo server. The dynamically assigned port and session ID are
                 replaced with <port> and <session>; the response is not included.
                 {{mcp_url}} is the local echo server URL.
mcp/list.wire    a tools/list session over HTTP; the echo server returns a second
                 page with cursor "page-two".
mcp/session-reuse.wire  call.toml sent twice in one kept session (a folder run or
                 the interface keeps it): initialize and notifications/initialized
                 once, then two tools/call requests with IDs 2 and 3.

Deliberate behaviour (documented in the application's FORMAT.md). The rules
below are design decisions, not defects: check that every capture follows them
and report any case that does not, but do not report the rules themselves or
ask for them to be removed.

MCP: a call starts a session with initialize, notifications/initialized, then
tools/call, resources/read (params: uri only; arguments are not sent) or
prompts/get (params: name, then arguments when the file has them). A kept
session (session-reuse.wire) skips initialize and continues the IDs. The first
request proposes protocol version 2025-06-18; subsequent
requests carry the negotiated Mcp-Protocol-Version and any Mcp-Session-Id. A missing
arguments field in a request file defaults to {}, which is sent as an object.

Request format rules that matter here: on HTTP/1 header names are written in
Title-Case ("x-api-key" is sent as "X-Api-Key"; names are case-insensitive), the
order between headers of different names may differ from the file (headers with the
same name stay in file order), and "Accept: */*" is added when the request defines
no enabled Accept header (added by the HTTP library). When [options] cookies = true,
matching cookies from the per-environment jar are sent in a Cookie header; on a redirect,
each hop gets the jar's Cookie header for its own URL (cookies the redirect response sets
are stored first). Cases 45 and 46 cover cookie storage and replay; case 49 follows a
redirect that sets a cookie, which the next hop sends.
Cases inherit/50 and inherit/51 get headers, a variable and a bearer auth from
inherit/folder.toml (folder settings): a request without [auth] uses the folder's
auth, auth type "none" turns it off, a request header replaces the folder header of
the same name, and a disabled request row stops the folder header of that name.
Case 52 uses Digest auth: the first request goes without Authorization, the echo
server answers 401 with a SHA-256 challenge, and the second request carries the
answer (the server checks it; cnonce and response change at each send and are
masked). Case 53 is signed with AWS SigV4: X-Amz-Date and Authorization are added
(the date and signature change at each send and are masked); the signed headers are
content-type, host and x-amz-date. Case 54 sends a HS256 JWT signed at send time
from the claims in the file, as Authorization: Bearer.
Case http/58 and case grpc/16 have a [scripts.lua] pre script. The script is part
of the request file: the method, headers, query, body and gRPC metadata it sets
(and the headers it removes) count as defined by the file. Case 58 changes GET to
POST, adds X-Request-Id, X-Sum and the query q, removes X-Remove and adds "b" to
the JSON body (compact, keys sorted); case 16 sets x-request-id and the message.
Case 59 uses {{ $response('http/01-get-minimal', '$.target', 'always') }}: the
policy "always" sends the dependency http/01 first (request 1 of 2, exactly as
case 01 defines it), then the request itself with the value "/echo" read from the
body of that response (request 2 of 2). Both requests count as defined by the files.
Cases 55 and 56 send GraphQL bodies as compact JSON: query, optional variables,
then optional operationName. Variables must be a JSON object; case 57 fails before
sending anything. Content-Type is application/json unless explicitly overridden.
Case 47 uses a keychain-sourced OAuth bearer token. Case 48 follows a redirect to another
origin (other scheme, host or port): the API-key header, like Authorization and Cookie,
is not sent to the new origin. Also: metadata keys ending in "-bin" take a
base64 value in the request file (as on the gRPC wire); the client decodes it
and sends those bytes, so the server sees the same base64 text and, decoded,
the bytes shown under it. SSE is a separate kind. It sends GET and adds Accept: text/event-stream only
when absent. It adds neither Accept-Encoding nor Last-Event-ID. It follows the
HTTP auth, TLS, proxy, cookie and redirect rules. Opening requires status 200,
event-stream Content-Type and absent or identity encoding. The parser accepts
UTF-8 replacement, BOM, CR/LF/CRLF and multiline data. IDs persist; retry is
informational; EOF drops incomplete events. No reconnect; 1 MiB event limit.
The timeout covers opening, not the stream. Max events, end, deadline and EOF
finish successfully. The SSE corpus has 13 cases.
An HTTP request with no body (type "none" or no
[body]) sends no body; POST, PUT and PATCH still carry Content-Length: 0
(RFC 9110, 8.6). When the request defines no enabled Content-Type header, one
is added from the body type: application/json (json), the body's content_type
(raw, when set), application/x-www-form-urlencoded (form), the file body's
content_type or else application/octet-stream (file), and for multipart file
parts the part's content_type or else a type guessed from the file extension
(application/octet-stream when unknown). A URL without a scheme is sent over http://. In a Postman
import, the request URL comes from url.raw and the collection name becomes the
folder name. Sendito's dynamic variables: {{$uuid}} random v4 UUID,
{{$timestamp}} Unix time in seconds (as in Postman), {{$timestampMs}}
milliseconds, {{$isoTimestamp}} RFC 3339 UTC, {{$randomInt}} 0-1000.
Template functions {{ $name(argument) }} take a 'literal', a variable or a
call: $uuidBytes gives the 16 bytes of a UUID as base64 (a protobuf bytes
field in JSON), $sha256 the lowercase hex digest; FORMAT.md lists the others.
{{ $lua.name(argument) }} calls a Lua function of the collection's
.sendito/lua/ catalog (FORMAT.md, "Lua function catalog").

{{name}} placeholders take their value from environment/corpus.toml, then
environment/collection.toml. Variables whose source is "local", "keychain" or "file"
hold random values generated for each run; in the captures they appear as
<secret:NAME>, and HTTP Basic credentials built from one appear as
<base64 of "ana:" + secret:NAME>. Paths in requests are relative to the collection
root, whose files/ directory is copied here (binary files as hex).

MQTT rules that matter here: mqtt:// is TCP (default port 1883), mqtts:// is
TLS (default port 8883). CONNECT carries the protocol level (3.1.1 or 5), the
clean session flag, the keep-alive (default 60 s), the client ID (empty
generates sendito-<8 hex>), the Last Will when [last_will] is set, and the user
name and password when set; when username is empty, a Basic auth gives them.
After CONNACK the client subscribes to each enabled subscription in file order;
after every SUBACK it sends each [[publish]] in file order with its QoS and
retain flag. payload is UTF-8 text; payload_base64 is decoded to bytes. Every
text field may hold {{placeholders}}. A deadline or a disconnect sends
DISCONNECT (no Last Will); a cancel closes without it (the broker sends the Last
Will). Folder headers do not apply. Auth kinds other than inherit, none and
basic stop the session before it connects. The MQTT corpus has 13 cases.

WebSocket rules that matter here: the handshake is one HTTP/1.1 GET with the
upgrade headers written by the library (Host, Connection: Upgrade, Upgrade:
websocket, Sec-WebSocket-Version: 13, a random Sec-WebSocket-Key), then the
request's headers in file order and Sec-WebSocket-Protocol when subprotocols are
set. The message document is sent right after the handshake, in order, with its
{{variables}} rendered; text frames carry the UTF-8 of the string and binary
frames the decoded base64. Server pings are answered with a pong carrying the
same payload. The client closes with code 1000 and waits at most 5 s for the
peer's answer; a session deadline also closes with 1000, with a
"deadline … reached" reason. Every string field may hold {{placeholders}},
which are rendered when the session opens.

A request with a [graphql] table (ws/13-graphql-transport-ws, ws/14-graphql-legacy)
sends no messages: it offers its protocol first in Sec-WebSocket-Protocol, sends
{"type":"connection_init"} (with "payload" when init_payload is set), then after
connection_ack {"id":"1","type":"subscribe","payload":{"query","variables",
"operationName"}} ("type":"start" for graphql-ws; empty variables and operation
name are left out), answers ping with {"type":"pong"}, and closes with 1000 after
the server's complete. The JSON is compact with keys in that order.

import/postman/input.json                a Postman collection that was imported;
import/postman/output/                    the request files it produced (one per request);
import/postman/collection-variables.toml  the collection variables it produced;
import/postman/report.txt                 the import report shown to the user.
import/curl/NAME.sh                       a curl command that was imported;
import/curl/NAME.toml                     the request file it produced;
import/curl/NAME.export.sh                that request exported back as a curl command.
TXT

# --- UX: recordings, screenshots, README and KEYMAP only
for d in docs/media/recordings/*/; do cp -r "$d" "$U/recordings/"; done
cp docs/media/screenshots/*.png "$U/screenshots/"
cp README.md KEYMAP.md "$U/"
cat > "$U/BRIEF.txt" <<'TXT'
Intentional features. Sendito does more than a minimal HTTP/gRPC client. The
features below are deliberate. Judge how well each one works: whether it is
clear, discoverable, consistent and readable. Do not report that a feature
exists, and do not recommend removing one. If a feature is confusing, say what
would make it clear.
- Command palette (Ctrl+P): actions, requests, environments and vim-style
  :commands. With an empty query its first row is "Show all keys", which is harmless.
- Request history (F7): open a copy, resend it, or save it to the tree. Copies are
  marked "(copy)" and "from history (time)".
- curl: paste a curl command into the URL field or the import dialog; copy a request
  as curl (Ctrl+W u, secrets kept as {{placeholders}}; :curl! includes them).
- Postman v2.1 import with a report of everything not imported as-is; :report
  shows the last report again.
- Environments with value sources plain / secrets / keychain / file. Secret values
  are masked in the editor and in echoed responses and history.
- Undefined variables: {{name}} in red, named in the status line, and a confirmation
  before sending.
- Scripts, assertions and extracted variables (Results tab).
- gRPC: schema from reflection or .proto files, a message template, streaming with a
  Log tab (message log, legend pinned above it), several messages per stream, end of stream.
- WebSocket: one session per request (handshake, headers, subprotocols, a JSON message
  document, live frames in both directions, close code), a Handshake tab and a Log tab;
  Send on an open session pushes one more frame and Ctrl+W d closes it.
- Completion (Ctrl+Space, or automatic at a key position): gRPC message keys and MCP
  argument keys, the gRPC method and the MCP server/tool names; type to filter,
  Enter/Tab inserts, Esc closes, the hint row describes the open list.
- Optional vim mode, mouse support (click, hover, drag in the tree, drag pane
  borders), sidebar on either side, pane zoom.
- Seven themes (auto, dusk, daylight, ember, linen, contrast, ansi) plus user themes
  read from TOML files (:theme NAME, :theme reload, `sendito theme list|print`).
- Optional Nerd Font icons (Settings > Icons); the default icon set works with any
  font.
- Response tools: pretty / raw / hex views, JSON folding, search, wrap, save body;
  a DNS / connect / TLS / first-byte timing breakdown.
- Dialog choice rows: Left/Right cycle the value and Space opens the list; Enter
  confirms the dialog from any field. In panes, Enter or Space opens a field's picker.

Settled design decisions. Earlier reviews weighed these against each other and
chose as below. Report a problem with how one of them is carried out, but do not
ask for the opposite choice.
- In dialogs, Enter always confirms (Ctrl+S too). On a choice row, Left/Right
  change the value and Space opens the list. The button names the action, e.g.
  "Save (Enter)".
- Layout: a top bar (name, request tabs, +, environment chip colored by name,
  "F1 all keys"), a URL strip above both panes (method chip, URL, Send), and pane
  tabs drawn in each pane's top border with the request's folder path on the right.
  Details ("✓ JSON", "Ln, Col", view mode, position) and scrollbars are in the
  bottom and right borders. The focused pane has a rounded accent border (heavy only
  in the contrast theme and without color). Dialogs open over a dimmed screen with a
  shadow (without color the screen is cleared instead).
- An unsaved new tab keeps a stable name ("untitled", "untitled 2"); its URL shows
  in the URL strip, not in the tab name.
- History rows show method, status and name, then the URL path (without the
  {{base}} variable or host) with the time (and the environment when the sidebar
  is wide enough). The full request opens with Enter.
- The command palette highlights its first row when it opens. That row is
  "Show all keys", so Enter on an untouched palette is harmless.
- `n` in the tree creates the request file at once (empty). Ctrl+N opens a scratch
  tab that has no file until Ctrl+S asks for a name and a folder.
- The hint row lists the keys for the focused pane by priority. When they do not
  fit, the rest are reached with "F1 all keys", which is always shown.
- The gRPC message log is the "Log" tab for every call kind, unary included.
- The status line shows segments without separators: vim mode, unsaved count,
  undefined variables and a message. The environment is only shown in the top bar.
  The last request's status, time and size appear in the status line only while
  the Response pane is hidden (otherwise they are in the Response border).
- Transient messages (copied, saved, kept, resent ...) appear as a chip with an
  icon at the right of the status line.
- The start screen (no request open) shows the wordmark, what the collection
  holds, the keys to start and the most recent requests.
- Help lists every key; on screens 180 columns wide or more it reads in two
  columns (the right column continues the left one).
- Values derived from a file (multipart filename and content type) are shown in
  italics and marked "(auto)".

recordings/NN-flow/ - numbered frames (00.png, 01.png, ...) of a scripted session in a
                      120x40 terminal. The yellow strip under each frame is a caption
                      describing the step; it is not part of the application.
screenshots/        - SIZE-THEME-VIEW.png: the application at 120x40 and 200x60 cells in
                      the dark ("dusk") and light ("daylight") themes, plus a few views
                      at 120x40 in ember, linen, contrast, ansi and dusk with Nerd Font
                      icons ("dusk-nerd").
README.md, KEYMAP.md - the documentation shipped with the application.
TXT
echo "bundles in $OUT"
