# Design: Turn call-transcript action items into YouTrack issues (LC-1135)

Date: 2026-11-08
Ticket: LC-1135

## Intent and why

Vas's idea from the 2026-11-02 dev call: after a call, Let's Chat proposes
YouTrack issues for the tasks and promises in its transcript so they are not
lost between meetings. Today a reviewer copies action items out of the saved
transcript by hand. We already extract "## Action items" from the transcript's
LLM summary (LC-527) and turn them into an in-app checklist; this feature takes
the next step and files the accepted ones into the org's YouTrack, with a human
in the loop.

## Goals

- After a call, a reviewer sees a list of **proposed** issues drawn from the
  transcript: a one-line summary, a proposed assignee (plain-text name), and the
  transcript source lines each proposal came from.
- The reviewer accepts the ones they want and files them as real YouTrack issues
  in **one step**. Nothing is filed without that accept.
- Issues are filed into a single org-wide YouTrack that an admin configures once
  in settings (base URL, API token, default project).

## Non-goals

- No per-user or per-room tracker. One org-wide YouTrack only.
- No automatic filing. Generation produces proposals; a human files.
- No automatic assignee resolution from a guessed name. The assignee is plain
  text; the reviewer may optionally map it to a real YouTrack user.
- No sync-back: once filed, the YouTrack issue is independent. We store the
  created issue's id/readable key for display and dedup, nothing more.
- No support for non-YouTrack trackers.

## Anti-goals (what would make this a failure even if it "works")

- A stray call participant spraying the org tracker with junk issues.
- Filing an issue twice from the same proposal.
- The YouTrack token being readable from the UI or logged.
- The admin-configured base URL being abusable as an SSRF pivot.

## Decisions made with the requester

1. **Connection: org-wide, admin-configured in settings.** One YouTrack. An
   admin sets the base URL, an API token, and a default project in the admin
   settings page. The token is a secret (write-only field, stored encrypted,
   never rendered back), mirroring the existing `imap_password` pattern
   (`routes/admin.rs`, sealed with `state.secret_key`).
2. **Assignee: plain text + optional manual mapping.** The LLM proposes an
   assignee as the spoken name. The reviewer may optionally type/confirm a
   YouTrack login to assign to; if they don't, the issue is filed unassigned and
   the proposed name is preserved in the issue description. We never auto-assign
   from a guessed name.
3. **Permissions: room members see and propose; a feature flag + mod/admin to
   file.** Any member who can view the transcript can generate and review
   proposals (behind the feature flag). The "File to YouTrack" action
   additionally requires the staff role set (site admin, enclave Owner/Admin, or
   room Moderator), reusing the LC-679 staff predicate. A disallowed direct
   request gets a 403, not just a hidden button.

## How it is built (decided)

### Configuration (admin settings)

Three settings, surfaced in the existing admin settings page next to the other
operator-configured integrations:

- `youtrack_base_url` - plain settings KV. e.g. `https://acme.youtrack.cloud`.
- `youtrack_token` - secret. Stored sealed with `state.secret_key` exactly like
  `imap_password` (empty submit preserves the existing value; the template sees
  only `youtrack_token_configured: bool`, never the plaintext).
- `youtrack_default_project` - plain settings KV. The project short name
  (e.g. `LC`) or id; resolved to a project id at file time.

### Feature flag (runtime kill switch)

A settings KV flag `youtrack_issues_enabled`, default OFF (absent reads as off),
read live like `llm_enabled` (`routes/ai_gate.rs` LC-679 pattern). Off means:
no panel is rendered, generation and filing routes return 403/400. This is the
"flag" half of the gate. Exposed as a toggle in `/admin/feature-flags`.

A pure predicate (in `perms`) decides the two capabilities:
- `can_review_youtrack` = flag on AND viewer can read the transcript.
- `can_file_youtrack` = flag on AND viewer is in the LC-679 staff set for the
  room (site admin, enclave Owner/Admin, or room Moderator).

### Data model

New table `transcript_issue_proposals` (chat db, new migration):

