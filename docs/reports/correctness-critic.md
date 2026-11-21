# Correctness critic — final report

The correctness critic is a fresh, isolated agent that sees only the correctness
bundle built by `scripts/critic-bundles.sh`: the corpus request files, the exact
bytes the echo servers received (`.wire`), the environment, files and protos,
and the Postman and curl import inputs and outputs. It has no access to the
source code or to earlier reports. Each iteration used a new agent.

| Iteration | Mismatches | Fixed |
|---|---|---|
| 1 | 7 | header interleave order; Postman raw body without language → `text/plain`; `{{$guid}}` → `{{$uuid}}` reported; curl export kept relative file paths; empty header exported as `-H 'X-Empty;'` (and `-H 'X: '` imported as disabled, since curl does not send it); undefined Postman variables reported |
| 2 | 3 | POST/PUT/PATCH with no body send `Content-Length: 0` (RFC 9110 §8.6); `-bin` metadata bytes shown in the capture and the base64 rule stated; Postman `url.raw` vs structured host reported |
| 3 | 1 | the Postman collection name (kept as the import folder) shown in the import artifacts |
| 4 | **0** | — |
| 5 | **0** | (re-run on the build after later UI changes) — noted, not counted: absolute Postman file paths were not mentioned; the report now lists them |
| 6 | **0** | — |
| 7 | **0** | — |
| 8 | 1 | the file body's default Content-Type was not documented in the bundle's rules; documented in the bundle README and the application README |
| 9 | 1 | a Postman file body now takes its Content-Type from the file extension (`mime_guess`, octet-stream skipped) and says so in the report |
| 10 | **0** | — |
| 11 | **0** | — |
| 12 | **0** | — |
| 13–15 | not run | the UX critic was the only one with findings; per the "skip a clean critic" rule the correctness critic was not re-run while code changes touched only the UI |
| 16 | 1 | the Postman report said "uses the URL as written" for a request whose `:id` path variable was substituted; the message now says "the URL comes from the raw field; the host field differs and was ignored" |
| 17 | **0** | (after the report wording fix) |
| 19 | **0** | **final build**: re-run after the vendored `reqwest`/`hyper` patches were dropped for the official crates (new documented rules: `Accept: */*` when none is defined, Title-Case HTTP/1 header names, same-name headers adjacent); the 44 captures changed and were re-checked in full |

Iterations 5–7 and 10–12 re-ran fresh critics on later builds and found zero
mismatches. Iterations 8, 9 and 16 each found one issue: two were rules that the
bundle did not state or the importer did not apply (file-body Content-Type), one a
wording error in the import report. Iteration 19 is the final build (iteration 18 only reviewed the UX).

Notes the critics made that are not mismatches: the `.wire` captures end with an
LF before the `### client saw:` line (a capture-format artifact; Content-Length
always matches the request body), `large.bin` appears in the bundle only as its
generator description, and gRPC Struct numbers are rendered as `1.0` by the echo
server (the bytes are correct).

## Iteration 4 report (summary)

Result: **zero mismatches**. No request in any section sends something other
than what its request file defines, within what the bundle lets the critic check.

* **HTTP, 44 cases**: methods; path and query encoding (cases 10, 11, 12, 13, 42,
  43); disabled params and headers; duplicate headers in order; UTF-8 and empty
  header values; auth placement (30–34); variable and nested substitution (35,
  37, 38); explicit Content-Type override (39); redirects (40, 41).
  Content-Length matches the body for 02, 03, 04, 18–21, 24, 25, 35, 39; the
  67-byte multipart boundary is consistent across 26–29 (326, 427, 268, 447
  bytes). Case 22: Adler-32 `023e76c4` and the first 16 bytes recomputed from the
  formula match; case 23 is bytes 00..ff; case 27's PNG matches the source file.
  Form encoding (24) and the disabled form field (25) are correct.
* **gRPC, 15 cases**: every protobuf payload decoded field by field — scalars
  (10-byte negative int32/int64, zigzag, fixed32/64, float, double), nested
  messages, oneof, maps with negative int32 keys, repeated/packed/enum,
  well-known types (Timestamp, Duration, Struct, Value, wrappers, Any,
  FieldMask), proto3 optional presence, the empty message. Metadata: `-bin`
  value `AAEC` sent as bytes 00 01 02, duplicate keys both sent. `grpc-timeout
  150000u` for a 150 ms deadline; DEADLINE_EXCEEDED (4) and PERMISSION_DENIED (7)
  observed as expected; streaming message counts correct.
* **Postman import**: all 32 requests with the right method and URL; auth
  inherited from the collection (bearer) and the Users/Admin folder (basic),
  `noauth` honoured; bodies, headers, disabled fields and params preserved;
  GraphQL conversion correct; collection variables correct (disabled `old`
  excluded); every unresolved `{{var}}` and every lossy change (scripts,
  description, saved example, oauth2/digest, url.raw vs structured URL) is in the
  report, with correct counts.
* **curl import, 20 cases**: the produced requests send what curl sends (`-d`
  joining, `-F`/`--form-string`, `-u` with colons in the password, `-b`/`-A`/`-e`,
  `--json`, `-H 'X-Empty;'`, `--data-binary @file`, `-k`, `-L`, `--max-time`, `-I`,
  a multi-line PATCH with an escaped apostrophe); every export reproduces the
  same method, URL, headers and body.

Not verifiable from the bundle (not counted): values hidden behind secret
placeholders (the Basic credential in HTTP 30, the secret length in HTTP 44's
Content-Length, the file secret in HTTP 36).

## Iteration 6 highlights

Zero mismatches across 44 HTTP, 15 gRPC, the Postman import (32 requests) and
20 curl round trips. Re-verified in particular: `Content-Length: 0` on POST with
no body and none on GET/DELETE/HEAD/OPTIONS/TRACE/PURGE; the 1 MiB and binary
file bodies; multipart framing with one 67-byte boundary across 26–29;
`-bin` metadata decoded to bytes 00 01 02; `grpc-timeout 150000u`; every
protobuf payload decoded by hand and by script; every lossy Postman change
listed in report.txt, including absolute file paths and the ignored host field.
