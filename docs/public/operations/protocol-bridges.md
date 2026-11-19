# Protocol bridges

An out-of-process daemon that translates between lets chat and a foreign protocol. Matrix is the shipping bridge in the current series; the schema is shared so other daemons can target it.

## Read this first

**A daemon that does not self-filter its own outgoing-webhook traffic creates an infinite cross-network amplification loop.** Every incoming message the daemon forwards generates an outgoing webhook; every webhook the daemon receives generates an outgoing forward. The daemon must drop events whose origin it just posted.

This is a bridge daemon's responsibility, not the server's. Verify your daemon's self-filter before enabling it.

## What the server provides

- **Registration**: an admin registers a bridge name and bounds in `/admin/bridges`.
- **Scoped API token**: one token per bridge. The daemon uses it to post messages and heartbeats.
- **Outgoing webhook stream**: the server sends every event in the registered bounds to a URL the daemon subscribes to.

What the server does NOT provide:
- The daemon itself. The operator runs `matrix-appservice-bridge` or any community daemon configured against the schema.
- Identity translation beyond the bridge-actor badge. Mapping foreign users to lets chat users is bridge-local state.

## Registering a bridge

An administrator creates a bridge registration with:

- A name (`matrix`, `irc-oftc`, whatever).
- The webhook URL the daemon listens on.
- The target: one room, one enclave, or the whole server. Narrower is safer.

The registration mints a scoped API token. Store the token in the daemon's secrets, not in a repo.

## What rides on the webhook

Each event is a JSON message with the room, the author (real user or bridge actor), the message body, attachments, and a monotonic sequence id. Replay from the last-known id is supported so a daemon that was offline catches up.

## Heartbeat

The daemon POSTs a heartbeat at least every 60 seconds. A missed heartbeat marks the bridge as unhealthy in `/admin/bridges` and triggers a notification. Three missed heartbeats stop forwarding outbound events until the daemon heartbeats again (so a stuck daemon does not build up a replay backlog it will never drain).

## Matrix (today)

The daemon configuration points `matrix-appservice-bridge` at this lets chat server's registration endpoint and at the Matrix homeserver you want to bridge to. One lets chat room maps to one Matrix room. Server-side federation is the Matrix homeserver's business; lets chat is not federated.

## What is NOT shipped

- IRC bridge daemon. The schema supports it, no official daemon ships with lets chat.
- XMPP bridge daemon. Same.
- A global presence or typing-indicator protocol. The webhook schema carries presence events; a daemon that maps them is on the operator.

## Disabling a bridge

Revoke the token from `/admin/bridges`. The daemon starts getting 401s on its next webhook delivery and cannot post messages. Delete the registration once the daemon is confirmed off.

## Loop detection

If the server detects a message that looks like a round-trip from the bridge (same body and author within a short window), it drops the duplicate and increments a `bridge_loops_prevented` metric. This is a guard, not a replacement for the daemon's self-filter.
