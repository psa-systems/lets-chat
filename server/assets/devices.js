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
  // LC-768: background-blur preference. A single boolean stored the same way as
  // the device pins, so it is remembered across calls for this browser.
  var BLUR_KEY = 'lc.dev.videoblur';
  // Delivered-frame-rate floor: if a blurred stream stays under this for
  // BLUR_FPS_WINDOW ms, blur is dropped and the user told, rather than shipping
  // a stuttering call.
  var BLUR_FPS_FLOOR = 12;
  var BLUR_FPS_WINDOW = 4000;
  var KIND_LABEL = {
    audioinput: window.__lcS('deviceMicrophone', 'Microphone'),
    videoinput: window.__lcS('deviceCamera', 'Camera'),
    audiooutput: window.__lcS('deviceSpeaker', 'Speaker'),
  };
  // LC-632: each device type is shown as an icon (not a text label) next to its
  // dropdown, so the picker reads visually. The icon carries the kind name as a
  // title/aria-label for hover + assistive tech; the <select> keeps its own
  // aria-label since the visible text label is gone.
  var ICONS = {
    audioinput: '<svg viewBox="0 0 20 20" fill="currentColor" aria-hidden="true" class="h-5 w-5"><path d="M7 4a3 3 0 016 0v4a3 3 0 11-6 0V4z"/><path d="M5.5 9.643a.75.75 0 00-1.5 0V10c0 3.06 2.29 5.585 5.25 5.954V17.5h-1.5a.75.75 0 000 1.5h4.5a.75.75 0 000-1.5h-1.5v-1.546A6.001 6.001 0 0016 10v-.357a.75.75 0 00-1.5 0V10a4.5 4.5 0 01-9 0v-.357z"/></svg>',
    videoinput: '<svg viewBox="0 0 20 20" fill="currentColor" aria-hidden="true" class="h-5 w-5"><path d="M1 4.75C1 3.784 1.784 3 2.75 3h8.5c.966 0 1.75.784 1.75 1.75v6.5A1.75 1.75 0 0111.25 13h-8.5A1.75 1.75 0 011 11.25v-6.5zM14.5 7.732l2.303-1.382A.75.75 0 0118 6.994v6.012a.75.75 0 01-1.197.644L14.5 12.268v-4.536z"/></svg>',
    audiooutput: '<svg viewBox="0 0 20 20" fill="currentColor" aria-hidden="true" class="h-5 w-5"><path d="M10 3.75a.75.75 0 00-1.264-.546L5.203 6.5H2.667a.75.75 0 00-.75.75v5.5c0 .414.336.75.75.75h2.536l3.533 3.296A.75.75 0 0010 16.25V3.75z"/><path d="M15.95 5.05a.75.75 0 10-1.06 1.061 5.5 5.5 0 010 7.778.75.75 0 001.06 1.06 7 7 0 000-9.899zM13.83 7.17a.75.75 0 10-1.06 1.06 2.5 2.5 0 010 3.54.75.75 0 101.06 1.06 4 4 0 000-5.66z"/></svg>',
  };
  // Down chevron: the dropdown affordance, drawn on top of an appearance-none
  // <select> so it matches across browsers instead of the OS default arrow.
  var CHEVRON = '<svg viewBox="0 0 20 20" fill="currentColor" aria-hidden="true" class="h-4 w-4"><path fill-rule="evenodd" d="M5.22 8.22a.75.75 0 011.06 0L10 11.94l3.72-3.72a.75.75 0 111.06 1.06l-4.25 4.25a.75.75 0 01-1.06 0L5.22 9.28a.75.75 0 010-1.06z" clip-rule="evenodd"/></svg>';
  var CLOSE_ICON = '<svg viewBox="0 0 20 20" fill="currentColor" aria-hidden="true" class="h-5 w-5"><path d="M6.28 5.22a.75.75 0 00-1.06 1.06L8.94 10l-3.72 3.72a.75.75 0 101.06 1.06L10 11.06l3.72 3.72a.75.75 0 101.06-1.06L11.06 10l3.72-3.72a.75.75 0 00-1.06-1.06L10 8.94 6.28 5.22z"/></svg>';
  // LC-768: sparkle glyph for the background-blur row, matching the icon-labelled
  // device rows.
  var BLUR_ICON = '<svg viewBox="0 0 20 20" fill="currentColor" aria-hidden="true" class="h-5 w-5"><path d="M10 1.5c.32 0 .6.2.71.5l1.2 3.24 3.24 1.2a.75.75 0 010 1.41l-3.24 1.2-1.2 3.24a.75.75 0 01-1.42 0l-1.2-3.24-3.24-1.2a.75.75 0 010-1.41l3.24-1.2 1.2-3.24c.11-.3.39-.5.71-.5z"/><path d="M15 12.5c.28 0 .53.17.63.44l.62 1.66 1.66.62a.68.68 0 010 1.26l-1.66.62-.62 1.66a.68.68 0 01-1.26 0l-.62-1.66-1.66-.62a.68.68 0 010-1.26l1.66-.62.62-1.66c.1-.27.35-.44.63-.44z"/></svg>';

  function get(kind) {
    try { return localStorage.getItem(KEYS[kind]) || ''; } catch (e) { return ''; }
  }
  function set(kind, id) {
    try {
      if (id) localStorage.setItem(KEYS[kind], id);
      else localStorage.removeItem(KEYS[kind]);
    } catch (e) { /* private mode / storage disabled: selection is session-only */ }
  }
  // LC-768: blur preference, persisted like the device pins.
  function getBlur() {
    try { return localStorage.getItem(BLUR_KEY) === '1'; } catch (e) { return false; }
  }
  function setBlur(on) {
    try {
      if (on) localStorage.setItem(BLUR_KEY, '1');
      else localStorage.removeItem(BLUR_KEY);
    } catch (e) { /* private mode / storage disabled: session-only */ }
  }
  // Feature-detect the browser's own background segmentation. Detected at
  // acquisition time, never by user agent, so a client that cannot do it gets
  // no blur constraint and the picker hides the toggle.
  function blurSupported() {
    try {
      return !!(navigator.mediaDevices
        && navigator.mediaDevices.getSupportedConstraints
        && navigator.mediaDevices.getSupportedConstraints().backgroundBlur);
    } catch (e) { return false; }
  }

  // ---- constraints --------------------------------------------------
  // A pinned id becomes an `exact` deviceId constraint; with no pin the
  // processing flags still apply. `exact` is deliberate so the browser fails
  // loudly (OverconstrainedError) when the device is gone, which the acquire
  // helpers below catch and retry without the pin. LC-628: the echo
  // cancellation / noise suppression / auto gain trio is centralized in
  // window.LetsChatMedia so every capture path requests it identically.
  function audioConstraint() {
    return window.LetsChatMedia.audio(get('audioinput'));
  }
  // LC-768: the camera constraint honors the pinned device and, when the user
  // has asked for it and the browser supports it, requests background blur. Pass
  // `usePin === false` for the unplugged-device retry so it drops only the pin,
  // keeping the (advisory) blur request so a restored camera stays blurred.
  function videoConstraint(usePin) {
    var id = usePin === false ? '' : get('videoinput');
    return window.LetsChatMedia.video(id, { blur: getBlur(), blurSupported: blurSupported() });
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
    var c = { audio: audioConstraint(), video: withVideo ? videoConstraint(true) : false };
    return navigator.mediaDevices.getUserMedia(c).catch(function (err) {
      if (isPinError(err)) {
        return navigator.mediaDevices.getUserMedia({
          audio: window.LetsChatMedia.audio(),
          video: withVideo ? videoConstraint(false) : false,
        });
      }
      throw err;
    }).then(guardBlurStream);
  }

  // Camera-only acquisition (camera toggle, screen-share restore). Same
  // unplugged-device fallback as getUserMedia, and the same blur treatment, so a
  // camera restored after a screen-share is blurred exactly like the call path.
  function getCamera() {
    if (!available()) return Promise.reject(new Error('getUserMedia unavailable'));
    return navigator.mediaDevices.getUserMedia({ video: videoConstraint(true) }).catch(function (err) {
      if (isPinError(err)) return navigator.mediaDevices.getUserMedia({ video: videoConstraint(false) });
      throw err;
    }).then(guardBlurStream);
  }

  // ---- blur performance guard ---------------------------------------
  // Watch delivered frames on a blurred stream; if the rate stays under the
  // floor for a sustained window, drop the blur effect on the live track and
  // tell the user, rather than letting the call stutter. Best-effort: where
  // requestVideoFrameCallback is absent the stream still plays, just
  // unmonitored. Returns the stream so it can sit in a promise chain.
  function guardBlurStream(stream) {
    if (!stream || !getBlur() || !blurSupported()) return stream;
    var track = stream.getVideoTracks ? stream.getVideoTracks()[0] : null;
    if (!track) return stream;
    var video = document.createElement('video');
    if (typeof video.requestVideoFrameCallback !== 'function') return stream;
    video.muted = true;
    video.playsInline = true;
    try { video.srcObject = stream; } catch (e) { return stream; }
    var play = video.play();
    if (play && play.catch) play.catch(function () {
      /* Muted, detached probe video: an autoplay rejection is expected and
         does not affect the frame sampling below. */
    });
    var frames = 0, start = 0, stopped = false;
    function stop() {
      stopped = true;
      try { video.pause(); video.srcObject = null; } catch (e) {}
    }
    track.addEventListener('ended', stop);
    function onFrame(now) {
      if (stopped || track.readyState === 'ended') { stop(); return; }
      if (!start) start = now;
      frames++;
      var elapsed = now - start;
      if (elapsed >= BLUR_FPS_WINDOW) {
        if ((frames * 1000) / elapsed < BLUR_FPS_FLOOR) { dropBlur(track); stop(); return; }
        frames = 0; start = now; // reset the window and keep watching
      }
      video.requestVideoFrameCallback(onFrame);
    }
    video.requestVideoFrameCallback(onFrame);
    return stream;
  }

  function dropBlur(track) {
    try {
      var p = track.applyConstraints({ advanced: [{ backgroundBlur: false }] });
      if (p && p.catch) p.catch(function () {
        /* If the browser refuses to drop the effect the blur just stays on,
           which is preferable to throwing out of the perf guard. */
      });
    } catch (e) { /* the effect just stays on; better than throwing */ }
    if (window.__lcToast) {
      window.__lcToast('info', window.__lcS('deviceBlurSlow',
        'Background blur was turned off to keep your video smooth.'));
    }
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
    // LC-632: themed via semantic tokens so the control is dark inside the dark
    // call UI (not the inverted light default), rounded to match the dialog, and
    // full-width. appearance-none drops the native arrow for the CHEVRON overlay
    // rowFor adds; pr-9 leaves room for it. aria-label replaces the removed text.
    sel.className = 'w-full appearance-none rounded-lg border border-border bg-surface-sunken py-2 pl-3 pr-9 text-sm text-content focus:outline-none focus:ring-2 focus:ring-ring';
    sel.setAttribute('aria-label', KIND_LABEL[kind]);
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

  // LC-632: [icon] [select + chevron]. The icon replaces the text label; the
  // select fills the remaining width with the chevron overlaid at its right.
  function rowFor(kind, sel) {
    var row = document.createElement('div');
    row.className = 'flex items-center gap-2.5';
    var icon = document.createElement('span');
    icon.className = 'shrink-0 text-content-muted';
    icon.setAttribute('title', KIND_LABEL[kind]);
    icon.innerHTML = ICONS[kind] || '';
    var wrap = document.createElement('div');
    wrap.className = 'relative flex-1';
    wrap.appendChild(sel);
    var chev = document.createElement('span');
    chev.className = 'pointer-events-none absolute inset-y-0 right-2.5 flex items-center text-content-muted';
    chev.innerHTML = CHEVRON;
    wrap.appendChild(chev);
    row.appendChild(icon);
    row.appendChild(wrap);
    return row;
  }

  // LC-768: the background-blur toggle. Rendered only when the browser supports
  // the effect (feature-detected), so an unsupported client sees no inert
  // control. Reuses the [icon] [control] row shape of the device pickers; the
  // native checkbox carries its own accessible name.
  function blurRow() {
    var row = document.createElement('div');
    row.className = 'flex items-center gap-2.5';
    var icon = document.createElement('span');
    icon.className = 'shrink-0 text-content-muted';
    var title = window.__lcS('deviceBlur', 'Blur my background');
    icon.setAttribute('title', title);
    icon.innerHTML = BLUR_ICON;
    var label = document.createElement('label');
    label.className = 'flex flex-1 cursor-pointer items-center justify-between gap-2 text-sm text-content';
    var text = document.createElement('span');
    text.textContent = title;
    var cb = document.createElement('input');
    cb.type = 'checkbox';
    cb.className = 'h-4 w-4 rounded border border-border bg-surface-sunken text-accent focus:outline-none focus:ring-2 focus:ring-ring';
    cb.checked = getBlur();
    cb.setAttribute('aria-label', title);
    cb.addEventListener('change', function () { setBlur(cb.checked); });
    label.appendChild(text);
    label.appendChild(cb);
    row.appendChild(icon);
    row.appendChild(label);
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
      // LC-768: the blur toggle sits directly under the camera row, and only
      // when there is a camera to blur and the browser can do the processing.
      if (kind === 'videoinput' && built.count > 0 && blurSupported()) {
        fields.appendChild(blurRow());
      }
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
    // LC-632: themed dialog - dark surface tokens (was a hardcoded white surface,
    // which rendered inverted over the dark call UI), a wider card that uses the space,
    // and a rounded radius matching the rest of the call chrome.
    overlay.innerHTML =
      '<div role="dialog" aria-modal="true" aria-label="' + window.__lcS('deviceDialogTitle', 'Call devices') + '" class="w-[26rem] max-w-[92vw] rounded-xl border border-border bg-surface-elevated p-5 text-content shadow-xl">' +
        '<div class="mb-4 flex items-center justify-between">' +
          '<h2 class="text-base font-semibold">' + window.__lcS('deviceDialogTitle', 'Call devices') + '</h2>' +
          '<button type="button" data-close aria-label="' + window.__lcS('deviceClose', 'Close') + '" class="rounded-md p-1 text-content-muted hover:bg-surface-sunken hover:text-content">' + CLOSE_ICON + '</button>' +
        '</div>' +
        '<div data-fields class="space-y-3"></div>' +
        '<p data-perm-hint class="mt-4 text-xs text-content-subtle" style="display:none">' +
          window.__lcS('devicePermissionHint', 'Allow microphone or camera access to see device names.') + ' ' +
          '<button type="button" data-grant class="btn btn-sm btn-ghost">' + window.__lcS('deviceShowNames', 'Show device names') + '</button>' +
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
  // LC-822: registered through the shared delegation so the huddle dock's
  // devices button still works after the dock is moved into a pop-out window.
  var regClick = window.LetsChatDelegate
    ? window.LetsChatDelegate.onClick
    : function (fn) { document.body.addEventListener('click', fn); };
  regClick(function (e) {
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
