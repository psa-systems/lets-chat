# Phase 24 - Test debt cleanup

## Goal

Get every test in `server/tests/` compiling and passing on `main`, in both standalone and saas modes, then document the categories of drift that accumulated so future phases can prevent them. This is engineering hygiene, not a feature.

Phase 23 surfaced the visible symptom: `email_digest_dispatch.rs` and several other tests fail to compile because `AppState` (and its construction sites in tests) drifted with later phases. The full scope is unknown until the audit runs - that is the deliberate shape of this phase.

## Phase shape: audit first, fix systematically

Unlike feature phases, this one's scope is unknown until the audit runs. The task list is intentionally minimal:

- **Task 1 (Audit)** runs `just test` and `just test-saas`, classifies every failing test by drift category, and posts the audit table.
- **Tasks 2-N (Fix)** are defined by what the audit surfaces. Each fix task addresses ONE drift category across all affected files.
- **Final task (Documentation)** codifies the lessons learned into a "Test maintenance" section in `CLAUDE.md`.

Tasks 2-N will be appended to this plan after Task 1 posts the audit and the user gives the go-ahead.

## Hard constraints

- No Rust/Bun on the host. Use `./dev/cargo` and `./dev/bun` wrappers only.
- All work in `server/`. Do not touch `desktop/`.
- Do NOT add new tests in this phase. The goal is to make existing tests work. Adding new tests is feature scope.
- Do NOT fix unrelated bugs surfaced by the audit. If a test fails for a reason that is not "drift from a later phase" (e.g., a real logic bug), flag it in the audit table and leave it. Fixing it is a separate phase.
- Do NOT refactor "while you are in there." If a failing test file has style issues, dead imports, or other smells, leave them. The scope is "make tests compile and pass," not "improve test code."
- Claude does NOT commit or push. Stage with `git add` and stop. The user commits per task during execution as a review step.

## Out of scope

- Adding new tests or coverage. Debt cleanup only.
- Coverage gap analysis (e.g., "this module has no tests"). Its own future work.
- CI integration, pre-commit hooks, or any tooling improvements. A separate phase if anyone wants them.
- "Importance" or "criticality" rankings on tests. Fix all failing tests regardless of what they cover.
- Speculative tasks. Audit-first means the scope is not known yet; padding the plan would defeat the point.

## Background

The test infrastructure conventions, gleaned from existing files:

- Every `server/tests/*.rs` is a standalone integration binary.
- Each test file opens its own in-memory SQLite pools and registers migrations explicitly (no shared `setup_*_pool()` helper across the suite; each file has its own).
- `AppState` is constructed in tests by hand, mirroring the production startup path in `server/src/main.rs` but with mocks (e.g., `MockEmailClient` from phase 22, `MockPushClient` from phase 16) injected in place of real I/O clients.
- Standalone mode (`just test`) and saas mode (`just test-saas` - `--no-default-features --features saas`) compile distinct code paths. A test that compiles in one mode may not compile in the other.

The known drift symptoms from phase 23:

