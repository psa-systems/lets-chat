-- The enclave-invite typeahead's `pending_invitee_ids_for_enclave` query
-- filters by `enclave_id`, but the only existing index on
-- `enclave_invitations` is on `invitee_id` (created in 0009). Without this
-- index a fast typist in a large enclave fires one full-table scan of
-- `enclave_invitations` every 200 ms (the debounce). The index turns each
-- request into an O(pending invitations for that enclave) lookup.
CREATE INDEX IF NOT EXISTS idx_enclave_invitations_enclave
    ON enclave_invitations(enclave_id);
