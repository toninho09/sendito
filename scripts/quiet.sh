#!/usr/bin/env bash
# Roda um comando, guarda a saída completa em target/<modo>.log e imprime só um
# resumo curto em caso de sucesso. Em falha, despeja o log e sai com erro.
#
# Uso: scripts/quiet.sh <modo> <comando...>
# Modos: test (cargo test), cov (cargo llvm-cov), plain (usa "modo: ok")
set -uo pipefail

mode=${1:?modo ausente}
shift
log="target/${mode}.log"
mkdir -p target

if ! "$@" >"$log" 2>&1; then
  cat "$log"
  printf '\nFALHOU — log completo: %s\n' "$log" >&2
  exit 1
fi

case $mode in
  test|corpus|snap|media)
    awk '/^test result:/ { suites++; pass += $4; fail += $6; ign += $8; next }
         /^running [0-9]+ tests?$/ { next }
         /^[.i]+$/ { next }
         /^$/ { next }
         { print }
         END { printf "testes: %d suites, %d passed, %d failed, %d ignored\n", suites, pass, fail, ign }' "$log"
    ;;
  cov)
    awk '$1 == "TOTAL" { printf "cobertura: linhas %s (gate 80%%)\n", $10; found = 1 }
         END { if (!found) print "cobertura: TOTAL ausente no log" }' "$log"
    ;;
  *)
    echo "${mode}: ok"
    ;;
esac
