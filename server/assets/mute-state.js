// LC-764: pure mute-toggle state derivation, shared by the SFU toggle
// (huddle_sfu.js) so the button's pressed state is always taken from the track's
// REAL post-toggle state, never the state we were aiming for. On the SFU path
// `setMicrophoneEnabled` can reject (or resolve without actually flipping the
// track), and the old code silently corrected the label while leaving the user
// unaware the unmute never took - the caller stayed muted to peers with a button
// reading "Mute". This makes the failure detectable.
//
// Loaded before huddle_sfu.js in base.html and attached to window.LetsChatMute.
// Also exported via module.exports so mute-state.test.js can assert it under
// `node --test` (just test-js) without a browser.
(function (root) {
  'use strict';

  // Derive how the UI should reflect a mute toggle from three readings:
  //   enabledBefore - `isMicrophoneEnabled` read BEFORE the toggle,
  //   enabledAfter  - `isMicrophoneEnabled` read AFTER awaiting the toggle,
  //   ok            - false if `setMicrophoneEnabled` rejected.
  // `muted` is always the negation of the real post-toggle reading, so the
  // button can never claim a state the track does not hold. `failed` is true
  // when the promise rejected OR the track did not actually change state, which
  // is the case the caller surfaces to the user.
  function nextState(enabledBefore, enabledAfter, ok) {
    var changed = enabledAfter !== enabledBefore;
    return {
      muted: !enabledAfter,
      failed: !ok || !changed,
    };
  }

  var api = { nextState: nextState };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  if (root) root.LetsChatMute = api;
})(typeof window !== 'undefined' ? window : null);
