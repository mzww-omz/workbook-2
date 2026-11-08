# AGENTS.md

## Project

Rust + GPUI + SQLite macOS app. The Electron Workbook is a read-only compatibility reference; do not edit or migrate its data in place.

## Commands

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check --workspace
cargo run -p workbook-app --bin workbook2
./scripts/package-macos.sh
```

Test-only time source: `workbook_scheduler::FixedClock`. All scheduling tests must use it.

## Boundaries

- `workbook-domain`: pure types, grading, content validation, study calendar
- `workbook-scheduler`: FSRS adapter, StepPolicy, QueueSelector, Clock
- `workbook-storage`: SQLite schema, attempts, sessions, backup/restore
- `workbook-import`: JSON/legacy v1 inspection and migration plan
- `workbook-service`: commands, transactions, views, snapshots
- `workbook-ui`: GPUI Kit only. Never call SQLite or FSRS directly.
- `workbook-app`: startup, data directory, native menu

Keep FSRS formulas in `fsrs-rs`; do not reimplement them. StepPolicy and queue policy belong in `workbook-scheduler`.

## Data safety

- New app id `1463962418`; legacy id `1463962443`
- Never write to the legacy DB
- Back up before import/delete/restore/reset
- Never copy a live SQLite file directly; use the backup API
- Do not print user data or tokens
- No push, release, notarization, or remote upload without explicit request

## Verification

- Golden fixture: `fixtures/legacy-fsrs7-golden.json`
- Any scheduling change must keep the golden tests and scenario A/B/C tests passing
- Do not claim VoiceOver/IME/Windows/Linux verification without an actual run
- Screenshots live in `docs/screenshots/`; do not fabricate them

## UI conventions

- Do not add rounded cards, gradients, glows, or decorative motion
- Do not show scheduler internals during Study
- Feedback must show correctness as text as well as color
- Keep text inputs usable with Japanese IME; do not bind rating digits while an input is focused
- Keep the sidebar hidden during Study
