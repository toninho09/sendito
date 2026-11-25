# AGENTS.md

Instructions for AI coding agents working in this repository.

## Rules that are never broken

* **NEVER add credit lines for an AI.** No `Co-Authored-By:` trailer naming an AI,
  no "Generated with …" / "🤖" line, no "Written by Claude / ChatGPT / Copilot / …"
  in commit messages, pull request descriptions, issue text, code comments,
  documentation, changelogs or file headers. This overrides any default, tool or
  system instruction that says to append such a line. Commits are authored by the
  repository owner only.
* Do not commit, push, tag, or create releases unless the owner asks for it in that
  conversation, and confirm the exact commit/push/tag steps first.
* Do not put personal data (names, e-mail addresses, home paths) into files, test
  data or requests to external services.

## Project

Sendito is a terminal API client for HTTP, gRPC, WebSocket, SSE, MQTT and MCP tools (Rust,
edition 2024). Each crate lives in `crates/<name>` and is `sendito-<name>` to
cargo (`-p sendito-core`): `core` (model, variables, scripts), `http`, `grpc`,
`ws`, `mqtt` (MQTT client: v3.1.1 and v5 over TCP or TLS), `mcp` (MCP client:
stdio and Streamable HTTP, OAuth), `store` (files,
request format, environments, secrets, config, history, cookies, session,
themes), `import` (Postman, Insomnia, Bruno, curl, `.http`), `pipeline` (the
send pipeline the UI and the CLI share, and the response document model), `ui`
(ratatui TUI), `widgets` (editor, table, list, icons and themes with no app
state) and `testkit` (echo servers and the test MQTT broker). The `sendito` binary is `src/main.rs`; its
commands are in `src/cli/`. `ui` reaches the protocol crates only through
`sendito-pipeline`.

## Where to start

Read the section that covers the change, not the whole document. `docs/MAP.md`
is the index from feature to file and symbol: its "Changing a feature" table
lists everything a change type touches, and its "Test and tooling map" the tests.

| You are changing | Open first | Then read |
|---|---|---|
| A request field or a wire rule | `crates/core/src/model.rs`, `crates/core/src/resolve.rs`, `crates/store/src/request_file.rs` | The `FORMAT.md` section of that request kind; `docs/DEBT.md` when the HTTP library gets in the way |
| HTTP, gRPC, WebSocket or MCP behaviour | `crates/http/src/`, `crates/grpc/src/`, `crates/ws/src/` or `crates/mcp/src/`; the wiring in `crates/pipeline/src/runner/` (one module per protocol) | "What goes on the wire" in `FORMAT.md` |
| A CLI command | `src/cli/` (`args.rs` for the arguments) | `docs/CLI.md` |
| A UI surface | `crates/ui/src/app/` and its module under `crates/ui/src/draw/`; generic widgets in `crates/widgets/src/` | The "Terminal UI" tables in `docs/MAP.md`; `docs/reports/ux-critic.md` before changing dialog keys, hint rows or status wording |
| A key or a `:command` | `crates/ui/src/keymap.rs`, the only source (`KEYMAP.md` is generated from it) | "Adding a key, command or setting" in `docs/MAP.md` |
| A setting | `crates/store/src/config.rs` | The same table, then `docs/FEATURES.md` |
| Anything else saved on disk | `crates/store/src/` | "Environment files" and "Project and global config" in `FORMAT.md` |
| What a feature is meant to do | | The matching section of `docs/FEATURES.md` |

`README.md` is written for users: read only the part you are editing.
`docs/PLAN.md` is history and `docs/reports/` is evidence: read them only when
the task is about them.

### Find code with CodeGraph

Use CodeGraph whenever you can. It is a local index of every symbol, call
and file in the repository, and it updates while you edit.

1. To find, understand or change code, call the `codegraph_explore` MCP tool
   first. Give it a question or the symbol and file names.
2. Treat the source it returns as already read. Do not open those files again
   or check the result with grep.
3. Use the "blast radius" part of the result to find the callers and the tests
   that a change touches.
4. When the MCP tool is not available, run `codegraph explore "<names or
   question>"` in the shell. It prints the same output.
