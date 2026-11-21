#!/usr/bin/env bash
# Performance benchmarks against the TARGET thresholds in docs/PLAN.md.
# Needs: hyperfine, /usr/bin/time (GNU on Linux, BSD on macOS), script(1), python3.
set -euo pipefail
cd "$(dirname "$0")/.."
BIN=${BIN:-target/release/sendito}
OUT=bench/results
COLL=${COLL:-$(mktemp -d)/collection}
PORT=${PORT:-18787}
mkdir -p "$OUT"
cargo build --release --bin sendito -p sendito >/dev/null
cargo build --release -p sendito-testkit --bin echo-servers >/dev/null
python3 bench/gen_collection.py "$COLL" "$PORT" >/dev/null
target/release/echo-servers "127.0.0.1:$PORT" "127.0.0.1:$((PORT+1))" >/dev/null &
ECHO=$!
trap 'kill $ECHO 2>/dev/null || true' EXIT
sleep 0.5

# Run the app in a 120x40 pseudo-terminal.
if [[ "$(uname)" == "Darwin" ]]; then
  pty() { script -q /dev/null bash -c "stty cols 120 rows 40; $1"; }
  TIME="/usr/bin/time -l"
else
  pty() { script -qec "stty cols 120 rows 40; $1" /dev/null; }
  if [[ -x /usr/bin/time ]]; then TIME="/usr/bin/time -v"; else TIME="python3 bench/maxrss.py"; fi
fi

echo "== cold start to first frame (20 runs)"
: > "$OUT/first-frame-us.txt"
for i in $(seq 1 20); do
  pty "$BIN '$COLL' --exit-after-first-frame 2>>'$OUT/first-frame-us.txt'" >/dev/null
done
hyperfine -N --warmup 3 --runs 20 --export-json "$OUT/startup-hyperfine.json" \
  "script -qec 'stty cols 120 rows 40' /dev/null" \
  "script -qec 'stty cols 120 rows 40; $BIN $COLL --exit-after-first-frame' /dev/null" > "$OUT/startup-hyperfine.txt" 2>&1 || true

echo "== RSS idle"
pty "$TIME -o '$OUT/rss-idle.txt' $BIN '$COLL' --script bench/scripts/idle.txt" >/dev/null
echo "== RSS with a 10 MB JSON response"
pty "$TIME -o '$OUT/rss-10mb.txt' $BIN '$COLL' --script bench/scripts/big-response.txt" >/dev/null
echo "== frame times over a 60 s scroll/type session"
rm -f "$OUT/frames.log"
pty "$BIN '$COLL' --script bench/scripts/frames-60s.txt --frame-log '$OUT/frames.log'" >/dev/null

python3 bench/report.py "$OUT"
