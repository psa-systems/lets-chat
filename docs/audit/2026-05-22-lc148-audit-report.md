# LC-148 Codebase Audit Report

Date: 2026-05-22. Scope: whole-codebase review for correctness bugs, security vulnerabilities, and deep-rooted systemic gaps (the recurring "every new surface re-derives a concept instead of inheriting it" problem). Method: four parallel read-only analysis passes (security/authZ, correctness, live-update inventory, reused-abstraction inventory).

This is a triage + design pass. Findings are split into (A) security, (B) correctness, (C) systemic abstraction gaps. Each actionable item has a follow-up issue; large items become their own epics. Fixing everything here is out of scope.

## Severity legend

crit (exploitable, high impact) > high (exploitable, scoped impact) > med (needs preconditions or limited impact) > low (defense-in-depth / hardening).

## A. Security findings

| # | Sev | Location | Problem | Follow-up |
|---|-----|----------|---------|-----------|
| S1 | high | `routes/reactions.rs:19` `toggle_reaction` | No room-access check before fetching the message by id, toggling a reaction, and broadcasting `ReactionAdded`/`Removed` into the room. Any authed user can react in private rooms / DMs / enclaves they cannot see, write a row, inject a live WS fragment, fire `reaction.added` webhook, and probe message existence. Every sibling handler gates on `is_room_accessible`. | LC-149 |
| S2 | med | `routes/reactions.rs:74` `get_picker` | No auth extractor and no access check. Anonymous caller passes a `message_id` and gets the room's custom-emoji inventory; unauth message-existence oracle. | LC-149 |
| S3 | med | `routes/reactions.rs:22` `{emoji}` path param | Raw, unbounded `emoji` string inserted into `message_reactions` with no length/charset validation (the shortcode path is validated; this POST is not). | LC-149 |
| S4 | high | `routes/unfurl.rs:91` redirect follow | Pre-flight resolves host and rejects non-global IPs, then follows up to 3 redirects with `Policy::limited` and NO per-hop re-validation. Attacker page `302`s to `169.254.169.254` / `127.0.0.1`; the pre-flight is defeated. AuthUser-gated (any logged-in user). | LC-150 |
| S5 | high | `routes/auth.rs:78` `post_login` | No rate limit on login or the `/login/2fa` + `/login/recovery` challenge endpoints (register IS throttled). Online password + 2FA brute force. Note per-IP limiting only works behind a trusted proxy (`trust_proxy_headers`). | LC-151 |
| S6 | med | `outgoing.rs:111` delivery | Outgoing-webhook URL is IP-validated only at creation, as a string parse (rejects IP literals + `localhost`, but lets any hostname pass). At delivery time the host is fetched with no `is_globally_routable` re-check, so a hostname resolving to an internal/metadata IP is fetched on every event with signed payloads. | LC-152 |
| S7 | low | `slash.rs:288` `is_public_ip` / `run_webhook` | IPv6 branch misses ULA `fc00::/7` (unfurl rejects it); host check only covers IP literals; client sets no redirect policy (default 10 hops, no re-validation). Admin-configured, so low. | LC-152 |
| S8 | low | `views/markdown.rs:109` `Tag::Link` | Explicit markdown links `[label](dest)` pass the destination straight to `push_html`; `javascript:`/`data:` schemes are not filtered -> click-to-execute XSS in any rendered message. (Bare-URL linkify is safe; this is the explicit-link path.) | LC-154 |
| S9 | med | `routes/room.rs:406` `post_message` | Only `is_empty()` checked; no max body length. Bounded only by Axum's 2 MiB default body cap, so ~2 MiB messages store + broadcast to all subscribers (amplification). | LC-153 |
| S10 | low | `db/auth.rs:414` token gen | Session, API-token, and webhook-secret generation use `rand::thread_rng()`. Currently CSPRNG-backed (incidental), but a security token should use `OsRng` explicitly. | LC-155 |
| S11 | low | `auth.rs:49` `extract_session_origin` | First `X-Forwarded-For` hop trusted unconditionally for stored session IP / login-alert, regardless of `trust_proxy_headers`. Spoofable audit-trail IP (cosmetic). | LC-155 |
| S12 | low | `partials/link_preview.html` | `og:image` URL lands in `<img src>` Askama-escaped but not scheme-validated. No script execution; harden by validating scheme. | LC-155 |

Verified clean (not bugs): SQL is fully parameterized (dynamic queries only interpolate a generated `?,?,?` placeholder string); no `| safe` filters in templates; markdown drops raw HTML (`Event::Html` stripped, tests cover `<script>`/`onerror`); cookies are `HttpOnly`+`Secure`+`SameSite=Strict` with 30-day expiry; sessions enforce expiry + ban; API tokens stored as HMAC only; incoming-webhook secret merged after TraceLayer (not logged); role checks re-enforced server-side in `room_rbac`, `webhooks`, `delete_message`, `AdminUser`. Most id-based handlers (`pinned`, `bookmarks`, `polls`, `uploads`, room/thread/edit/delete) correctly re-check `is_room_accessible`; the reactions handlers are the outlier.

## B. Correctness findings

