#!/usr/bin/env bash
# Runs a command, keeps the full output in target/<mode>.log and prints only a
# short summary on success. On failure, dumps the log and exits with an error.
#
# Usage: scripts/quiet.sh <mode> <command...>
# Modes: test (cargo test), cov (cargo llvm-cov), plain (prints "mode: ok")
set -uo pipefail

mode=${1:?missing mode}
shift
log="target/${mode}.log"
mkdir -p target

if ! "$@" >"$log" 2>&1; then
  cat "$log"
  printf '\nFAILED — full log: %s\n' "$log" >&2
  exit 1
fi

case $mode in
  test|corpus|snap|media)
    awk '/^test result:/ { suites++; pass += $4; fail += $6; ign += $8; next }
         /^running [0-9]+ tests?$/ { next }
         /^[.i]+$/ { next }
         /^$/ { next }
         { print }
         END { printf "tests: %d suites, %d passed, %d failed, %d ignored\n", suites, pass, fail, ign }' "$log"
    ;;
  cov)
    awk '$1 == "TOTAL" { printf "coverage: lines %s (gate 80%%)\n", $10; found = 1 }
         END { if (!found) print "coverage: TOTAL missing from the log" }' "$log"
    ;;
  *)
    echo "${mode}: ok"
    ;;
esac
