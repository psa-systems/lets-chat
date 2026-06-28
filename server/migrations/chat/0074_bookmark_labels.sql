-- LC-479: optional per-bookmark label ("folder"), e.g. follow-up / read-later.
--
-- A nullable free-text label on each saved-message row, private to the owning
-- user (the bookmarks table is already user-scoped via its composite PK). NULL
-- means "unlabeled". The /saved page derives its filter chips from the distinct
-- labels a user has in use; setting an empty label clears it back to NULL.
ALTER TABLE bookmarks ADD COLUMN label TEXT;
