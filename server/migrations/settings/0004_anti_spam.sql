-- LC-94: anti-spam toggles + IP-keyed rate-limit caps.
--
-- The pre-existing `rate_limit_messages` key (seeded in 0001 at '30')
-- is now actually enforced. The two new IP-keyed caps default to '0'
-- (disabled) because operators behind a reverse proxy that already
-- limits at the edge would otherwise double-rate-limit, and operators
-- on a single-tenant local deployment generally do not need them.
--
-- `link_filter_enabled` and `honeypot_enabled` default to 'true'
-- because both are safe-by-default - the link filter is a no-op if
-- the admin has not added any rules beyond the seeded TLDs, and the
-- honeypot has no false-positive impact on real users.
INSERT OR IGNORE INTO settings (key, value) VALUES
    ('rate_limit_registrations',  '0'),
    ('rate_limit_password_resets', '0'),
    ('link_filter_enabled',       'true'),
    ('honeypot_enabled',          'true'),
    -- Whether to trust client-IP headers (X-Forwarded-For,
    -- X-Real-IP) when keying per-IP rate limits. Default 'true'
    -- because the documented deployment shape puts the server
    -- behind a reverse proxy (Traefik) that overwrites these
    -- headers. Set to 'false' when the server is exposed directly
    -- to the internet without a header-stripping front-end: the
    -- per-IP limits then effectively no-op (no IP is extracted)
    -- and the operator should rely on the per-user message cap +
    -- an external WAF instead.
    ('trust_proxy_headers',       'true');

-- The pre-existing `rate_limit_messages` setting was seeded in
-- `settings/0001_create_tables.sql` at '30' but never enforced.
-- Now that the rate limiter is wired, that 30/min cap would start
-- firing on upgrade and surprise operators of long-running
-- deployments who never opted in. Reset it to '0' (disabled) ONLY
-- when the value is still the original seeded default; a value
-- the admin customized stays untouched.
UPDATE settings SET value = '0'
 WHERE key = 'rate_limit_messages' AND value = '30';
