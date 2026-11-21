# VHS tapes

Real-terminal recordings of the same flows as `docs/media/recordings` (which are
rendered from the application's own frames by `cargo test -p sendito-ui --test media
-- --ignored && python3 docs/media/render.py`).

Requirements: [VHS](https://github.com/charmbracelet/vhs) (needs `ttyd` and `ffmpeg`),
the demo collection and the echo servers:

```sh
cargo build --release && export PATH=$PWD/target/release:$PATH
sendito init /tmp/sendito-demo && cp tests/fixtures/acme.postman_collection.json /tmp/sendito-demo/
cargo run --release -p sendito-testkit --bin echo-servers &   # :8787 and :50051
for t in docs/vhs/*.tape; do vhs "$t"; done
```
