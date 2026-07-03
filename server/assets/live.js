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

  // LC-230: optimistic-echo dedupe. The composer renders a pending
  // placeholder tagged data-client-id into #messages at submit time; the
  // canonical render arrives over the WS as an OOB beforeend fragment whose
  // wrapper carries the same id in data-lc-client-id (author's connections
  // only). Remove the placeholder just before the swap so the placeholder
  // removal and the canonical insert land in the same swap cycle: the message
  // never renders twice and never disappears in between. The wrapper
  // attribute itself never reaches the DOM (beforeend swaps discard the
  // wrapper), which is why this must read the *incoming* fragment here.
  document.body.addEventListener('htmx:oobBeforeSwap', function (evt) {
    // For a beforeend OOB swap, htmx 2.0.4 passes the cloned wrapper element
    // itself as detail.fragment (isInlineSwap is true only for outerHTML
    // swaps), so a single attribute read resolves the id. Non-echo OOB swaps
    // (typing, badges, reactions, edits) pay one null attribute read here
    // and return.
    var frag = evt.detail && evt.detail.fragment;
    if (!frag || !frag.getAttribute) return;
    var cid = frag.getAttribute('data-lc-client-id');
    if (!cid) return;
    // The cid is server-sanitized to [A-Za-z0-9-], so it is selector-safe.
    var pending = document.querySelector('#messages [data-client-id="' + cid + '"]');
    if (pending) pending.remove();
  });

  // LC-415 follow-up: tell the server which enclave's sidebar the viewer is
  // currently looking at, on every htmx request. Routes that re-render the
  // whole sidebar (category add/rename/delete/reorder, star toggle/reorder,
  // mark-read / mark-unread) need the viewer's CURRENT enclave to render the
  // right shape; deriving it from HX-Current-URL alone is fragile (a URL the
  // path heuristic does not recognise, a stripped header, or a sidebar OOB
  // swap that raced the submit all yield None, collapsing the re-render to the
  // DM-only sidebar and dropping the just-changed row). The rendered sidebar
  // already encodes it in the `#sidebar-nav-{id}` element id (plain
  // `sidebar-nav` when not in an enclave), so read it live from the DOM and
  // send it as an explicit header the server prefers. Always present and
  // always correct because it is the very element the user is looking at.
  document.body.addEventListener('htmx:configRequest', function (evt) {
    var nav = document.querySelector('nav[id^="sidebar-nav-"]');
    if (!nav) return;
    var m = /^sidebar-nav-(\d+)$/.exec(nav.id);
    if (m && evt.detail && evt.detail.headers) {
      evt.detail.headers['X-LC-Current-Enclave'] = m[1];
    }
  });

  // LC-528: read a message aloud via the browser SpeechSynthesis API. Entirely
  // client-side - no route, no LLM. Clicking the "Read aloud" menu item speaks
  // the message's rendered text; clicking it again (or reading another message)
  // stops. When the browser has no speech synthesis, the item is hidden by the
  // `.lc-no-tts` CSS rule instead of failing silently on click.
  var synth = window.speechSynthesis;
  if (!synth || typeof window.SpeechSynthesisUtterance !== 'function') {
    document.documentElement.classList.add('lc-no-tts');
  } else {
    // The single in-flight read: the button that started it, its original
    // label, and the message id (so we can stop if that message leaves the DOM).
    var reading = null;

    function stopReading() {
      if (!reading) return;
      var btn = reading.btn;
      var orig = reading.origLabel;
      reading = null;
      // cancel() fires the utterance 'end' handler, which is now a no-op
      // because `reading` is already null.
      synth.cancel();
      if (btn && btn.isConnected) btn.textContent = orig;
    }

    document.body.addEventListener('click', function (evt) {
      var btn = evt.target.closest('[data-lc-read-aloud]');
      if (!btn) return;
      evt.preventDefault();
      // Toggle off if this same button is the one currently reading.
      var wasThis = reading && reading.btn === btn;
      stopReading();
      if (wasThis) return;

      var id = btn.getAttribute('data-lc-read-aloud');
      var body = document.querySelector('#msg-' + CSS.escape(id) + ' .lc-md');
      var text = body ? body.innerText.trim() : '';
      if (!text) return;

      var u = new SpeechSynthesisUtterance(text);
      var docLang = document.documentElement.lang;
      if (docLang) u.lang = docLang;
      var origLabel = btn.textContent;
      var stopLabel = btn.getAttribute('data-lc-reading-label') || origLabel;
      u.onend = function () {
        // Only reset if this utterance is still the active one (a newer read
        // or an explicit stop clears `reading` first).
        if (reading && reading.utterance === u) {
          var b = reading.btn;
          reading = null;
          if (b && b.isConnected) b.textContent = origLabel;
        }
      };
      u.onerror = u.onend;
      reading = { btn: btn, origLabel: origLabel, id: id, utterance: u };
      btn.textContent = stopLabel;
      synth.speak(u);
    });

    // Stop if the message being read is swapped out of the DOM (deleted,
    // room switch, reconnect soft-refresh). htmx swaps don't reload the page,
    // so speech would otherwise keep playing with a stale button reference.
    document.body.addEventListener('htmx:afterSettle', function () {
      if (reading && !document.getElementById('msg-' + reading.id)) stopReading();
    });
    // Belt-and-suspenders for a real navigation away from the app.
    window.addEventListener('pagehide', function () { synth.cancel(); });
  }

  // LC-532: composer AI writing-assistant apply / dismiss. The result panel
  // (server-rendered into #composer-ai-panel) carries the suggestion as plain
  // text; "Use this" writes it into the composer textarea and fires an input
  // event so the autosize / highlight / draft-autosave hooks pick it up.
  document.body.addEventListener('click', function (evt) {
    var apply = evt.target.closest && evt.target.closest('[data-lc-ai-apply]');
    if (apply) {
      var panel = apply.closest('[data-lc-ai-panel]');
      var sug = panel && panel.querySelector('[data-lc-ai-suggestion]');
      var form = apply.closest('form');
      var ta = form && form.querySelector('textarea[name="body"]');
      if (sug && ta) {
        ta.value = sug.textContent;
        ta.dispatchEvent(new Event('input', { bubbles: true }));
        ta.focus();
      }
      if (panel) panel.innerHTML = '';
      return;
    }
    var dismiss = evt.target.closest && evt.target.closest('[data-lc-ai-dismiss]');
    if (dismiss) {
      var p = dismiss.closest('[data-lc-ai-panel]');
      if (p) p.innerHTML = '';
    }
  });
})();
