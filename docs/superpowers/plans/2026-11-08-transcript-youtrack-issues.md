# Transcript -> YouTrack Issues Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** After a call, let a reviewer turn the transcript's action items into proposed YouTrack issues and file the accepted ones into an org-wide, admin-configured YouTrack in one step.

**Architecture:** A new `youtrack` module holds the admin-configured connection (base URL, sealed token, default project) and a thin create-issue client over `http_client::outbound_trusted_post`. An LLM call (`complete_guarded`) turns the transcript into JSON proposals stored in a new `transcript_issue_proposals` table. The transcript page gains a "Proposed issues" panel: room members generate/review; a settings feature flag plus the LC-679 staff role gate the "File accepted" action, which creates the issues and marks rows filed with audit.

**Tech Stack:** Rust, Axum, Askama (SSR), HTMX, SQLite (sqlx), the existing `llm` + `http_client` + `feature-flag`/`ai_gate` + admin-settings modules.

**Spec:** docs/superpowers/specs/2026-11-08-transcript-youtrack-issues-design.md

## Global Constraints

- Outbound HTTP ONLY through `http_client`; use `outbound_trusted_post` for the admin-configured endpoint (LC-152 grep-ban forbids raw `reqwest::Client::*` and `outbound_unchecked` in `server/src/`).
- Migrations are append-only and immutable (LC-212): add a new migration file, never edit a shipped one.
- Secrets never rendered to a template or logged; seal the token with `state.secret_key`, mirror `imap_password` (write-only field, empty submit preserves, expose only `*_configured: bool`).
- Settings reads follow LC-919: match on the `get_setting` Result, `tracing::warn!` + safe fallback on `Err`; split pure narrowing into a helper taking `Result<Option<String>, sqlx::Error>` so it is unit-testable without a tracing subscriber (LC-782).
- Feature flag default OFF (absent reads off), read live from the settings KV (no restart), LC-679 pattern.
- `cargo test` takes ONE positional TESTNAME; extra filters go after `--`.
- CHANGELOG: add an `Added` entry (user-facing). Section order Security, Changed, Added, Fixed, Deprecated.
- Commit attribution footer:
  `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` + `Claude-Session: https://claude.ai/code/session_014ZM3DYgYxuaxiYr8KddAd7`.

## Review Focus

- YouTrack returns a non-2xx or is unreachable at file time -> that row stays `proposed`, its error is shown, nothing is marked filed, the token is not logged. (Task 4 + Task 6 tests.)
- A non-staff room member POSTs `file-issues` directly with the flag on -> 403, no issue created. (Task 6 test.)
- The flag is off (or base URL / token / project unset) -> generate and file routes return a clear 4xx and the panel is hidden. (Task 2 + Task 5 + Task 6 tests.)
- The LLM returns malformed JSON, a code-fence-wrapped array, or more than the cap -> parser drops bad entries and bounds the count/length rather than erroring or creating an unbounded list. (Task 5 test.)
- Filing the same accepted proposal twice (double submit) -> the already-`filed` row is skipped, no duplicate YouTrack issue. (Task 6 test.)

---

### Task 1: Data model - `transcript_issue_proposals` table + db CRUD

**Files:**
- Create: `server/migrations/chat/<next>_transcript_issue_proposals.sql`
- Create: `server/src/db/transcript_proposals.rs`
- Modify: `server/src/db/mod.rs` (register the module)
- Test: unit tests in `server/src/db/transcript_proposals.rs` (sqlx against an in-memory/temp pool, mirroring existing `db::followups` tests)

