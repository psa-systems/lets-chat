-- LC-22: server-side scratch space for in-flight bunyip OIDC dances. One row
-- is inserted at `GET /auth/bunyip/start` carrying the PKCE verifier + nonce
-- that the matching callback must produce to complete the exchange. Rows are
-- delete-on-consume in the callback and swept by a periodic broom after
-- ~5 minutes (the value bunyip's authorize endpoint will reject anyway).
--
-- Pure-RP cutover: the additive design's `redirect_after` discriminator is
-- gone. There is only one dance shape (login -> session); no settings-Connect
-- branch exists post-cutover.
CREATE TABLE oidc_pending (
    state         TEXT PRIMARY KEY,
    code_verifier TEXT NOT NULL,
    nonce         TEXT NOT NULL,
    created_at    INTEGER NOT NULL
);
CREATE INDEX oidc_pending_created_at ON oidc_pending(created_at);
