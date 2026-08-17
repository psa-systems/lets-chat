// LC-463: Enclave settings page interactivity.
//
//  - Tabs: one panel visible at a time (General / Members / Moderation /
//    Customization / Danger zone), synced to location.hash + sessionStorage,
//    mirroring the Settings + room-info tab behaviour. All panels stay in the
//    DOM so their forms keep working; this only toggles visibility.
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

  function tabsInit(root) {
    var tabs = Array.prototype.slice.call(root.querySelectorAll('[data-lc-tab]'));
    var panels = Array.prototype.slice.call(root.querySelectorAll('[data-lc-tabpanel]'));
    if (!tabs.length) return;

    function valid(key) {
      return tabs.some(function (t) { return t.getAttribute('data-lc-tab') === key; });
    }
    function select(key, focus) {
      tabs.forEach(function (t) {
        var on = t.getAttribute('data-lc-tab') === key;
        t.setAttribute('aria-selected', on ? 'true' : 'false');
        t.tabIndex = on ? 0 : -1;
        if (on && focus) { try { t.focus(); } catch (e) {} }
      });
      panels.forEach(function (p) {
        p.hidden = p.getAttribute('data-lc-tabpanel') !== key;
      });
      try { sessionStorage.setItem('lc-enclave-tab', key); } catch (e) {}
    }
    function initialKey() {
      var h = (location.hash || '').replace(/^#/, '');
      if (valid(h)) return h;
      var stored;
      try { stored = sessionStorage.getItem('lc-enclave-tab'); } catch (e) {}
      if (valid(stored)) return stored;
      return tabs[0].getAttribute('data-lc-tab');
    }

    root.addEventListener('click', function (e) {
      var tab = e.target.closest && e.target.closest('[data-lc-tab]');
      if (!tab || !root.contains(tab)) return;
      var key = tab.getAttribute('data-lc-tab');
      if (location.hash.replace(/^#/, '') !== key) history.replaceState(null, '', '#' + key);
      select(key, false);
    });
    root.addEventListener('keydown', function (e) {
      var tab = e.target.closest && e.target.closest('[data-lc-tab]');
      if (!tab) return;
      var idx = tabs.indexOf(tab);
      if (idx < 0) return;
      var next = null;
      if (e.key === 'ArrowDown' || e.key === 'ArrowRight') next = tabs[(idx + 1) % tabs.length];
      else if (e.key === 'ArrowUp' || e.key === 'ArrowLeft') next = tabs[(idx - 1 + tabs.length) % tabs.length];
      else if (e.key === 'Home') next = tabs[0];
      else if (e.key === 'End') next = tabs[tabs.length - 1];
      if (!next) return;
      e.preventDefault();
      var key = next.getAttribute('data-lc-tab');
      history.replaceState(null, '', '#' + key);
      select(key, true);
    });
    window.addEventListener('hashchange', function () {
      var h = (location.hash || '').replace(/^#/, '');
      if (valid(h)) select(h, false);
    });
    select(initialKey(), false);
  }

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
    tabsInit(root);
    flashToast(root);
    copyInit(root);
  }

  if (document.readyState !== 'loading') init();
  else document.addEventListener('DOMContentLoaded', init);
  document.body.addEventListener('htmx:afterSettle', function (e) {
    if (e.target && e.target.querySelector && e.target.querySelector('[data-lc-enclave-settings]')) init();
  });
})();
