// LC-821 / LC-822 / LC-823: pop the huddle dock out of the room foot.
//
// The dock (room/huddle.html, driven by voice.js) is bound to the room page:
// leaving the room ends the call, and nothing keeps the huddle in view while
// reading another room. This module lifts the LIVE dock element out of the page
// without touching the media session:
//  - Chromium: Document Picture-in-Picture. requestWindow() gives an always-on-
//    top window whose document shares this page's JS context, so the dock is
//    simply MOVED into it: the <video> elements keep playing, voice.js keeps
//    its element references, the WebSocket signalling is untouched. This is
//    how Discord and Meet implement pop-out.
//  - Otherwise (Firefox, Safari): an in-page floating panel, fixed over the
//    chat, draggable by its bar, corner-resizable (native CSS resize), with
//    its geometry remembered per room.
// While popped out the root lives outside #main, so a page swap no longer
// tears the huddle down: voice.js scan() asks isPopped() and hands the page's
// own freshly rendered dock to adoptPageDock(), which turns it into a "bring
// back" placeholder for the same room, or a busy note for another room (one
// voice session per tab). Delegated click handlers reach the pop-out window
// through LetsChatDelegate (live.js), which attaches them to any document the
// dock is moved into.
(function () {
  'use strict';

  var st = { mode: null, root: null, roomId: null, pip: null, ro: null };

  function S(k, fb) { return window.__lcS ? window.__lcS(k, fb) : fb; }
  function delegate() { return window.LetsChatDelegate || null; }
  function floatKey() { return 'lc-huddle-float:' + st.roomId; }
  function pipSupported() {
    return !!(window.documentPictureInPicture && window.documentPictureInPicture.requestWindow);
  }
  function placeholderInPage() {
    if (st.roomId == null) return null;
    return document.querySelector('[data-lc-huddle-placeholder][data-room-id="' + st.roomId + '"]');
  }

  // ---- chrome: the button label and the in-room placeholder -------------
  function setBtn(root, on) {
    if (!root) return;
    var b = root.querySelector('[data-lc-huddle-popout]');
    if (!b) return;
    var txt = on ? S('huddleBringBack', 'Bring back') : S('huddlePopOut', 'Pop out');
    var l = b.querySelector('.lc-cbtn-label');
    if (l) l.textContent = txt; else b.textContent = txt;
    b.setAttribute('aria-label', txt);
    b.setAttribute('data-lc-tip', txt);
    b.setAttribute('aria-pressed', on ? 'true' : 'false');
  }
  function makePlaceholder() {
    var ph = document.createElement('div');
    ph.setAttribute('data-lc-huddle-placeholder', '');
    ph.setAttribute('data-room-id', String(st.roomId));
    ph.className = 'lc-huddle lc-huddle--placeholder border-t border-border bg-surface-sunken';
    var bar = document.createElement('div');
    bar.className = 'lc-huddle-bar flex items-center gap-2 px-4 py-1.5';
    var lab = document.createElement('span');
    lab.className = 'text-xs font-semibold uppercase tracking-wide text-content-muted';
    lab.textContent = S('huddlePoppedOut', 'Huddle is popped out');
    var b = document.createElement('button');
    b.type = 'button';
    b.setAttribute('data-lc-huddle-bringback', '');
    b.className = 'lc-cbtn ml-auto';
    var bl = document.createElement('span');
    bl.className = 'lc-cbtn-label';
    bl.textContent = S('huddleBringBack', 'Bring back');
    b.appendChild(bl);
    bar.appendChild(lab);
    bar.appendChild(b);
    ph.appendChild(bar);
    return ph;
  }
  function clearHostStyles(root) {
    root.classList.remove('lc-huddle--popout', 'lc-huddle--float', 'lc-huddle--dragging');
    ['left', 'top', 'right', 'bottom', 'width', 'height'].forEach(function (p) {
      root.style.removeProperty(p);
    });
  }

  // ---- host 1: Document Picture-in-Picture window -----------------------
  // The window starts as a blank document: bring the app's stylesheets over
  // (a <link> per external sheet, a clone of each inline <style>) and mirror
  // the <html> attributes + body surface classes so theme, density and text
  // size match the page.
  function copyStyles(doc) {
    Array.prototype.forEach.call(document.styleSheets, function (ss) {
      try {
        if (ss.href) {
          var l = doc.createElement('link');
          l.rel = 'stylesheet';
          l.href = ss.href;
          doc.head.appendChild(l);
        } else if (ss.ownerNode) {
          doc.head.appendChild(ss.ownerNode.cloneNode(true));
        }
      } catch (e) { /* a cross-origin sheet: nothing of ours in it */ }
    });
    var src = document.documentElement, dst = doc.documentElement;
    Array.prototype.forEach.call(src.attributes, function (a) {
      try { dst.setAttribute(a.name, a.value); } catch (e) {}
    });
    doc.body.className = 'bg-surface text-content';
  }
  function closePip() {
    var p = st.pip;
    if (!p) return;
    st.pip = null;
    try { p.removeEventListener('pagehide', onPipGone); } catch (e) {}
    var d = delegate();
    if (d) { try { d.detach(p.document); } catch (e) {} }
    try { p.close(); } catch (e) {}
  }
  // The user closed the window (or the browser did): re-dock when this room's
  // placeholder is on screen, otherwise keep the call alive as a floating panel.
  function onPipGone() {
    if (st.mode !== 'pip' || !st.root) return;
    st.pip = null;
    var ph = placeholderInPage();
    if (ph) dockInto(ph); else toFloat();
  }

  // ---- host 2: in-page floating panel -----------------------------------
  function clampX(x, root) { return Math.max(0, Math.min(x, window.innerWidth - (root.offsetWidth || 0))); }
  function clampY(y, root) { return Math.max(0, Math.min(y, window.innerHeight - (root.offsetHeight || 0))); }
  function loadGeometry() {
    try { return JSON.parse(localStorage.getItem(floatKey()) || 'null'); } catch (e) { return null; }
  }
  function saveGeometry() {
    var root = st.root;
    if (!root || st.mode !== 'float') return;
    var r = root.getBoundingClientRect();
    try {
      localStorage.setItem(floatKey(), JSON.stringify({
        x: Math.round(r.left), y: Math.round(r.top), w: Math.round(r.width), h: Math.round(r.height),
      }));
    } catch (e) { /* storage off */ }
  }
  function stopWatch() {
    if (st.ro) { try { st.ro.disconnect(); } catch (e) {} st.ro = null; }
  }
  function toFloat() {
    var root = st.root;
    if (!root) return;
    closePip();
    st.mode = 'float';
    document.body.appendChild(root);
    root.classList.remove('lc-huddle--popout', 'lc-huddle--collapsed');
    root.classList.add('lc-huddle--float');
    var g = loadGeometry();
    if (g && g.w > 0 && g.h > 0) { root.style.width = g.w + 'px'; root.style.height = g.h + 'px'; }
    if (g && g.x != null && g.y != null) {
      root.style.right = 'auto';
      root.style.bottom = 'auto';
      root.style.left = clampX(g.x, root) + 'px';
      root.style.top = clampY(g.y, root) + 'px';
    }
    // Native corner resize changes the size without any event of ours;
    // observe it so the chosen size persists like the position does.
    stopWatch();
    if (window.ResizeObserver) {
      var t = null;
      st.ro = new ResizeObserver(function () { clearTimeout(t); t = setTimeout(saveGeometry, 200); });
      st.ro.observe(root);
    }
    setBtn(root, true);
  }
  // Drag the floating panel by its bar (not by the controls in it).
  document.addEventListener('pointerdown', function (e) {
    if (st.mode !== 'float' || !st.root || e.button !== 0) return;
    var bar = e.target.closest && e.target.closest('.lc-huddle-bar');
    if (!bar || !st.root.contains(bar)) return;
    if (e.target.closest('button, a, select, input, label')) return;
    e.preventDefault();
    var root = st.root, r = root.getBoundingClientRect();
    var sx = e.clientX, sy = e.clientY, ox = r.left, oy = r.top;
    root.style.right = 'auto';
    root.style.bottom = 'auto';
    root.style.left = ox + 'px';
    root.style.top = oy + 'px';
    root.classList.add('lc-huddle--dragging');
    try { bar.setPointerCapture(e.pointerId); } catch (err) { /* unsupported */ }
    function mv(ev) {
      root.style.left = clampX(ox + ev.clientX - sx, root) + 'px';
      root.style.top = clampY(oy + ev.clientY - sy, root) + 'px';
    }
    function up() {
      bar.removeEventListener('pointermove', mv);
      bar.removeEventListener('pointerup', up);
      bar.removeEventListener('pointercancel', up);
      root.classList.remove('lc-huddle--dragging');
      saveGeometry();
    }
    bar.addEventListener('pointermove', mv);
    bar.addEventListener('pointerup', up);
    bar.addEventListener('pointercancel', up);
  });

  // ---- pop out / bring back ---------------------------------------------
  function popOut(root) {
    if (st.mode || !root) return;
    st.root = root;
    st.roomId = root.getAttribute('data-room-id') || '';
    // Leave a placeholder where the dock was so Bring back knows where to go.
    if (root.parentNode) root.parentNode.insertBefore(makePlaceholder(), root);
    if (!pipSupported()) { toFloat(); return; }
    var r = root.getBoundingClientRect();
    var w = Math.min(960, Math.max(360, Math.round(r.width || 480)));
    var h = Math.min(640, Math.max(240, Math.round(r.height || 320)));
    // Must run inside the click's user activation, hence no awaits before it.
    window.documentPictureInPicture.requestWindow({ width: w, height: h }).then(function (pip) {
      st.mode = 'pip';
      st.pip = pip;
      copyStyles(pip.document);
      pip.document.body.appendChild(root);
      root.classList.remove('lc-huddle--collapsed');
      root.classList.add('lc-huddle--popout');
      var d = delegate();
      if (d) d.attach(pip.document);
      pip.addEventListener('pagehide', onPipGone);
      setBtn(root, true);
    }).catch(function (e) {
      console.warn('huddle: pop-out window unavailable, floating instead', e);
      toFloat();
    });
  }
  function dockInto(ph) {
    var root = st.root;
    if (!root) return;
    closePip();
    stopWatch();
    clearHostStyles(root);
    if (ph.parentNode) ph.parentNode.replaceChild(root, ph);
    st.mode = null;
    setBtn(root, false);
  }
  function bringBack() {
    if (!st.mode || !st.root) return;
    var ph = placeholderInPage();
    if (ph) { dockInto(ph); return; }
    // Not on the huddle's page: nowhere to dock, so keep it floating.
    if (st.mode === 'pip') toFloat();
  }

  // ---- page-swap integration (voice.js scan / leave) ---------------------
  function markBusy(el) {
    if (el.hasAttribute('data-lc-huddle-busy')) return;
    el.setAttribute('data-lc-huddle-busy', '');
    var join = el.querySelector('[data-lc-voice-join]');
    if (join) join.setAttribute('disabled', '');
    var bar = el.querySelector('.lc-huddle-bar');
    if (bar) {
      var a = document.createElement('a');
      a.setAttribute('data-lc-huddle-busy-note', '');
      a.href = '/room/' + encodeURIComponent(st.roomId);
      a.className = 'text-xs text-accent hover:underline';
      a.textContent = S('huddleBusy', 'You are in a huddle in another room');
      bar.insertBefore(a, bar.querySelector('.lc-callbar-controls'));
    }
    el.classList.remove('hidden');
  }
  function unmarkBusy() {
    var els = document.querySelectorAll('[data-lc-huddle-busy]');
    Array.prototype.forEach.call(els, function (el) {
      el.removeAttribute('data-lc-huddle-busy');
      var join = el.querySelector('[data-lc-voice-join]');
      if (join) join.removeAttribute('disabled');
      var n = el.querySelector('[data-lc-huddle-busy-note]');
      if (n) n.remove();
    });
  }
  // A page rendered its own dock while the live one is popped out.
  function adoptPageDock(freshEl) {
    if (!st.mode || !st.root || !freshEl || freshEl === st.root) return;
    if (freshEl.getAttribute('data-room-id') === String(st.roomId)) {
      // The huddle's own room re-rendered: swap its dock for the placeholder
      // the live one docks back into.
      if (placeholderInPage()) freshEl.remove();
      else if (freshEl.parentNode) freshEl.parentNode.replaceChild(makePlaceholder(), freshEl);
    } else {
      markBusy(freshEl);
    }
  }
  // voice.js leave(): hand the dock back. Re-dock into this room's placeholder
  // when it is on screen, otherwise drop the element (voice.js rebinds from
  // the page); either way clear any busy note left on another room's dock.
  function release() {
    var root = st.root;
    if (root) {
      closePip();
      stopWatch();
      clearHostStyles(root);
      var ph = placeholderInPage();
      if (ph && ph.parentNode) ph.parentNode.replaceChild(root, ph);
      else if (root.parentNode) root.parentNode.removeChild(root);
      setBtn(root, false);
    }
    unmarkBusy();
    st.mode = null;
    st.root = null;
    st.roomId = null;
    st.pip = null;
  }

  function onClick(e) {
    var t = e.target;
    if (!t || !t.closest) return;
    var pb = t.closest('[data-lc-huddle-popout]');
    if (pb) {
      if (st.mode) bringBack(); else popOut(pb.closest('[data-lc-huddle]'));
      return;
    }
    if (t.closest('[data-lc-huddle-bringback]')) { bringBack(); return; }
    // Join on another room's dock, or the header Huddle button, while a huddle
    // is live elsewhere: voice.js ignores it (one session per tab). Reveal the
    // busy note so the click is not silent.
    if (st.mode && t.closest('[data-lc-voice-join]')) {
      var busy = document.querySelector('[data-lc-huddle-busy]');
      if (busy) busy.classList.remove('hidden');
    }
  }
  var d = delegate();
  if (d) d.onClick(onClick); else document.addEventListener('click', onClick);

  window.LetsChatHuddlePopout = {
    isPopped: function () { return !!st.mode; },
    roomId: function () { return st.roomId; },
    adoptPageDock: adoptPageDock,
    release: release,
    popOut: popOut,
    bringBack: bringBack,
  };
})();
