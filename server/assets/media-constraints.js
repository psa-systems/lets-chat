// LC-628: shared audio-capture constraints.
//
// Echo cancellation, noise suppression, and auto gain control are requested on
// every microphone path (devices.js, call.js, voice.js, transcribe.js) so a
// participant on speakers does not hear their own voice echoed back with a
// delay. echoCancellation is the headline fix; the other two clean up the same
// open-mic-in-a-room scenario. Browsers default echoCancellation to true for
// the boolean `audio: true` form, but not reliably once the constraint is an
// object (e.g. a pinned deviceId), so we set all three explicitly.
//
// Loaded before devices.js in base.html and attached to window.LetsChatMedia.
// Also exported via module.exports so media_constraints.test.js can assert the
// shape under `node --test` without a browser.
(function (root) {
  'use strict';

  // Build the WebRTC `audio` constraint. The processing flags always apply; a
  // pinned deviceId, when given, is merged in as an exact constraint so the
  // browser fails loudly (OverconstrainedError) if that device is gone, which
  // the acquire helpers catch and retry without the pin.
  function audio(deviceId) {
    var c = {
      echoCancellation: true,
      noiseSuppression: true,
      autoGainControl: true,
    };
    if (deviceId) c.deviceId = { exact: deviceId };
    return c;
  }

  var api = { audio: audio };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  if (root) root.LetsChatMedia = api;
})(typeof window !== 'undefined' ? window : null);
