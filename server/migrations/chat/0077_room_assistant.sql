-- LC-492: per-room opt-in for the in-channel AI assistant (/ask). Off by
-- default; a room manager turns it on, and it only functions when the operator
-- has also configured an LLM endpoint (LETS_CHAT_LLM_URL). Kept as a column on
-- rooms (read via a dedicated getter, like retention_days / broadcast policy)
-- rather than widening the Room projection.
ALTER TABLE rooms ADD COLUMN assistant_enabled INTEGER NOT NULL DEFAULT 0;
