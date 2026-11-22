# Coverage report

Line coverage of the non-UI crates (`cargo llvm-cov -p sendito-core -p sendito-http -p sendito-grpc -p sendito-store -p sendito-import`), measured on Linux x86_64 on 2026-11-21 on the v0.2.0 build. CI fails below 80% (`--fail-under-lines 80`).

| File | Lines | Line coverage |
|---|---|---|
| core/src/model.rs | 272 | 95.59% |
| core/src/resolve.rs | 614 | 97.72% |
| core/src/script.rs | 261 | 100.00% |
| core/src/template.rs | 128 | 100.00% |
| core/src/vars.rs | 318 | 98.74% |
| grpc/src/lib.rs | 317 | 89.27% |
| grpc/src/schema.rs | 346 | 75.72% |
| http/src/lib.rs | 561 | 81.46% |
| import/src/curl.rs | 455 | 91.21% |
| import/src/lib.rs | 135 | 97.04% |
| import/src/postman.rs | 633 | 87.20% |
| store/src/collection.rs | 489 | 93.66% |
| store/src/config.rs | 139 | 97.12% |
| store/src/env.rs | 494 | 88.06% |
| store/src/history.rs | 103 | 91.26% |
| store/src/init.rs | 91 | 94.51% |
| store/src/lib.rs | 6 | 100.00% |
| store/src/request_file.rs | 545 | 97.98% |
| store/src/session.rs | 44 | 97.73% |
| store/src/themes.rs | 102 | 99.02% |
| store/src/tomlfmt.rs | 162 | 96.30% |
| **TOTAL** | 6215 | **91.89%** |
