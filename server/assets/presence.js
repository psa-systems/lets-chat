// LC-1057: Discord-style presence. The server marks a user idle after 10
// minutes without activity, and only these signals count as activity:
//
// - real input on a visible page (pointer, key, wheel, touch, a throttled mouse
//   move, window focus, the page becoming visible) sends {type:"activity"},
//   at most once per minute;
// - opening the socket sends one when the page is visible and focused, so a
//   fresh load reads as present while a silent reconnect (network blip,
//   watchdog, laptop wake) does not.
//
// It also answers every server `<!-- ping -->` with {type:"pong"}; the server
// drops a socket that sends nothing for 90s, so a half-open or abandoned one
// stops counting as online. Loaded once from base.html after live.js.
(function () {
  'use strict';
  var THROTTLE_MS = 60 * 1000;
  var lastSent = 0;

  // Pure gate, exported for presence.test.js: may an activity frame go out now?
  function shouldSend(now, last, visible) {
    return visible && (last === 0 || now - last >= THROTTLE_MS);
  }

  function visible() {
    return document.visibilityState === 'visible';
  }

  function send(frame) {
    var ws = window.__lcWS;
    if (!ws) return false;
    try {
      ws.send(JSON.stringify(frame));
      return true;
    } catch (e) {
      return false; // socket not ready; the next input or wsOpen retries
    }
  }

  function activity(force) {
    var now = Date.now();
    if (!force && !shouldSend(now, lastSent, visible())) return;
    if (force && !visible()) return;
    if (send({ type: 'activity' })) lastSent = now;
  }

  var onInput = function () { activity(false); };
  ['pointerdown', 'keydown', 'wheel', 'touchstart'].forEach(function (t) {
    document.addEventListener(t, onInput, { passive: true, capture: true });
  });
  // mousemove fires constantly; the throttle inside activity() bounds it.
  document.addEventListener('mousemove', onInput, { passive: true, capture: true });
  window.addEventListener('focus', onInput);
  document.addEventListener('visibilitychange', function () {
    if (visible()) activity(false);
  });

  document.body.addEventListener('htmx:wsOpen', function (evt) {
    if (evt && evt.detail && evt.detail.socketWrapper) window.__lcWS = evt.detail.socketWrapper;
    var focused = typeof document.hasFocus !== 'function' || document.hasFocus();
    if (focused) activity(true);
  });

  document.body.addEventListener('htmx:wsAfterMessage', function (evt) {
    var msg = evt && evt.detail && evt.detail.message;
    if (typeof msg === 'string' && msg.indexOf('<!-- ping -->') === 0) send({ type: 'pong' });
  });

  window.LetsChatPresence = { shouldSend: shouldSend, THROTTLE_MS: THROTTLE_MS };
})();
