ALTER TABLE rooms ADD COLUMN invite_code TEXT UNIQUE;
CREATE INDEX IF NOT EXISTS idx_rooms_invite_code ON rooms(invite_code);
