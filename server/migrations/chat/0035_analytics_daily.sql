-- LC-97: pre-aggregated daily analytics so the admin dashboard renders
-- fast (a handful of indexed reads) instead of scanning the messages
-- table on every page load. One row per (date, scope, metric); the
-- background aggregator upserts these once a day and on demand.
--
-- scope_kind / scope_id exist so SaaS builds can later store per-enclave
-- breakdowns ('enclave', <enclave_id>) alongside the global rollup
-- ('global', 0) without a schema change. The standalone build only ever
-- writes the global scope today.
--
-- metric is a short string key: 'messages', 'dau', 'mau', 'active_rooms',
-- 'signups'. value is always a non-negative count; the dashboard renders
-- only counts, never per-user activity, so no privacy-sensitive data
-- lands here.
CREATE TABLE IF NOT EXISTS analytics_daily (
    date       TEXT    NOT NULL,
    scope_kind TEXT    NOT NULL DEFAULT 'global',
    scope_id   INTEGER NOT NULL DEFAULT 0,
    metric     TEXT    NOT NULL,
    value      INTEGER NOT NULL,
    PRIMARY KEY (date, scope_kind, scope_id, metric)
);

CREATE INDEX IF NOT EXISTS idx_analytics_daily_metric_date
    ON analytics_daily (metric, scope_kind, scope_id, date);
