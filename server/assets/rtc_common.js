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

  window.LetsChatRtc = {
    watchBus: watchBus,
    bindControls: bindControls,
  };
})();
