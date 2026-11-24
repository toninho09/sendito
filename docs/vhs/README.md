# VHS tapes

Optional real-terminal recordings, captured with
[VHS](https://github.com/charmbracelet/vhs). These are a separate path: the GIFs
committed in [`docs/media/recordings`](../media/recordings) are **not** produced
here — they are rendered from the application's own frames by `cargo test -p
sendito-ui --test media -- --ignored && python3 docs/media/render.py` (see
[media/README.md](../media/README.md)). Use the tapes when you want a capture of
the real binary in a real terminal emulator.

They cover the same flows as the recordings, except `12-errors-cancel-delete`,
which has no tape yet.

Requirements: VHS (which needs `ttyd` and `ffmpeg`), the demo collection and the
echo servers:

```sh
cargo build --release && export PATH=$PWD/target/release:$PATH
sendito init /tmp/sendito-demo && cp tests/fixtures/acme.postman_collection.json /tmp/sendito-demo/
cargo run --release -p sendito-testkit --bin echo-servers &   # :8787 and :50051
for t in docs/vhs/*.tape; do vhs "$t"; done
```

Each tape writes `docs/vhs/out/<name>.gif`. The tapes are not run in CI.
