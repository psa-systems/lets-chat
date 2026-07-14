-- LC-580: detect a country-level login-location change and notify the user.
--
-- last_login_country holds the ISO 3166-1 alpha-2 code of the country the user
-- last logged in from (NULL until the first geolocatable login). It is compared
-- against the current login's country at the SSO callback to detect a
-- significant change. Country is resolved from the session IP via IP2Location
-- when IP2LOCATION_DB_PATH is configured; the feature is a no-op otherwise.
ALTER TABLE users ADD COLUMN last_login_country TEXT;
