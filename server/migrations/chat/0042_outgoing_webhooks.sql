-- LC-75: outgoing webhooks / event subscriptions. An admin registers a URL +
-- event filter + scope (global / enclave / room). When a matching event
-- fires, a delivery row is enqueued; a background task POSTs the signed JSON
-- with retries + exponential backoff, and auto-disables a webhook after too
-- many consecutive failures.
--
-- signing_secret is stored in plaintext: the server needs it to compute the
-- per-delivery HMAC signature, and the receiver holds the same value to
-- verify. It is shown to the admin and rotatable. Never logged.
CREATE TABLE IF NOT EXISTS outgoing_webhooks (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    scope_kind           TEXT    NOT NULL,            -- 'global' | 'enclave' | 'room'
    scope_id             INTEGER,                     -- NULL for global
    events               TEXT    NOT NULL,            -- space-separated event names
    url                  TEXT    NOT NULL,
    signing_secret       TEXT    NOT NULL,
    created_by           TEXT    NOT NULL,
    created_at           TEXT    NOT NULL DEFAULT (datetime('now')),
    last_success_at      TEXT,
    last_failure_at      TEXT,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    disabled_at          TEXT
);

CREATE TABLE IF NOT EXISTS outgoing_webhook_deliveries (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    webhook_id    INTEGER NOT NULL REFERENCES outgoing_webhooks(id) ON DELETE CASCADE,
    event         TEXT    NOT NULL,
    payload       TEXT    NOT NULL,
    attempt       INTEGER NOT NULL DEFAULT 0,
    status        INTEGER,                            -- HTTP status, 0 = network error, NULL = pending
    response_body TEXT,
    scheduled_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    delivered_at  TEXT,
    created_at    TEXT    NOT NULL DEFAULT (datetime('now'))
);

-- Due-delivery scan: pending rows (delivered_at IS NULL) ordered by schedule.
CREATE INDEX IF NOT EXISTS idx_owh_deliveries_due
    ON outgoing_webhook_deliveries (delivered_at, scheduled_at);
CREATE INDEX IF NOT EXISTS idx_owh_deliveries_webhook
    ON outgoing_webhook_deliveries (webhook_id, id DESC);
