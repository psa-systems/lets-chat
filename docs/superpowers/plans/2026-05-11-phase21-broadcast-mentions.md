# Plan Phase 21: `@here` and `@channel` (broadcast mentions)

Ship the deferred-from-phase-14 broadcast-mention work. Two tokens, room-scoped: `@here` (online room members, excluding DND) and `@channel` (every room member). Reuses the phase 14 `mentions` table, the phase 16 Push fan-out, the phase 17 mute paths, and the phase 18 presence signal. Adds a small live "this will notify N people" count in the composer to surface the cost at type-time.

`@everyone` is explicitly out of scope - it crosses rooms and changes the notification's `room_id` invariant, which would ripple through every Push/mute/sidebar code path. Separate future phase if demand surfaces.

## Background and what's already in place

Reading the code surfaces three things that shape the plan:

1. **The parser is already broadcast-token-friendly.** `db::mentions::parse_mention_tokens` is `(?:^|\s)@([A-Za-z0-9_-]{1,32})` which already matches `@here` and `@channel`. Today those tokens are returned by the parser and then silently dropped at the resolver step in `routes/room.rs:335` because `find_user_by_username("here")` returns `None`. No parser change is needed; only the resolver step branches.

2. **The current fan-out is sequential and `.await`s every per-user Push dispatch.** `routes/room.rs:353-367` loops `for t in &added` and does `crate::push::dispatch(&state, &t.user_id, &event).await` per iteration. With `@channel` in a 200-person room that's 200 sequential roundtrips before the last person hears about it (latency, not throughput). The phase 16 sketch (Semaphore + bounded concurrency) lands here.