| column | notes |
| --- | --- |
| `id` | PK |
| `transcript_id` | FK to the transcript/call session |
| `summary` | proposed issue title (bounded length) |
| `assignee_name` | plain-text proposed assignee, nullable |
| `source_lines` | the transcript lines this came from (text) |
| `status` | `proposed` \| `filed` \| `dismissed` |
| `youtrack_issue_id` | set when filed (the created issue's readable key, e.g. `LC-1234`) |
| `filed_by` | user id, set when filed |
| `filed_at` | timestamp, set when filed |
| `created_at` | timestamp |

Bounds mirror `parse_action_items`: at most 50 proposals per transcript, summary
and source-line lengths capped, so a runaway model response can't create an
unbounded list.

### Generation

New route `POST /transcripts/{id}/propose-issues`:
- Gated: `require_access` (room member) + `ai_gate::require_llm_in_room` (LLM
  flag/role) + the `youtrack_issues_enabled` flag.
- Builds the transcript prompt with `build_transcript_prompt_text` (same roster +
  line format the summary/brief already use).
- Calls `llm.complete_guarded(SYSTEM, user_content)` with a system prompt that
  asks for a STRICT JSON array of `{summary, assignee, source_lines}` objects,
  grounded only in the transcript, assignee being a name that appears in the
  roster or null. The transcript (including names) is DATA, guarded by the
  existing guardrail wrapper so a line in the transcript can't be treated as an
  instruction.
- Parses the JSON defensively (tolerate a code-fence wrapper; drop malformed
  entries; apply the count/length bounds). Replaces the transcript's
  **un-filed** proposals with the new set; filed proposals are left untouched.
- Returns the rendered "Proposed issues" fragment.

### Review + file UI

On the transcript page (`templates/transcripts/show.html` +
`summary_fragment.html`), behind `can_review_youtrack`, a "Proposed issues"
panel modeled on the existing brief / follow-ups controls:

- A "Propose issues" button (generates/regenerates).
- Each proposal row: a checkbox to accept, an editable summary, an editable
  assignee field (prefilled with the plain-text name; optional YouTrack login
  mapping), and the source lines shown read-only.
- A single "File accepted" button, shown only when `can_file_youtrack`. Filed
  rows render as a link to the created YouTrack issue and can't be re-filed.

### Filing

New route `POST /transcripts/{id}/file-issues`:
- Gated: `require_access` + the flag + the `can_file_youtrack` staff check
  (403 otherwise).
- Reads the accepted proposal ids + any per-row assignee override from the form.
- For each accepted, still-`proposed` row, creates a YouTrack issue:
  `POST {base}/api/issues?fields=idReadable` via
  `http_client::outbound_trusted_post` (the blessed operator-configured trusted
  path; the base URL is admin config, same trust posture as SMTP/IMAP), with
  `Authorization: Bearer {token}`, body
  `{"project": {...resolved from youtrack_default_project...}, "summary": ..., "description": ...}`.
  The description carries the source lines, the proposed assignee name, and a
  link back to `/transcripts/{id}`. If the reviewer mapped a YouTrack login, set
  the Assignee custom field (best-effort; a failed assignee set does not fail the
  issue creation, it just leaves it unassigned and notes the name).
- On success, mark the row `filed`, store `youtrack_issue_id`, `filed_by`,
  `filed_at`; write a `db::moderation::log_mod_action` audit entry
  (`youtrack_issue_filed`, naming the transcript and the created key).
- Partial failure is per-row: filed rows are marked filed; a row whose API call
  failed stays `proposed` and the fragment shows the error for that row. The
  action is idempotent - a row already `filed` is skipped.

### Errors / edge cases

- Flag off, or base URL / token / project not configured: the panel is hidden;
  a direct generate/file request returns a clear 400/403.
- YouTrack unreachable or returns non-2xx: surfaced per-row; nothing is marked
  filed; the token is never logged (log status + a redacted message only).
- Empty transcript or no action items: generation returns a friendly "nothing to
  propose" state, same posture as `create_followups`.
- Token at rest: encrypted with `state.secret_key`; if the secret key is absent
  or rotated, the feature reports "not configured" rather than erroring, same as
  the IMAP creds fallback.

## Constraints

- Outbound HTTP only through `http_client` (LC-152 grep-ban); use
  `outbound_trusted_post` for the admin-configured endpoint.
- Migrations are append-only and immutable (LC-212): a new migration, never an
  edit.
- Secrets never rendered or logged (the admin secret-field pattern).
- Settings reads follow the LC-919 pattern: match on the `get_setting` Result,
  warn + safe fallback on `Err`, pure narrowing split into a unit-testable
  helper (avoids the LC-782 tracing-subscriber flake).
- CHANGELOG: user-facing, add an `Added` entry on implementation.

## What is left to the builder

- Exact YouTrack REST request/response shapes for issue create and for the
  optional assignee custom-field set (verify against the configured instance's
  API version; keep assignee best-effort).
- How the default project string is resolved to a project id (a one-shot
  `/api/admin/projects` lookup, cached, vs. accepting a raw id). Builder's call,
  kept simple.
- Template/markup details and i18n keys (en + es), following existing transcript
  fragments.
- Whether generation is also offered live right after a call ends, or only on the
  saved transcript page. Default: the saved transcript page (same surface as the
  summary/brief), which satisfies "after a call ends."
- Test shape: a `MockLlmClient`-style stub for proposal JSON, an outbound stub
  (`outbound_unchecked` test seam) for the YouTrack create, route tests for the
  gates (flag off, non-staff file attempt -> 403), and the JSON parser's bounds.
