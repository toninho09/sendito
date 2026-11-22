# Coverage report

Line coverage of the non-UI crates on Linux x86_64, measured with:

```sh
cargo llvm-cov -j 4 -p sendito-core -p sendito-http -p sendito-grpc \
  -p sendito-store -p sendito-import -p sendito-mcp \
  --fail-under-lines 80 --summary-only -- --test-threads=2
```

| File | Lines | Line coverage |
|---|---:|---:|
| core/src/model.rs | 291 | 94.16% |
| core/src/resolve.rs | 635 | 94.49% |
| core/src/script.rs | 261 | 100.00% |
| core/src/template.rs | 128 | 100.00% |
| core/src/vars.rs | 318 | 98.74% |
| grpc/src/lib.rs | 317 | 89.27% |
| grpc/src/schema.rs | 346 | 75.72% |
| http/src/lib.rs | 561 | 81.46% |
| import/src/curl.rs | 455 | 91.21% |
| import/src/lib.rs | 135 | 97.04% |
| import/src/postman.rs | 633 | 87.20% |
| mcp/src/http.rs | 182 | 84.07% |
| mcp/src/lib.rs | 583 | 90.57% |
| mcp/src/oauth.rs | 603 | 83.75% |
| mcp/src/protocol.rs | 22 | 100.00% |
| mcp/src/stdio.rs | 82 | 80.49% |
| store/src/collection.rs | 489 | 93.66% |
| store/src/config.rs | 192 | 96.35% |
| store/src/env.rs | 494 | 88.06% |
| store/src/history.rs | 103 | 91.26% |
| store/src/init.rs | 91 | 94.51% |
| store/src/lib.rs | 6 | 100.00% |
| store/src/request_file.rs | 592 | 98.14% |
| store/src/session.rs | 44 | 97.73% |
| store/src/themes.rs | 102 | 99.02% |
| store/src/tomlfmt.rs | 162 | 96.30% |
| **TOTAL** | **7827** | **90.66%** |

The 80% CI threshold applies to the combined line coverage, not each file.
