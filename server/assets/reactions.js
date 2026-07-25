// LC-553: reaction picker runtime. The reaction bar + get_picker route were
// authored around a shared floating picker host (#lc-reaction-popover) and a
// set of data hooks (tabs / filter / recent / close), but the script that made
// them work was never in the tree - so the picker could not open and reacting
// via the UI did nothing. This restores that behavior: open the picker into the
// fixed host, position it by the trigger, switch category tabs, filter by name,
// keep a recent-emoji MRU, and close on react / Escape / outside-click.
(function () {
  'use strict';
  var HOST_ID = 'lc-reaction-popover';
  var RECENT_KEY = 'lc-react-recent';
  var RECENT_MAX = 16;
  var lastTrigger = null;

  function host() { return document.getElementById(HOST_ID); }
  function isOpen() { var h = host(); return h && !h.classList.contains('hidden'); }
  function close() {
    var h = host();
    if (!h) return;
    h.classList.add('hidden');
    h.replaceChildren();
    if (lastTrigger && lastTrigger.focus) { try { lastTrigger.focus(); } catch (e) {} }
    lastTrigger = null;
  }

  // Remember which [data-lc-reaction-open] opened the picker (capture phase, so
  // it is recorded before htmx fires the hx-get that swaps the host).
  document.addEventListener('click', function (e) {
    var t = e.target.closest && e.target.closest('[data-lc-reaction-open]');
    if (t) lastTrigger = t;
  }, true);

  // Position the fixed host near the trigger: prefer above and right-aligned,
  // flip below when there is not enough room, and clamp into the viewport.
  function position() {
    var h = host();
    if (!h || !lastTrigger) return;
    var card = h.firstElementChild;
    if (!card) return;
    var vw = window.innerWidth, vh = window.innerHeight, m = 8;
    // LC-598 follow-up: cap the picker to the viewport so a tall card opened near
    // a screen edge is never clipped off-screen with no way to reach the hidden
    // rows. Shrink the inner emoji grid (the scrollable part) rather than the whole
    // card, so the filter box and category tabs stay put and only the grid scrolls.
    // Clear the previous clamp first so a resize back to a tall window restores the
    // natural height before re-measuring.
    var grid = card.querySelector('[data-lc-emoji-grid]');
    if (grid) grid.style.maxHeight = '';
    var overflow = card.offsetHeight - (vh - 2 * m);
    if (grid && overflow > 0) {
      grid.style.maxHeight = Math.max(96, grid.offsetHeight - overflow) + 'px';
    }
    var tr = lastTrigger.getBoundingClientRect();
    var cw = card.offsetWidth, ch = card.offsetHeight;
    var top = tr.top - ch - 6;
    if (top < m) top = tr.bottom + 6;
    if (top + ch > vh - m) top = Math.max(m, vh - ch - m);
    var left = tr.right - cw;
    if (left < m) left = m;
    if (left + cw > vw - m) left = Math.max(m, vw - cw - m);
    h.style.top = top + 'px';
    h.style.left = left + 'px';
  }

  // Recent MRU: stored as an array of glyph strings; custom (image) emojis are
  // skipped (their button text is empty) to keep the row Unicode-only.
  function readRecent() {
    try { return JSON.parse(localStorage.getItem(RECENT_KEY) || '[]'); } catch (e) { return []; }
  }
  function recordRecent(btn) {
    var glyph = (btn.textContent || '').trim();
    if (!glyph || glyph.length > 6) return; // skip empty / image buttons
    var arr = readRecent().filter(function (g) { return g !== glyph; });
    arr.unshift(glyph);
    arr = arr.slice(0, RECENT_MAX);
    try { localStorage.setItem(RECENT_KEY, JSON.stringify(arr)); } catch (e) {}
  }
  function renderRecent(card) {
    var row = card.querySelector('[data-lc-emoji-recent]');
    if (!row) return;
    var arr = readRecent();
    if (!arr.length) { row.classList.add('hidden'); return; }
    var cells = card.querySelectorAll('[data-lc-emoji-name]');
    row.replaceChildren();
    var label = document.createElement('span');
    label.textContent = row.getAttribute('data-lc-recent-label') || '';
    label.className = 'mr-1';
    row.appendChild(label);
    var added = 0;
    arr.forEach(function (g) {
      var src = Array.prototype.find.call(cells, function (c) { return c.textContent.trim() === g; });
      if (!src) return;
      row.appendChild(src.cloneNode(true)); // carries hx-post/hx-target
      added++;
    });
    if (!added) { row.classList.add('hidden'); return; }
    if (window.htmx) window.htmx.process(row); // wire the cloned buttons
    row.classList.remove('hidden');
  }

  function initPicker(h) {
    var card = h.querySelector('[id^="picker-"]');
    if (!card) return;
    var sections = card.querySelectorAll('[data-lc-emoji-cat]');
    var tabs = card.querySelectorAll('[data-lc-emoji-tab]');
    var grid = card.querySelector('[data-lc-emoji-grid]');

    function showCat(slug) {
      sections.forEach(function (s) {
        s.style.display = (s.getAttribute('data-lc-emoji-cat') === slug) ? '' : 'none';
      });
      tabs.forEach(function (t) {
        t.classList.toggle('is-active', t.getAttribute('data-lc-emoji-tab') === slug);
      });
      if (grid) grid.scrollTop = 0;
    }
    if (tabs.length) {
      tabs.forEach(function (t) {
        t.addEventListener('click', function () { showCat(t.getAttribute('data-lc-emoji-tab')); });
      });
      showCat(tabs[0].getAttribute('data-lc-emoji-tab'));
    }

    renderRecent(card);

    var filter = card.querySelector('[data-lc-emoji-filter]');
    if (filter) {
      filter.addEventListener('input', function () {
        var q = filter.value.trim().toLowerCase();
        var cells = card.querySelectorAll('[data-lc-emoji-name]');
        if (!q) {
          // back to tab mode
          var active = card.querySelector('[data-lc-emoji-tab].is-active');
          cells.forEach(function (c) { c.style.display = ''; });
          if (tabs.length) showCat((active || tabs[0]).getAttribute('data-lc-emoji-tab'));
          return;
        }
        // filtering spans all categories: reveal every section, hide non-matches
        sections.forEach(function (s) { s.style.display = ''; });
        cells.forEach(function (c) {
          var n = (c.getAttribute('data-lc-emoji-name') || '').toLowerCase();
          c.style.display = n.indexOf(q) >= 0 ? '' : 'none';
        });
      });
    }

    var x = card.querySelector('[data-lc-reaction-close]');
    if (x) x.addEventListener('click', close);

    // React + close. htmx fires the hx-post on this same click; defer the close
    // so the request is already dispatched before the host is emptied.
    card.addEventListener('click', function (e) {
      var b = e.target.closest && e.target.closest('button[hx-post]');
      if (!b) return;
      recordRecent(b);
      setTimeout(close, 0);
    });
  }

  // ── Quick-react bar ───────────────────────────────────────────────────────
  // LC-553: every message row carries a `data-lc-quick-bar` placeholder that was
  // never populated (it shipped `hidden` with no filler), so the one-tap row
  // never appeared. Seed it from the caller's frequency-ranked emoji
  // (/api/reactions/frequent, cross-device) merged with the device-local MRU,
  // topped up with sensible defaults, and fill each row lazily on first hover.
  var QCAP = 5;
  var QUICK_DEFAULTS = ['👍', '🎉', '❤️', '😄', '👀'];
  var frequentSeed = null;
  function seedFrequent() {
    if (frequentSeed) return frequentSeed;
    frequentSeed = fetch('/api/reactions/frequent', { headers: { accept: 'application/json' } })
      .then(function (r) { return r.ok ? r.json() : []; })
      .catch(function () { return []; });
    return frequentSeed;
  }
  function quickList(freq) {
    var out = [];
    function push(g) { if (g && out.indexOf(g) < 0) out.push(g); }
    (Array.isArray(freq) ? freq : []).forEach(push);
    readRecent().forEach(push);
    QUICK_DEFAULTS.forEach(push);
    return out.slice(0, QCAP);
  }
  function fillQuick(bar) {
    if (bar.getAttribute('data-lc-quick-filled') === '1') return;
    var row = bar.closest('[id^="msg-"]');
    if (!row) return;
    var mid = row.id.replace('msg-', '');
    if (!mid) return;
    bar.setAttribute('data-lc-quick-filled', '1');
    seedFrequent().then(function (freq) {
      var list = quickList(freq);
      if (!list.length) return;
      bar.replaceChildren();
      list.forEach(function (g) {
        var b = document.createElement('button');
        b.type = 'button';
        b.className = 'lc-msg-action';
        b.setAttribute('hx-post', '/messages/' + mid + '/reactions/' + encodeURIComponent(g));
        b.setAttribute('hx-target', '#reactions-' + mid);
        b.setAttribute('hx-swap', 'outerHTML');
        b.setAttribute('aria-label', g);
        b.textContent = g;
        bar.appendChild(b);
      });
      if (window.htmx) window.htmx.process(bar);
      bar.classList.remove('hidden');
      bar.classList.add('inline-flex');
    });
  }
  function quickFromEvent(e) {
    var row = e.target.closest && e.target.closest('[id^="msg-"]');
    if (!row) return;
    var q = row.querySelector('[data-lc-quick-bar]');
    if (q) fillQuick(q);
  }
  document.addEventListener('pointerover', quickFromEvent, true);
  document.addEventListener('focusin', quickFromEvent, true);
  // A one-tap quick react counts toward the recent MRU too.
  document.addEventListener('click', function (e) {
    var b = e.target.closest && e.target.closest('[data-lc-quick-bar] button[hx-post]');
    if (b) recordRecent(b);
  }, true);

  // Reveal + position + wire the picker once htmx swaps it into the host.
  document.body.addEventListener('htmx:afterSwap', function (e) {
    if (!e.target || e.target.id !== HOST_ID) return;
    var h = host();
    if (!h || !h.firstElementChild) return;
    h.classList.remove('hidden');
    position();
    initPicker(h);
    var f = h.querySelector('[data-lc-emoji-filter]');
    if (f) f.focus();
  });

  // Close on outside pointerdown (but not on a trigger - htmx reopens it) and Escape.
  document.addEventListener('pointerdown', function (e) {
    if (!isOpen()) return;
    var h = host();
    if (h.contains(e.target)) return;
    if (e.target.closest && e.target.closest('[data-lc-reaction-open]')) return;
    close();
  });
  document.addEventListener('keydown', function (e) {
    if (e.key === 'Escape' && isOpen()) { e.stopPropagation(); close(); }
  });
  // Reposition a still-open picker if the timeline scrolls under it.
  window.addEventListener('scroll', function () { if (isOpen()) position(); }, true);
})();
