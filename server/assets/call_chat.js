// LC-1127: in-call chat drawer controls, moved out of an inline <script> in
// voice/page.html into this always-loaded external file.
//
// The inline version registered its toggle/close/drag listeners only when the
// inline script ran, which did not survive htmx boosted navigation into the
// voice room, so the Chat button, the close (x), and dragging all went dead
// (LC-1124 follow-up). An external file loads once and its delegated document
// listeners persist across every boosted swap, exactly like the transcript
// toggle (transcribe.js) and the call controls (call.js) already do.
//
// The drawer markup (#lc-call-chat, [data-lc-chat-toggle], [data-lc-chat-close],
// [data-lc-chat-drag], [data-lc-call-chat-form]) still lives in voice/page.html;
// this only wires the behavior. Everything is a no-op on pages without the
// drawer.
(function () {
  'use strict';
  if (typeof document === 'undefined') return;

  function panel() { return document.getElementById('lc-call-chat'); }
  function toggleBtn() { return document.querySelector('[data-lc-chat-toggle]'); }
  function bodyEl() { var p = panel(); return p ? p.querySelector('textarea[name="body"]') : null; }
  // LC-1136: the panel is a `flex` element, so the author `.flex` rule beats the
  // UA `[hidden]` rule and the `hidden` attribute never hides it. Use the Tailwind
  // `hidden` CLASS for visibility instead (what the rest of the app uses).
  function isOpen() { var p = panel(); return !!p && !p.classList.contains('hidden'); }
  function messagesEl() { return document.getElementById('messages'); }
  function setMessagesBottom() { var m = messagesEl(); if (m) m.scrollTop = m.scrollHeight; }

  // LC-1124: drag the panel by its header, clamped to the viewport, position
  // remembered per browser. Dragging switches off the Tailwind bottom/right
  // anchor and drives left/top directly.
  var POS_KEY = 'lc-call-chat-pos';
  function clamp(v, lo, hi) { return Math.max(lo, Math.min(v, hi)); }
  function applyPos(x, y) {
    var p = panel(); if (!p) return;
    var r = p.getBoundingClientRect();
    x = clamp(x, 4, Math.max(4, window.innerWidth - r.width - 4));
    y = clamp(y, 4, Math.max(4, window.innerHeight - r.height - 4));
    p.style.right = 'auto'; p.style.bottom = 'auto';
    p.style.left = x + 'px'; p.style.top = y + 'px';
  }
  function restorePos() {
    try {
      var raw = localStorage.getItem(POS_KEY);
      if (!raw) return;
      var pos = JSON.parse(raw);
      if (pos && isFinite(pos.x) && isFinite(pos.y)) applyPos(pos.x, pos.y);
    } catch (e) { /* storage blocked / malformed: keep the default anchor */ }
  }
  function savePos(x, y) { try { localStorage.setItem(POS_KEY, JSON.stringify({ x: x, y: y })); } catch (e) {} }

  function open() {
    var p = panel(); if (!p) return;
    p.classList.remove('hidden');
    restorePos();
    var b = toggleBtn(); if (b) b.setAttribute('aria-pressed', 'true');
    var t = bodyEl(); if (t) t.focus();
    setMessagesBottom();
  }
  function close() {
    var p = panel(); if (!p) return;
    p.classList.add('hidden');
    var b = toggleBtn(); if (b) b.setAttribute('aria-pressed', 'false');
  }

  // Toggle / close via a delegated click listener.
  document.addEventListener('click', function (e) {
    if (!e.target || !e.target.closest) return;
    if (e.target.closest('[data-lc-chat-toggle]')) { if (isOpen()) close(); else open(); return; }
    if (e.target.closest('[data-lc-chat-close]')) { close(); return; }
  });

  // Enter submits / Shift+Enter newline, scoped to the chat form's body.
  document.addEventListener('keydown', function (e) {
    if (e.key !== 'Enter' || e.shiftKey) return;
    var t = e.target;
    if (!t || t.name !== 'body' || !t.closest) return;
    var form = t.closest('[data-lc-call-chat-form]');
    if (!form) return;
    e.preventDefault();
    form.requestSubmit();
  });

  // Drag by the header.
  var dragging = false, offX = 0, offY = 0;
  document.addEventListener('pointerdown', function (e) {
    if (!e.target || !e.target.closest) return;
    var handle = e.target.closest('[data-lc-chat-drag]');
    var p = panel();
    if (!handle || !p || p.classList.contains('hidden')) return;
    if (e.target.closest('[data-lc-chat-close]')) return; // the close button is not a drag handle
    var r = p.getBoundingClientRect();
    dragging = true; offX = e.clientX - r.left; offY = e.clientY - r.top;
    try { handle.setPointerCapture(e.pointerId); } catch (err) {}
    e.preventDefault();
  });
  document.addEventListener('pointermove', function (e) {
    if (dragging) applyPos(e.clientX - offX, e.clientY - offY);
  });
  document.addEventListener('pointerup', function () {
    if (!dragging) return;
    dragging = false;
    var p = panel(); if (!p) return;
    var r = p.getBoundingClientRect();
    savePos(r.left, r.top);
  });

  // Keep the log pinned to newest while open; close on call end (the toggle is
  // hidden on leave, so an open drawer would otherwise linger into the lobby).
  document.addEventListener('htmx:wsAfterMessage', function () { if (isOpen()) setMessagesBottom(); });
  document.addEventListener('lc:rtc-session-ended', function () { close(); });
})();
