// LC-449: Room info page tabs.
//
// Mirrors the settings-page tab behaviour (server/assets/settings.js): one
// panel visible at a time, synced to location.hash, with roving-tabindex arrow
// navigation. All panels stay in the DOM so their htmx wiring (description /
// wiki inline edit, files filter + load-more, nickname) keeps working; this
// only toggles visibility. Storage key is distinct from settings so the two
// pages remember their tab independently.
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
      try { sessionStorage.setItem('lc-roominfo-tab', key); } catch (e) {}
    }

    function initialKey() {
      var h = (location.hash || '').replace(/^#/, '');
      if (valid(h)) return h;
      var stored;
      try { stored = sessionStorage.getItem('lc-roominfo-tab'); } catch (e) {}
      if (valid(stored)) return stored;
      // Fall back to whichever tab the server pre-selected (active_tab).
      var pre = tabs.filter(function (t) { return t.getAttribute('aria-selected') === 'true'; })[0];
      return pre ? pre.getAttribute('data-lc-tab') : tabs[0].getAttribute('data-lc-tab');
    }

    root.addEventListener('click', function (e) {
      var tab = e.target.closest && e.target.closest('[data-lc-tab]');
      if (!tab || !root.contains(tab)) return;
      var key = tab.getAttribute('data-lc-tab');
      if (location.hash.replace(/^#/, '') !== key) {
        history.replaceState(null, '', '#' + key);
      }
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

  function init() {
    var root = document.querySelector('[data-lc-roominfo]');
    if (root) tabsInit(root);
  }

  if (document.readyState !== 'loading') init();
  else document.addEventListener('DOMContentLoaded', init);
  // Re-init after an htmx content swap navigates into the info page.
  document.body.addEventListener('htmx:afterSettle', function (e) {
    if (e.target && e.target.querySelector && e.target.querySelector('[data-lc-roominfo]')) init();
  });
})();
