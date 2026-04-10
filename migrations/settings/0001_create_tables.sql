CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

INSERT OR IGNORE INTO settings (key, value) VALUES
    ('site_name', 'Let''s Chat'),
    ('registration_open', 'true'),
    ('require_invite_code', 'false'),
    ('default_role', 'user'),
    ('welcome_message', 'Welcome to Let''s Chat!'),
    ('max_message_length', '4000'),
    ('motd', ''),
    ('maintenance_mode', 'false'),
    ('rate_limit_messages', '30'),
    ('smtp_host', ''),
    ('smtp_port', '587'),
    ('smtp_user', ''),
    ('smtp_pass', '');
