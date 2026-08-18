// LC-463: Enclave settings page interactivity.
//
//  - Tabs: General / Members / Moderation / Customization / Danger zone. The
//    controller is the shared assets/tabs.js (LC-747); this only supplies the
//    root and the storage key.
//  - Flash toast: a post-action redirect carries ?ok=<code>, the server renders
//    a hidden [data-lc-flash-toast] node, and we surface it via the global
//    toast so every redirect-based action confirms.
//  - Emoji upload: mirror the profile-avatar validation (show filename,
//    validate png/gif/webp + <=256 KiB before submit, inline error).
//  - Copy buttons: one-click copy (invite code) with "Copied" feedback.
//
// The type-to-confirm delete gate is handled globally by roominfo.js
// (data-lc-confirm-gate), so it is not duplicated here.
(function () {
  'use strict';

  // Surface a post-redirect success toast, then drop the node so a later
  // re-init does not replay it.
  function flashToast(root) {
    var el = root.querySelector('[data-lc-flash-toast]');
    if (!el) return;
    var msg = el.textContent.trim();
    if (msg && window.__lcToast) window.__lcToast('ok', msg);
    el.parentNode && el.parentNode.removeChild(el);
  }

  // One-click copy for [data-lc-copy] (invite code). Flips the label to the
  // data-lc-copied text for ~1.5s.
  function copyInit(root) {
    root.addEventListener('click', function (e) {
      var btn = e.target.closest && e.target.closest('[data-lc-copy]');
      if (!btn || !root.contains(btn)) return;
      var val = btn.getAttribute('data-lc-copy');
      if (!val) return;
      var done = function () {
        var label = btn.querySelector('[data-lc-copy-label]') || btn;
        var orig = label.textContent;
        label.textContent = btn.getAttribute('data-lc-copied') || 'Copied';
        setTimeout(function () { label.textContent = orig; }, 1500);
      };
      if (navigator.clipboard && navigator.clipboard.writeText) {
        navigator.clipboard.writeText(val).then(done, function () {
          if (window.__lcCopyFallback) window.__lcCopyFallback(val);
          done();
        });
      } else if (window.__lcCopyFallback) {
        window.__lcCopyFallback(val);
        done();
      }
    });
  }

  function init() {
    var root = document.querySelector('[data-lc-enclave-settings]');
    if (!root) return;
    window.lcInitTabs(root, 'lc-enclave-tab');
    flashToast(root);
    copyInit(root);
  }

  if (document.readyState !== 'loading') init();
  else document.addEventListener('DOMContentLoaded', init);
  document.body.addEventListener('htmx:afterSettle', function (e) {
    if (e.target && e.target.querySelector && e.target.querySelector('[data-lc-enclave-settings]')) init();
  });
})();
