-- LC-78: per-message synthetic actor snapshot for bridge-posted messages.
-- Unlike messages.webhook_id (LC-74) and messages.email_inbox_id (LC-77),
-- which join to a fixed per-channel actor row at render time, a bridge
-- message carries its OWN foreign_name (and kind) snapshotted at post time:
-- Matrix alice@server.org and bob@server.org posting into the same bridged
-- room are distinct actors with an open-ended set, so the snapshot is the
-- only place their identity can live.
--
-- ON DELETE SET NULL on bridge_id is what makes "remove bridge" non-
-- destructive (stop-new, not delete-history): deleting the bridges row nulls
-- out the FK on each message but leaves bridge_foreign_name + bridge_kind
-- intact, so historical render still shows "alice" with the "via matrix"
-- badge. If a future requirement flips to delete-history, that's an additive
-- admin branch that hard-deletes the messages BEFORE the bridges row.
--
-- bridge_foreign_avatar is nullable now and ALWAYS NULL in v1: the bridge
-- endpoint 400s any non-null avatar_url. The column exists so the
-- LC-78-AVATAR-PROXY follow-up (proxy / cache foreign avatars) can fill it
-- without a migration. Foreign avatar URLs are wider attack and privacy
-- surface than webhook avatars (per-render fetch from arbitrary federated
-- homeservers leaks viewer IP), so v1 rejects them outright.
ALTER TABLE messages ADD COLUMN bridge_id INTEGER REFERENCES bridges(id) ON DELETE SET NULL;
ALTER TABLE messages ADD COLUMN bridge_foreign_name TEXT;
ALTER TABLE messages ADD COLUMN bridge_foreign_avatar TEXT;
ALTER TABLE messages ADD COLUMN bridge_kind TEXT;

CREATE INDEX IF NOT EXISTS idx_messages_bridge_id ON messages (bridge_id) WHERE bridge_id IS NOT NULL;
