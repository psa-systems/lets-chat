-- LC-1059: Discord-style welcome message when a user joins an enclave.
--
-- One optional row per enclave. No row = enabled, posting to the enclave's
-- default room (first public text room by name). room_id NULL = default room.
CREATE TABLE enclave_welcome (
    enclave_id INTEGER PRIMARY KEY REFERENCES enclaves(id) ON DELETE CASCADE,
    enabled    INTEGER NOT NULL DEFAULT 1,
    room_id    INTEGER REFERENCES rooms(id) ON DELETE SET NULL
);
