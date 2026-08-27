// LC-825: floating reactions in a call (huddle dock, its pop-out hosts, and
// the enclave voice page), mirroring Google Meet.
//
// A smiley toggle in the call bar opens a quick-tray of the user's top emoji
// (the template's eight, reordered from /api/reactions/frequent - the same
// signal the message quick-react bar uses); More loads the EXISTING composer
// emoji picker fragment into the tray (no second picker). Picking one sends a
// `voice_reaction` frame over the chat socket. The server gates it to call
// participants, validates the glyph, rate-limits per user, and fans it back to
// the participants (sender included) over the voice bus as kind="reaction";
// voice.js hands that to `receive` with the sender's roster name.
//
// Every reaction is ephemeral: a <span> spawned into the stage's reaction
// layer, animated in CSS (transform + opacity only, randomised custom
// properties for lane / sway / duration / delay), removed on animationend (with
// a timeout backstop); a short-lived name badge on the sender's tile; and a
// throttled, coalesced aria-live announcement. Nothing is stored anywhere. A
// concurrency cap drops extras so a spam burst cannot fill the DOM.
//
// The tray and layer live inside the voice root, so they follow the dock into
// a pop-out window (LC-821); clicks are delegated through LetsChatDelegate so
// they work there too, and the keyboard handler is bound on the tray element
// itself so it travels with it.
//
// The pure helpers are exported via module.exports for call_reactions.test.js.
(function () {
  'use strict';

  var DEFAULTS = ['👍', '🎉', '❤️', '😂', '😮', '😢', '🤔', '👏'];
  var TRAY_CAP = 8;      // cells in the quick row
  var MAX_LIVE = 24;     // floats animating at once; extras are dropped
  var LANES = 8;         // start-x lanes across the bottom band
  var ANNOUNCE_MS = 2000; // at most one live-region announcement per window
  var SWEEP_MS = 6000;   // backstop removal if animationend never fires
  // LC-827: the device-local recent-reaction MRU, the SAME key the message
  // quick-react bar records and reads (reactions.js), so a call reaction and a
  // message reaction share one "recently used" history.
  var RECENT_KEY = 'lc-react-recent';
  var RECENT_MAX = 16;
  // Some emoji render text-style unless followed by VS-16 (U+FE0F). The picker
  // grid emits the qualified form while an older MRU entry may hold the bare
  // code point; canonicalise so the two dedupe (mirrors layout.html, LC-390).
  var VS16 = '❤✊✋✌☝☺☹☕☘✂✈⚠♠♣♥♦';
  function canon(g) {
    if (typeof g !== 'string' || !g) return g;
    if (g.length === 1 && VS16.indexOf(g) !== -1) return g + '️';
    return g;
  }

  // ---- pure helpers (unit-tested) --------------------------------------
  // Quick-row order: the caller's preferred list first (LC-827: device-local
  // recents, then the cross-device frequent seed), then the defaults,
  // canonicalised, de-duplicated, capped.
  function trayOrder(preferred, defaults, cap) {
    var out = [];
    (preferred || []).concat(defaults || []).forEach(function (raw) {
      var e = canon(raw);
      if (typeof e === 'string' && e && out.indexOf(e) === -1 && out.length < cap) out.push(e);
    });
    return out;
  }
  // Client-side sanity for one emoji (mirrors the server's validate_reaction_emoji):
  // non-empty, bounded, and no ASCII at all - so text, markup, and custom
  // :shortcodes: never leave the client.
  function isLikelyEmoji(s) {
    if (typeof s !== 'string') return false;
    s = s.trim();
    if (!s || s.length > 32) return false;
    for (var i = 0; i < s.length; i++) {
      if (s.charCodeAt(i) < 128) return false;
    }
    return true;
  }
  // Start x (%) for the n-th float: round-robin across lanes plus a jitter in
  // [-1, 1] lane-widths * 0.6, clamped inside the band so nothing clips.
  function laneX(index, lanes, jitter) {
    var w = 100 / lanes;
    var base = w * (index % lanes) + w / 2;
    return Math.max(6, Math.min(94, base + (jitter || 0) * w * 0.6));
  }
  // One announcement for a batch: "{name} reacted {emoji}" or "{name} and {n}
  // others reacted {emoji}" (the batch's most common emoji).
  function coalesce(items, labels) {
    if (!items || !items.length) return '';
    var first = items[0];
    if (items.length === 1) {
      return labels.one.replace('%name%', first.name).replace('%emoji%', first.emoji);
    }
    var counts = {}, best = first.emoji, bestN = 0;
    items.forEach(function (it) {
      counts[it.emoji] = (counts[it.emoji] || 0) + 1;
      if (counts[it.emoji] > bestN) { bestN = counts[it.emoji]; best = it.emoji; }
    });
    return labels.many
      .replace('%name%', first.name)
      .replace('%n%', String(items.length - 1))
      .replace('%emoji%', best);
  }

  var api = { trayOrder: trayOrder, isLikelyEmoji: isLikelyEmoji, laneX: laneX, coalesce: coalesce, canon: canon, DEFAULTS: DEFAULTS, MAX_LIVE: MAX_LIVE };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  if (typeof window === 'undefined' || typeof document === 'undefined') return;

  // ---- DOM ------------------------------------------------------------
  function S(k, fb) { return window.__lcS ? window.__lcS(k, fb) : fb; }
  function cssEscape(s) { return String(s).replace(/["\\]/g, '\\$&'); }
  function rootOf(el) { return el && el.closest ? el.closest('[data-lc-voice-root]') : null; }
  function trayOf(root) { return root ? root.querySelector('[data-lc-react-tray]') : null; }
  function toggleOf(root) { return root ? root.querySelector('[data-lc-voice-react]') : null; }
  function isOpen(root) { var t = trayOf(root); return !!(t && !t.hidden); }

  var laneIdx = 0;
  var frequentPromise = null;
  var frequentList = null; // resolved value of frequentPromise, once known
  function fetchFrequent() {
    if (!frequentPromise) {
      frequentPromise = fetch('/api/reactions/frequent', { credentials: 'same-origin' })
        .then(function (r) { return r.ok ? r.json() : []; })
        .catch(function () { return []; })
        .then(function (list) { frequentList = Array.isArray(list) ? list : []; return frequentList; });
    }
    return frequentPromise;
  }
  function readRecent() {
    try {
      var a = JSON.parse(localStorage.getItem(RECENT_KEY) || '[]');
      return Array.isArray(a) ? a : [];
    } catch (e) { return []; }
  }
  function recordRecent(glyph) {
    glyph = canon((glyph || '').trim());
    if (!glyph || glyph.length > 6) return;
    var arr = readRecent().map(canon).filter(function (g) { return g !== glyph; });
    arr.unshift(glyph);
    try { localStorage.setItem(RECENT_KEY, JSON.stringify(arr.slice(0, RECENT_MAX))); } catch (e) { /* storage off */ }
  }
  // Rebuild the quick row on EVERY open (LC-827; it used to be seeded once per
  // root from the frequent list alone, so the emoji just used never appeared):
  // device-local recents first, so the row changes the instant you react; then
  // the cross-device frequent seed; then the defaults to fill the row.
  function renderRow(root) {
    var row = root.querySelector('[data-lc-react-row]');
    if (!row) return;
    var preferred = readRecent().concat(frequentList || []).filter(isLikelyEmoji);
    var order = trayOrder(preferred, DEFAULTS, TRAY_CAP);
    var more = row.querySelector('[data-lc-react-more]');
    Array.prototype.forEach.call(row.querySelectorAll('[data-lc-react-emoji]'), function (c) { c.remove(); });
    var doc = row.ownerDocument;
    order.forEach(function (g) {
      var b = doc.createElement('button');
      b.type = 'button';
      b.setAttribute('data-lc-react-emoji', g);
      b.className = 'lc-emoji-cell';
      b.setAttribute('tabindex', '-1');
      b.textContent = g;
      row.insertBefore(b, more || null);
    });
  }

  // ---- tray -------------------------------------------------------------
  function cellsOf(root) {
    var tray = trayOf(root);
    return tray ? Array.prototype.slice.call(tray.querySelectorAll('[data-lc-react-row] [data-lc-react-emoji], [data-lc-react-more]')) : [];
  }
  function focusCell(root, idx) {
    var cells = cellsOf(root);
    if (!cells.length) return;
    idx = (idx + cells.length) % cells.length;
    cells.forEach(function (c, i) { c.setAttribute('tabindex', i === idx ? '0' : '-1'); });
    try { cells[idx].focus(); } catch (e) {}
  }
  function placeTray(root) {
    var tray = trayOf(root), wrap = root.querySelector('[data-lc-react-wrap]');
    if (!tray || !wrap) return;
    var win = wrap.ownerDocument.defaultView || window;
    var r = wrap.getBoundingClientRect();
    // Above by default; below when there is no headroom (a bar at the top of
    // its window: the voice page, or a pop-out). Then keep it inside the window
    // horizontally.
    tray.classList.toggle('lc-react-tray--below', r.top < 260);
    tray.classList.remove('lc-react-tray--left');
    var tr = tray.getBoundingClientRect();
    if (tr.left < 4) tray.classList.add('lc-react-tray--left');
  }
  function onTrayKey(e) {
    var tray = e.currentTarget, root = rootOf(tray);
    if (!root) return;
    var cells = cellsOf(root);
    var cur = cells.indexOf(e.target);
    if (e.key === 'Escape') { e.preventDefault(); closeTray(root, true); return; }
    if (cur === -1) return; // keys inside the full picker (filter input) are its own
    if (e.key === 'ArrowRight' || e.key === 'ArrowDown') { e.preventDefault(); focusCell(root, cur + 1); }
    else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') { e.preventDefault(); focusCell(root, cur - 1); }
    else if (e.key === 'Home') { e.preventDefault(); focusCell(root, 0); }
    else if (e.key === 'End') { e.preventDefault(); focusCell(root, cells.length - 1); }
  }
  function openTray(root) {
    var tray = trayOf(root), btn = toggleOf(root);
    if (!tray) return;
    renderRow(root);
    tray.hidden = false;
    if (btn) btn.setAttribute('aria-expanded', 'true');
    if (!tray.__lcReactKeys) { tray.__lcReactKeys = true; tray.addEventListener('keydown', onTrayKey); }
    placeTray(root);
    focusCell(root, 0);
    // The cross-device frequent seed arrives once per page; fold it in when it
    // lands the first time (later opens already rendered with it above).
    if (frequentList === null) {
      fetchFrequent().then(function () {
        if (isOpen(root)) { renderRow(root); focusCell(root, 0); }
      });
    }
  }
  function closeTray(root, refocus) {
    var tray = trayOf(root), btn = toggleOf(root);
    if (!tray || tray.hidden) return;
    tray.hidden = true;
    if (btn) {
      btn.setAttribute('aria-expanded', 'false');
      if (refocus) { try { btn.focus(); } catch (e) {} }
    }
  }
  function closeAllIn(doc) {
    var open = doc.querySelectorAll('[data-lc-react-tray]:not([hidden])');
    for (var i = 0; i < open.length; i++) closeTray(rootOf(open[i]), false);
  }

  // More: load the composer's picker fragment (the one emoji picker in the
  // app) into the slot, re-pointed at reactions: its insert hooks become
  // reaction cells (so the composer's insert handler never sees them), custom
  // :shortcode: emoji are dropped (v1 sends Unicode only), and the tabs +
  // filter are wired here since reactions.js only wires its own hosts.
  function loadMore(root) {
    var tray = trayOf(root), more = root.querySelector('[data-lc-react-more]');
    var slot = tray && tray.querySelector('[data-lc-react-picker-slot]');
    if (!slot) return;
    if (slot.childElementCount) {
      var show = slot.hidden;
      slot.hidden = !show;
      tray.classList.toggle('lc-react-tray--picker', show);
      if (more) more.setAttribute('aria-expanded', show ? 'true' : 'false');
      placeTray(root);
      return;
    }
    var roomId = root.getAttribute('data-room-id');
    fetch('/rooms/' + encodeURIComponent(roomId) + '/emoji-picker', { credentials: 'same-origin' })
      .then(function (r) { return r.ok ? r.text() : ''; })
      .then(function (html) {
        if (!html) return;
        html = html
          .replace(/data-lc-emoji-insert=/g, 'data-lc-react-emoji=')
          .replace(/data-lc-emoji-picker-panel/g, 'data-lc-react-picker')
          .replace(/id="picker-composer"/g, 'id="picker-react"')
          .replace(/\sautofocus\b/g, '');
        slot.innerHTML = html;
        slot.hidden = false;
        // LC-827: the fragment's root is the composer's floating panel
        // (`absolute bottom-full w-72`, positioned against its slot), which
        // inside the tray popped OUT above it as a clipped strip. Make it flow
        // in place at the tray's width; main.css does the same in CSS as a
        // belt-and-braces for the positioning utilities.
        var panel = slot.querySelector('[data-lc-react-picker]');
        if (panel) {
          panel.classList.remove('absolute', 'bottom-full', 'left-0', 'mb-1', 'z-30', 'w-72', 'shadow-lg');
          panel.classList.add('w-full');
        }
        tray.classList.add('lc-react-tray--picker');
        if (more) more.setAttribute('aria-expanded', 'true');
        Array.prototype.forEach.call(slot.querySelectorAll('[data-lc-react-emoji^=":"]'), function (c) { c.remove(); });
        var sections = slot.querySelectorAll('[data-lc-emoji-cat]');
        var tabs = slot.querySelectorAll('[data-lc-emoji-tab]');
        function showCat(slug) {
          Array.prototype.forEach.call(sections, function (s) {
            s.style.display = (!slug || s.getAttribute('data-lc-emoji-cat') === slug) ? '' : 'none';
          });
          Array.prototype.forEach.call(tabs, function (t) {
            t.classList.toggle('is-active', t.getAttribute('data-lc-emoji-tab') === slug);
          });
        }
        Array.prototype.forEach.call(tabs, function (t) {
          t.addEventListener('click', function () { showCat(t.getAttribute('data-lc-emoji-tab')); });
        });
        if (tabs.length) showCat(tabs[0].getAttribute('data-lc-emoji-tab'));
        var filter = slot.querySelector('[data-lc-emoji-filter]');
        if (filter) {
          filter.addEventListener('input', function () {
            var qv = filter.value.trim().toLowerCase();
            if (qv) showCat(null); else if (tabs.length) showCat(tabs[0].getAttribute('data-lc-emoji-tab'));
            Array.prototype.forEach.call(slot.querySelectorAll('[data-lc-react-emoji]'), function (c) {
              var nm = (c.getAttribute('data-lc-emoji-name') || '').toLowerCase();
              c.classList.toggle('lc-react-cell-hidden', !!qv && nm.indexOf(qv) === -1);
            });
          });
          try { filter.focus(); } catch (e) {}
        }
        placeTray(root);
      })
      .catch(function () {});
  }

  // ---- send / receive --------------------------------------------------
  function send(root, emoji) {
    var sock = window.__lcWS;
    var roomId = parseInt(root.getAttribute('data-room-id'), 10);
    if (!sock || !roomId || !isLikelyEmoji(emoji)) return;
    try { sock.send(JSON.stringify({ type: 'voice_reaction', room_id: roomId, emoji: emoji.trim() })); } catch (e) { /* reconnecting */ }
    recordRecent(emoji); // LC-827: surfaces first in the row on the next open
  }
  function spawn(root, emoji) {
    var layer = root.querySelector('[data-lc-react-layer]');
    if (!layer || layer.childElementCount >= MAX_LIVE) return;
    var doc = layer.ownerDocument;
    var el = doc.createElement('span');
    el.className = 'lc-react-float';
    var g = doc.createElement('span');
    g.className = 'lc-react-glyph';
    g.textContent = emoji;
    el.appendChild(g);
    el.style.setProperty('--lc-rx', laneX(laneIdx++, LANES, Math.random() * 2 - 1).toFixed(1) + '%');
    el.style.setProperty('--lc-amp', (12 + Math.random() * 22).toFixed(0) + 'px');
    el.style.setProperty('--lc-dur', (3000 + Math.random() * 1000).toFixed(0) + 'ms');
    el.style.setProperty('--lc-delay', (Math.random() * 150).toFixed(0) + 'ms');
    el.style.setProperty('--lc-rot', ((Math.random() * 16) - 8).toFixed(1) + 'deg');
    var done = false;
    function rm() { if (done) return; done = true; if (el.parentNode) el.parentNode.removeChild(el); }
    el.addEventListener('animationend', function (ev) { if (ev.target === el) rm(); });
    setTimeout(rm, SWEEP_MS);
    layer.appendChild(el);
  }
  function badge(root, userId, emoji, name) {
    var tile = root.querySelector('[data-lc-voice-tile="' + cssEscape(userId) + '"]');
    if (!tile) return;
    var old = tile.querySelector('.lc-react-badge');
    if (old) old.remove();
    var doc = tile.ownerDocument;
    var b = doc.createElement('span');
    b.className = 'lc-react-badge';
    b.setAttribute('aria-hidden', 'true');
    var g = doc.createElement('span');
    g.textContent = emoji;
    var n = doc.createElement('span');
    n.className = 'lc-react-badge-name';
    n.textContent = name;
    b.appendChild(g);
    b.appendChild(n);
    var done = false;
    function rm() { if (done) return; done = true; if (b.parentNode) b.parentNode.removeChild(b); }
    b.addEventListener('animationend', function (ev) { if (ev.target === b) rm(); });
    setTimeout(rm, 3500);
    tile.appendChild(b);
  }
  // Polite announcements, at most one per ANNOUNCE_MS, coalescing a burst.
  var live = typeof WeakMap !== 'undefined' ? new WeakMap() : null;
  function announce(root, name, emoji) {
    var region = root.querySelector('[data-lc-react-live]');
    if (!region || !live) return;
    var st = live.get(root);
    if (!st) { st = { items: [], timer: null }; live.set(root, st); }
    st.items.push({ name: name, emoji: emoji });
    if (st.timer) return; // flushed when the window elapses
    flush();
    function flush() {
      if (!st.items.length || !region.isConnected) { st.timer = null; st.items = []; return; }
      var text = coalesce(st.items, {
        one: S('reactAnnounce', '%name% reacted %emoji%'),
        many: S('reactAnnounceMany', '%name% and %n% others reacted %emoji%'),
      });
      st.items = [];
      region.textContent = '';
      region.textContent = text;
      st.timer = setTimeout(flush, ANNOUNCE_MS);
    }
  }
  function receive(root, userId, name, emoji) {
    if (!root || !isLikelyEmoji(emoji)) return;
    emoji = emoji.trim();
    spawn(root, emoji);
    badge(root, userId, emoji, name || '');
    announce(root, name || '', emoji);
  }
  // Leave / teardown: nothing may linger into the lobby or the next call.
  function reset(root) {
    if (!root) return;
    closeTray(root, false);
    var layer = root.querySelector('[data-lc-react-layer]');
    if (layer) layer.replaceChildren();
    Array.prototype.forEach.call(root.querySelectorAll('.lc-react-badge'), function (b) { b.remove(); });
    var region = root.querySelector('[data-lc-react-live]');
    if (region) region.textContent = '';
    if (live) {
      var st = live.get(root);
      if (st) { if (st.timer) clearTimeout(st.timer); live.delete(root); }
    }
  }

  // ---- wiring -----------------------------------------------------------
  var reg = window.LetsChatDelegate
    ? window.LetsChatDelegate.onClick
    : function (fn) { document.addEventListener('click', fn); };
  reg(function (e) {
    var t = e.target;
    if (!t || !t.closest) return;
    var btn = t.closest('[data-lc-voice-react]');
    if (btn) {
      var r = rootOf(btn);
      if (!r) return;
      if (isOpen(r)) closeTray(r, false); else openTray(r);
      return;
    }
    var cell = t.closest('[data-lc-react-emoji]');
    if (cell) {
      var r2 = rootOf(cell);
      if (!r2) return;
      send(r2, cell.getAttribute('data-lc-react-emoji') || '');
      closeTray(r2, true);
      return;
    }
    var more = t.closest('[data-lc-react-more]');
    if (more) {
      var r3 = rootOf(more);
      if (r3) loadMore(r3);
      return;
    }
    if (t.closest('[data-lc-react-wrap]')) return; // tabs / filter inside the tray
    closeAllIn(t.ownerDocument || document);
  });

  window.LetsChatCallReactions = { receive: receive, reset: reset, spawn: spawn };
})();
