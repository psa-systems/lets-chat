-- Per-flow provider id. sso_flows predates the multi-provider plan;
-- the callback handler needs to know which provider initiated the
-- flow so it can look up the right entry in sso_providers when
-- exchanging the code. Defaults to 'default' so any existing rows
-- (none in practice; the table is pruned every 10 min) keep working.
ALTER TABLE sso_flows ADD COLUMN provider_id TEXT NOT NULL DEFAULT 'default';
