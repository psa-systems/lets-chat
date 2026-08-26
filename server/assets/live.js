// LC-822: document-agnostic click delegation. The huddle dock can be MOVED into
// a Document Picture-in-Picture window (huddle_popout.js), and a listener on
// this page's `document` never sees clicks from that window's document. Modules
// whose controls live in the dock (voice/call controls, device picker,
// transcription toggles) register their delegated handler here instead; it is
// attached to this document now and to any document the dock is later moved
// into, so the same `e.target.closest(...)` handlers keep working unchanged.
// Lives in live.js because it is the first script base.html loads.
(function () {
  'use strict';
  var handlers = [];
  var docs = [];
  function attach(doc) {
    if (!doc || docs.indexOf(doc) !== -1) return;
    docs.push(doc);
    handlers.forEach(function (h) { doc.addEventListener('click', h); });
  }
  function detach(doc) {
    var i = docs.indexOf(doc);
    if (i === -1) return;
    docs.splice(i, 1);
    handlers.forEach(function (h) { try { doc.removeEventListener('click', h); } catch (e) {} });
  }
  function onClick(handler) {
    handlers.push(handler);
    docs.forEach(function (d) { d.addEventListener('click', handler); });
  }
  attach(document);
  window.LetsChatDelegate = { onClick: onClick, attach: attach, detach: detach };
})();

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
    // LC-655: close any open sparkle mode menu on an outside click (native
    // <details> only closes on its own summary).
    var openMenus = document.querySelectorAll('details.lc-ai-menu[open]');
    for (var mi = 0; mi < openMenus.length; mi++) {
      if (!openMenus[mi].contains(evt.target)) openMenus[mi].open = false;
    }
    var apply = evt.target.closest && evt.target.closest('[data-lc-ai-apply]');
    if (apply) {
      var panel = apply.closest('[data-lc-ai-panel]');
      // LC-548: the suggested-reply panel renders several chips, each its own
      // apply button carrying its draft; prefer the chip-local suggestion and
      // fall back to the panel-level one (LC-532 single-suggestion assistant).
      var sug =
        (apply.querySelector && apply.querySelector('[data-lc-ai-suggestion]')) ||
        (panel && panel.querySelector('[data-lc-ai-suggestion]'));
      var form = apply.closest('form');
      var ta = form && form.querySelector('textarea[name="body"]');
      if (sug && ta) {
        var original = ta.value;
        var setDraft = function (text) {
          ta.value = text;
          // Caret to the end so the focus-scroll is deterministic.
          var end = ta.value.length;
          try {
            ta.setSelectionRange(end, end);
          } catch (e) {}
          ta.focus();
          // Drives the LC-399 highlight backdrop, autosize, and draft hooks (a
          // programmatic value set fires no native input); the rAF re-sync keeps
          // the highlight backdrop aligned after the autosize settles (LC-653).
          ta.dispatchEvent(new Event('input', { bubbles: true }));
          requestAnimationFrame(function () {
            ta.scrollTop = 0;
            ta.dispatchEvent(new Event('scroll', { bubbles: true }));
          });
        };
        setDraft(sug.textContent);
        // LC-655: replace the preview with an inline "Applied - Undo" affordance
        // (the toast API has no action button) so the user never loses their
        // original draft without a way back. The applied text stays editable.
        var outer = document.getElementById('composer-ai-panel');
        if (outer) {
          outer.innerHTML = '';
          var row = document.createElement('div');
          row.className = 'lc-ai-applied';
          row.setAttribute('role', 'status');
          var msg = document.createElement('span');
          msg.className = 'lc-ai-applied-msg';
          msg.textContent = (window.__lcS && window.__lcS('composeApplied', 'Applied to your message')) || 'Applied to your message';
          var undo = document.createElement('button');
          undo.type = 'button';
          undo.className = 'lc-ai-retry';
          undo.textContent = (window.__lcS && window.__lcS('composeUndo', 'Undo')) || 'Undo';
          undo.addEventListener('click', function () {
            setDraft(original);
            outer.innerHTML = '';
          });
          row.appendChild(msg);
          row.appendChild(undo);
          outer.appendChild(row);
          // Auto-dismiss the affordance (the change persists) unless a new assist
          // has already replaced it.
          window.setTimeout(function () {
            if (outer.firstChild === row) outer.innerHTML = '';
          }, 7000);
        }
      } else if (panel) {
        panel.innerHTML = '';
      }
      return;
    }
    var dismiss = evt.target.closest && evt.target.closest('[data-lc-ai-dismiss]');
    if (dismiss) {
      var p = dismiss.closest('[data-lc-ai-panel]');
      if (p) p.innerHTML = '';
    }
  });

  // LC-692: native <details> flyout menus (the rail Settings gear, the sidebar
  // org switcher) only toggle from their own <summary> - the browser never
  // closes them on an outside click. Any <details data-lc-flyout> opts into the
  // shared dismissal here so every dropdown behaves like the account menu
  // (sidebar_self.html) and the composer AI menu (LC-655): an outside click or
  // Escape collapses it. A click INSIDE the flyout leaves it open, so item
  // links / htmx actions still fire before anything hides them.
  document.body.addEventListener('click', function (evt) {
    var flyouts = document.querySelectorAll('details[data-lc-flyout][open]');
    for (var fi = 0; fi < flyouts.length; fi++) {
      if (!flyouts[fi].contains(evt.target)) flyouts[fi].open = false;
    }
  });
  document.addEventListener('keydown', function (evt) {
    if (evt.key !== 'Escape') return;
    var flyouts = document.querySelectorAll('details[data-lc-flyout][open]');
    for (var fi = 0; fi < flyouts.length; fi++) {
      flyouts[fi].open = false;
      var s = flyouts[fi].querySelector('summary');
      if (s) s.focus(); // return focus to the trigger, matching the account menu
    }
  });

  // LC-694: remember the viewer's last translation target (per browser) so the
  // message Translate picker defaults to it - a frequent user does not re-pick
  // every time. Persist on any target change; pre-select only the picker-only
  // <select> (data-lc-translate-preset), never the translated-state one (whose
  // value reflects the active translation, not the viewer's preference).
  var LC_TRANSLATE_KEY = 'lc-translate-lang';
  document.body.addEventListener('change', function (evt) {
    var sel = evt.target.closest && evt.target.closest('[data-lc-translate-select]');
    if (!sel) return;
    try { localStorage.setItem(LC_TRANSLATE_KEY, sel.value); } catch (e) { /* storage off */ }
  });
  function lcTranslatePreset(root) {
    var scope = root && root.querySelectorAll ? root : document;
    var presets = scope.querySelectorAll('[data-lc-translate-preset]');
    if (!presets.length) return;
    var pref;
    try { pref = localStorage.getItem(LC_TRANSLATE_KEY); } catch (e) { pref = null; }
    // Codes are 2-letter target keys (the allowlist); guard the attribute
    // selector and ignore a stored code no longer offered.
    if (!pref || !/^[a-z]{2}$/.test(pref)) return;
    for (var i = 0; i < presets.length; i++) {
      if (presets[i].querySelector('option[value="' + pref + '"]')) presets[i].value = pref;
    }
  }
  document.body.addEventListener('htmx:load', function (e) { lcTranslatePreset(e.target); });
  document.body.addEventListener('htmx:afterSettle', function (e) { lcTranslatePreset(e.target); });

  // LC-700: huddle dock collapse/expand. Pure UI (no WebRTC), so it lives here
  // rather than in voice.js: flips the root's lc-huddle--collapsed class and the
  // toggle's aria-expanded, and remembers the choice per room across reloads.
  function lcHuddleKey(root) {
    return 'lc-huddle-collapsed:' + (root.getAttribute('data-room-id') || '');
  }
  function lcApplyHuddleCollapsed(root, collapsed) {
    root.classList.toggle('lc-huddle--collapsed', collapsed);
    var btn = root.querySelector('[data-lc-huddle-collapse]');
    if (btn) btn.setAttribute('aria-expanded', collapsed ? 'false' : 'true');
  }
  document.body.addEventListener('click', function (evt) {
    var btn = evt.target.closest && evt.target.closest('[data-lc-huddle-collapse]');
    if (!btn) return;
    var root = btn.closest('[data-lc-huddle]');
    if (!root) return;
    var collapsed = !root.classList.contains('lc-huddle--collapsed');
    lcApplyHuddleCollapsed(root, collapsed);
    try { localStorage.setItem(lcHuddleKey(root), collapsed ? '1' : '0'); } catch (e) { /* storage off */ }
  });
  // LC-820: user-resizable dock. The handle ([data-lc-huddle-resize]) on the
  // dock's top edge drives --lc-huddle-h on the root (the participant area's
  // max-height; the tile size derives from it in main.css). Pointer drag (mouse
  // + touch via pointer events), arrow keys on the focused handle (ARIA window
  // splitter), double-click to reset to the CSS default. Clamped between a
  // usable minimum and 70% of the viewport so the timeline always stays
  // visible; the choice persists per room like the collapse state.
  var LC_HUDDLE_MIN_H = 120;
  var LC_HUDDLE_STEP = 24;
  function lcHuddleHeightKey(root) {
    return 'lc-huddle-height:' + (root.getAttribute('data-room-id') || '');
  }
  function lcHuddleMaxH() {
    return Math.max(LC_HUDDLE_MIN_H, Math.floor(window.innerHeight * 0.7));
  }
  function lcHuddleClamp(h) {
    return Math.min(lcHuddleMaxH(), Math.max(LC_HUDDLE_MIN_H, Math.round(h)));
  }
  // Current effective height: the explicit property if set, else the computed
  // default from the stylesheet (so the first drag starts from where it is).
  function lcHuddleCurrentH(root) {
    var v = parseFloat(root.style.getPropertyValue('--lc-huddle-h'));
    if (v > 0) return v;
    var grid = root.querySelector('[data-lc-voice-grid]');
    var c = grid ? parseFloat(getComputedStyle(grid).maxHeight) : NaN;
    return c > 0 ? c : 224;
  }
  function lcHuddleSyncAria(root) {
    var h = root.querySelector('[data-lc-huddle-resize]');
    if (!h) return;
    h.setAttribute('aria-valuemin', String(LC_HUDDLE_MIN_H));
    h.setAttribute('aria-valuemax', String(lcHuddleMaxH()));
    h.setAttribute('aria-valuenow', String(Math.round(lcHuddleCurrentH(root))));
  }
  function lcApplyHuddleHeight(root, h, persist) {
    if (h == null) {
      root.style.removeProperty('--lc-huddle-h');
      if (persist) { try { localStorage.removeItem(lcHuddleHeightKey(root)); } catch (e) { /* storage off */ } }
    } else {
      h = lcHuddleClamp(h);
      root.style.setProperty('--lc-huddle-h', h + 'px');
      if (persist) { try { localStorage.setItem(lcHuddleHeightKey(root), String(h)); } catch (e) { /* storage off */ } }
    }
    lcHuddleSyncAria(root);
  }
  document.body.addEventListener('pointerdown', function (evt) {
    var handle = evt.target.closest && evt.target.closest('[data-lc-huddle-resize]');
    if (!handle || evt.button !== 0) return;
    var root = handle.closest('[data-lc-huddle]');
    if (!root) return;
    evt.preventDefault();
    var startY = evt.clientY;
    var startH = lcHuddleCurrentH(root);
    var moved = false;
    root.classList.add('lc-huddle--resizing');
    try { handle.setPointerCapture(evt.pointerId); } catch (e) { /* unsupported */ }
    function onMove(e) {
      moved = true;
      // The handle is on the TOP edge: dragging up grows the dock.
      lcApplyHuddleHeight(root, startH + (startY - e.clientY), false);
    }
    function onUp() {
      handle.removeEventListener('pointermove', onMove);
      handle.removeEventListener('pointerup', onUp);
      handle.removeEventListener('pointercancel', onUp);
      root.classList.remove('lc-huddle--resizing');
      if (moved) lcApplyHuddleHeight(root, lcHuddleCurrentH(root), true);
    }
    handle.addEventListener('pointermove', onMove);
    handle.addEventListener('pointerup', onUp);
    handle.addEventListener('pointercancel', onUp);
  });
  document.body.addEventListener('dblclick', function (evt) {
    var handle = evt.target.closest && evt.target.closest('[data-lc-huddle-resize]');
    var root = handle && handle.closest('[data-lc-huddle]');
    if (root) lcApplyHuddleHeight(root, null, true);
  });
  document.body.addEventListener('keydown', function (evt) {
    var handle = evt.target.closest && evt.target.closest('[data-lc-huddle-resize]');
    var root = handle && handle.closest('[data-lc-huddle]');
    if (!root) return;
    var cur = lcHuddleCurrentH(root);
    var step = evt.shiftKey ? LC_HUDDLE_STEP * 4 : LC_HUDDLE_STEP;
    var next = null;
    // Up = taller dock (matches the drag direction), Down = shorter.
    if (evt.key === 'ArrowUp') next = cur + step;
    else if (evt.key === 'ArrowDown') next = cur - step;
    else if (evt.key === 'Home') next = LC_HUDDLE_MIN_H;
    else if (evt.key === 'End') next = lcHuddleMaxH();
    else if (evt.key === 'Escape' || evt.key === 'Backspace') { evt.preventDefault(); lcApplyHuddleHeight(root, null, true); return; }
    if (next == null) return;
    evt.preventDefault();
    lcApplyHuddleHeight(root, next, true);
  });

  function lcRestoreHuddle(scope) {
    var root = scope && scope.querySelectorAll ? scope : document;
    var roots = root.querySelectorAll('[data-lc-huddle]');
    Array.prototype.forEach.call(roots, function (r) {
      var v = null;
      try { v = localStorage.getItem(lcHuddleKey(r)); } catch (e) { v = null; }
      if (v === '1') lcApplyHuddleCollapsed(r, true);
      // LC-820: restore the dock height (re-clamped, in case the viewport is
      // smaller than when it was saved); otherwise just sync the ARIA range.
      var h = null;
      try { h = parseFloat(localStorage.getItem(lcHuddleHeightKey(r))); } catch (e) { h = null; }
      if (h > 0) lcApplyHuddleHeight(r, h, false); else lcHuddleSyncAria(r);
    });
  }
  lcRestoreHuddle(document);
  document.body.addEventListener('htmx:afterSettle', function (e) { lcRestoreHuddle(e.target); });

  // LC-650: uniform "AI is working" feedback. AI actions call a possibly-slow
  // LLM (seconds on a CPU model), and their htmx targets are empty until the
  // response lands, so without this the click feels dead. Any trigger tagged
  // `data-lc-ai-pending` gets a spinner + label injected into its htmx target
  // the moment the request starts; htmx overwrites the target with the real
  // result on completion, which clears it. A "still working" note swaps in
  // after a few seconds for the slow case.
  function lcAiTarget(el) {
    // AI triggers all use a plain `#id` hx-target; skip htmx's extended
    // selectors (closest/find/this) which querySelector can't resolve.
    var sel = el.getAttribute('hx-target');
    if (!sel || sel.indexOf(' ') !== -1 || /^(this|closest|find|next|previous)/.test(sel)) {
      return null;
    }
    try {
      return document.querySelector(sel);
    } catch (e) {
      return null;
    }
  }

  function lcAiSpan(cls, text) {
    var s = document.createElement('span');
    s.className = cls;
    if (text != null) s.textContent = text;
    return s;
  }

  // The default inline spinner + label (translate, compose-assist, replies).
  function lcAiShowPending(target, label) {
    var wrap = document.createElement('div');
    wrap.className = 'lc-ai-pending';
    wrap.setAttribute('role', 'status');
    wrap.setAttribute('aria-live', 'polite');
    var spin = lcAiSpan('lc-spinner', null);
    spin.setAttribute('aria-hidden', 'true');
    wrap.appendChild(spin);
    wrap.appendChild(lcAiSpan('lc-ai-pending-label', label || ''));
    target.innerHTML = '';
    target.appendChild(wrap);
  }

  // LC-654: a summary-shaped skeleton for the AI summary surfaces. The output is
  // a text summary, so previewing its shape (a heading bar, a few summary lines,
  // then a short action-items cluster) reads far better than a bare spinner
  // while the local model works. Widths prefixed "gap " get extra top spacing.
  // Skeleton shapes by surface. "summary" previews a heading + a few lines + a
  // short action-items cluster; "writing" (LC-655) previews a short rewritten
  // message (a few lines). Defaults to summary for any unknown value.
  var LC_AI_SKEL = {
    summary: ['38%', '100%', '92%', '80%', 'gap 34%', '64%'],
    writing: ['96%', '100%', '72%']
  };
  function lcAiShowSkeleton(target, shape, label) {
    var bars = LC_AI_SKEL[shape] || LC_AI_SKEL.summary;
    var wrap = document.createElement('div');
    wrap.className = 'lc-ai-skel';
    wrap.setAttribute('role', 'status');
    wrap.setAttribute('aria-live', 'polite');
    var status = document.createElement('div');
    status.className = 'lc-ai-skel-status';
    var spin = lcAiSpan('lc-spinner', null);
    spin.setAttribute('aria-hidden', 'true');
    status.appendChild(spin);
    status.appendChild(lcAiSpan('lc-ai-pending-label', label || ''));
    wrap.appendChild(status);
    var block = document.createElement('div');
    block.className = 'lc-skel-block';
    block.setAttribute('aria-hidden', 'true');
    bars.forEach(function (w) {
      var gap = w.indexOf('gap ') === 0;
      if (gap) w = w.slice(4);
      var bar = document.createElement('div');
      bar.className = 'lc-skel-bar' + (gap ? ' lc-skel-gap' : '');
      bar.style.width = w;
      block.appendChild(bar);
    });
    wrap.appendChild(block);
    target.innerHTML = '';
    target.appendChild(wrap);
  }

  function lcAiSetLabel(target, text) {
    var lbl = target.querySelector('.lc-ai-pending-label');
    if (lbl && text) lbl.textContent = text;
  }
  function lcAiClearTimers(target) {
    if (target._lcAiTimers) {
      target._lcAiTimers.forEach(function (t) { window.clearTimeout(t); });
      target._lcAiTimers = null;
    }
  }
  // Staged status: swap the label at each {ms, text} step. Honest and
  // time-based (the backend exposes no real phases) - a sense of progress
  // without faking a progress bar.
  function lcAiStage(target, stages) {
    var real = stages.filter(Boolean);
    if (!real.length) return;
    target._lcAiTimers = real.map(function (s) {
      return window.setTimeout(function () { lcAiSetLabel(target, s.text); }, s.ms);
    });
  }

  document.body.addEventListener('htmx:beforeRequest', function (evt) {
    var el = evt.detail && evt.detail.elt;
    if (!el || !el.hasAttribute || !el.hasAttribute('data-lc-ai-pending')) return;
    var target = lcAiTarget(el);
    if (!target) return;
    // Stash the target's current content so a failed request can restore it
    // (some triggers live inside their own target, e.g. the Summarize button;
    // without this a 4xx would leave an empty box and no way to retry).
    target._lcAiOriginal = target.innerHTML;
    // Everything the inline Retry needs to re-fire the same request (LC-654).
    target._lcAiRetry = {
      url: el.getAttribute('hx-post'),
      sel: el.getAttribute('hx-target'),
      skeleton: el.getAttribute('data-lc-ai-skeleton'),
      label: el.getAttribute('data-lc-ai-pending-label'),
      read: el.getAttribute('data-lc-ai-read-label'),
      slow: el.getAttribute('data-lc-ai-slow-label')
    };
    target.setAttribute('aria-busy', 'true');
    var mainLabel = el.getAttribute('data-lc-ai-pending-label') || '';
    var readLabel = el.getAttribute('data-lc-ai-read-label') || '';
    var slowLabel = el.getAttribute('data-lc-ai-slow-label') || '';
    var skeleton = el.getAttribute('data-lc-ai-skeleton');
    if (skeleton) {
      // Progressive: reading/thinking -> summarizing/writing -> (slow if long).
      lcAiShowSkeleton(target, skeleton, readLabel || mainLabel);
      lcAiStage(target, [
        readLabel ? { ms: 1200, text: mainLabel } : null,
        { ms: 4500, text: slowLabel }
      ]);
    } else {
      lcAiShowPending(target, mainLabel);
      lcAiStage(target, [{ ms: 3000, text: slowLabel }]);
    }
  });

  function lcAiShowError(target, r) {
    var wrap = document.createElement('div');
    wrap.className = 'lc-ai-error';
    wrap.setAttribute('role', 'alert');
    var msg = window.__lcS ? window.__lcS('aiFailed', 'AI request failed. Please try again.') : 'AI request failed. Please try again.';
    wrap.appendChild(lcAiSpan('lc-ai-error-msg', msg));
    // The Retry button carries the SAME htmx + data-lc-ai-* attrs as the trigger,
    // so clicking it re-runs through the normal pending/skeleton path.
    var btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'lc-ai-retry flex-none';
    btn.textContent = window.__lcS ? window.__lcS('aiRetry', 'Retry') : 'Retry';
    btn.setAttribute('hx-post', r.url);
    btn.setAttribute('hx-target', r.sel);
    btn.setAttribute('hx-swap', 'innerHTML');
    btn.setAttribute('hx-disabled-elt', 'this');
    btn.setAttribute('data-lc-ai-pending', '');
    if (r.skeleton) btn.setAttribute('data-lc-ai-skeleton', r.skeleton);
    if (r.label) btn.setAttribute('data-lc-ai-pending-label', r.label);
    if (r.read) btn.setAttribute('data-lc-ai-read-label', r.read);
    if (r.slow) btn.setAttribute('data-lc-ai-slow-label', r.slow);
    wrap.appendChild(btn);
    target.innerHTML = '';
    target.appendChild(wrap);
    if (window.htmx && window.htmx.process) window.htmx.process(target);
  }

  document.body.addEventListener('htmx:afterRequest', function (evt) {
    var el = evt.detail && evt.detail.elt;
    if (!el || !el.hasAttribute || !el.hasAttribute('data-lc-ai-pending')) return;
    var target = lcAiTarget(el);
    if (!target) return;
    target.removeAttribute('aria-busy');
    lcAiClearTimers(target);
    // On success htmx has already swapped the real result into the target.
    if (evt.detail && evt.detail.successful) {
      target._lcAiOriginal = null;
      target._lcAiRetry = null;
      return;
    }
    // Failure: htmx does NOT swap a non-2xx body, so the pending block would
    // otherwise spin forever. Skeleton surfaces get an inline error + one-click
    // Retry; the rest restore the original trigger and toast (LC-650).
    var retry = target._lcAiRetry;
    if (el.getAttribute('data-lc-ai-skeleton') && retry && retry.url) {
      lcAiShowError(target, retry);
    } else if (typeof target._lcAiOriginal === 'string') {
      target.innerHTML = target._lcAiOriginal;
      if (window.htmx && window.htmx.process) window.htmx.process(target);
      if (window.__lcToast) {
        window.__lcToast('err', window.__lcS ? window.__lcS('aiFailed', 'AI request failed. Please try again.') : 'AI request failed. Please try again.');
      }
    }
    target._lcAiOriginal = null;
    target._lcAiRetry = null;
  });
})();
