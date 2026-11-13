// LC-1180: make the admin in-call diagnostics panel draggable.
//
// The panel (voice/page.html) is a server-rendered <details> pinned bottom-left
// with no other JS. Pinned, it can overlap page chrome - the report was it sat
// on top of the profile at the foot of the nav rail - and there was no way to
// move it. This adds drag-by-header plus a remembered position, and nothing
// else: a plain click on the header still toggles the <details> open/closed;
// only a real drag (past a small threshold) moves it and suppresses that toggle.
//
// Pointer events cover mouse + touch and work in the macOS <= 12 desktop webview
// (Safari 15). The panel is admin-only and only on the voice-channel page, so
// this no-ops everywhere else.
(function () {
  'use strict';

  var KEY = 'lc-call-diag-pos';
  var DRAG_THRESHOLD = 4; // px of movement before a press becomes a drag

  function clamp(v, min, max) {
    return v < min ? min : (v > max ? max : v);
  }

  // Place the panel at (left, top), clamped so it stays fully on screen. Clears
  // the Tailwind bottom/right pins so left/top win. Returns the applied point.
  function applyPos(panel, left, top) {
    var r = panel.getBoundingClientRect();
    var maxX = Math.max(0, window.innerWidth - r.width);
    var maxY = Math.max(0, window.innerHeight - r.height);
    left = clamp(left, 0, maxX);
    top = clamp(top, 0, maxY);
    panel.style.right = 'auto';
    panel.style.bottom = 'auto';
    panel.style.left = left + 'px';
    panel.style.top = top + 'px';
    return { left: left, top: top };
  }

  function save(pos) {
    try { localStorage.setItem(KEY, JSON.stringify(pos)); } catch (e) { /* private mode */ }
  }
  function load() {
    try {
      var p = JSON.parse(localStorage.getItem(KEY));
      if (p && typeof p.left === 'number' && typeof p.top === 'number') return p;
    } catch (e) { /* absent or blocked */ }
    return null;
  }

  function wire(panel) {
    if (panel.__lcDiagDrag) return; // idempotent across htmx re-settles
    panel.__lcDiagDrag = true;
    var handle = panel.querySelector('summary');
    if (!handle) return;
    handle.style.touchAction = 'none'; // a finger drag on the header moves, not scrolls

    // Restore a remembered position, clamped to the current viewport.
    var saved = load();
    if (saved) applyPos(panel, saved.left, saved.top);

    var startX = 0, startY = 0, originLeft = 0, originTop = 0, dragging = false, pid = null;

    handle.addEventListener('pointerdown', function (e) {
      if (e.button != null && e.button !== 0) return; // primary / touch only
      var r = panel.getBoundingClientRect();
      startX = e.clientX; startY = e.clientY;
      originLeft = r.left; originTop = r.top;
      dragging = false;
      pid = e.pointerId;
      try { if (handle.setPointerCapture) handle.setPointerCapture(e.pointerId); } catch (err) {}
      // Listen on document so the drag survives the pointer leaving the header,
      // whether or not pointer capture took.
      document.addEventListener('pointermove', onMove);
      document.addEventListener('pointerup', onUp);
      document.addEventListener('pointercancel', onUp);
    });

    function onMove(e) {
      var dx = e.clientX - startX, dy = e.clientY - startY;
      if (!dragging && Math.abs(dx) < DRAG_THRESHOLD && Math.abs(dy) < DRAG_THRESHOLD) return;
      if (!dragging) {
        dragging = true;
        panel.setAttribute('data-lc-diag-dragging', '');
      }
      e.preventDefault();
      applyPos(panel, originLeft + dx, originTop + dy);
    }

    function onUp() {
      document.removeEventListener('pointermove', onMove);
      document.removeEventListener('pointerup', onUp);
      document.removeEventListener('pointercancel', onUp);
      try { if (handle.releasePointerCapture && pid != null) handle.releasePointerCapture(pid); } catch (err) {}
      if (dragging) {
        panel.removeAttribute('data-lc-diag-dragging');
        var r = panel.getBoundingClientRect();
        save({ left: r.left, top: r.top });
        // `dragging` stays true until the click below fires so it can be swallowed.
      }
    }

    // The click after a drag would otherwise toggle the <details>. Swallow it
    // only when we actually dragged; a clean click still toggles.
    handle.addEventListener('click', function (e) {
      if (dragging) { e.preventDefault(); dragging = false; }
    });
  }

  function init() {
    var panel = document.querySelector('[data-lc-call-diag-panel]');
    if (panel) wire(panel);
  }

  // One window listener (not one per wired panel): keep a moved panel on screen
  // when the window shrinks under it. A panel still on its CSS pin (never moved,
  // no restored position) has no inline left, so it is left alone.
  window.addEventListener('resize', function () {
    var panel = document.querySelector('[data-lc-call-diag-panel]');
    if (!panel || !panel.style.left) return;
    var r = panel.getBoundingClientRect();
    applyPos(panel, r.left, r.top);
  });

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
  // The voice page can arrive via an htmx swap; re-check after each settle.
  if (document.body) document.body.addEventListener('htmx:afterSettle', init);
})();
