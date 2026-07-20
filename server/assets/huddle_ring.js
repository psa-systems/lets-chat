// LC-611: huddle ring.
//
// Starting a huddle used to broadcast only `VoiceJoined`, which goes to the
// room topic - so the only people told were the ones already looking at the
// room. The server now addresses a `HuddleStarted` event to each room member
// (mute-respecting, once per huddle) and OOB-appends it to #lc-huddle-ring-bus.
// This module drains that bus into a banner offering Join.
//
// Ignore is deliberately local: it removes the banner and remembers the room
// for this page only. Nothing is sent to the server, so nobody learns that you
// declined - the ticket calls for exactly that.
(function () {
  'use strict';

  var BUS_ID = 'lc-huddle-ring-bus';
  var dismissed = Object.create(null);

  function root() { return document.querySelector('[data-lc-huddle-ring-root]'); }

  function s(key, fallback) {
    var t = window.__lcS && window.__lcS[key];
    return t || fallback;
  }

  function render(ring) {
    var host = root();
    if (!host) return;
    if (dismissed[ring.roomId]) return;
    // One banner at a time: a second huddle replaces the first rather than
    // stacking dialogs over the page.
    host.innerHTML = '';

    var box = document.createElement('div');
    box.className = 'lc-huddle-ring';
    box.setAttribute('role', 'status');
    box.setAttribute('data-room-id', ring.roomId);

    var text = document.createElement('span');
    text.className = 'lc-huddle-ring-text';
    // Built with textContent, never innerHTML: room and display names are
    // user-controlled.
    text.textContent = ring.starterName + ' ' +
      s('huddleRingStarted', 'started a huddle in') + ' ' + ring.roomName;
    box.appendChild(text);

    var join = document.createElement('a');
    join.className = 'btn btn-primary btn-sm';
    join.setAttribute('data-lc-huddle-ring-join', '');
    // ?huddle=1 makes Join one click from anywhere: voice.js reads it on load
    // and joins, instead of landing the user on the room to hunt for the bar.
    join.href = '/room/' + encodeURIComponent(ring.roomId) + '?huddle=1';
    join.textContent = s('huddleRingJoin', 'Join');
    box.appendChild(join);

    var ignore = document.createElement('button');
    ignore.type = 'button';
    ignore.className = 'btn btn-ghost btn-sm';
    ignore.setAttribute('data-lc-huddle-ring-ignore', '');
    ignore.textContent = s('huddleRingIgnore', 'Ignore');
    box.appendChild(ignore);

    host.appendChild(box);
    host.classList.remove('hidden');
  }

  function clear(roomId) {
    var host = root();
    if (host) { host.innerHTML = ''; host.classList.add('hidden'); }
    if (roomId != null) dismissed[roomId] = true;
  }

  function drain() {
    var bus = document.getElementById(BUS_ID);
    if (!bus) return;
    var nodes = bus.querySelectorAll('[data-lc-huddle-ring]');
    for (var i = 0; i < nodes.length; i++) {
      var n = nodes[i];
      var roomId = n.getAttribute('data-room-id');
      // Already in this room's huddle, or viewing it: no point ringing.
      if (String(window.__lcVoiceRoom || '') !== roomId) {
        render({
          roomId: roomId,
          roomName: n.getAttribute('data-room-name') || '',
          starterId: n.getAttribute('data-starter-id') || '',
          starterName: n.getAttribute('data-starter-name') || ''
        });
      }
      n.parentNode && n.parentNode.removeChild(n);
    }
  }

  document.addEventListener('click', function (e) {
    if (!e.target.closest) return;
    var ig = e.target.closest('[data-lc-huddle-ring-ignore]');
    if (ig) {
      var box = ig.closest('[data-room-id]');
      clear(box && box.getAttribute('data-room-id'));
      return;
    }
    // Joining navigates; drop the banner so it cannot outlive the click.
    if (e.target.closest('[data-lc-huddle-ring-join]')) clear(null);
  });

  // The bus is filled by OOB swaps, same as the call/voice/transcript buses.
  var bus = document.getElementById(BUS_ID);
  if (bus) new MutationObserver(drain).observe(bus, { childList: true });
  document.body.addEventListener('htmx:wsAfterMessage', drain);

  // A ring for the room we are already in is stale once we join it.
  document.addEventListener('lc:voice-joined', function () { clear(null); });
})();