**Interfaces:**
- Produces:
  - `pub struct IssueProposal { pub id: i64, pub transcript_id: i64, pub summary: String, pub assignee_name: Option<String>, pub source_lines: String, pub status: String, pub youtrack_issue_id: Option<String>, pub filed_by: Option<String>, pub filed_at: Option<i64>, pub created_at: i64 }`
  - `pub async fn replace_unfiled(pool, transcript_id: i64, proposals: &[NewProposal]) -> Result<(), sqlx::Error>` (deletes rows for this transcript whose status = 'proposed', inserts the new set; leaves 'filed'/'dismissed' rows)
  - `pub async fn list_for_transcript(pool, transcript_id: i64) -> Result<Vec<IssueProposal>, sqlx::Error>`
  - `pub async fn get(pool, id: i64) -> Result<Option<IssueProposal>, sqlx::Error>`
  - `pub async fn mark_filed(pool, id: i64, youtrack_issue_id: &str, filed_by: &str) -> Result<bool, sqlx::Error>` (updates only when status = 'proposed'; returns true if a row changed)
  - `pub struct NewProposal { pub summary: String, pub assignee_name: Option<String>, pub source_lines: String }`

- [ ] **Step 1: Write failing tests** - `replace_unfiled_then_list_round_trips`, `replace_unfiled_keeps_filed_rows`, `mark_filed_sets_fields_and_is_idempotent` (second call returns false, no duplicate). Use a temp sqlx pool with the migration applied, same harness as `db::followups` tests.
- [ ] **Step 2: Run tests, verify they fail** (module/functions not defined).
- [ ] **Step 3: Write the migration** (columns per spec; index on `transcript_id`) and implement the CRUD in `transcript_proposals.rs`; register the module in `db/mod.rs`.
- [ ] **Step 4: Run the tests, verify they pass.** `./dev/cargo test -p lets-chat-server --lib db::transcript_proposals`
- [ ] **Step 5: Commit.**

---

### Task 2: Config + feature flag + perms predicates

**Files:**
- Create: `server/src/youtrack/mod.rs` (config read only in this task)
- Modify: `server/src/routes/admin.rs` (settings form: three fields, token sealed), relevant admin settings template under `server/templates/admin/`
- Modify: `server/src/perms.rs` (predicates) and/or `server/src/routes/ai_gate.rs` (reuse staff set)
- Modify: `server/src/routes/feature_flags.rs` (register the flag for `/admin/feature-flags`)
- Test: unit tests for the narrowing helpers + predicate in the module(s) they live in

**Interfaces:**
- Produces:
  - `pub const YOUTRACK_ENABLED_KEY: &str = "youtrack_issues_enabled";`
  - `pub struct YoutrackConfig { pub base_url: String, pub token: String, pub default_project: String }`
  - `pub async fn load_config(state: &AppState) -> Option<YoutrackConfig>` (None when any of base_url/token/project is missing or token cannot be unsealed; warns + falls back per LC-919)
  - `pub async fn flag_on(state: &AppState) -> bool` (narrowing helper `resolve_flag_on(Result<...>) -> bool` is the unit-tested seam)
  - `pub fn can_file_youtrack(...)`: reuse the LC-679 staff predicate (site admin / enclave Owner|Admin / room moderator). A room member who can read the transcript can review; filing needs staff AND flag on.

- [ ] **Step 1: Write failing tests** - `resolve_flag_on_covers_true_false_absent_and_err`, `load_config_is_none_when_any_field_missing`. Keep them pure (pass a `Result`/field map), no live tracing subscriber.
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** the keys, `resolve_flag_on`/`flag_on`, `load_config` (seal/unseal token with `state.secret_key` exactly like `read_imap_config`), add the three admin settings fields (write-only token, empty preserves; template shows `youtrack_token_configured`), register the flag in `/admin/feature-flags`, and the `can_file_youtrack` predicate reusing the staff set.
- [ ] **Step 4: Run tests, verify pass.**
- [ ] **Step 5: Commit.**

---

### Task 3: YouTrack create-issue client

**Files:**
- Modify: `server/src/youtrack/mod.rs` (add `client.rs` submodule or inline)
- Test: unit tests using the `http_client::outbound_unchecked` test seam against a loopback stub (mirror an existing outbound test, e.g. stt / unfurl tests)

