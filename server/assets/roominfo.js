// LC-449: Room info page tabs.
//
// The tab controller is the shared assets/tabs.js (LC-747); this only supplies
// the root and the storage key, which is distinct from settings so the two
// pages remember their tab independently.
(function () {
  'use strict';

  // LC-454: type-to-confirm gate for destructive room actions (Delete room),
  // matching the Settings "Delete account" friction. The submit button stays
  // disabled until the typed phrase matches; a native confirm() on the form's
  // onsubmit is the second guard. Purely client-side - the server handler is
  // unchanged.
  function confirmGatesInit() {
    var forms = document.querySelectorAll('[data-lc-confirm-gate]');
    Array.prototype.forEach.call(forms, function (form) {
      if (form._lcGate) return; // idempotent across re-inits
      var input = form.querySelector('[data-lc-confirm-input]');
      var btn = form.querySelector('[data-lc-confirm-submit]');
      if (!input || !btn) return;
      form._lcGate = true;
      var phrase = (input.getAttribute('data-lc-confirm-phrase') || '').trim().toLowerCase();
      function sync() { btn.disabled = input.value.trim().toLowerCase() !== phrase; }
      input.addEventListener('input', sync);
      sync();
    });
  }

  function init() {
    var root = document.querySelector('[data-lc-roominfo]');
    if (root) window.lcInitTabs(root, 'lc-roominfo-tab');
    confirmGatesInit();
  }

  if (document.readyState !== 'loading') init();
  else document.addEventListener('DOMContentLoaded', init);
  // Re-init after an htmx content swap navigates into the info / manage page.
  document.body.addEventListener('htmx:afterSettle', function (e) {
    if (!e.target || !e.target.querySelector) return;
    if (e.target.querySelector('[data-lc-roominfo]') || e.target.querySelector('[data-lc-confirm-gate]')) init();
  });
})();
