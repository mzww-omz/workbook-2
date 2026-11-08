# Agent Instructions
Rust + GPUI + SQLite macOS app. The legacy Electron Workbook is a read-only compatibility reference.

## Commands

| Task | Command |
|------|---------|
| Full verification | `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo check --workspace` |
| Scheduler tests | `cargo test -p workbook-scheduler` |
| Service scenarios | `cargo test -p workbook-service --test service` |
| Storage/backup tests | `cargo test -p workbook-storage --test storage` |
| Run app | `cargo run -p workbook-app --bin workbook2` |
| Build `.app` | `./scripts/package-macos.sh` |
| Regenerate golden fixture | `npm --prefix scripts install && npm --prefix scripts run golden` |

Use `workbook_scheduler::FixedClock` for all scheduling tests. Do not wait on real time.

## External References

| Need | File |
|------|------|
| Setup and usage | `README.md` |
| Architecture and data flow | `docs/ARCHITECTURE.md`, `docs/DECISIONS.md` |
| FSRS compatibility and legacy audit | `docs/COMPATIBILITY.md`, `docs/AUDIT.md` |
| Migration and restore | `docs/MIGRATION.md` |
| Product and UI direction | `PRODUCT.md`, `DESIGN.md` |
| Original requirements | `Workbook-2-GPUI-Coding-Agent-Master-Prompt.md` |

## Boundaries

- `workbook-domain` / `workbook-scheduler`: pure types, grading, calendar, FSRS adapter, StepPolicy, QueueSelector, Clock. Keep FSRS formulas in `fsrs-rs`.
- `workbook-storage` / `workbook-import`: SQLite, attempts, sessions, backup/restore; JSON and legacy v1 migration.
- `workbook-service`: commands, transaction boundaries, snapshots, views.
- `workbook-ui` / `workbook-app`: GPUI Kit views and startup. UI must not call SQLite or FSRS directly.

## Scheduling Invariants

- Commit FSRS state, Attempt, Session, item status, note introduction, and sibling bury in one transaction.
- Corrections update the current Attempt; never append a new Attempt.
- Recompute the next queue from SQLite after every answer; do not use a fixed queue.
- Learn Ahead is limited to 20 minutes and the same 4 a.m. learning day. Never rewrite saved `due_at`.
- The daily new limit counts distinct committed New questions. Skip, Practice, and uncommitted evaluation do not count.
- Store UTC instants; use the configured IANA timezone only for the 4 a.m. day boundary.

## Data Safety

- New app id `1463962418`; legacy id `1463962443`. Never write to the legacy DB.
- Back up before import, delete, restore, or history reset.
- Never copy a live SQLite file directly; use the backup API.
- Do not print user data, secrets, or tokens.
- Do not push, release, notarize, or upload without an explicit request.

## UI And Verification

- Keep GPUI dependencies pinned; do not mix crate versions. See `vendor/` and `docs/DECISIONS.md`.
- Do not add decorative cards, gradients, glows, or motion. Keep the sidebar hidden during Study.
- Do not show scheduler internals during Study. Feedback must use text as well as color.
- Do not bind rating digits while a text input is focused; keep IME input usable.
- Do not fabricate screenshots or claim VoiceOver, IME, Windows, or Linux verification without an actual run.
- Screenshots live in `docs/screenshots/`; regenerate with `./scripts/capture-screenshots.sh`.