**Interfaces:**
- Produces:
  - `pub struct NewIssue<'a> { pub summary: &'a str, pub description: &'a str, pub assignee_login: Option<&'a str> }`
  - `pub async fn create_issue(cfg: &YoutrackConfig, issue: &NewIssue<'_>) -> Result<String, YoutrackError>` - resolves the project to an id if needed, `POST {base}/api/issues?fields=idReadable` with `Authorization: Bearer {token}`, returns the created `idReadable` (e.g. `"LC-1234"`). Assignee set is best-effort: a failure to set it does not fail the call.
  - `pub enum YoutrackError { Http(...), Status(u16), Decode(...) }` - Display never includes the token.

- [ ] **Step 1: Write failing tests** - `create_issue_posts_summary_and_returns_readable_key` (stub returns `{"idReadable":"LC-7"}`), `create_issue_surfaces_non_2xx_as_status`, `error_display_does_not_leak_token`. Inject the base URL as the loopback stub; use the `outbound_unchecked` seam in the TEST only (production path uses `outbound_trusted_post`).
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** `create_issue` over `outbound_trusted_post` (bearer header, JSON body, parse `idReadable`); map non-2xx to `Status`; keep the token out of every error.
- [ ] **Step 4: Run tests, verify pass.**
- [ ] **Step 5: Commit.**

---

### Task 4: LLM proposal generation (prompt + parser)

**Files:**
- Modify: `server/src/youtrack/mod.rs` or a new `server/src/routes/transcripts.rs` helper region
- Test: unit tests for the parser (no network/LLM)

