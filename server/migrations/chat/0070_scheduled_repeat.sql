-- LC-485: recurring scheduled messages. `repeat` controls whether a delivered
-- row re-enqueues its next occurrence. 'none' (default) preserves the one-shot
-- behavior; 'daily'/'weekly'/'weekdays' (Mon-Fri) drive standups, reminders,
-- digests. The next occurrence is enqueued as a fresh row inside the same
-- transaction that marks this one delivered (crash-safe), so each fire is its
-- own auditable row and cancelling the pending row stops the series.
ALTER TABLE scheduled_messages ADD COLUMN repeat TEXT NOT NULL DEFAULT 'none'
  CHECK (repeat IN ('none', 'daily', 'weekly', 'weekdays'));
