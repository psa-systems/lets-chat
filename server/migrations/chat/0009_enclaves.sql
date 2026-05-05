CREATE TABLE enclaves (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL UNIQUE,
    description TEXT,
    is_public   INTEGER NOT NULL DEFAULT 0,
    invite_code TEXT,
    created_by  TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE UNIQUE INDEX idx_enclaves_invite_code
    ON enclaves(invite_code) WHERE invite_code IS NOT NULL;

CREATE TABLE enclave_members (
    enclave_id  INTEGER NOT NULL REFERENCES enclaves(id) ON DELETE CASCADE,
    user_id     TEXT NOT NULL,
    role        TEXT NOT NULL CHECK (role IN ('owner','admin','member')),
    joined_at   TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (enclave_id, user_id)
);
CREATE UNIQUE INDEX idx_enclaves_one_owner
    ON enclave_members(enclave_id) WHERE role = 'owner';
CREATE INDEX idx_enclave_members_user ON enclave_members(user_id);

CREATE TABLE enclave_invitations (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    enclave_id  INTEGER NOT NULL REFERENCES enclaves(id) ON DELETE CASCADE,
    invitee_id  TEXT NOT NULL,
    invited_by  TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (enclave_id, invitee_id)
);
CREATE INDEX idx_enclave_invitations_invitee ON enclave_invitations(invitee_id);

ALTER TABLE rooms ADD COLUMN enclave_id INTEGER REFERENCES enclaves(id) ON DELETE CASCADE;
CREATE INDEX idx_rooms_enclave ON rooms(enclave_id);

INSERT INTO enclaves (name, description, created_by) VALUES ('General', 'Default enclave', 'system');
UPDATE rooms SET enclave_id = (SELECT id FROM enclaves WHERE name='General') WHERE room_type != 'dm';
