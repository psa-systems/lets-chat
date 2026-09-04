# STT comparison fixtures (LC-861)

Audio fixtures the transcription comparison harness (`server/tests/stt_comparison.rs`)
replays, identically, through every configured STT service so they can be judged
on the same input instead of on separate live meetings.

## Layout

One directory per fixture:

```
single-speaker/       one clear speaker
poor-mic/             a speaker on a rough/noisy microphone
concurrent-speakers/  two or more people talking at once (the jumble case)
```

Each contains:

- `audio.wav` - the clip replayed through every service.
- `reference.txt` - the hand-corrected transcript the output is scored against
  (word error rate). Lowercase, punctuation-optional; the scorer normalizes both
  sides, so exact casing/punctuation do not matter.

`server/tests/stt_comparison.rs` discovers fixtures by scanning the subdirectories
here, so adding a case is: make a directory, drop an `audio.*` and a
`reference.txt` in it. Supported audio extensions map to the MIME the adapters
expect (`.wav`, `.webm`, `.ogg`, `.mp3`, `.m4a`, `.flac`).

## The committed audio is a PLACEHOLDER

The checked-in `audio.wav` files are **synthetic tones, not speech** - this repo's
CI/dev environment has no TTS engine, so real recordings cannot be generated here.
They exist so the fixture layout is complete and the harness has real bytes to
replay end to end. A real STT service transcribes a tone to little or nothing, so
its WER against `reference.txt` will read ~100%. That is expected for the
placeholders and is not a harness bug.

To get a meaningful comparison, **replace each `audio.wav` with a genuine
recording** of its `reference.txt` sentence (or drop in your own clip and rewrite
`reference.txt` to match). The concurrent case should be a real mix of two people
talking over each other - that is the condition the ticket cares about most.

Regenerate the placeholders (byte-stable) with:

```
python3 server/tests/fixtures/stt/generate_placeholders.py
```

## Running the comparison

The deterministic harness tests (WER scoring, report shaping) run in CI with mock
adapters and need none of this audio. The live run is opt-in and hits real
endpoints - see the module docs at the top of `server/tests/stt_comparison.rs`,
or `just stt-bench`.
