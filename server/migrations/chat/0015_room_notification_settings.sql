CREATE TABLE IF NOT EXISTS room_notification_settings (
    user_id      TEXT    NOT NULL,
    room_id      INTEGER NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    mute_mode    TEXT    NOT NULL CHECK (mute_mode IN ('none', 'except_mentions', 'all')),
    muted_until  TEXT,
    updated_at   TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (user_id, room_id)
);

CREATE INDEX IF NOT EXISTS idx_room_notify_settings_user
    ON room_notification_settings (user_id);
