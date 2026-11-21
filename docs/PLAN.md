Build Sendito, an open-source terminal API client for HTTP and gRPC (Postman-style) that runs locally in a developer's terminal, with full mouse, keyboard and vim-style control. It only needs to send correct HTTP and gRPC requests, stay simple to use, and organize saved requests in a sidebar tree.

PRIORITIES (in this order when they conflict)
1. Correctness: the bytes on the wire match the request as defined (method, URL, headers, body, metadata, protobuf encoding).
2. UX: simple to use with mouse, conventional keys and vim mode, with no feature behind an undocumented key.
3. Performance: startup, memory and frame time within TARGET.

CONSTRAINTS
- Rust (edition 2024), ratatui + crossterm for the TUI, edtui (or equivalent) for text editing. No Electron, no webview, no JS engine.
- HTTP via reqwest with rustls. gRPC via tonic + prost-reflect, using server reflection and local `.proto` files compiled at runtime (protox). No `protoc` needed at runtime.
- No `unsafe` code. Workspace lints deny `unsafe_code`, `unwrap_used`, `expect_used`, `dbg_macro`, `todo`. `cargo clippy -- -D warnings` and `cargo fmt --check` are clean.
- Workspace split into crates: core (model and variables), http, grpc, store (files and secrets), import, ui, plus the binary. Every non-UI crate builds and tests without a terminal.
- Each saved request is its own TOML file, and each folder on disk is a tree node. The collection is a plain directory that can be versioned in git. Re-saving an unchanged request produces a byte-identical file.
- Secrets: each variable has one source. It can be (a) plain text in the env file, (b) a gitignored `.sendito/secrets.toml`, (c) the OS keychain (Linux Secret Service, macOS Keychain), or (d) the contents of a file path. Sources can be mixed within one environment. Secret values are masked in the UI and never written to the history or to exported curl unless the user asks explicitly. `sendito init` adds `secrets.toml` to `.gitignore`.
- Config is TOML. The global file is `$XDG_CONFIG_HOME/sendito/config.toml`; the project file is `<project>/.sendito/config.toml`, found by searching upward. Precedence: defaults < global < project.
- Deliverables: the source repo; release binaries as `.tar.gz` + `.sha256` for x86_64-unknown-linux-gnu and aarch64-apple-darwin, built by GitHub Actions on a `v*` tag; README.md; KEYMAP.md; FORMAT.md; VHS recordings. Report the size of each binary and the line coverage of the core crates.

TARGET
- Linux x86_64 (Ubuntu 24.04, kitty/alacritty/gnome-terminal) and macOS arm64 (macOS 14+, iTerm2/Terminal.app). Terminal size from 120×40 up to 300×100.
- Scale: a collection of 1,000 requests in 50 folders, 5 environments × 100 variables, 10,000 history entries, response bodies up to 10 MB.
- Cold start to first rendered frame ≤ 100 ms with the 1,000-request collection.
- Idle RSS ≤ 30 MB. RSS ≤ 120 MB while showing a 10 MB JSON response.
- Frame render ≤ 16 ms p99 during scroll and typing. No input event blocked for more than 50 ms while a request or stream is in flight.
- Stripped release binary ≤ 20 MB.
- Line coverage ≥ 80% on the non-UI crates (cargo-llvm-cov).

FEATURES
- Layout: a left sidebar tree (collections, folders, requests, with HTTP/gRPC method badges) and a main tab bar of open requests. The request editor (URL/target bar, tabs Params | Headers | Body | Auth | Scripts) sits above the response pane (tabs Body | Headers | Timing | Messages). An environment selector, a hint row, and a status line (env, last status code, latency, size). The sidebar can go on the right and every panel can be resized.
- Tree: create, rename, duplicate, delete, and move folders and requests by drag or keyboard, with `/` to filter. Every action writes to disk immediately.
- HTTP: every method, query params, headers, raw/JSON/form-urlencoded/multipart (with file parts) bodies, Basic/Bearer/API-key auth (header or query), timeout, redirect toggle, TLS-verify toggle.
- gRPC: discover services and methods via server reflection or `.proto` files plus import paths. Unary, server-streaming, client-streaming and bidi calls. Request messages are edited as JSON with a template generated from the schema. Metadata, TLS and plaintext, deadline. Streams show each message with a timestamp and can be cancelled.
- Response: JSON pretty-printed with syntax highlighting, folding and search. Raw and hex views. Headers, status, total time with a DNS/connect/TLS/TTFB breakdown for HTTP, and size. Copy the body or save it to a file.
- Environments: `{{var}}` substitution in the URL, params, headers, body, metadata and auth. A collection-level env plus a selectable active env. Unresolved variables are highlighted before sending, and sending is blocked until they resolve or the user confirms.
- Scripts (no JS): a pre-request step sets variables. A post-response step extracts values into variables via JSONPath (body), header name, or status. Assertions on status, header or JSONPath show pass/fail in the Messages tab.
- History: every sent request is appended to `history.jsonl` with secrets masked. A browsable list can resend an entry or save it to the tree.
- Import/export: import a Postman Collection v2.1 (folders, requests, auth, variables) and a pasted `curl` command. Export any HTTP request as `curl`. Report items that were skipped during import.
- Persistence: open tabs, layout, active env and unsaved edits are restored on restart. Unsaved changes are marked with `●`.
- Themes: auto, ansi, plus 3 named themes. `NO_COLOR=1` is respected.

