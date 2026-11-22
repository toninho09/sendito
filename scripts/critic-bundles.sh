#!/usr/bin/env bash
# Assembles the two critic bundles (see VERIFICATION in docs/PLAN.md) into $1.
# Run the tests and the media generator first:
#   cargo test --workspace && cargo test -p sendito-ui --test media -- --ignored && python3 docs/media/render.py
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=${1:?usage: critic-bundles.sh OUTDIR}
C="$OUT/correctness"; U="$OUT/ux"
rm -rf "$C" "$U"; mkdir -p "$C"/{http,grpc,environment,files,import} "$U"/{recordings,screenshots}

# --- correctness: wire captures next to the request files and import inputs
for kind in http grpc; do
  for f in tests/corpus/collection/$kind/*.toml; do
    n=$(basename "$f" .toml)
    cp "$f" "$C/$kind/$n.toml"
    cp "target/corpus/captures/$kind/$n.wire" "$C/$kind/$n.wire"
  done
done
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

Deliberate behaviour (documented in the application's FORMAT.md). The rules
below are design decisions, not defects: check that every capture follows them
and report any case that does not, but do not report the rules themselves or
ask for them to be removed.

Request format rules that matter here: on HTTP/1 header names are written in
Title-Case ("x-api-key" is sent as "X-Api-Key"; names are case-insensitive), the
order between headers of different names may differ from the file (headers with the
same name stay in file order), and "Accept: */*" is added when the request defines
no enabled Accept header (added by the HTTP library). Also: metadata keys ending in "-bin" take a
base64 value in the request file (as on the gRPC wire); the client decodes it
and sends those bytes, so the server sees the same base64 text and, decoded,
the bytes shown under it. An HTTP request with no body (type "none" or no
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

{{name}} placeholders take their value from environment/corpus.toml, then
environment/collection.toml. Variables whose source is "local", "keychain" or "file"
hold random values generated for each run; in the captures they appear as
<secret:NAME>, and HTTP Basic credentials built from one appear as
<base64 of "ana:" + secret:NAME>. Paths in requests are relative to the collection
root, whose files/ directory is copied here (binary files as hex).

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
