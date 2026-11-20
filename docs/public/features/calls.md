# Calls

Voice and video across every room shape. The caller never leaves the chat UI.

## 1:1 audio and video

Direct calls go through WebRTC in a simple mesh. The caller sees a ring state and a running duration once the call starts.

Both sides pick mic, camera, and speaker in the pre-join sheet. Floating emoji reactions during a call are a lightweight way to signal without interrupting.

## Huddles

A huddle is a multi-party voice channel inside a room. Joining a huddle rings the room's members who are not already viewing the room so they can drop in. A huddle persists until the last person leaves.

Huddles above a handful of participants require a LiveKit Selective Forwarding Unit (see `LETS_CHAT_LIVEKIT_*` in [Configuration](../configuration.md)). Without LiveKit the mesh path works for small huddles but degrades past a certain size.

## Stage mode

Stage mode puts the room into a one-to-many audio layout. Speakers hold the mic; listeners request to speak. Admins and moderators promote listeners into the speaker row. Stage mode uses the LiveKit path when configured.

## Screen share

Any call participant can share a window or an entire screen. The share stream rides alongside the audio and video streams.

On Linux, screen share needs PipeWire and the right portal backend installed. See [Installation](../installation.md).

## In-call chat

Chat is one tab in the call's unified dock, alongside Transcript and Diagnostics; selecting the Chat tab in the dock's tab strip shows the panel where anyone in the call can send and read messages. Those messages post to the voice room's own message channel so they persist after the call ends.

## Call transcription

Captions appear during a call either from the speaker's browser (Web Speech API, no server needed) or from a server-side transcription agent against an endpoint the operator runs. See [Transcription agent](transcription-agent.md).

## Remote control

During a 1:1 call, one participant can request keyboard and mouse control of a peer's shared screen. Shipping constraints:

- Desktop app only, Windows only.
- Off by default. An administrator enables remote control at `/admin/remote-control` before any user can request control.
- Only verified-email accounts can request control.
- The controlling party can revoke control at any time.
- The admin can review every consent event on the audit page.

Treat remote control as a security-sensitive feature. The audit trail is the operator's record.
