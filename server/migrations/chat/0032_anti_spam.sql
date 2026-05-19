-- LC-94: anti-spam rate limits, link filter, quarantine.
--
-- The quarantined column lets the link filter hold a message after
-- insert (so we can show it to the admin for review) without
-- soft-deleting it. `list_messages` adds `AND quarantined = 0` so
-- held messages never reach the room until a moderator approves.
ALTER TABLE messages ADD COLUMN quarantined INTEGER NOT NULL DEFAULT 0;

-- Admin-managed link-filter rules. `pattern` is either a literal host
-- ("example.com") or a simple glob ("*.example.com", "*.tk"); the
-- matcher converts `*` to regex `.*` at check time. `action` decides
-- how a match is handled:
--   'block'      - reject the send with 400
--   'quarantine' - insert with quarantined=1; admin reviews at /admin/quarantine
--   'warn'       - insert normally + audit-log the match
CREATE TABLE IF NOT EXISTS link_filter_rules (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    pattern    TEXT NOT NULL UNIQUE,
    action     TEXT NOT NULL CHECK (action IN ('block', 'quarantine', 'warn')),
    created_by TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_link_filter_rules_pattern ON link_filter_rules(pattern);

-- One row per quarantined message; records which rule caught it and
-- carries the moderator's eventual decision. Cascades on message
-- delete so a rejected message drags its quarantine row with it.
CREATE TABLE IF NOT EXISTS link_filter_quarantine (
    message_id      INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    matched_pattern TEXT NOT NULL,
    matched_url     TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    reviewed_by     TEXT,
    reviewed_at     TEXT,
    decision        TEXT CHECK (decision IN ('approve', 'reject'))
);
CREATE INDEX IF NOT EXISTS idx_quarantine_unreviewed
    ON link_filter_quarantine(created_at)
    WHERE reviewed_at IS NULL;

-- Curated default deny-list: the Freenom free-TLDs (Tokelau, Mali,
-- Gabon, Central African Republic) have been the dominant source of
-- throwaway spam domains for years. Seeded with `quarantine` rather
-- than `block` so the admin sees what would otherwise be silently
-- dropped and can upgrade to `block` (or remove the rule) per their
-- own threat model. The `system` actor distinguishes seeded rules
-- from admin-authored ones in the audit log.
INSERT OR IGNORE INTO link_filter_rules (pattern, action, created_by) VALUES
    ('*.tk',  'quarantine', 'system'),
    ('*.ml',  'quarantine', 'system'),
    ('*.ga',  'quarantine', 'system'),
    ('*.cf',  'quarantine', 'system');
