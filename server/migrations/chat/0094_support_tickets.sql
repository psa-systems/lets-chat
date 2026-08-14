-- LC-714: AI help desk, Phase 3. Support tickets filed when a user asks for a
-- human (`/human`) and no admin is available to take it right then. Site admins
-- triage the open queue at /admin/support. Modeled on message_reports (LC-334):
-- requester_id / handled_by are auth-db user ids (TEXT); room_id + room_name are
-- denormalized from the origin conversation so the queue shows where the request
-- came from and can build a jump link without a cross-db join (and survives the
-- room being renamed). status is 'open' until an admin resolves it.
CREATE TABLE support_tickets (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    requester_id TEXT NOT NULL,
    room_id      INTEGER,
    room_name    TEXT NOT NULL DEFAULT '',
    body         TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'open',
    handled_by   TEXT,
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    handled_at   TEXT
);

-- The queue reads open tickets newest-first; the index also serves count_open.
CREATE INDEX idx_support_tickets_status ON support_tickets (status, created_at DESC);
