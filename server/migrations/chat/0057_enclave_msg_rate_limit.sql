-- LC-217: per-enclave message-send rate limit override.
-- 0 = use the global (`rate_limit_messages` settings KV) default.
-- Non-zero = burst per minute applied to ALL members posting in any room
-- in this enclave, in addition to the global cap. The enforcement chains
-- both checks (per-enclave key first, then global key) so a per-enclave
-- override never relaxes the global cap, only tightens it for the enclave.
-- See `routes::room::post_message` and `rate_limit::RateLimitKind::Message`.
ALTER TABLE enclaves ADD COLUMN msg_rate_limit_burst INTEGER NOT NULL DEFAULT 0;
