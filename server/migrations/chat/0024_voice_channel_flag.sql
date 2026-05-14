-- Voice channels are an independent axis from visibility: a channel is text
-- or voice (`is_voice`), and separately public or private (`room_type`).
ALTER TABLE rooms ADD COLUMN is_voice INTEGER NOT NULL DEFAULT 0;

-- Migrate any channels created under the interim scheme where "voice" was a
-- `room_type` value; treat those as public voice channels.
UPDATE rooms SET is_voice = 1, room_type = 'public' WHERE room_type = 'voice';
