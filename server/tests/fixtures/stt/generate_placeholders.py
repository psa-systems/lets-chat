#!/usr/bin/env python3
# LC-861: deterministic PLACEHOLDER audio for the STT comparison harness.
#
# These are synthetic tones, NOT speech - this environment has no TTS engine, so
# real recordings cannot be produced here. They exist so the fixture layout is
# complete and the harness has bytes to replay end to end; a real service will
# transcribe them to little or nothing, so their WER against the reference reads
# ~100%. Replace each audio.wav with a genuine recording of the sentence in the
# sibling reference.txt (or your own clip + reference) to get a meaningful score.
#
# Regenerate with:  python3 server/tests/fixtures/stt/generate_placeholders.py
# Output is byte-for-byte stable so a checked-in .wav never shows a spurious diff.
import math
import os
import struct
import wave

HERE = os.path.dirname(os.path.abspath(__file__))
RATE = 8000  # 8 kHz mono is plenty for a placeholder and keeps the blobs tiny.


def _write(name, samples):
    path = os.path.join(HERE, name, "audio.wav")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    clipped = [max(-1.0, min(1.0, s)) for s in samples]
    frames = b"".join(struct.pack("<h", int(s * 30000)) for s in clipped)
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(frames)
    print(f"wrote {name}/audio.wav ({len(frames) + 44} bytes)")


def tone(freq, secs, amp=0.6):
    return [amp * math.sin(2 * math.pi * freq * n / RATE) for n in range(int(secs * RATE))]


def lcg_noise(secs, amp):
    # A tiny deterministic PRNG (no `random` seeding surprises across versions).
    out = []
    state = 0x2545F491
    for _ in range(int(secs * RATE)):
        state = (1103515245 * state + 12345) & 0x7FFFFFFF
        out.append(amp * ((state / 0x3FFFFFFF) - 1.0))
    return out


# 1) single clear speaker: one clean tone.
_write("single-speaker", tone(440, 2.0))

# 2) poor microphone: quieter tone buried in steady noise.
poor = tone(300, 2.0, amp=0.25)
noise = lcg_noise(2.0, amp=0.35)
_write("poor-mic", [a + b for a, b in zip(poor, noise)])

# 3) concurrent speakers: two tones summed, the case that jumbles output.
a = tone(440, 2.0, amp=0.5)
b = tone(660, 2.0, amp=0.5)
_write("concurrent-speakers", [x + y for x, y in zip(a, b)])
