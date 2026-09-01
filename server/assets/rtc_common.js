// LC-616: machinery shared by the real-time-call clients (call.js, voice.js).
//
// call.js (1:1) and voice.js (huddle) each open a WebSocket "bus" that the
// server pushes OOB fragments into, and each binds one delegated click listener
// for its control bar. The drain loop and the click-dispatch idiom were
// byte-for-byte identical across the two files (and call.js had two buses), so
// they live here once. Loaded before both consumers in base.html; exposes
// `window.LetsChatRtc`.
//
// NOT here: the media operations (mute/camera/screen) and the signaling frames,
// which are genuinely different between a single RTCPeerConnection, a mesh, and
// the SFU - see LC-613 for why forcing those together would be worse.
(function () {
  'use strict';

  // Drain an OOB "bus" div: a MutationObserver fires `handler(node)` for each
  // element carrying `eventAttr` (or containing a descendant that does) that the
  // server appends, then empties the bus. `busId` absent from the page (a client
  // on a surface without this bus) is a no-op.
  function watchBus(busId, eventAttr, handler) {
    var bus = document.getElementById(busId);
    if (!bus) return;
    var sel = '[' + eventAttr + ']';
    new MutationObserver(function (muts) {
      muts.forEach(function (m) {
        Array.prototype.forEach.call(m.addedNodes, function (n) {
          if (n.nodeType !== 1) return;
          if (n.hasAttribute(eventAttr)) {
            handler(n);
          } else if (n.querySelector) {
            var c = n.querySelector(sel);
            if (c) handler(c);
          }
        });
      });
      bus.replaceChildren();
    }).observe(bus, { childList: true });
  }

  // Bind one delegated `click` listener on document.body that maps a control
  // selector to its handler. `map` is an object of `selector -> fn(el, event)`;
  // the first selector the clicked target is inside wins, and its handler runs
  // with the matched element and the event (so a handler can preventDefault or
  // read data-attributes off `el`). Insertion order is the match order.
  function bindControls(map) {
    var selectors = Object.keys(map);
    // LC-822: via the shared delegation so the handler also fires inside a
    // pop-out window the huddle dock has been moved into.
    var reg = window.LetsChatDelegate
      ? window.LetsChatDelegate.onClick
      : function (fn) { document.body.addEventListener('click', fn); };
    reg(function (e) {
      var t = e.target;
      if (!t || !t.closest) return;
      for (var i = 0; i < selectors.length; i++) {
        var el = t.closest(selectors[i]);
        if (el) {
          map[selectors[i]](el, e);
          return;
        }
      }
    });
  }

  // ---- remote-control input capture (LC-854) ------------------------
  // The controller-side capture + coordinate mapping + key policy, shared by
  // the 1:1 call (call.js, over the RTCDataChannel) and the huddle
  // (huddle_control.js, over the LiveKit data channel). Only the transport
  // differs; the frames on the wire and the key policy are identical, so the
  // desktop injector (LC-185) parses one format from either surface.

  // Map a viewport point on an object-contain <video> to normalized [0,1]
  // surface coordinates, or null for a point in the letterbox bars (so a bogus
  // coord is never sent). The controlled side maps [0,1] back to its own screen
  // pixels + DPI, keeping the protocol resolution-independent. REQUIRES the
  // video to render object-contain; a cover-fit video crops the source and the
  // math is wrong, so bindCapture forces contain on the target while active.
  function normCoords(video, clientX, clientY) {
    if (!video) return null;
    var vw = video.videoWidth, vh = video.videoHeight;
    if (!vw || !vh) return null;
    var rect = video.getBoundingClientRect();
    if (!rect.width || !rect.height) return null;
    var scale = Math.min(rect.width / vw, rect.height / vh);
    var dispW = vw * scale, dispH = vh * scale;
    var offX = (rect.width - dispW) / 2, offY = (rect.height - dispH) / 2;
    var x = (clientX - rect.left - offX) / dispW;
    var y = (clientY - rect.top - offY) / dispH;
    if (x < 0 || x > 1 || y < 0 || y > 1) return null;
    return { x: x, y: y };
  }

  // Modifier bitmask: ctrl=1, shift=2, alt=4, meta=8.
  function modMask(e) {
    return (e.ctrlKey ? 1 : 0) | (e.shiftKey ? 2 : 0) | (e.altKey ? 4 : 0) | (e.metaKey ? 8 : 0);
  }

  // Key policy (LC-854). Every key is preventDefault'd locally while a session
  // is active (so a browser shortcut like Ctrl+W never fires on the
  // controller's own machine); this decides which are additionally FORWARDED to
  // the controlled peer. Blocked:
  //   - the kill hotkey (Ctrl/Cmd+Alt+F9, LC-186): forwarding it would inject
  //     the sharer's own panic combo; it must only ever fire locally on the
  //     sharer, never be driven from the controller.
  //   - the OS secure-attention / lock combos the injector cannot synthesize
  //     anyway (Ctrl+Alt+Delete, Meta+L): dropped so they are not a misleading
  //     no-op mid-stream.
  // Browser-level shortcuts are intentionally NOT in this list: they are
  // handled by the local preventDefault, and their key events are still
  // forwarded so the same keystroke drives the peer (Ctrl+C in the controlled
  // app, etc.). Enumerated, not heuristic, so the policy is reviewable.
  function isForwardableKey(e) {
    var code = e.code;
    // Kill hotkey: Alt + (Ctrl or Meta) + F9.
    if (code === 'F9' && e.altKey && (e.ctrlKey || e.metaKey)) return false;
    // Ctrl+Alt+Delete (Windows secure attention) - cannot be injected.
    if (code === 'Delete' && e.ctrlKey && e.altKey) return false;
    // Meta+L (OS lock on Windows/most Linux) - injector can't drive the lock.
    if (code === 'KeyL' && e.metaKey) return false;
    return true;
  }

  // Bind controller-side capture. `getVideo()` resolves the CURRENT target
  // video (it can change - a tile re-render, a swapped remote), `send(obj)`
  // ships one input frame. Movement is coalesced to one frame per animation
  // frame; clicks/keys/wheel send immediately. Returns an unbind fn. Frame
  // shapes (unchanged from the 1:1 flow, so the injector parses one format):
  //   move  { t:'m', x, y }
  //   down  { t:'d', x, y, b }        up { t:'u', x, y, b }
  //   wheel { t:'w', x, y, dx, dy }
  //   key   { t:'k', c, m }           up { t:'K', c, m }   (c = KeyboardEvent.code)
  function bindCapture(getVideo, send) {
    var pendingMove = null;
    var moveRaf = 0;
    // Force object-contain on the target so the whole surface is visible and
    // normCoords' letterbox math holds; remember what to restore.
    var styled = null;
    function target() {
      var v = getVideo();
      if (v && v !== styled) {
        if (styled) styled.classList.remove('lc-rc-contain');
        v.classList.add('lc-rc-contain');
        v.style.cursor = 'crosshair';
        v.style.touchAction = 'none';
        styled = v;
      }
      return v;
    }
    function flushMove() {
      moveRaf = 0;
      if (pendingMove) { send(pendingMove); pendingMove = null; }
    }
    function coordAt(e) { return normCoords(target(), e.clientX, e.clientY); }
    function onMove(e) {
      var c = coordAt(e);
      if (!c) return;
      pendingMove = { t: 'm', x: c.x, y: c.y };
      if (!moveRaf) moveRaf = requestAnimationFrame(flushMove);
    }
    function onDown(e) {
      var c = coordAt(e); if (!c) return;
      e.preventDefault();
      send({ t: 'd', x: c.x, y: c.y, b: e.button });
    }
    function onUp(e) {
      var c = coordAt(e); if (!c) return;
      e.preventDefault();
      send({ t: 'u', x: c.x, y: c.y, b: e.button });
    }
    function onWheel(e) {
      var c = coordAt(e); if (!c) return;
      e.preventDefault();
      send({ t: 'w', x: c.x, y: c.y, dx: e.deltaX, dy: e.deltaY });
    }
    function onCtx(e) { e.preventDefault(); }
    function onKeyDown(e) {
      e.preventDefault();
      if (isForwardableKey(e)) send({ t: 'k', c: e.code, m: modMask(e) });
    }
    function onKeyUp(e) {
      e.preventDefault();
      if (isForwardableKey(e)) send({ t: 'K', c: e.code, m: modMask(e) });
    }
    var v0 = target();
    if (v0) {
      v0.addEventListener('pointermove', onMove);
      v0.addEventListener('pointerdown', onDown);
      v0.addEventListener('pointerup', onUp);
      v0.addEventListener('wheel', onWheel, { passive: false });
      v0.addEventListener('contextmenu', onCtx);
    }
    window.addEventListener('keydown', onKeyDown, true);
    window.addEventListener('keyup', onKeyUp, true);
    return function unbind() {
      if (moveRaf) { cancelAnimationFrame(moveRaf); moveRaf = 0; }
      pendingMove = null;
      if (v0) {
        v0.removeEventListener('pointermove', onMove);
        v0.removeEventListener('pointerdown', onDown);
        v0.removeEventListener('pointerup', onUp);
        v0.removeEventListener('wheel', onWheel, false);
        v0.removeEventListener('contextmenu', onCtx);
      }
      window.removeEventListener('keydown', onKeyDown, true);
      window.removeEventListener('keyup', onKeyUp, true);
      if (styled) {
        styled.classList.remove('lc-rc-contain');
        styled.style.cursor = '';
        styled.style.touchAction = '';
        styled = null;
      }
    };
  }

  window.LetsChatRtc = {
    watchBus: watchBus,
    bindControls: bindControls,
    control: {
      normCoords: normCoords,
      modMask: modMask,
      isForwardableKey: isForwardableKey,
      bindCapture: bindCapture,
    },
  };
})();
