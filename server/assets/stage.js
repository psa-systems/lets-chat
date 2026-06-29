// LC-494: stage control plane. Tiny delegated click -> WS frame bridge.
// The roster panel itself is server-rendered and live-swapped on StageChanged;
// this only sends the intent frames over the shared chat socket (window.__lcWS,
// set by voice.js on htmx:wsOpen). No media here - audio is the LC-512 SFU work.
(function () {
  'use strict';

  function send(obj) {
    var ws = window.__lcWS;
    if (!ws) return;
    try { ws.send(JSON.stringify(obj)); } catch (e) { /* socket reconnecting */ }
  }
  function roomId(el) {
    var p = el.closest('[data-lc-stage]');
    if (!p) return null;
    var n = parseInt(p.getAttribute('data-room-id'), 10);
    return isNaN(n) ? null : n;
  }
  // [attr, frame-type, takes-target]
  var SELF = [
    ['data-lc-stage-join', 'stage_join'],
    ['data-lc-stage-leave', 'stage_leave'],
    ['data-lc-stage-raise-hand', 'stage_raise_hand'],
    ['data-lc-stage-lower-hand', 'stage_lower_hand'],
    ['data-lc-stage-step-down', 'stage_step_down'],
  ];
  var TARGETED = [
    ['data-lc-stage-promote', 'stage_promote'],
    ['data-lc-stage-demote', 'stage_demote'],
  ];

  document.addEventListener('click', function (e) {
    var t = e.target;
    if (!t || !t.closest) return;
    for (var i = 0; i < SELF.length; i++) {
      var b = t.closest('[' + SELF[i][0] + ']');
      if (b) {
        var r = roomId(b);
        if (r != null) send({ type: SELF[i][1], room_id: r });
        return;
      }
    }
    for (var j = 0; j < TARGETED.length; j++) {
      var bt = t.closest('[' + TARGETED[j][0] + ']');
      if (bt) {
        var rr = roomId(bt);
        var uid = bt.getAttribute(TARGETED[j][0]);
        if (rr != null && uid) send({ type: TARGETED[j][1], room_id: rr, user_id: uid });
        return;
      }
    }
  });
})();
