# Transcription agent

Server-side call transcription. Off by default. Only call captions and post-call transcripts rely on it; everything else works without the agent.

## What it does

The transcription agent joins a call as an invisible participant, transcribes the audio stream, and writes the resulting captions to the voice room. Call participants see captions appear in the UI during the call; after the call ends, a transcript message is posted in the voice room with the final text.

## When to enable it

- You want captions to persist after the call, not only during it.
- Your users' browsers do not all support the Web Speech API path.
- You want a consistent engine across every call rather than relying on each participant's browser.

If none of those apply, leave the agent off and let the browser do captions.

## Requirements

- A configured LiveKit Selective Forwarding Unit (`LETS_CHAT_LIVEKIT_*` variables; see [Configuration](../configuration.md)).
- A shared token the app and the agent both hold: `LETS_CHAT_TRANSCRIBE_AGENT_TOKEN`.
- A matching agent identity name: `LETS_CHAT_TRANSCRIBE_AGENT_NAME`, defaulting to `transcriber`.

The agent is its own container. The repo ships a compose overlay at `compose.transcription-agent.yml` that stands it up alongside the app:

```
docker compose -f compose.yml -f compose.transcription-agent.yml up
```

The overlay reads the four LiveKit variables, the shared token, and the agent name. The guards in the overlay refuse to start if any of them is unset.

## How it joins a call

The app dispatches the agent by name onto the LiveKit room a call creates. The agent authenticates to LiveKit with the API key and secret, authenticates to the app with the shared token, listens to the audio stream, and posts captions back to the room through the app's API.

There is no direct caller-to-agent handshake. If dispatch fails (wrong agent name, wrong token, agent is down), captions do not appear; the call still works.

## Stopping the agent

`docker compose -f compose.yml -f compose.transcription-agent.yml stop transcription-agent`. The app recovers automatically: future calls fall back to no server-side captions. The browser path still works if you have not disabled it.

## Where captions live

During the call: in the caption bar of the call UI. After the call ends: in the voice room as a dedicated transcript message, pinnable like any other message.

## What the agent does not do

- It does not store audio. The agent transcribes in-stream and discards the audio.
- It does not translate. Captions come out in the spoken language.
- It is not a call recorder. Recording calls is a separate feature outside this doc.
