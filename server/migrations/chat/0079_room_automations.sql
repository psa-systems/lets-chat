-- LC-495: workflow automations - no-code "when X happens in this room, do Y"
-- rules, configured by a room manager. This is the persistence for the engine
-- in `server/src/automations.rs`.
--
-- v1 triggers (trigger_kind):
--   message_posted  - a human posts a message; match_text is a case-insensitive
--                     substring the body must contain (NULL/empty = any message).
--   reaction_added  - a human adds a reaction; match_text is the emoji that must
--                     match (NULL/empty = any emoji).
-- v1 action (action_kind):
--   post_message    - the `automation` bot posts action_body to the room. The
--                     template supports {user} (triggering user's label),
--                     {text} (the triggering message body), and {emoji}.
--
-- trigger_kind and action_kind are open TEXT, not CHECK-constrained enums, so a
-- later migration can add triggers/actions without rewriting this table; the
-- engine validates known kinds on the write path and ignores unknown ones on
-- the read path.
CREATE TABLE room_automations (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    room_id      INTEGER NOT NULL,
    -- Optional human label shown in the manage UI; the rule still works without.
    name         TEXT,
    enabled      INTEGER NOT NULL DEFAULT 1,
    trigger_kind TEXT NOT NULL,
    -- Trigger-specific filter (keyword for message_posted, emoji for
    -- reaction_added). NULL = fire on every occurrence of the trigger.
    match_text   TEXT,
    action_kind  TEXT NOT NULL DEFAULT 'post_message',
    action_body  TEXT NOT NULL,
    created_by   TEXT NOT NULL,
    created_at   TEXT NOT NULL DEFAULT (datetime('now'))
);

-- The engine's hot path is "active rules for this room + this trigger".
CREATE INDEX idx_room_automations_lookup
    ON room_automations (room_id, trigger_kind, enabled);
