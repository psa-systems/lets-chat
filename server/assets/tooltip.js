// LC-370: lightweight, app-wide tooltip.
//
// One shared, fixed-positioned element driven by `data-lc-tip="text"` on any
// trigger (+ optional `data-lc-tip-pos="top|right|bottom|left"`, default top).
// Fixed positioning so a tooltip escapes any `overflow:auto` ancestor - the
// enclave rail clips horizontally, which a CSS-only `::after` tooltip cannot
// survive. The trigger's `aria-label` stays the accessible name; this layer is
// purely visual, so the tooltip element is aria-hidden.
//
// Shown after a short delay on hover, immediately on keyboard focus. Hidden on
// leave/blur and on scroll/resize/Escape so a stale tooltip never lingers.
(function () {
  'use strict';

  var DELAY_MS = 400;
  var GAP = 8;
  var el = null;
  var timer = 0;
  var current = null;

  function tipEl() {
    if (!el) {
      el = document.createElement('div');
      el.id = 'lc-tooltip';
      el.setAttribute('aria-hidden', 'true');
      document.body.appendChild(el);
    }
    return el;
  }

  function place(trigger) {
    var text = trigger.getAttribute('data-lc-tip');
    if (!text) return;
    var t = tipEl();
    t.textContent = text;
    // Measure off-screen-but-rendered so width/height are real.
    t.style.visibility = 'hidden';
    t.style.display = 'block';
    var r = trigger.getBoundingClientRect();
    var tr = t.getBoundingClientRect();
    var pos = trigger.getAttribute('data-lc-tip-pos') || 'top';
    var x, y;
    if (pos === 'right') { x = r.right + GAP; y = r.top + (r.height - tr.height) / 2; }
    else if (pos === 'left') { x = r.left - tr.width - GAP; y = r.top + (r.height - tr.height) / 2; }
    else if (pos === 'bottom') { x = r.left + (r.width - tr.width) / 2; y = r.bottom + GAP; }
    else { x = r.left + (r.width - tr.width) / 2; y = r.top - tr.height - GAP; } // top
    // Clamp into the viewport.
    x = Math.max(4, Math.min(x, window.innerWidth - tr.width - 4));
    y = Math.max(4, Math.min(y, window.innerHeight - tr.height - 4));
    t.style.left = Math.round(x) + 'px';
    t.style.top = Math.round(y) + 'px';
    t.style.visibility = 'visible';
    t.classList.add('lc-tooltip-visible');
  }

  function hide() {
    if (timer) { clearTimeout(timer); timer = 0; }
    current = null;
    if (el) { el.classList.remove('lc-tooltip-visible'); el.style.display = 'none'; }
  }

  function closestTip(node) {
    return node && node.closest ? node.closest('[data-lc-tip]') : null;
  }

  document.addEventListener('mouseover', function (e) {
    var trigger = closestTip(e.target);
    if (!trigger || trigger === current) return;
    current = trigger;
    if (timer) clearTimeout(timer);
    timer = setTimeout(function () { if (current === trigger) place(trigger); }, DELAY_MS);
  });
  document.addEventListener('mouseout', function (e) {
    if (closestTip(e.target) === current && current) hide();
  });
  // Keyboard focus: show immediately (no hover delay), matching native title-ish
  // expectations for tab navigation.
  document.addEventListener('focusin', function (e) {
    var trigger = closestTip(e.target);
    if (trigger) { current = trigger; place(trigger); }
  });
  document.addEventListener('focusout', function (e) {
    if (closestTip(e.target) === current && current) hide();
  });
  window.addEventListener('scroll', hide, true);
  window.addEventListener('resize', hide);
  document.addEventListener('keydown', function (e) { if (e.key === 'Escape') hide(); });
})();
