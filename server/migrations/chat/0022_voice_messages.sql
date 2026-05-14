-- Voice messages reuse the file_uploads pipeline. The only extra state a
-- voice clip needs over a normal attachment is its waveform: a JSON array of
-- normalized peak amplitudes (floats in 0..1) computed client-side at record
-- time. NULL for every non-voice upload.
ALTER TABLE file_uploads ADD COLUMN waveform TEXT;
