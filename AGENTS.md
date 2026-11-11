# Agent Instructions
Rust + GPUI + SQLite macOS app. The legacy Electron Workbook is a read-only compatibility reference.

## Commands

| Task | Command |
|------|---------|
| Full verification | `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo check --workspace` |
| Scheduler tests | `cargo test -p workbook-scheduler` |
| Lint a workbook | `cargo run -p workbook-lint -- path/to/workbook.json` |
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
| 試験のスコープ・学習モード・統計 | `docs/EXAMS.md` |
| Linter CLI and diagnostic codes | `docs/WORKBOOK-LINTER.md` |
| AI authoring workflow and samples | `.agents/skills/workbook-authoring/SKILL.md` |
| Migration and restore | `docs/MIGRATION.md` |
| Product and UI direction | `PRODUCT.md`, `DESIGN.md` |
| Original requirements | `Workbook-2-GPUI-Coding-Agent-Master-Prompt.md` |

## Boundaries

- `workbook-domain` / `workbook-scheduler`: pure types, grading, calendar, FSRS adapter, StepPolicy, QueueSelector, Clock. Keep FSRS formulas in `fsrs-rs`.
- `workbook-storage` / `workbook-import`: SQLite, attempts, sessions, backup/restore; JSON and legacy v1 migration.
- `workbook-service`: commands, transaction boundaries, snapshots, views.
- `workbook-lint`: GUI不要の静的検証CLI。Domain検証を再利用し、教材品質のWarningを追加する。
- `workbook-ui` / `workbook-app`: GPUI Kit views and startup. UI must not call SQLite or FSRS directly.

## Scheduling Invariants

- Commit FSRS state, Attempt, Session, item status, note introduction, and sibling bury in one transaction.
- Moving a Question to another Note never drops its FSRS state. Create the target Note, upsert the Question, then delete removed rows.
- A semantic Question edit (prompt, answer, options, format, Flashcard target) advances `questions.revision` and deletes that Question's `review_states` in the same transaction. Explanation/hint/topic edits keep state. Attempts keep the `question_revision` they were answered under.
- Corrections update the current Attempt; never append a new Attempt.
- Automatic rating is conservative: wrong/unknown → Again, hinted correct → Hard, unhinted correct → Good. Never auto-assign Easy. Ungraded questions fall back to manual.
- Persist `attempts.rating_source` as automatic/manual. An automatic commit must be atomic with evaluation.
- Recompute the next queue from SQLite after every answer; do not use a fixed queue.
- Learn Ahead is limited to 20 minutes and the same 4 a.m. learning day. Never rewrite saved `due_at`.
- The daily new limit counts distinct committed New questions. Skip, Practice, and uncommitted evaluation do not count.
- Store UTC instants; use the configured IANA timezone only for the 4 a.m. day boundary.
- Time budgets select an item list; they are never a hard timer. Do not auto-finish an in-progress answer or mark unfitted reviews complete. Unselected items stay due.
- Scope and Goal planning reuse the existing planner and FSRS. Do not create a second memory model.
- Selecting a parent Topic includes its descendants at resolution time. Never merge Topics by name alone; identity is the stable ID.
- Export `topicCatalog` IDs and resolve them first on import, so a renamed Topic keeps its identity. Do not guess identity for JSON without a catalog.
- Refuse to delete a Topic referenced by a Goal/Exam scope (included or excluded) and name the owner.

## Authoring Invariants

- Saving an existing workbook must preserve `local_id` and every unchanged Note/Question ID so FSRS states and attempts stay attached.
- Back up with the SQLite backup API before replacing a workbook (`pre-edit-*`). Refuse edits while that workbook has an active session.
- Removed questions may lose their FSRS state through cascade, but attempt history must remain.
- Custom AI providers, API keys, and cloud calls are out of scope. Do not add them.
- Export embeds bundled assets as base64 and restores Topic paths; imported external AI-generated JSON still goes through the same Domain Validation and Linter.

## Data Safety

- New app id `1463962418`; legacy id `1463962443`. Never write to the legacy DB.
- Back up before import, delete, restore, or history reset. A normal workbook import is included: validate the workbook and all assets first, then create a uniquely named `pre-import-*.sqlite`, and abort if the backup fails.
- Never copy a live SQLite file directly; use the backup API. Action backups use `create_new` names so simultaneous imports cannot overwrite one another.
- `seed_demo` may only modify a directory carrying its exact marker file. Never call `remove_dir_all` on an arbitrary path.
- SVG safety is decided on a parsed XML tree with element/attribute allowlists; never substring or regex scans. Reject DTD/entities, foreign namespaces, scripts, event handlers, external references, and `style`.
- PNG/JPEG assets must decode completely. A header-only, truncated, wrong-CRC, or MIME-mismatched file is an import error; do not trust dimensions read from a header alone.
- Do not print user data, secrets, or tokens.
- Do not push, release, notarize, or upload without an explicit request.

## UI And Verification

- Keep GPUI dependencies pinned; do not mix crate versions. See `vendor/` and `docs/DECISIONS.md`.
- Do not add decorative cards, gradients, glows, or motion. Keep the sidebar hidden during Study.
- Do not show scheduler internals during Study. Feedback must use text as well as color.
- Hotspot answers use the real pointer position normalized to the drawn image area. Do not reintroduce a fixed click grid; keep the coordinate math in `workbook-ui/src/ui_logic.rs` and tested.
- Ordering, Matching, and Labeling options must render `Content` through the shared `content.rs` renderer. Do not flatten them to `plain_text()` or add per-format content renderers.
- Math blocks are typeset by `workbook-ui/src/math.rs` (LaTeX subset -> layout tree -> GPUI flex/text). Do not render raw LaTeX as the normal path and do not add a WebView. Unknown syntax must fall back to readable source text plus a reason, with the LaTeX exposed as the accessibility label. Keep the parser and layout decisions in pure functions with tests.
- Rich-content images use the typed `asset` source; do not infer bundled assets from URL prefixes. Serve bundled images from `StudyView.assets` and cache decoded images by workbook + asset + content fingerprint in the bounded LRU in `workbook-ui/src/image_cache.rs`. Do not clear the cache merely because the question changed. Invalidate on import, asset content change, workbook delete, and restore, and free evicted textures with `Image::remove_asset`.
- Image display boxes come from `workbook-ui/src/ui_logic.rs::image_box`. `contain` shows the whole image; `cover` fills a 4:3 viewport and crops. Hotspot and Labeling must keep using `contain` so normalized coordinates cannot shift under cropping.
- Both Hotspot clicks and Labeling target placement use the measured image bounds plus `ui_logic::contained_image_rect` / `image_point_to_container`. Do not position Labeling targets with percentages of the outer element.
- Do not bind rating digits while a text input is focused; keep IME input usable.
- Do not fabricate screenshots or claim VoiceOver, IME, Windows, or Linux verification without an actual run.
- Screenshots live in `docs/screenshots/`; regenerate with `./scripts/capture-screenshots.sh`.