3. **The `mentions` table indexes are already correctly shaped for broadcast traffic.** Migration `0014_mentions.sql` has `idx_mentions_unread (mentioned_user_id, read_at)`, `idx_mentions_room_user (room_id, mentioned_user_id)`, and `idx_mentions_message (message_id)`. All three queries the broadcast path will hit (`count_unread_mentions_per_room`, `mark_mentions_read_for_room`, `reconcile_mentions`'s message-id scan) use these prefixes. No new migration needed unless the scale-sanity step below surfaces a real regression.

## Architecture decisions (already settled, recorded here for the implementation step)

### Tokens

`@here` and `@channel` only. No `@everyone`. No admin gating in v1 (anyone in the room can use either token).

### Resolution

- **`@here`**: WS-connected AND `status != 'dnd'`. Idle is included (idle = "stepped away briefly," not "do not interrupt"). Author excluded.
- **`@channel`**: every `room_members` row for the room, regardless of status. Author excluded.
- **DM gate**: broadcast tokens are skipped entirely for `room_type == "dm"`. Parser still matches them; resolver does not insert mention rows and no events fire. DMs already have implicit-mention semantics for the peer.
- **Mute respect**: mute is mute. `MuteMode::All` recipients get neither WS nor Push. `MuteMode::ExceptMentions` recipients get both, since a broadcast mention is still a mention.

### Schema

No new migration. One row per resolved user in the existing `mentions` table. A 200-person `@channel` writes 200 rows. Each user reads/unreads their own row independently. Storage cost is negligible.

No `kind` column. Broadcast mentions are indistinguishable from `@username` mentions in v1 (same `ChatEvent::Mentioned`, same Push payload, same chip styling, same badge). Add a discriminator the day the notification surface actually differentiates.

### Parser

Stays dumb. Returns `Vec<String>` of raw tokens. The resolver branches on token name in `post_message`:

```text
for token in tokens:
  if token == "here"    -> resolve_here(state, room_id, author_id)
  elif token == "channel" -> resolve_channel(state, room_id, author_id)
  else                    -> find_user_by_username(token) (existing path)
```

This keeps the parser free of domain knowledge and leaves room for future broadcast tokens (`@admins`, `@mods`) without rework.

### Fan-out concurrency

`tokio::sync::Semaphore` cap = 16, plus `tokio::task::JoinSet` to run per-user dispatches concurrently. Replaces the existing sequential `for t in &added { ...await... }` loop. Each task acquires a permit, fires both the WS `broadcast_to_user` (cheap, kept inside for ordering simplicity) and the awaited `crate::push::dispatch(...)`, releases the permit, returns. The push-internal spawn-per-subscription stays as-is; with the outer cap of 16 and typical 1-3 subscriptions per user, peak in-flight HTTP is 16-48, well within FCM/Mozilla autopush limits.

WS fan-out (`Hub::broadcast_to_user`) does not need bounding: local `tokio::sync::broadcast` send + mpsc write per recipient, microseconds at 200-recipient scale. Leave alone.

### Autocomplete

Extend `MentionSuggestion` with a `kind: "user" | "broadcast"` discriminator. Broadcast tokens (`@here`, `@channel`) are **always sorted to the top of the dropdown**, including when the user's prefix matches a real username (`@h` puts `@here` above `@harry`). Documented explicitly: this is a UX choice, not an implementation accident. Broadcast tokens are higher-stakes than user mentions and deserve visibility.

Rendering: broadcast suggestions use a megaphone (or `@`) icon next to the token, plus a short subtitle ("Notify online members" / "Notify the entire room"). Existing user rows are unchanged.

Permission-aware filtering: trivial no-op for v1 (everyone can use both tokens). Seam exists in the response shape; lights up later if gating ships.

### Composer count

New endpoint `GET /api/rooms/:room_id/broadcast-count?token=here|channel` returns JSON `{ count, label }`. Composer textarea listens for `@here` / `@channel` token presence on `input`, debounces ~300ms, fires `htmx.ajax` to load a small label fragment into a `#lc-broadcast-count` slot under the textarea. Empty body or no broadcast token = empty slot. The label reads "This will notify 47 people in #general." Clears on submit.

No confirmation step. Live count is the friction-as-feature.

### Rendering

Chip with `.lc-mention-chip` styling, text `@here` or `@channel`, no link, no tooltip.

**The chip comes from the body text, not the mention rows.** The existing body renderer (`MessageView::body_html()` and whatever helper inside it turns `@harry` into a chip) currently looks up each `@token` and renders it as a chip iff the token resolved to a real user. For broadcast tokens, the renderer needs to learn that `@here` and `@channel` are **no-lookup-required chips**: any occurrence of those two literals in a message body renders as a chip without consulting the mentions table or the user directory. The author's body text retains `@here` / `@channel` literally; the chip rendering is a render-time treatment.

This is a real change to the body renderer (not just "the existing path Just Works"). The change is small (two extra branches in the chip-rendering helper) but it must land for the chip to appear at all. Documented as part of Task 1 below.

The recipient does not see "you were @channel-ed" specifically - they see a chip with the broadcast token text, which is honest about what happened.

## Tasks

0. **Scale sanity check, run before Task 1.** Build the `#[ignore]`-gated test described in Task 8 first and run it as a read-only diagnostic against the current code (no broadcast changes yet, just the existing `count_unread_mentions_per_room` and `mark_mentions_read_for_room` queries seeded with 10K rows). If both queries clear the 50ms budget, Task 8 below collapses to "rerun the same test under the new code paths and confirm no regression." If either query misses the budget, a new migration `0020_mentions_unread_partial_index.sql` lands before Task 1, adding `WHERE read_at IS NULL` to `idx_mentions_unread`. The plan does not branch on the result; the followup migration is cheap and non-breaking either way - the question is whether it ships in this phase or never.

1. **Add `db::mentions::resolve_here` and `resolve_channel`, plus broadcast-aware body rendering.** Three sub-pieces:
   - `resolve_here(state: &AppState, room_id: i64, author_id: &str) -> Result<Vec<MentionRef>, AppError>`: list `room_members` for `room_id`; filter to those where `state.hub.is_user_connected(user_id)` AND `users.status != STATUS_DND` (loaded from auth pool in one bulk query); exclude `author_id`. Uses the existing `db::auth::display_names_for_ids` helper from phase 19 for the bulk auth-pool lookup. Sets `MentionRef::username` to each resolved user's username so the existing render path works.
   - `resolve_channel(state: &AppState, room_id: i64, author_id: &str) -> Result<Vec<MentionRef>, AppError>`: list `room_members` for `room_id`; exclude `author_id`; bulk-load usernames via the same helper. No status filter.
   - Both helpers return the resolved user list. Mention rows carry the resolved users' real `username` values - no special-cased "here"/"channel" rows. Read-tracking and notification routing stay clean.
   - **Body renderer change.** Find the helper that turns `@harry` text into a chip (likely inside `MessageView::body_html` or `views::room`-adjacent). Add two no-lookup-required cases: literal `@here` and literal `@channel` in the body become chips unconditionally (no DB lookup, no user resolution, no permission check at render time - if the token survived parser + resolver, it earned a chip). Implementation shape: before the existing username-resolution branch, match the token against the two literals and emit a chip span with the token text.
   - Confirm the rendering is rendered identically in three call sites: the room-page message list, the WS `NewMessage` fragment, and the thread-panel reply list. Likely all three go through the same `body_html()` helper, but verify - missing one means the chip silently disappears in one of those surfaces.

2. **Update `post_message` (`routes/room.rs:328-367`) to branch the resolver.** Replace the single `find_user_by_username` lookup with:
   - Iterate tokens; collect three buckets: `here_seen: bool`, `channel_seen: bool`, `user_tokens: Vec<&str>`.
   - If `here_seen` AND room is not DM, append `resolve_here(...)` to the `targets` vec.
   - If `channel_seen` AND room is not DM, append `resolve_channel(...)` to the `targets` vec.
   - For each `user_token`, do the existing per-username lookup with self-exclusion + candidate-set check.
   - Dedupe `targets` by `user_id` before passing to `reconcile_mentions` (a user matched by both `@here` and `@username` should only write one row).

3. **Update `patch_message` (the edit path, `routes/room.rs:525+`) to use the same branching.** The existing edit code calls `parse_mention_tokens` and `find_user_by_username` per token. Apply the same three-bucket pattern from Task 2 so editing `Hey @harry` → `Hey @channel` reconciles correctly via `reconcile_mentions` (existing function works as-is once the resolver feeds it the correct user list).

4. **Bound per-user fan-out concurrency.** Wrap the post-write `for t in &added` loop (and its sibling in `patch_message`) with a `tokio::sync::Semaphore::new(16)` + `tokio::task::JoinSet`. Each task acquires a permit, fires `state.hub.broadcast_to_user(...)` and awaits `crate::push::dispatch(...)`, releases the permit. After spawning all tasks, `joinset.join_all().await`. Inline; no new module.

5. **Extend the autocomplete endpoint.** Modify `views::mentions::MentionSuggestion` to add `kind: &'static str` ("user" or "broadcast") and optional `subtitle: &'static str`. In `routes/mentions.rs::get_autocomplete`:
   - Build broadcast suggestions first (`@here`, `@channel`) when the room is not a DM. Filter to those matching `q_lower` if `q` is non-empty (`"h"` matches `"here"`, `"c"` matches `"channel"`, both match empty).
   - Then build user suggestions as today.
   - Concatenate with broadcasts first. Truncate the combined list to `MAX = 8`.
   - The template `partials/mention_popover.html` (or wherever `MentionPopoverFragment` renders) gets a branch on `s.kind`: broadcast entries show a megaphone SVG icon + bold token + subtitle line; user entries unchanged.

6. **Add the broadcast-count endpoint.** New handler `routes::mentions::get_broadcast_count`:
   - Route: `GET /api/rooms/:room_id/broadcast-count?token=here|channel`.
   - Auth: same `is_room_accessible` gate as the autocomplete endpoint.
   - Token validation: reject anything other than `"here"` or `"channel"` with 400.
   - DM gate: 400 for `room_type == "dm"` (broadcast tokens don't resolve there).
   - Call the matching `resolve_*` helper from Task 1; return a tiny HTML fragment for the composer slot: `<span class="text-xs text-slate-500">This will notify {n} people in #{room.name}.</span>` (or `@{peer}` if we ever extend to DMs; for v1, room only). Empty resolution = empty span.
   - This is an HTML fragment, not JSON, so it slots directly into the composer via `htmx.ajax(..., {target:'#lc-broadcast-count', swap:'innerHTML'})`. No JSON parsing in JS.
   - Register in `routes/mod.rs` alongside the existing `/users/mentions` route.

7. **Wire the composer.** Modify `templates/room/composer.html`:
   - Add `<div id="lc-broadcast-count" class="text-xs text-slate-500 px-1 min-h-[1rem]"></div>` below the textarea, above the staged-file slot.
   - Extend the existing composer mention IIFE (the one that runs `activeToken()` against the textarea) so that on every `input` it also checks the **active token at the cursor**. The probe fires iff the active token (per the existing `activeToken()` helper) is exactly `"here"` or `"channel"`. Debounce ~300ms via the same pattern the autocomplete already uses for the popover.
   - **Explicit choice for the mixed-token case (`@here ... @harry`):** the count slot reflects only the **currently-active token**, not the union of all broadcast tokens in the body. Concretely:
     - Typing `@here` with the cursor at the end of `here`: count slot shows "47 people".
     - Typing a space after `@here` (now `@here `): active token is gone; **count slot clears**.
     - Continuing to type `@here team @harry`: active token becomes `harry` (user autocomplete fires); count slot stays cleared.
     - Going back and re-positioning the cursor inside `@here` (still no whitespace between cursor and `@`): active token is `here` again; count slot re-populates.
     This matches the autocomplete probe's existing "active token at cursor" semantics one-to-one and avoids a separate scan over the entire body. It does mean a fully-typed `@here ... @harry` message shows nothing in the count slot at submit time - the user saw the count earlier while typing `@here`, and the friction-as-feature already happened. Documented here so it doesn't drift to "show union of all broadcast tokens" during implementation.
   - On successful `hx-on::after-request` of the form submission, clear `#lc-broadcast-count`'s innerHTML (the existing `after-request` handler already clears the textarea and staged file; add one line for the count slot).
   - The teardown for the composer IIFE added in phase 20 must keep working - the new probe is part of the same IIFE, so its teardown is already covered by the existing `htmx:beforeCleanupElement` handler.

8. **Scale sanity rerun.** The harness was built and run before Task 1 (see Task 0). This task is: rerun the same `#[ignore]`-gated test against the new code paths, confirming that the broadcast write path doesn't introduce a regression (e.g. a missing index hit, a per-row query that should have been a bulk write). Same budget: both queries under 50ms on 10K rows.

9. **Tests.** Integration tests in new file `server/tests/routes_broadcast_mentions.rs`:
   - `at_here_resolves_to_online_room_members_excluding_dnd_and_author`: seed 4 users in a room, 1 author + 3 others; mock 2 of the others connected via hub, 1 of them set to DND; post `Hey @here`; assert one `mentions` row for the non-DND connected user only.
   - `at_channel_resolves_to_all_room_members_excluding_author`: seed 5 users, post `Notice @channel`; assert 4 mention rows (everyone except author).
   - `at_channel_in_dm_does_not_write_rows_or_fire_events`: post in a DM with `@channel`; assert zero new mention rows and no `MessagePinned`-style WS events for the peer beyond the existing implicit DM mention.
   - `at_here_with_no_connected_users_is_noop`: post `@here` in a room where the author is the only connected user; zero rows, no events.
   - `at_here_dedup_with_explicit_username`: post `@here @harry` where harry is online and matches both tokens; assert exactly one row for harry.
   - `edit_reconciliation_channel_to_here`: post `@channel`, then edit to `@here`; assert mention rows correctly diff (rows for previously-resolved channel users who are NOT in the new `@here` resolution get deleted; rows for users in both sets keep their `read_at`).
   - `mute_all_recipient_gets_no_ws_or_push`: seed a room with one muted-all recipient; post `@channel`; assert the mention row is written but the WS broadcast and Push dispatch for that recipient are skipped (via the existing mute path in both layers).
   - `bounded_concurrency_caps_concurrent_push_sends`: seed a 100-person room (use a small helper to bulk-insert `room_members`); post `@channel`; mock `MockPushClient` to count peak concurrent `send()` invocations; assert peak ≤ 16.
   - Autocomplete-shape tests in the existing autocomplete test file or new ones: empty `q` returns `[@here, @channel, ...users]`; `q="h"` returns `[@here, harry, ...]` (broadcast still first); `q="c"` returns `[@channel, ...users matching c]`; DM rooms return user-only suggestions (no `@here`/`@channel`).
   - Broadcast-count endpoint: 200 + correct count for valid token; 400 for unknown token; 400 for DM room; 403 for non-member.

10. **Final verification.** `just check`, `just test`, `just test-saas`, `just verify`. Manual smoke list:
    - Type `@h` in composer → `@here` appears above `@harry` in dropdown with megaphone icon.
    - Type `@here` fully → `#lc-broadcast-count` populates with "This will notify N people in #general." within ~300ms.
    - Send the message → composer clears, count slot clears.
    - Recipient who is online + not DND sees a chip in the body, gets the WS toast + Push notification.
    - Recipient who is DND sees nothing (no chip in their unread-mentions count, no Push).
    - Recipient who muted-all the room sees nothing.
    - Two-tab test: tab 1 sends `@channel`, tab 2 (different user in the room) sees the message arrive once - not twice and not delayed past the first second.

## Things to confirm during execution

These resolve before the final task; do not defer:

1. **Scale sanity baseline result (Task 0).** Both queries under 50ms on 10K rows against the unchanged code. If either misses budget, the partial-index migration `0020_mentions_unread_partial_index.sql` lands before Task 1.

2. **Scale sanity post-change result (Task 8).** No regression vs the baseline; broadcast write path doesn't introduce a hidden per-row query or unindexed scan.

3. **Edit-path reconciliation produces the right diff.** The test `edit_reconciliation_channel_to_here` must show that `reconcile_mentions` correctly identifies the symmetric difference of the two resolved sets and updates rows accordingly. If it doesn't (e.g. because the existing function's HashSet behavior misbehaves on broadcast inputs), file a fix here, not later.

4. **Composer count debounce doesn't fire on the autocomplete popover's request.** The composer already does `htmx.ajax` for `@username` autocomplete on the same `input` event. Both probes need to coexist without one canceling the other. With the "active token at cursor" rule from Task 7, the two probes are mutually exclusive by construction (one of `@here`/`@channel`/`@harry` is the active token at any moment, never two), but verify in DevTools Network that typing `@h` fires exactly two requests in sequence as the active token changes from "h" → no-token-yet → "here", not three or zero.

5. **Body-renderer change reaches every render surface.** Task 1 names three surfaces (room message list, WS `NewMessage` fragment, thread-panel replies). After implementing the renderer change, send a `@here` from one tab and verify the chip appears in all three contexts in another tab. A regression here is silent - the message ships and the notification fires but the recipient sees raw `@here` text and wonders if it was a typo.

## Out of scope

- `@everyone` (cross-room scope; separate future phase).
- `@admins` / `@mods` role-based tokens (parser/resolver support it; not implementing now).
- Admin RBAC gating for `@channel` (settings.db row added later if demand surfaces; default permissive in v1).
- Read-receipts visible to author ("32 of 47 have seen this"; own future feature with privacy implications).
- Different visual rendering for broadcast vs `@username` mentions in the recipient view (revisit on user feedback).
- "Recall Push" on edit (OS owns the notification after firing; same as `@username` semantics today). Document this constraint in a comment near the Push dispatch site.
- Confirmation step before send (live composer count is the friction-as-feature; no extra click).
- Worker-pool fan-out (Semaphore + JoinSet is enough at this scale; revisit if rooms grow past ~1000).

## Deliverables

- `server/src/db/mentions.rs`: two new resolver helpers (`resolve_here`, `resolve_channel`).
- `server/src/routes/room.rs`: branched resolver in `post_message` and `patch_message`; bounded concurrent fan-out.
- `server/src/routes/mentions.rs`: extended autocomplete; new `get_broadcast_count` handler.
- `server/src/routes/mod.rs`: register the broadcast-count route.
- `server/src/views/mentions.rs`: `MentionSuggestion` gains `kind` + `subtitle`.
- `server/templates/partials/mention_popover.html` (or equivalent): branch on `kind` for broadcast rendering.
- `server/templates/room/composer.html`: count slot + IIFE probe extension; submit-clear.
- `server/tests/routes_broadcast_mentions.rs`: new integration tests.
- `server/tests/scale_mentions.rs` or similar: ignored-by-default scale sanity check.
- Comment near `push::dispatch` documenting the no-recall-Push contract.
- Plan staged with `git add`. No commits, no pushes - user reviews and commits per-task.

## Constraints reminder

- No Rust/Bun on host: `./dev/cargo` and `./dev/bun` only.
- All work in `server/`. Desktop untouched.
- Server-rendered HTML + HTMX. The broadcast-count endpoint returns an HTML fragment, not JSON. No bespoke JS beyond the composer IIFE extension.
- WebSocket payloads stay pre-rendered HTML fragments.
- **Claude does not commit or push.** Stage with `git add`, stop. User commits per-task during execution as a review step.