UX
- `sendito [PATH]` opens the collection at PATH (default: the nearest `.sendito/` up the tree, else the current directory). `sendito init` creates a collection. `sendito run <request-path> [--env NAME]` sends one request headless and prints the response (exit code ≠ 0 on a failed assertion).
- Keys follow the sqln model. Conventional mode by default, with a `Ctrl+W` prefix: `h/j/k/l` focus, `n/p` tabs, `c` close, `q` quit, `,` settings, `?` help. `Ctrl+P` opens the command palette (accepts `:send`, `:env <name>`, `:w`, `:import`, `:curl`). `Ctrl+Enter`/`F5`/`Ctrl+W e` sends, `Ctrl+W x` cancels, `Ctrl+S` saves.
- Optional vim mode (setting): normal/insert/visual in the editors, `hjkl`/`gg`/`G` in the tree and response, `/` search, `:` commands.
- Mouse: click to focus, select, switch or close tabs and press buttons. Double-click opens a request. Drag moves tree nodes and resizes panels. Wheel scrolls every pane.
- The keymap can be customized per action and per mode in the config. KEYMAP.md lists every action, and the in-app help (`F1`) matches it.

VERIFICATION
- UX critic: a separate subagent that sees ONLY the VHS recordings, the README/KEYMAP.md, and the terminal screenshots at 120×40 and 200×60 in both themes. It never sees code or reasoning. Inputs: recordings of 10 scripted flows (create a folder and request, send HTTP GET/POST JSON, multipart upload, switch env, unary gRPC via reflection, bidi stream, Postman import, curl paste/export, history resend, vim-mode editing, mouse drag in the tree). It returns prioritized, concrete defects (unclear state, unreachable action, truncated text, key mismatch with KEYMAP.md). Iterate until it reports nothing above "minor".
- Correctness critic: a separate subagent that sees ONLY the wire captures (an echo server's request dumps and the gRPC server's received messages) next to the source TOML files and the import inputs. Corpus: 40 HTTP cases (every method, Unicode/encoded query, duplicate headers, empty/large/binary bodies, multipart with files, every auth type, variables from all 4 secret sources) and 15 gRPC cases (4 call types, nested/oneof/map/repeated/well-known types, metadata, deadline exceeded, server error status). It returns every mismatch. Iterate until it reports zero mismatches.
- Critic isolation (both critics): each iteration uses a fresh critic. It receives only the current artifacts, never earlier critic reports, a changelog, comments or rebuttals that explain, justify or dismiss earlier defects. Every reported defect is fixed in the artifact. No defect is closed by arguing it is not a defect. If the same defect comes back, the fix failed.
- Integration tests in CI (Linux and macOS): a local HTTP echo server and a tonic test server with reflection plus `.proto` files, both started by the test harness. Every case from the correctness corpus is asserted automatically. A Postman v2.1 fixture with ≥ 30 requests imports with zero unexplained losses. A curl round-trip (curl → request → curl) matches semantically for 20 fixtures. Report pass/fail counts.
- UI tests: ratatui TestBackend snapshot tests (insta) for every pane at 120×40 and 200×60, and scripted key/mouse event sequences for each of the 10 critic flows. Report the count and 0 failures.
- Format stability: load → save of every fixture collection produces byte-identical files (diff = 0 bytes). Secrets never appear in `history.jsonl`, exported curl, or the git-tracked files (grep for seeded secret values: 0 hits).
- Performance: a hyperfine benchmark of cold start to first frame (instrumented exit flag) with the 1,000-request collection, 20 runs. Measure RSS idle and with a 10 MB response via `/usr/bin/time -v` (Linux) and `/usr/bin/time -l` (macOS). Log frame time over a 60 s scripted scroll/type session. Report mean, p99 and max for each, against the TARGET thresholds.
- Static gates: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo llvm-cov` ≥ 80% on the non-UI crates, `cargo deny check` clean. Report coverage %.

DONE WHEN: every VERIFICATION gate passes (both critics at nothing above "minor" and zero mismatches, all tests green on Linux and macOS, every performance number within TARGET, coverage ≥ 80%). Deliver the source repo with CI, the v0.1.0 GitHub Release with Linux x86_64 and macOS arm64 `.tar.gz` + `.sha256`, README.md, KEYMAP.md, FORMAT.md (TOML request/env/secret schema), the VHS recordings and screenshots, the performance report, the coverage report, and the final reports from both critics.
