// LC-144: call/voice input + output device selection.
//
// Persistent-shell script: loaded once per page from base.html, before
// call.js and voice.js. Owns the user's microphone / camera / speaker
// preference (persisted in localStorage), the getUserMedia constraints that
// honor it, speaker routing via setSinkId, and the shared device-picker
// modal opened from any `[data-lc-open-devices]` control.
//
// call.js (1:1) and voice.js (enclave mesh) acquire media through the
// helpers here so a single selection applies to both. Selection is applied
// at call/join start; mid-call input switching is out of scope for v1, but
// the speaker can be re-routed live (setSinkId on already-attached elements
// via the `lc:speaker-change` event this module dispatches).
(function () {
  'use strict';

  // localStorage keys, one per MediaDeviceKind we let the user pin.
  var KEYS = {
    audioinput: 'lc.dev.audioinput',
    videoinput: 'lc.dev.videoinput',
    audiooutput: 'lc.dev.audiooutput',
  };
  var KIND_LABEL = {
    audioinput: window.__lcS('deviceMicrophone', 'Microphone'),
    videoinput: window.__lcS('deviceCamera', 'Camera'),
    audiooutput: window.__lcS('deviceSpeaker', 'Speaker'),
  };

  function get(kind) {
    try { return localStorage.getItem(KEYS[kind]) || ''; } catch (e) { return ''; }
  }
  function set(kind, id) {
    try {
      if (id) localStorage.setItem(KEYS[kind], id);
      else localStorage.removeItem(KEYS[kind]);
    } catch (e) { /* private mode / storage disabled: selection is session-only */ }
  }

  // ---- constraints --------------------------------------------------
  // A pinned id becomes an `exact` deviceId constraint; with no pin we fall
  // back to the boolean form (system default). `exact` is deliberate so the
  // browser fails loudly (OverconstrainedError) when the device is gone,
  // which the acquire helpers below catch and retry without the pin.
  function audioConstraint() {
    var id = get('audioinput');
    return id ? { deviceId: { exact: id } } : true;
  }
  function videoConstraint() {
    var id = get('videoinput');
    return id ? { deviceId: { exact: id } } : true;
  }

  function available() {
    return !!(navigator.mediaDevices && navigator.mediaDevices.getUserMedia);
  }
  function isPinError(err) {
    return err && (err.name === 'OverconstrainedError' || err.name === 'NotFoundError');
  }

  // Acquire mic (+ camera when withVideo) honoring the pinned devices. If a
  // pinned device has been unplugged the exact constraint throws; retry once
  // with plain defaults so the call still connects on whatever is available.
  function getUserMedia(withVideo) {
    if (!available()) return Promise.reject(new Error('getUserMedia unavailable'));
    var c = { audio: audioConstraint(), video: withVideo ? videoConstraint() : false };
    return navigator.mediaDevices.getUserMedia(c).catch(function (err) {
      if (isPinError(err)) {
        return navigator.mediaDevices.getUserMedia({ audio: true, video: !!withVideo });
      }
      throw err;
    });
  }

  // Camera-only acquisition (camera toggle, screen-share restore). Same
  // unplugged-device fallback as getUserMedia.
  function getCamera() {
    if (!available()) return Promise.reject(new Error('getUserMedia unavailable'));
    return navigator.mediaDevices.getUserMedia({ video: videoConstraint() }).catch(function (err) {
      if (isPinError(err)) return navigator.mediaDevices.getUserMedia({ video: true });
      throw err;
    });
  }

  // ---- speaker routing ----------------------------------------------
  function sinkSupported() {
    return typeof HTMLMediaElement !== 'undefined'
      && typeof HTMLMediaElement.prototype.setSinkId === 'function';
  }
  // Route a media element's audio to the pinned speaker. No-op where
  // setSinkId is unsupported (Firefox) or no speaker is pinned.
  function applySpeaker(el) {
    if (!el || !sinkSupported()) return;
    var id = get('audiooutput');
    if (!id) return;
    try {
      var p = el.setSinkId(id);
      if (p && p.catch) p.catch(function () { /* device gone: keep default sink */ });
    } catch (e) { /* invalid id: keep default sink */ }
  }

  // ---- device picker modal ------------------------------------------
  var openOverlay = null;     // the live overlay element, or null
  var trap = null;            // focus-trap handle from __lcDialogTrap
  var prevFocus = null;
  var deviceChangeBound = null;

  function enumerate() {
    if (!navigator.mediaDevices || !navigator.mediaDevices.enumerateDevices) {
      return Promise.resolve([]);
    }
    return navigator.mediaDevices.enumerateDevices().catch(function () { return []; });
  }

  // Device labels are blank until the page holds a getUserMedia grant.
  function labelsHidden(devices) {
    return devices.length > 0 && devices.every(function (d) { return !d.label; });
  }

  function buildSelect(kind, devices) {
    var sel = document.createElement('select');
    sel.setAttribute('data-kind', kind);
    sel.className = 'mt-1 w-full rounded border border-slate-300 px-2 py-1 text-sm';
    var current = get(kind);
    var def = document.createElement('option');
    def.value = '';
    def.textContent = window.__lcS('deviceSystemDefault', 'System default');
    sel.appendChild(def);
    var n = 0;
    devices.forEach(function (d) {
      if (d.kind !== kind) return;
      n++;
      var opt = document.createElement('option');
      opt.value = d.deviceId;
      opt.textContent = d.label || (KIND_LABEL[kind] + ' ' + n);
      if (d.deviceId === current) opt.selected = true;
      sel.appendChild(opt);
    });
    sel.addEventListener('change', function () {
      set(kind, sel.value);
      // Speaker can be re-routed live on already-attached call/voice
      // elements; mic/camera apply at the next call start.
      if (kind === 'audiooutput') {
        document.dispatchEvent(new CustomEvent('lc:speaker-change'));
      }
    });
    return { row: rowFor(kind, sel), count: n };
  }

  function rowFor(kind, sel) {
    var row = document.createElement('label');
    row.className = 'block';
    var span = document.createElement('span');
    span.className = 'text-sm font-medium text-slate-700';
    span.textContent = KIND_LABEL[kind];
    row.appendChild(span);
    row.appendChild(sel);
    return row;
  }

  function closePicker() {
    if (!openOverlay) return;
    if (deviceChangeBound && navigator.mediaDevices) {
      navigator.mediaDevices.removeEventListener('devicechange', deviceChangeBound);
    }
    deviceChangeBound = null;
    if (trap) { trap.dispose(); trap = null; }
    openOverlay.remove();
    openOverlay = null;
    if (prevFocus && document.contains(prevFocus) && typeof prevFocus.focus === 'function') {
      try { prevFocus.focus(); } catch (e) {}
    }
    prevFocus = null;
  }

  function renderBody(card, devices) {
    var fields = card.querySelector('[data-fields]');
    fields.replaceChildren();
    ['audioinput', 'videoinput', 'audiooutput'].forEach(function (kind) {
      if (kind === 'audiooutput' && !sinkSupported()) return; // hide where setSinkId is absent
      var built = buildSelect(kind, devices);
      if (built.count === 0 && kind !== 'audioinput') return;  // no devices of this kind: hide row
      fields.appendChild(built.row);
    });
    var hint = card.querySelector('[data-perm-hint]');
    hint.style.display = labelsHidden(devices) ? '' : 'none';
  }

  function refresh(card) {
    enumerate().then(function (devices) {
      if (!openOverlay) return; // picker closed while enumeration was pending
      renderBody(card, devices);
    });
  }

  function openPicker() {
    if (openOverlay) return;
    prevFocus = document.activeElement;
    var overlay = document.createElement('div');
    overlay.className = 'fixed inset-0 z-[60] flex items-center justify-center bg-black/50';
    overlay.innerHTML =
      '<div role="dialog" aria-modal="true" aria-label="' + window.__lcS('deviceDialogTitle', 'Call devices') + '" class="w-80 rounded-lg bg-white p-5 shadow-xl">' +
        '<div class="mb-3 flex items-center justify-between">' +
          '<h2 class="text-base font-semibold">' + window.__lcS('deviceDialogTitle', 'Call devices') + '</h2>' +
          '<button type="button" data-close class="text-sm text-slate-500 hover:text-slate-900">' + window.__lcS('deviceClose', 'Close') + '</button>' +
        '</div>' +
        '<div data-fields class="space-y-3"></div>' +
        '<p data-perm-hint class="mt-3 text-xs text-slate-500" style="display:none">' +
          window.__lcS('devicePermissionHint', 'Allow microphone or camera access to see device names.') + ' ' +
          '<button type="button" data-grant class="font-medium text-accent hover:underline">' + window.__lcS('deviceShowNames', 'Show device names') + '</button>' +
        '</p>' +
      '</div>';
    document.body.appendChild(overlay);
    openOverlay = overlay;
    var card = overlay.querySelector('[role="dialog"]');

    overlay.addEventListener('click', function (e) {
      if (e.target === overlay) closePicker();              // backdrop
      else if (e.target.closest('[data-close]')) closePicker();
      else if (e.target.closest('[data-grant]')) {
        // Transient grant so enumerateDevices returns real labels, then stop
        // the tracks immediately (no call is in progress).
        getUserMedia(false).then(function (stream) {
          stream.getTracks().forEach(function (t) { try { t.stop(); } catch (e) {} });
          refresh(card);
        }).catch(function () { /* user denied: labels stay generic */ });
      }
    });
    overlay.addEventListener('keydown', function (e) {
      if (e.key === 'Escape') { e.stopPropagation(); closePicker(); }
    });

    refresh(card);

    // Refresh the lists when devices are plugged/unplugged while open.
    if (navigator.mediaDevices) {
      deviceChangeBound = function () { refresh(card); };
      navigator.mediaDevices.addEventListener('devicechange', deviceChangeBound);
    }

    var mk = window.__lcDialogTrap;
    trap = mk ? mk(card) : null;
    var first = card.querySelector('button:not([disabled]),select');
    if (first) { try { first.focus(); } catch (e) {} }
  }

  // Single delegated listener: any control marked [data-lc-open-devices]
  // opens the picker, surviving htmx swaps that re-render the buttons.
  document.body.addEventListener('click', function (e) {
    if (e.target && e.target.closest && e.target.closest('[data-lc-open-devices]')) {
      e.preventDefault();
      openPicker();
    }
  });

  window.LetsChatDevices = {
    getUserMedia: getUserMedia,
    getCamera: getCamera,
    applySpeaker: applySpeaker,
    sinkSupported: sinkSupported,
    open: openPicker,
  };
})();
