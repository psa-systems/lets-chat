-- LC-91: native mobile push channels, kept as separate tables from the
-- Web Push `push_subscriptions` (Option B in the ticket). The subscription
-- shapes differ enough per kind that one polymorphic table would be either
-- wide-and-nullable or an opaque JSON blob; three narrow tables keep each
-- channel's credential shape explicit.
--
-- APNs (iOS): a device token issued by Apple, plus the app's topic (bundle
-- id) the token is registered against. The server signs requests with its
-- own APNs auth key (stored in settings.db when the live sender lands), so
-- only the per-device token + topic live here.
CREATE TABLE IF NOT EXISTS apns_subscriptions (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id      TEXT NOT NULL,
    device_token TEXT NOT NULL UNIQUE,
    topic        TEXT,
    user_agent   TEXT,
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_apns_subscriptions_user ON apns_subscriptions (user_id);

-- FCM (Android): a registration token issued by Firebase for the install.
-- The server authenticates to FCM with a service account (stored in
-- settings.db when the live sender lands), so only the per-device
-- registration token lives here.
CREATE TABLE IF NOT EXISTS fcm_subscriptions (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id            TEXT NOT NULL,
    registration_token TEXT NOT NULL UNIQUE,
    user_agent         TEXT,
    created_at         TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen_at       TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_fcm_subscriptions_user ON fcm_subscriptions (user_id);
