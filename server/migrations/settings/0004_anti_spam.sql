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
    ('honeypot_enabled',          'true');
