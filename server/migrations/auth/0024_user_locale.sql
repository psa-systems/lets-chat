-- LC-100: per-user UI locale preference. NULL = fall back to the request's
-- Accept-Language header (then to English). Stores a BCP-47-ish code like
-- "en" or "es"; an unsupported value resolves to English at request time.
ALTER TABLE users ADD COLUMN locale TEXT;
