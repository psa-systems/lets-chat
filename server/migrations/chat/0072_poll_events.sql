-- LC-491: event / RSVP cards reuse the poll engine. A poll with event_at set is
-- an event: the question is the title, the options are the RSVP choices (Going /
-- Maybe / Can't go), and event_at drives the card header + the room iCal feed.
-- NULL event_at = an ordinary poll (unchanged behavior).
ALTER TABLE polls ADD COLUMN event_at TEXT;
ALTER TABLE polls ADD COLUMN event_location TEXT;
