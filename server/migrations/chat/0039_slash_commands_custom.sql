-- LC-76: admin-defined custom slash commands. Built-in commands (/help,
-- /me, /shrug, /poll, /remind) live in code; this table holds operator
-- additions.
--
-- kind:
--   'static_text'  - `target` is a template; `{args}` is replaced with the
--                    text the user typed after the command, then posted as a
--                    normal message.
--   'webhook_post' - `target` is a URL; the server POSTs the args and posts
--                    the response body as a message.
--
-- scope_kind/scope_id mirror the branding table so a future build can scope
-- commands per enclave; v1 manages the global scope ('global', 0) only.
-- admin_only gates execution: a non-admin invoking an admin-only command is
-- refused (RBAC acceptance criterion).
CREATE TABLE IF NOT EXISTS slash_commands_custom (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    scope_kind  TEXT    NOT NULL DEFAULT 'global',
    scope_id    INTEGER NOT NULL DEFAULT 0,
    name        TEXT    NOT NULL,
    description TEXT    NOT NULL DEFAULT '',
    kind        TEXT    NOT NULL CHECK (kind IN ('static_text', 'webhook_post')),
    target      TEXT    NOT NULL,
    admin_only  INTEGER NOT NULL DEFAULT 0,
    created_by  TEXT    NOT NULL DEFAULT '',
    created_at  TEXT    NOT NULL DEFAULT (datetime('now')),
    UNIQUE (scope_kind, scope_id, name)
);