**Interfaces:**
- Produces:
  - `const PROPOSE_SYSTEM: &str` - asks for a STRICT JSON array of `{summary, assignee, source_lines}`, grounded only in the transcript, assignee a roster name or null; transcript is DATA (guarded).
  - `pub fn parse_proposals(model_output: &str) -> Vec<NewProposal>` - tolerates a ```json code fence, drops malformed entries, bounds to `MAX_PROPOSALS = 50`, caps summary and source-line lengths (mirror `parse_action_items` bounds).

- [ ] **Step 1: Write failing tests** - `parse_proposals_reads_a_clean_json_array`, `parse_proposals_tolerates_a_code_fence`, `parse_proposals_drops_malformed_and_caps_count_and_length`, `parse_proposals_empty_on_garbage`.
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** `parse_proposals` (serde_json with a lenient wrapper strip) and the system prompt.
- [ ] **Step 4: Run tests, verify pass.**
- [ ] **Step 5: Commit.**

---

### Task 5: Generate route - `POST /transcripts/{id}/propose-issues`

**Files:**
- Modify: `server/src/routes/transcripts.rs` (handler), `server/src/routes/mod.rs` or the router wiring, `server/src/views/transcripts.rs` (fragment view struct)
- Create: `server/templates/transcripts/issue_proposals_fragment.html`
- Test: integration test in `server/tests/it/` using the `MockLlmClient`

**Interfaces:**
- Consumes: Task 1 `replace_unfiled`/`list_for_transcript`, Task 2 `flag_on`, Task 4 `parse_proposals`/`PROPOSE_SYSTEM`.
- Produces: `pub async fn propose_issues(State, AuthUser, Path<i64>) -> Result<Html, AppError>`; `ProposedIssuesFragment` view.

- [ ] **Step 1: Write failing test** - `propose_issues_stores_and_renders_proposals` (mock LLM returns two proposals; assert rows stored + fragment contains both summaries) and `propose_issues_403_when_flag_off`.
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** the handler: `require_access` + `ai_gate::require_llm_in_room` + flag gate; build prompt via `build_transcript_prompt_text`; `complete_guarded`; `parse_proposals`; `replace_unfiled`; render the fragment. Wire the route.
- [ ] **Step 4: Run tests, verify pass.** (`./dev/cargo test -p lets-chat-server --test it -- propose_issues`)
- [ ] **Step 5: Commit.**

---

### Task 6: File route - `POST /transcripts/{id}/file-issues`

**Files:**
- Modify: `server/src/routes/transcripts.rs`, router wiring, `server/src/views/transcripts.rs`
- Test: integration tests in `server/tests/it/` (outbound stub via the trusted path pointed at a loopback receiver, or a seam that lets the test inject the create-issue result)

**Interfaces:**
- Consumes: Task 1 `get`/`mark_filed`, Task 2 `load_config`/`flag_on`/`can_file_youtrack`, Task 3 `create_issue`.
- Produces: `pub async fn file_issues(State, AuthUser, Path<i64>, Form<FileIssuesForm>) -> Result<Html, AppError>`; `FileIssuesForm { accepted_ids: Vec<i64>, assignee overrides... }`.

- [ ] **Step 1: Write failing tests** - `file_issues_403_for_non_staff` (flag on, plain member), `file_issues_creates_marks_and_audits` (staff; stub create returns a key; assert row `filed` + `log_mod_action` row), `file_issues_skips_already_filed` (idempotent, no second create), `file_issues_row_error_leaves_row_proposed` (stub returns 500 -> row stays proposed, surfaced).
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** the handler: `require_access` + flag + `can_file_youtrack` (403 otherwise); for each accepted still-`proposed` row build the description (source lines + proposed assignee + link to `/transcripts/{id}`), call `create_issue`, `mark_filed`, `log_mod_action("youtrack_issue_filed", ...)`; per-row error handling; render the updated fragment.
- [ ] **Step 4: Run tests, verify pass.**
- [ ] **Step 5: Commit.**

---

### Task 7: UI panel + i18n + CHANGELOG

**Files:**
- Modify: `server/templates/transcripts/show.html`, `server/templates/transcripts/summary_fragment.html`, the fragment templates from Tasks 5-6
- Modify: `server/src/views/transcripts.rs` (page booleans `can_review_youtrack`, `can_file_youtrack`), `server/src/routes/transcripts.rs` (compute them on the page handler)
- Modify: `server/locales/en/*.ftl`, `server/locales/es/*.ftl`
- Modify: `CHANGELOG.md`
- Test: an integration assertion that the panel is hidden when the flag is off and shown (with the file button only for staff) when on

**Interfaces:**
- Consumes: Task 2 predicates, Tasks 5-6 fragments.

- [ ] **Step 1: Write failing test** - `transcript_page_hides_youtrack_panel_when_flag_off` and `_shows_panel_without_file_button_for_plain_member`.
- [ ] **Step 2: Run, verify fail.**
- [ ] **Step 3: Implement** the panel (button to propose, proposal rows with accept/assignee/source lines, file button gated on `can_file_youtrack`), compute the page booleans, add en+es strings, add the CHANGELOG `Added` entry. Follow a11y conventions (required inputs get `aria-required`), reuse existing token utilities.
- [ ] **Step 4: Run tests + the UI/i18n checks.** `./dev/cargo test -p lets-chat-server --test it -- youtrack_panel`; `nu ci-build/check-ui-conventions.nu`; `nu ci-build/check-locale-ellipsis.nu`; i18n catalog parity.
- [ ] **Step 5: Commit.**

---

## Self-review notes

- Spec coverage: config (T2), flag+perms (T2), data model (T1), YouTrack client (T3), generation (T4+T5), review+file (T6), UI (T7). All spec sections map to a task.
- Review Focus lines each have an owning task's test (T4 parser, T6 non-staff/non-2xx/double-file, T2/T5/T6 flag-off).
- Interfaces are named once in the producing task and consumed by key; no duplicated bodies.
- Build note: the containerized `./dev/cargo` is slow/wedges under host load this session; rely on CI as the final gate and push `--no-verify` when the local pre-push hook wedges.
