# Roadmap

Durable narrative for multi-step work: goals, sequencing, and the reasoning
behind the order. Status is not recorded here. Each item links its YouTrack
issue, which is the only place its state lives.

## Client-side navigation

[LC-833](https://yt.a8n.run/issue/LC-833)

Before this work, every in-app navigation was a full browser page load, and
`server/src/routes/ws.rs` recorded the model directly. Three costs followed: a
voice call could not survive a page move, because the unload destroyed the JS
context and every `RTCPeerConnection`; the WebSocket was torn down and rebuilt
on every navigation, which is what LC-318's reconnect-banner grace period
exists to hide; and the whole shell re-rendered on every move, in an app where
moving between rooms is the dominant interaction.

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
4. [LC-837](https://yt.a8n.run/issue/LC-837) - the flip. **Shipped**: boosted
   links target `#main` explicitly, never hx-boost's default of `body`, which
   would swap the `ws-connect` element and cycle the socket on every move. See
   [ui-conventions.md](ui-conventions.md#boosted-navigation-lc-837) for the
   current model.
5. [LC-832](https://yt.a8n.run/issue/LC-832) - a joined voice channel or huddle
   survives navigation. The dock-lifting machinery already exists from
   LC-821/822/823; phase 4 is what makes it reachable.

A note worth keeping, because it already cost a wrong issue: before phase 4
shipped, several comments in the tree described an `hx-boost` model this
application had not yet built. Phase 2 corrected the two misleading template
comments, in `room/page.html` and `layout.html`. See
[ui-conventions.md](ui-conventions.md#boosted-navigation-lc-837) for the model
`hx-boost` actually implements now that phase 4 has landed.
