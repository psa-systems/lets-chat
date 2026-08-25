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
// LC-768 added the `video` builder here too, so the camera constraint (pinned
// device + optional background blur) is built the same requestable, testable
// way as the mic constraint.
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

  // LC-768: build the WebRTC `video` constraint. A pinned deviceId is an exact
  // constraint (same loud-failure contract as audio). Background blur is
  // requested only when the caller both wants it and has feature-detected the
  // browser's own segmentation (`getSupportedConstraints().backgroundBlur`), and
  // even then it is advisory - NOT `{ exact: true }` - so a client that ignores
  // it returns a plain working stream instead of throwing, and no black or
  // broken stream is ever published. With neither a pin nor blur this returns
  // the bare `true` the callers used before, unchanged.
  function video(deviceId, opts) {
    opts = opts || {};
    var c = {};
    if (deviceId) c.deviceId = { exact: deviceId };
    if (opts.blur && opts.blurSupported) c.backgroundBlur = true;
    if (!c.deviceId && !('backgroundBlur' in c)) return true;
    return c;
  }

  var api = { audio: audio, video: video };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  if (root) root.LetsChatMedia = api;
})(typeof window !== 'undefined' ? window : null);
