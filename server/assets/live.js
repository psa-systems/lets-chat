// LC-156: declarative live-update subscription.
//
// A page opts into WebSocket room updates by putting `data-lc-live-room="<id>"`
// on any element in its markup. This single module - loaded once from base.html
// - sends the `{type:"subscribe",room_id}` frame when the socket opens and when
// such an element is loaded into the DOM, replacing the per-page copy-paste
// `htmx:wsOpen` IIFEs that previously lived in room/dm/voice page templates.
//
// LC-160: a page may also subscribe to a typed topic with
// `data-lc-live-topic="enclave:5"` / `"user:abc"` / `"admin"`, which sends a
// `{type:"subscribe_topic",topic}` frame. The server authorizes each topic
// before adding the connection to its fan-out set.
//
// Because the module is loaded once and registers document-level listeners, it
// never accumulates duplicate listeners across reconnect soft-refreshes (the
// reason the old per-page IIFEs needed an explicit teardown). Re-subscribing to
// a room already subscribed is harmless server-side (the subscriber set is a
// HashSet). No unsubscribe is sent: subscriptions persist for the life of the
// socket, matching the prior behavior.
(function () {
  function sendSubscribe(ws, roomId) {
    if (!ws || roomId == null || roomId === '') return;
    var id = Number(roomId);
    if (!Number.isFinite(id)) return;
    try {
      ws.send(JSON.stringify({ type: 'subscribe', room_id: id }));
    } catch (e) {
      /* socket not ready / closed; a later htmx:wsOpen will retry */
    }
  }

  function sendSubscribeTopic(ws, topic) {
    if (!ws || !topic) return;
    try {
      ws.send(JSON.stringify({ type: 'subscribe_topic', topic: topic }));
    } catch (e) {
      /* socket not ready / closed; a later htmx:wsOpen will retry */
    }
  }

  // Subscribe every live element within `root` (and `root` itself if tagged),
  // for both the room shorthand and typed topics.
  function subscribeWithin(ws, root) {
    if (root && root.getAttribute) {
      var ownRoom = root.getAttribute('data-lc-live-room');
      if (ownRoom != null) sendSubscribe(ws, ownRoom);
      var ownTopic = root.getAttribute('data-lc-live-topic');
      if (ownTopic != null) sendSubscribeTopic(ws, ownTopic);
    }
    var scope = root && root.querySelectorAll ? root : document;
    scope.querySelectorAll('[data-lc-live-room]').forEach(function (el) {
      sendSubscribe(ws, el.getAttribute('data-lc-live-room'));
    });
    scope.querySelectorAll('[data-lc-live-topic]').forEach(function (el) {
      sendSubscribeTopic(ws, el.getAttribute('data-lc-live-topic'));
    });
  }

  // Socket just opened: subscribe everything currently in the DOM. Use the
  // wrapper from the event detail directly so this does not depend on whether
  // layout.html's own wsOpen listener (which sets window.__lcWS) ran first.
  document.body.addEventListener('htmx:wsOpen', function (evt) {
    subscribeWithin(evt.detail.socketWrapper, document);
  });

  // A live page swapped into an already-open socket (notably the reconnect
  // soft-refresh, which replaces #main without tearing the socket down):
  // subscribe any live elements in the freshly-settled subtree. We listen on
  // both htmx:load and htmx:afterSettle because the soft-refresh path drives
  // re-scans via afterSettle (the same event the sidebar mention-count and
  // voice re-scan logic key on), while a plain content load fires htmx:load;
  // covering both avoids depending on which one a given swap emits.
  // Re-subscribing is idempotent server-side, so the overlap is free.
  function onContentSwap(evt) {
    if (window.__lcWS) subscribeWithin(window.__lcWS, evt.target);
  }
  document.body.addEventListener('htmx:load', onContentSwap);
  document.body.addEventListener('htmx:afterSettle', onContentSwap);
})();
