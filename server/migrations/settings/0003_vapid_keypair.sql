CREATE TABLE IF NOT EXISTS vapid_keypair (
    id                         INTEGER PRIMARY KEY CHECK (id = 1),
    public_key_b64url          TEXT NOT NULL,
    private_key_pem_encrypted  BLOB NOT NULL,
    private_key_pem_nonce      BLOB NOT NULL,
    created_at                 TEXT NOT NULL DEFAULT (datetime('now'))
);
