#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
OUT="$ROOT/docs/screenshots"
mkdir -p "$OUT"

# A fresh temporary root keeps the Seeder marker contract intact: every run
# starts with new directories and never reuses or deletes a user path.
TMP_BASE="${TMPDIR:-/tmp}"
DATA_ROOT="$(mktemp -d "${TMP_BASE%/}/workbook2-screenshots.XXXXXX")"

cargo build -p workbook-app --bin workbook2 --bin seed_demo >/dev/null
./target/debug/seed_demo "$DATA_ROOT/today" today >/dev/null
./target/debug/seed_demo "$DATA_ROOT/study" study >/dev/null
./target/debug/seed_demo "$DATA_ROOT/feedback" feedback >/dev/null
./target/debug/seed_demo "$DATA_ROOT/result" result >/dev/null
./target/debug/seed_demo "$DATA_ROOT/upcoming" upcoming >/dev/null
./target/debug/seed_demo "$DATA_ROOT/v2-numeric" v2-numeric >/dev/null
./target/debug/seed_demo "$DATA_ROOT/v2-ordering" v2-ordering >/dev/null
./target/debug/seed_demo "$DATA_ROOT/v2-matching" v2-matching >/dev/null
./target/debug/seed_demo "$DATA_ROOT/v2-hotspot" v2-hotspot >/dev/null
./target/debug/seed_demo "$DATA_ROOT/v2-labeling" v2-labeling >/dev/null
./target/debug/seed_demo "$DATA_ROOT/v2-near-miss" v2-near-miss >/dev/null
./target/debug/seed_demo "$DATA_ROOT/v2-math" v2-math >/dev/null

caffeinate -d -t 180 &
CAFFEINE=$!
cleanup() {
  pkill -f 'target/debug/workbook2' 2>/dev/null || true
  kill "$CAFFEINE" 2>/dev/null || true
  rm -rf "$DATA_ROOT"
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

capture today "$DATA_ROOT/today" today
capture library "$DATA_ROOT/today" library
capture history "$DATA_ROOT/today" history
capture settings "$DATA_ROOT/today" settings
capture study "$DATA_ROOT/study" study
capture feedback "$DATA_ROOT/feedback" study
capture result "$DATA_ROOT/result" study
capture upcoming "$DATA_ROOT/upcoming" today
capture numeric "$DATA_ROOT/v2-numeric" study
capture ordering "$DATA_ROOT/v2-ordering" study
capture matching "$DATA_ROOT/v2-matching" study
capture hotspot "$DATA_ROOT/v2-hotspot" study
capture labeling "$DATA_ROOT/v2-labeling" study
capture near-miss "$DATA_ROOT/v2-near-miss" study
capture math "$DATA_ROOT/v2-math" study
