# Phase 10 — Emoji Reactions

**Date:** 2026-04-14  
**Status:** In progress

---

## Goal

React to messages with emoji. High UX value, self-contained schema, no dependency on upcoming phases.

---

## Tasks

- [ ] 1. DB migration `0006_reactions.sql` — `message_reactions` table
- [ ] 2. `src/db/chat.rs` — add `add_reaction`, `remove_reaction`, `list_reactions` DB functions
- [ ] 3. `src/models/` — add `Reaction` model (`emoji`, `count`, `reacted_by_me`)
- [ ] 4. `src/ws/events.rs` — add `ReactionAdded` / `ReactionRemoved` variants
- [ ] 5. `src/server_fns/reactions.rs` — add `add_reaction`, `remove_reaction`, `list_reactions` server fns
- [ ] 6. `src/server_fns/mod.rs` — register reactions module
- [ ] 7. `src/components/reaction_bar.rs` — reaction display + emoji picker component
- [ ] 8. Wire `reaction_bar` into `room_view.rs` and `dm_view.rs`
- [ ] 9. Handle `ReactionAdded` / `ReactionRemoved` WS events to update counts in place
- [ ] 10. `tests/db_reactions.rs` — integration tests

---

## Schema

```sql
CREATE TABLE IF NOT EXISTS message_reactions (
    message_id  INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    user_id     TEXT NOT NULL,
    emoji       TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (message_id, user_id, emoji)
);

CREATE INDEX IF NOT EXISTS idx_reactions_message ON message_reactions(message_id);
```

Composite PK enforces one reaction per (message, user, emoji). Emoji stored as Unicode characters.

---

## Emoji Picker

Static set — no third-party library:

```
👍 👎 ❤️ 😂 😮 😢 🔥 🎉 👀 ✅
```

Displayed as a hover popover on each message, toggled by a `+` button.

---

## WS Live Updates

On `ReactionAdded`/`ReactionRemoved`, update the reaction signal for that message locally — no server refetch. Each message's reactions stored in a `Signal<Vec<Reaction>>` keyed by `message_id`.

---

## Risks

- Reactions fetched lazily per message view (not bundled into `list_messages`).
- Validate `emoji.chars().count() <= 8` and reject control characters.
- On `ReactionAdded` for my own action, skip the WS update (we already updated optimistically).
