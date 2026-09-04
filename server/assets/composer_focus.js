// LC-858: focus the message composer when a room is entered.
//
// The composer <textarea name="body"> (room/composer.html) carries a native
// `autofocus`, but the browser applies that only on the initial full-page
// parse. Moving between rooms is an HTMX boosted swap of #main
// (partials/nav_boost.html), where autofocus never re-fires - so the caret was
// left nowhere and the first keystroke went into the void until the user
// clicked the field. This was noticed dogfooding on staging.
//
// Focus the composer explicitly after a #main swap. Running on
// `htmx:afterSettle` (which fires after `htmx:afterSwap`, once the swapped
// content has settled) means every component that mounts in the same swap has
// already initialized, so none of them can steal the caret back afterwards.
// Scoped to #main swaps so an out-of-band update (a new message row, a sidebar
// badge, a typing indicator) never yanks focus out of wherever the user is.
(function () {
  'use strict';

  var COMPOSER = '#composer textarea[name="body"]';

  function focusComposer() {
    var ta = document.querySelector(COMPOSER);
    // Absent in a read-only room (composer.html is gated on `can_post`) or
    // otherwise disabled: nothing to focus.
    if (!ta || ta.disabled) return;
    // Already there (e.g. the layout afterSwap cascade or native autofocus got
    // here first): do not fight it.
    if (document.activeElement === ta) return;
    // preventScroll: the composer sits in the footer and the timeline has just
    // been scrolled to the newest message; focusing must not jump the page.
    try {
      ta.focus({ preventScroll: true });
    } catch (e) {
      ta.focus();
    }
  }

  // A room-page navigation replaces #main; an OOB / WebSocket swap targets some
  // other element. Only the former is a room entry, so only it refocuses.
  function onSettle(evt) {
    var t = evt && evt.target;
    if (t && t.id === 'main') focusComposer();
  }

  function onReady(fn) {
    if (document.readyState !== 'loading') fn();
    else document.addEventListener('DOMContentLoaded', fn);
  }

  // Initial full-page entry: native autofocus usually wins here, but focus
  // explicitly too so a direct room URL still lands the caret if the browser
  // skipped autofocus (e.g. the element was off-screen at parse time).
  onReady(focusComposer);
  document.body.addEventListener('htmx:afterSettle', onSettle);
})();
