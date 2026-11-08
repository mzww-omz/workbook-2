#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
OUT="$ROOT/docs/screenshots"
mkdir -p "$OUT"

cargo build -p workbook-app --bin workbook2 --bin seed_demo >/dev/null
./target/debug/seed_demo /tmp/wb2-today today >/dev/null
./target/debug/seed_demo /tmp/wb2-study study >/dev/null
./target/debug/seed_demo /tmp/wb2-feedback feedback >/dev/null
./target/debug/seed_demo /tmp/wb2-result result >/dev/null
./target/debug/seed_demo /tmp/wb2-upcoming upcoming >/dev/null

caffeinate -d -t 180 &
CAFFEINE=$!
cleanup() {
  pkill -f 'target/debug/workbook2' 2>/dev/null || true
  kill "$CAFFEINE" 2>/dev/null || true
}
trap cleanup EXIT

capture() {
  local name="$1"
  local data="$2"
  local route="$3"
  WORKBOOK2_DATA_DIR="$data" WORKBOOK2_ROUTE="$route" \
    ./target/debug/workbook2 >"/tmp/workbook2-${name}.log" 2>&1 &
  local pid=$!
  sleep 3
  local id
  id="$(swift scripts/window-id.swift | head -1)"
  if [[ -z "$id" ]]; then
    echo "window not found for $name (see /tmp/workbook2-${name}.log)" >&2
    return 1
  fi
  screencapture -x -l "$id" "$OUT/$name.png"
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  echo "captured $OUT/$name.png"
}

capture today /tmp/wb2-today today
capture library /tmp/wb2-today library
capture history /tmp/wb2-today history
capture settings /tmp/wb2-today settings
capture study /tmp/wb2-study study
capture feedback /tmp/wb2-feedback study
capture result /tmp/wb2-result study
capture upcoming /tmp/wb2-upcoming today