Most high-traffic paths are sound. Audited and correct: scheduled dispatcher (atomic `BEGIN IMMEDIATE` claim-then-process, broadcast after commit), reminders (claim-then-fire, re-validates access at fire time), retention sweep (bounded, post-commit broadcast), `finalize_message_send` (commit before fan-out), rate limiter (atomic check+increment), `relay_call_signal` (member + payload-cap + block fail-closed), digest byte-slice math (ASCII-safe). WS mutex `unwrap()`s are on non-panicking critical sections (poisoning effectively impossible). `Response::builder().body(empty).unwrap()` sites are infallible static builders.

Real correctness items folded into the security follow-ups: S3 (unvalidated emoji), S9 (no message cap). One cosmetic note (no ticket): `db/chat.rs` DM-read upsert returns the freshly-computed `updated_at` even on a non-advancing call; the live path only calls it on advancing reads, so no user-visible effect today.

## C. Systemic abstraction gaps (the main ask)

The recurring pattern: a new surface looks complete but silently lacks a behavior every comparable surface should have, because the behavior is copy-pasted (and forgotten) rather than inherited. Two headline gaps get their own design docs + epics; the rest get a consolidation chore.

### C1. Live updates are opt-in, not default (epic: LC-156)

Only **3** pages subscribe to WebSocket updates: `room/page.html`, `dm/page.html`, `voice/page.html`. Each hand-rolls an **identical** `htmx:wsOpen` subscribe + `htmx:beforeCleanupElement` teardown IIFE (3 copies, no shared helper). Every other page is static-on-load and needs a manual reload to reflect changes:

- `/activity` (mentions/reminders), `/inbox` (unread), `/enclave/{id}` (member + room lists), `/invitations`, `/saved`, `/settings` (own profile/status), and all `/admin/*` pages.

Two distinct sub-problems:
1. **Broadcast exists but no OOB consumer**: `EnclaveMemberAdded/Removed`, `EnclaveRoomAdded/Removed`, `EnclaveInvitationCreated/Resolved` fire `broadcast_to_user` but only the sidebar consumes anything; the `/enclave/{id}` and `/invitations` pages show stale lists.
2. **No broadcast at all**: admin user/room lists (role/ban/mute/counts), room info (description/wiki/moderators), search results, saved-message deletions, settings profile edits.

Design: `docs/superpowers/specs/2026-05-22-live-updates-by-default-design.md`. Goal: a shared "this view subscribes to topics T and merges OOB fragments" wrapper so a new page is live by construction, plus filling the missing broadcasts.

### C2. Inconsistent search / typeahead (epic: LC-157)

**6** distinct search implementations. Four generic searches (sidebar messages, sidebar people, enclave invite, group add-members) share a near-identical `input changed delay:200ms, keyup[Enter]` -> HTML-fragment trigger but each re-specifies it with its own endpoint/result-container/empty-state string and **none support keyboard navigation**. The mention autocomplete (composer) is a full `aria-combobox` with arrow/enter/escape nav; the slash-command autocomplete has no keyboard nav. So three tiers of quality for the same concept.

Design: `docs/superpowers/specs/2026-05-22-unified-search-component-design.md`. Goal: one reusable typeahead (debounce, a11y/keyboard nav, result-fragment shape, empty state) + a migration path for the 4 generic searches and ideally the two comboboxes.

### C3. Smaller consolidations (chore: LC-158)

- **Modal focus-trap**: a shared `window.__lcDialogTrap` exists and is used by 4 modals (poll, scheduled, 2 call dialogs), but the status picker and reminder picker hand-roll their own escape/backdrop/focus-restore. Migrate the two stragglers.
- **Confirm dialogs**: 3 patterns (`hx-confirm` attr, native `confirm()`, custom text-input verification). Pick one default (`hx-confirm` for simple, text-verify for destructive-irreversible) and document.
- **Form-error rendering**: 2 patterns (JS-toggled `hidden` slot in modals vs server-side template interpolation in auth pages). Unify on one fragment shape.
- **Avatar/status**: mostly unified via `partials/avatar.html`; only the mention popover re-implements the badge inline. Fold it back.

## Prioritized backlog

1. **LC-149** (high) reactions authZ: access check on toggle + picker + emoji validation. Small, self-contained, exploitable now.
2. **LC-150** (high) unfurl redirect SSRF: `Policy::none()` + per-hop `is_globally_routable`.
3. **LC-151** (high) login + 2FA rate limiting.
4. **LC-152** (med) webhook delivery-time SSRF guard (outgoing + slash; add `fc00::/7`, redirect policy).
5. **LC-153** (med) message body max-length cap.
6. **LC-154** (low) markdown link-scheme allowlist (javascript: XSS).
7. **LC-155** (low) token CSPRNG + XFF-trust + og:image scheme hardening bundle.
8. **LC-156** (epic) live updates by default.
9. **LC-157** (epic) unified search/typeahead component.
10. **LC-158** (chore) consolidate modal/confirm/form-error/avatar patterns.

Security items (1-7) should land before or alongside the abstraction epics. LC-149/150/151 are the fix-first set.