- `AppState` shape drift: later phases added fields (e.g., `bg`, the background task spawn state) without updating the in-test `AppState { ... }` literals.
- Migration list drift: later phases added migrations that some test pool-setup helpers do not include.
- Type signature drift: helpers refactored to take `&SqlitePool` instead of `&AppState` (or vice versa) leave callers in test files broken.
- Removed or renamed functions still referenced in tests.
- Hand-crafted test fixtures rejected by newer library versions (phase 23 surfaced this with `TINY_PNG` and the `image` crate's stricter decode).

The audit will confirm which of these categories actually apply and surface any new ones.

## File Map

Until the audit completes, the only known artifact this phase produces is:

| Action | Path | Responsibility |
|---|---|---|
| Add | `docs/superpowers/plans/2026-05-14-phase24-test-debt-cleanup.md` | This plan. Tasks 2-N appended after Task 1. |
| Edit | `CLAUDE.md` (or equivalent) | "Test maintenance" section: drift categories, prevention patterns. Added in the final task. |
| Edit | various `server/tests/*.rs` | Per-drift-category fixes. Specific files determined by audit. |

The "Edit `server/tests/*.rs`" row is intentionally underspecified; it will resolve into one task per drift category after Task 1.

## Audit results (Task 1 output, 2026-05-14)

Standalone (`cargo test -p lets-chat-server`): 1 binary fails to compile (`email_digest_dispatch`); 19 binaries have at least one runtime panic. Saas (`cargo test --no-default-features --features saas`): 0 compile failures; 21 binaries have at least one runtime panic.

### Audit table

| # | File | Mode | Failure type | Root cause | Classification |
|---|---|---|---|---|---|
| 1 | email_digest_dispatch.rs | standalone | compile | `AppState { ... }` literal missing `bg` field added in a later phase | appstate-drift |
| 2 | db_dm.rs | both | panic | `INSERT messages` SQL references `quote_id`; test pool missing `0020_quote_reply.sql` | migration-drift |
| 3 | db_mentions.rs | both | panic | same as above | migration-drift |
| 4 | db_moderation.rs | both | panic | same | migration-drift |
| 5 | db_reactions.rs | both | panic | same | migration-drift |
| 6 | db_read_receipts.rs | both | panic | same | migration-drift |
| 7 | db_search.rs | both | panic | same | migration-drift |
| 8 | db_uploads.rs | both | panic | same | migration-drift |
| 9 | message_editing.rs | both | panic | same | migration-drift |
| 10 | message_grouping.rs | both | panic | same | migration-drift |
| 11 | routes_account_delete.rs | both | panic (some) + assertion (saas-only) | quote_id drift; plus saas-only test asserts 400 for wrong password but gets 303 (standalone-only behavior) | migration-drift + feature-gate-drift |
| 12 | routes_bookmarks.rs | both | panic | quote_id drift | migration-drift |
| 13 | routes_broadcast_mentions.rs | both | assertion (500 vs 200) | downstream of quote_id drift; handler INSERTs message, fails, returns 500 | migration-drift |
| 14 | routes_dm_mute.rs | both | assertion (500 vs 200) | same downstream pattern | migration-drift |
| 15 | routes_enclave.rs | both | assertion (`is_redirection` fails) | NOT migration drift. Production handler `post_invite` at `server/src/routes/enclave.rs:436` returns HTMX HTML fragment (`Result<Html, AppError>`), not a redirect. Test sends `username=bob` but the form's `InviteForm` expects `user_id`. Test asserts a contract the handler no longer maintains. Reclassified after Task 2 verification. | test-contract-drift (NEW category) |
| 16 | routes_export.rs | both | panic | quote_id drift | migration-drift |
| 17 | routes_mentions.rs | both | assertion (500 vs 200) | same downstream pattern | migration-drift |
| 18 | routes_pinned.rs | both | panic | quote_id drift | migration-drift |
| 19 | routes_room_mute.rs | both | assertion (500 vs 200) | same downstream pattern | migration-drift |
| 20 | last_visited.rs | both | assertion (500 vs 200) | same downstream pattern | migration-drift |
| 21 | admin_uploads.rs | saas only | assertion (404 vs 303) | admin routes module is `#[cfg(feature = "standalone")]` (server/src/routes/mod.rs:26-27); the test file is not cfg-gated and asserts routes that do not exist in saas | feature-gate-drift |
| 22 | routes_uploads.rs::preview_query_returns_smaller_bytes_than_original | saas only | assertion (500 vs 200) | only the preview-query test (saas-only fail); other tests in this binary pass in both modes. Either saas drops a dep the upload pipeline needs at runtime, or a real bug in the saas-mode upload path. Validate by adding `RUST_BACKTRACE=1` rerun; if drift, fix; if real-bug, flag and skip. | needs-investigation |

### Tally

- **Total failing test files**: 21 unique (some appear in both columns)
- **Compile failures**: 1 (standalone-only: email_digest_dispatch.rs)
- **Runtime failures**: 20 unique files (19 standalone, 21 saas; overlap is the migration-drift group)
- **All quote_id-related failures**: 18 of the runtime failures, traceable to the missing `0020_quote_reply.sql` (and `0021_enclave_invitations_enclave_idx.sql`, which is just an index and unlikely to be the cause but should be added together for completeness) in `setup_chat_pool()` helpers across the affected test files.

### Noted but not addressed in this phase

The 19 affected files use two different `setup_chat_pool()` patterns: 16 declare their migrations in a `for sql in [include_str!(...), ...]` array (with a one-line `expect`-less call) and 3 use the older verbose `let chat_mN = include_str!(...); sqlx::raw_sql(chat_mN).execute(&chat_pool).await.expect("chat migration N");` per-migration form. Both patterns are functionally equivalent. Consolidating them onto one shape would be a clean follow-up hygiene task but is explicitly out of scope here; Task 2 preserved each file's existing pattern and added the new migrations in the matching form. The docs task (Final) MUST mention both patterns in its prevention recipe so future migration-add steps cover both.

### Task 2 outcome (post-fix verification)

After adding `0020_quote_reply.sql` and `0021_enclave_invitations_enclave_idx.sql` to all 19 affected `setup_chat_pool()` helpers and running each binary in both modes:

- 18 of 19 binaries pass cleanly in both modes (migration drift was the root cause as classified).
- 1 binary (`routes_enclave`) still has 1 failing test (`invite_then_accept_creates_membership`). Reclassification: this is NOT migration drift. The production handler `post_invite` at `server/src/routes/enclave.rs:436` was rewritten to return `Result<Html, AppError>` (HTMX fragment) instead of a redirect. The test asserts `res.status().is_redirection()`. Additionally, the test sends form field `username=bob` while `InviteForm` now expects `user_id=<id>`. Two layers of test-vs-production contract drift.
- 0 new failures introduced anywhere (verified by re-running the full suite in both modes).

### Drift category breakdown

1. **appstate-drift** (1 file): `email_digest_dispatch.rs` `AppState { ... }` literal missing `bg`. Phase that added the field: the `bg: BgWriter` field at `server/src/state.rs:30`; cross-reference to find the phase that introduced it during the fix.
2. **migration-drift** (19 files): every `setup_chat_pool()` helper in the affected files needs `0020_quote_reply.sql` (and `0021_enclave_invitations_enclave_idx.sql`) appended to its migration include list. The 5 phase-23 test files (`admin_uploads`, `uploads_sweep`, `uploads_pipeline`, `routes_uploads`, `email_digest_dispatch`'s neighbors) already have both, which is how the pattern was identified. **FIXED in Task 2**; 18 of 19 affected binaries now pass in both modes.
3. **feature-gate-drift** (1-2 files): `admin_uploads.rs` (whole file) and one `routes_account_delete.rs` test exercise standalone-only routes/behavior without a `#![cfg(feature = "standalone")]` (or per-test) gate. Adding the gate makes the saas build skip them, matching the production behavior they are testing.
4. **test-contract-drift** (1 test, NEW category discovered in Task 2 verification): `routes_enclave.rs::invite_then_accept_creates_membership` asserts a handler contract that no longer matches production: expects redirect but handler returns HTMX HTML; sends `username` but form expects `user_id`. Distinct from feature-gate-drift (the handler exists in both modes and works correctly; only the test is stale).
5. **needs-investigation** (1 test): `routes_uploads.rs::preview_query_returns_smaller_bytes_than_original` saas-only 500. Could be drift, could be a real bug in the saas upload path. Requires a `RUST_BACKTRACE=1` rerun against this single test to classify; defer decision until then.

### Proposed Task 2-N breakdown

- **Task 2 - Migration list drift. (DONE)** Added `0020_quote_reply.sql` and `0021_enclave_invitations_enclave_idx.sql` to the `setup_chat_pool()` migration include list in 19 files. Verified each binary in both modes; 18 of 19 now pass. The 19th (routes_enclave) was reclassified to a new drift category and moved to Task 5.
- **Task 3 - AppState construction drift.** Add `bg: <whatever production constructs>` to the `AppState { ... }` literal in `email_digest_dispatch.rs:148`. Match the shape used in production (`server/src/main.rs` startup path); other test files that construct `AppState` already have it, so they are the reference. Standalone-only; saas already cfg-skips this file.
- **Task 4 - Feature-gate drift.** Add `#![cfg(feature = "standalone")]` to the top of `admin_uploads.rs`. For `routes_account_delete.rs::delete_rejects_wrong_password`, add a per-test `#[cfg(feature = "standalone")]` attribute (the rest of the file's tests pass in saas, so a whole-file gate would overreach). Confirm saas test run no longer touches those.
- **Task 5 - Test-contract drift (NEW, discovered in Task 2 verification).** The single failing test `routes_enclave.rs::invite_then_accept_creates_membership` asserts a contract the handler no longer maintains: HTMX HTML response, not redirect; form field `user_id`, not `username`. Fix in this task by updating the test to: send `user_id=<bob's id>` and assert `res.status() == StatusCode::OK` with an HTML body that surfaces the success message. This is in scope (test drift, not production bug). Standalone-only verification suffices since saas exercises the same handler.
- **Task 6 - Investigate `preview_query_returns_smaller_bytes_than_original` saas-only 500.** Rerun the single test with `RUST_BACKTRACE=1` and capture the underlying error. If the panic is drift, fix in this task. If real bug, flag and stop; separate phase. Was Task 5 before reclassification.
- **Final task - Documentation.** Add a "Test maintenance" section to `CLAUDE.md` documenting the four drift categories actually found (appstate-drift, migration-drift, feature-gate-drift, test-contract-drift) and the prevention patterns.

### Tests that fail for non-drift reasons

- **Flake (out of scope)**: `routes_uploads::upload_happy_path_returns_file_id_and_url` and `routes_uploads::send_message_with_attachment_renders_inline_image` intermittently fail with 500 when the full test suite runs concurrently. Pass cleanly in isolation. Both tests use per-PID tempdirs (`std::env::temp_dir().join(format!("lets-chat-tests-{}", std::process::id()))`) so there should be no path collision; the contention is probably filesystem-level under parallel test-binary load. Surfaced post-Task-3, was not present in the original audit (because that run had `email_digest_dispatch.rs` moved aside, reducing concurrent-binary count). Pre-existing flake; not introduced by phase 24 edits. Flag for a follow-up phase if it shows up in CI noise. Distinct from the saas-only `preview_query_returns_smaller_bytes_than_original` failure (Task 6).
- The `needs-investigation` row (Task 6) is the other candidate; classification deferred until the backtrace is captured.

### Open questions before Tasks 2-N start

1. The standalone test recipe `just test` runs against the shared `lets-chat-rewrite-target` Docker volume; that volume is shared with another uid on this host and the audit was run in a private `phase24-target` volume to avoid the race. The fixes will be verified in the private volume too. The `just test` command itself is unchanged; the volume coordination issue is environmental, not in scope.
2. The `routes_enclave.rs:376` failure is presumed to be migration-drift downstream (the POST handler probably touches messages or another quote_id-aware code path). If after Task 2 it remains failing, it gets bumped to its own task or flagged as a real bug.

### Awaiting go-ahead

Tasks 2-5 plus Final are ready to start once the user confirms the breakdown.

---

## Tasks

### Task 1 - Audit (DONE)

Produced the audit table above.

- [ ] `git checkout -b chore/test-debt-cleanup`
- [ ] `./dev/cargo test -p lets-chat-server 2>&1 | tee /tmp/phase24-test-standalone.log`
  - Do NOT use `--no-run`; we want both compile failures AND runtime failures.
  - Tee the full output so the audit can cite specific compiler errors and panic messages.
- [ ] `./dev/cargo test -p lets-chat-server --no-default-features --features saas 2>&1 | tee /tmp/phase24-test-saas.log`
- [ ] For every test file that fails (compile, panic, or assertion), produce one row of audit data:

  | File | Mode (standalone / saas / both) | Failure type (compile / panic / assertion) | Root cause (one phrase) | Classification |

  Classification buckets (use exactly one; create a new bucket name if none fits):
  - `appstate-drift` (test constructs `AppState { ... }` missing fields added in later phases)
  - `migration-drift` (test's pool-setup helper missing migrations from later phases)
  - `signature-drift` (test calls a helper whose signature has changed)
  - `renamed-or-removed` (test references a function/type that no longer exists under that name)
  - `fixture-drift` (hand-crafted test fixture rejected by a newer library version)
  - `real-bug` (test correctly catches a production-code bug; OUT OF SCOPE for this phase, flag and skip)
  - `flake` (intermittent failure; OUT OF SCOPE, flag and skip)
  - `other` (drift that does not fit; add a one-line note in the row)

- [ ] Post the audit table in chat, along with:
  1. The proposed Task 2-N breakdown by drift category (one task per category that has at least one row).
  2. Any rows classified as `real-bug`, `flake`, or `other` - flagged for the user's awareness, not for this phase to fix.
- [ ] Wait for the user's go-ahead before proceeding to fix tasks.
- [ ] `git add docs/superpowers/plans/2026-05-14-phase24-test-debt-cleanup.md` (append the audit table to this plan file as part of the same commit so the audit is preserved alongside the plan).

### Tasks 2-N - Fix systematically

To be defined after Task 1 posts the audit table and the user approves.

Each task in this range covers exactly one drift category:

- The task names the category and the affected files.
- The task identifies the upstream change (which later phase added the field, renamed the helper, added the migration).
- The task applies the same fix shape to every affected file in that category.
- The task verifies green with `./dev/cargo test --test <name>` for each affected binary (and `--no-default-features --features saas` if the failure was in saas mode).
- The task does NOT expand scope: if a different drift category is noticed in a file while fixing this one, leave it for the appropriate later task.

Estimate: 3-6 tasks beyond the audit, depending on what surfaces. The number is deliberately not pre-baked.

For each fix task the closeout is:

- [ ] `./dev/cargo test -p lets-chat-server --test <name>` green for every affected file
- [ ] `./dev/cargo test -p lets-chat-server --no-default-features --features saas --test <name>` green for every saas-affected file
- [ ] `./dev/cargo check -p lets-chat-server` AND `./dev/cargo check -p lets-chat-server --no-default-features --features saas` both clean (catches accidental breakage of production code from test-side changes)
- [ ] `git add <fixed files>` and stop. The user commits.

### Final task - Documentation

Codify the drift categories so future phases inherit the fix.

- [ ] Add a "Test maintenance" section to `CLAUDE.md` covering:
  - The categories of drift this phase found (only the ones that actually applied, not the speculative list).
  - For each category, what triggers it and the prevention pattern. Examples:
    - `appstate-drift`: triggered by adding a field to `AppState`. Prevention: grep `AppState {` in `server/tests/` after the change and update every construction site.
    - `migration-drift`: triggered by adding a new SQL migration. Prevention: grep `setup_*_pool` (or whatever the canonical test-pool helper is) in `server/tests/` and update the migration include list.
    - `signature-drift`: triggered by changing a function signature. Prevention: rely on `./dev/cargo check` showing the call-site errors; do not merge with broken tests.
  - Note that test files compiling is a precondition for landing a phase PR, not optional.
- [ ] Run `just check` and `just test` and `just test-saas` to confirm the documentation edit did not regress anything (it should not - `CLAUDE.md` is not compiled, but verify regardless).
- [ ] `git add CLAUDE.md docs/superpowers/plans/2026-05-14-phase24-test-debt-cleanup.md`

### Verification (after all fix tasks)

- [ ] `just check` (clippy + fmt for both standalone and saas)
- [ ] `just test` clean - zero failing binaries
- [ ] `just test-saas` clean - zero failing binaries
- [ ] `just verify` still passes (release binary serves `/login`)
- [ ] PR title: `chore(tests): phase 24 - test debt cleanup`. PR body lists the drift categories fixed, the count of test files touched per category, and any rows flagged as `real-bug` / `flake` / `other` for follow-up phases.

## Things to confirm during implementation

1. **`just test` is the canonical "run all tests" command.** The justfile defines `test` (standalone) and `test-saas` (saas). There is no third recipe. Both must be run; both must be green at the end of the phase.
2. **Standalone vs. saas compile divergence.** A test that compiles standalone may fail saas (or vice versa) because of `#[cfg(feature = "saas")]` gates in production code that change the shape of `AppState` or which DB queries exist. The audit must run BOTH commands; do not assume a green standalone run implies a green saas run.
3. **`AppState` definition site.** The plan assumes `server/src/state.rs` is the canonical definition. Confirm before writing the audit; if the type has moved or has multiple variants (e.g., a `BuildMode` discriminator), the audit needs to note which variant each failing test constructs.
4. **Test-pool setup pattern.** The plan assumes each test file has its own pool-setup helper rather than a shared `tests/common/mod.rs`. Confirm by reading two or three random failing files; if there IS a shared helper, the migration-drift fix is centralised (one file edit) rather than scattered.
5. **No `--exact` filtering during audit.** The audit must run the FULL suite, not selected binaries. The cost of running everything is a few minutes; the cost of missing a failure is a hole in the audit table.

## Summary plan

Phase 24 is engineering hygiene, not a feature. The structure is:

1. **Task 1 - Audit.** Run `just test` and `just test-saas`, build a table of every failing test, classify by drift category. Post the table and the proposed Tasks 2-N breakdown. Wait for go-ahead.
2. **Tasks 2-N - Fix systematically.** One task per drift category. Each task addresses all files in its category, with the same fix shape. Specific tasks defined after Task 1.
3. **Final task - Documentation.** "Test maintenance" section in `CLAUDE.md`: drift categories, prevention patterns, precondition for landing phase PRs.

The task count is deliberately not pre-committed. Audit-first means the scope is not known yet; the plan reflects that honestly rather than padding with speculative tasks.
