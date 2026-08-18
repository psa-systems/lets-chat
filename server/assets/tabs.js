// LC-747: the one tablist controller, shared by the settings, enclave-settings
// and room-info pages.
//
// Markup contract (unchanged from the three copies this replaces):
//   [data-lc-tab="<key>"]      role="tab" triggers inside the root
//   [data-lc-tabpanel="<key>"] role="tabpanel" panels inside the root
// All panels stay in the DOM so their htmx wiring survives; this only toggles
// `hidden`, `aria-selected` and the roving tabindex.
//
// Usage: window.lcInitTabs(root, 'lc-settings-tab'). The storage key is the
// only per-page difference, so each surface remembers its tab independently.
(function () {
  'use strict';

  function initTabs(root, storageKey) {
    if (!root) return;
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
        // A focus() that throws (detached or hidden tab) is not a failure worth
        // reporting: the selection below still applies, only the caret moves.
        if (on && focus) { try { t.focus(); } catch (e) {} }
      });
      panels.forEach(function (p) {
        p.hidden = p.getAttribute('data-lc-tabpanel') !== key;
      });
      // Remember the tab so a full-page reload (a no-JS fallback redirect, or a
      // form that redirects) returns to it instead of resetting to the first.
      // Storage being disabled or full only costs that memory, and nothing
      // downstream treats the write as having happened.
      try { sessionStorage.setItem(storageKey, key); } catch (e) {}
    }

    function hashKey() {
      return (location.hash || '').replace(/^#/, '');
    }

    function initialKey() {
      var h = hashKey();
      if (valid(h)) return h;
      // Unreadable storage and "nothing remembered" are the same answer here -
      // no remembered tab - so both fall through to the next candidate.
      var stored;
      try { stored = sessionStorage.getItem(storageKey); } catch (e) {}
      if (valid(stored)) return stored;
      // Fall back to whichever tab the server pre-selected, then the first.
      var pre = tabs.filter(function (t) { return t.getAttribute('aria-selected') === 'true'; })[0];
      return pre ? pre.getAttribute('data-lc-tab') : tabs[0].getAttribute('data-lc-tab');
    }

    // Re-init on the same root (an htmx swap that reuses the node) must not
    // stack a second set of listeners; just re-apply the current selection.
    if (root._lcTabs) { select(initialKey(), false); return; }
    root._lcTabs = true;

    root.addEventListener('click', function (e) {
      var tab = e.target.closest && e.target.closest('[data-lc-tab]');
      if (!tab || !root.contains(tab)) return;
      var key = tab.getAttribute('data-lc-tab');
      // pushState-free: update the hash without a jarring scroll jump.
      if (hashKey() !== key) history.replaceState(null, '', '#' + key);
      select(key, false);
    });

    // Roving-tabindex arrow navigation across the tablist.
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

    // Only a hash that names a tab moves the panel: following an in-page anchor
    // to anything else must leave the visible panel alone.
    window.addEventListener('hashchange', function () {
      if (!root.isConnected) return; // root replaced by an htmx swap
      var h = hashKey();
      if (valid(h)) select(h, false);
    });

    select(initialKey(), false);
  }

  window.lcInitTabs = initTabs;
})();
