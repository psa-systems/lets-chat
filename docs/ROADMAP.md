# Roadmap

Durable narrative for multi-step work: goals, sequencing, and the reasoning
behind the order. Status is not recorded here. Each item links its YouTrack
issue, which is the only place its state lives.

## Client-side navigation

[LC-833](https://yt.a8n.run/issue/LC-833)

Every in-app navigation is currently a full browser page load. `hx-boost`
appears nowhere in `server/templates/`, and `server/src/routes/ws.rs` records
the model directly. Three costs follow: a voice call cannot survive a page
move, because the unload destroys the JS context and every
`RTCPeerConnection`; the WebSocket is torn down and rebuilt on every
navigation, which is what LC-318's reconnect-banner grace period exists to
hide; and the whole shell re-renders on every move, in an app where moving
between rooms is the dominant interaction.

The layout is already shaped for the change. `layout.html` puts the
`ws-connect` element and the sidebar outside `<main id="main">`, so a swap
targeting `#main` never closes the socket. That is why the reconnect flash is
a targeting decision rather than a problem needing new machinery.

Sequenced so the first three phases are independently safe to land while
navigation is still a full page load. Each is a no-op or a redundancy until
the flip, which means the risky change arrives after its prerequisites are
already in production rather than alongside them.

1. [LC-834](https://yt.a8n.run/issue/LC-834) - per-connection WebSocket state
   survives a navigation. `current_enclave`, `subscribed` and `dm_seen_msg`
   are stable today only because each navigation opens a fresh socket.
2. [LC-835](https://yt.a8n.run/issue/LC-835) - inline page scripts become safe
   to re-run. 34 templates carry one; under a swap each re-runs and stacks
   handlers, and the failure is silent.
3. [LC-836](https://yt.a8n.run/issue/LC-836) - the sidebar stays correct
   without a page load. It sits outside the swap target, so the active-room
   highlight and unread counts would otherwise go stale.
4. [LC-837](https://yt.a8n.run/issue/LC-837) - the flip. Boost navigation with
   an explicit `#main` target, never hx-boost's default of `body`, which would
   swap the `ws-connect` element and cycle the socket on every move.
5. [LC-832](https://yt.a8n.run/issue/LC-832) - a joined voice channel or huddle
   survives navigation. The dock-lifting machinery already exists from
   LC-821/822/823; phase 4 is what makes it reachable.

A note worth keeping, because it already cost a wrong issue: several comments
in the tree described an `hx-boost` model this application has never used. The
attribute has never appeared in a template on any branch; the only commit
carrying the literal `hx-boost="true"` is the one that added this file. Phase 2
corrected the two misleading template comments, in `room/page.html` and
`layout.html`. The remaining mention, in `ws.rs`, describes the current model
accurately and is phase 4's to update when the model changes.
