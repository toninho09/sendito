# Coverage report

Line coverage of everything but `crates/ui` and `crates/testkit`, by the whole
test suite. Measured with `make cov` on macOS arm64 (rustc 1.100.0,
cargo-llvm-cov 0.9.2) on 2026-11-23:

```sh
cargo llvm-cov -j 4 -q --workspace --ignore-filename-regex 'crates/(ui|testkit)/' \
  --fail-under-lines 80 --summary-only -- --test-threads=2
```

| File | Lines | Line coverage |
|---|---:|---:|
| core/src/complete.rs | 36 | 72.22% |
| core/src/model.rs | 326 | 96.63% |
| core/src/resolve.rs | 753 | 98.80% |
| core/src/script.rs | 261 | 100.00% |
| core/src/template.rs | 128 | 100.00% |
| core/src/vars.rs | 381 | 99.48% |
| grpc/src/lib.rs | 317 | 89.59% |
| grpc/src/schema.rs | 467 | 81.37% |
| http/src/lib.rs | 1312 | 93.98% |
| import/src/bruno.rs | 487 | 66.94% |
| import/src/curl.rs | 487 | 95.69% |
| import/src/http_file.rs | 253 | 92.49% |
| import/src/insomnia.rs | 391 | 65.47% |
| import/src/lib.rs | 277 | 96.75% |
| import/src/postman.rs | 634 | 88.64% |
| mcp/src/http.rs | 189 | 86.24% |
| mcp/src/lib.rs | 592 | 90.37% |
| mcp/src/oauth.rs | 751 | 82.16% |
| mcp/src/protocol.rs | 22 | 100.00% |
| mcp/src/stdio.rs | 82 | 80.49% |
| pipeline/src/doc.rs | 318 | 98.74% |
| pipeline/src/lib.rs | 7 | 57.14% |
| pipeline/src/runner.rs | 1188 | 85.86% |
| store/src/collection.rs | 563 | 94.32% |
| store/src/config.rs | 267 | 97.00% |
| store/src/cookies.rs | 446 | 85.65% |
| store/src/env.rs | 537 | 89.01% |
| store/src/history.rs | 103 | 91.26% |
| store/src/init.rs | 91 | 94.51% |
| store/src/lib.rs | 6 | 100.00% |
| store/src/request_file.rs | 687 | 98.54% |
| store/src/session.rs | 44 | 97.73% |
| store/src/themes.rs | 102 | 99.02% |
| store/src/tomlfmt.rs | 162 | 96.30% |
| ws/src/lib.rs | 992 | 87.30% |
| src/main.rs | 944 | 48.62% |
| **TOTAL** | **14603** | **87.25%** |

The 80% CI threshold applies to the combined line coverage, not each file. CI
runs the same command on Linux x86_64 and uploads the table as `coverage.txt`.
`src/main.rs` is the CLI entry point: the tests reach only the commands they
run.
