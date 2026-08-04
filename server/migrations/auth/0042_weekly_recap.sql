-- LC-671: the last time a personal weekly recap DM was sent to a user, used to
-- dedupe (send at most once per 7 days). NULL until the first recap. Distinct
-- from last_digest_sent_at (the email digest). Off by default; the operator
-- opts in with LETS_CHAT_WEEKLY_RECAP and it only runs when an LLM is configured.
ALTER TABLE users ADD COLUMN last_weekly_recap_at TEXT;