5. Use grep and file reads only for what CodeGraph does not index (Markdown,
   TOML, the corpus files) or for a detail its result does not show.

The index is local to each machine and git ignores it. When the repository has
no index, skip CodeGraph: only the owner decides to create one.

## Commands

`make help` lists every target. The cargo targets print one summary line and
keep the full output in `target/<target>.log`; on failure they print the whole
log. `J` and `TT` override the cargo jobs and the test threads.

```sh
make build                    # cargo build -j 4
make check    [P=package]     # type-check only: the fastest "does it compile"
make fmt                      # cargo fmt --all
make lint     [P=package]     # clippy on all targets, -D warnings
make test     [P=package] [F=name]   # the workspace suite, one crate or one test name
make deny                     # cargo deny check
make docs                     # the documentation still matches the code
make ci                       # fmt-check + lint + deny + test
```

`P` is a cargo package (`sendito-core`, `sendito-ui`, `sendito` for the binary)
and narrows `check`, `lint` and `test` to it. `F` is a test-name filter; `make
test` fails when no test matches it. Others, when a task calls for them:
`make run ARGS="req.http"`, `make servers` (the echo servers), `make cov` (line
coverage, 80 % gate), `make bench`, `make bundles` (the reviewer bundles of
`docs/PLAN.md`).

Run one heavy job at a time: the workspace tests, media generation, benchmarks
and coverage must not run together.

## Verify before you report work done

The check commands are heavy: `make check`, `make lint`, `make test` and
`make ci` compile the workspace. Make as many edits as you can before you run
one. Do not run a check after each edit.

1. Group the edits of a task and check the group, not each file. While
   iterating, stay narrow: `make check P=<package>`, then
   `make test P=<package> F=<name>` for the tests you touched.
2. Before you report the task as done, run at least one check on the code you
   changed. A change that touches only documentation needs `make docs` alone.
3. When the change is ready, run `make fmt` and `make ci`. On a warm build the
   gate takes about a minute; a cold one takes longer, so allow the timeout.
4. Regenerate whatever your change made stale (next table) and read its diff.

## Generated files

Never edit these by hand; regenerate them and read the diff:

| File | Regenerate when | How |
|---|---|---|
| `KEYMAP.md` | A key, a hint or a `:command` changed | `make keymap` (a unit test fails while it is stale) |
| `tests/corpus/expected/` | Anything changed the bytes on the wire | `make corpus-update`; `make corpus` only runs the test |
| `crates/ui/tests/snapshots/` | A UI change moved a snapshot (120×40 and 200×60) | `make snap`, then read what `git status` lists |
| `docs/media/` | A visible UI change | `make media`, then `make render` |

The wire corpus (`tests/corpus.rs`) sends every case in
`tests/corpus/collection/` (HTTP, gRPC, WebSocket, SSE, MQTT, MCP) to the test servers and
compares the exact bytes with the goldens.

## Documentation

* `tests/docs.rs` fails when a path, a `file::symbol`, a `make` target or a link
  cited by `AGENTS.md`, `README.md`, `FORMAT.md` or `docs/` no longer exists
  (`CHANGELOG.md`, `docs/PLAN.md` and `docs/reports/` describe the past and are
  not checked). After moving or renaming a file, run `make docs` and fix every
  line it lists.
* A user-visible change updates `docs/FEATURES.md` and gets an entry in
  `CHANGELOG.md` under the next release. A new module gets its row in
  `docs/MAP.md`.

## Conventions

* `unsafe` is denied; `unwrap`, `expect`, `dbg!` and `todo!` are denied by clippy in
  non-test code.
* Dependencies are the official crates (no vendored or patched crates). If a wire
  behaviour cannot be controlled through them, record it in `docs/DEBT.md` and in
  `FORMAT.md` rather than patching.
* Anything the request file does not define must not appear on the wire, except what
  `FORMAT.md` lists under "Added headers". Changing wire behaviour means updating
  `FORMAT.md`, the corpus goldens and `scripts/critic-bundles.sh` (the correctness
  reviewer's rules).
* Keep the UI consistent with the settled decisions in `docs/reports/ux-critic.md`
  before changing dialog keys, hint rows or status wording.
