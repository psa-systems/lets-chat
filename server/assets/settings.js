// LC-426: Settings page interactivity.
//
//  - Real tabs: one panel visible at a time, synced to location.hash, with
//    arrow-key navigation. All panels stay in the DOM so every form keeps its
//    htmx wiring; this only toggles visibility.
//  - Avatar live preview: show the chosen image immediately and a "not applied
//    until you save" hint; clear it once the profile form saves successfully.
//  - Loading affordances for the non-htmx actions (data download, account
//    delete) that cannot return a status fragment.
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
      // Remember the tab so a full-page reload (no-JS fallback redirect, or the
      // DnD pause/resume forms) returns to it instead of resetting to the first.
      try { sessionStorage.setItem('lc-settings-tab', key); } catch (e) {}
    }

    function keyFromHash() {
      var h = (location.hash || '').replace(/^#/, '');
      if (valid(h)) return h;
      var stored;
      try { stored = sessionStorage.getItem('lc-settings-tab'); } catch (e) {}
      return valid(stored) ? stored : tabs[0].getAttribute('data-lc-tab');
    }

    root.addEventListener('click', function (e) {
      var tab = e.target.closest && e.target.closest('[data-lc-tab]');
      if (!tab || !root.contains(tab)) return;
      var key = tab.getAttribute('data-lc-tab');
      if (location.hash.replace(/^#/, '') !== key) {
        // pushState-free: update the hash without a jarring scroll jump.
        history.replaceState(null, '', '#' + key);
      }
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

    window.addEventListener('hashchange', function () { select(keyFromHash(), false); });
    select(keyFromHash(), false);
  }

  function avatarInit(root) {
    var input = root.querySelector('[data-lc-avatar-input]');
    if (!input) return;
    // LC-432: one preview <img> (no fallback circle); the native input is
    // sr-only, so we report the chosen filename ourselves.
    var preview = root.querySelector('[data-lc-avatar-preview]');
    var pending = root.querySelector('[data-lc-avatar-pending]');
    var filename = root.querySelector('[data-lc-avatar-filename]');
    var errEl = root.querySelector('[data-lc-avatar-error]');
    var noFile = input.getAttribute('data-lc-no-file') || '';
    var form = input.closest('form');
    var saveBtn = form && form.querySelector('button[type="submit"]');
    // LC-439: validate BEFORE preview/save so an oversized/wrong-type file is
    // rejected with a clear message instead of silently failing on Save.
    var MAX = parseInt(input.getAttribute('data-lc-max-bytes'), 10) || 1048576;
    var TYPES = { 'image/png': 1, 'image/jpeg': 1, 'image/webp': 1 };

    function showError(msg) {
      if (errEl) { errEl.textContent = msg; errEl.hidden = false; }
      if (saveBtn) saveBtn.disabled = true; // block save while the pick is invalid
    }
    function clearError() {
      if (errEl) { errEl.textContent = ''; errEl.hidden = true; }
      if (saveBtn) saveBtn.disabled = false;
    }

    input.addEventListener('change', function () {
      var file = input.files && input.files[0];
      if (!file) { if (filename) filename.textContent = noFile; clearError(); return; }
      // Wrong type (the OS dialog can ignore `accept`). Only reject a known-bad
      // MIME; an empty type passes here and the server's byte-sniff is the gate.
      if (file.type && !TYPES[file.type]) {
        if (filename) filename.textContent = file.name;
        if (pending) pending.hidden = true;
        showError(input.getAttribute('data-lc-err-type') || 'Unsupported format - use PNG, JPEG, or WebP.');
        try { input.value = ''; } catch (e) {} // drop the bad pick so Save can't submit it
        return;
      }
      if (file.size > MAX) {
        if (filename) filename.textContent = file.name;
        if (pending) pending.hidden = true;
        showError(input.getAttribute('data-lc-err-size') || 'Image must be under 1 MiB.');
        try { input.value = ''; } catch (e) {}
        return;
      }
      // Valid: preview + mark not-yet-saved.
      clearError();
      if (preview) preview.src = URL.createObjectURL(file);
      if (filename) filename.textContent = file.name;
      if (pending) pending.hidden = false;
    });

    // Clear the "not applied yet" hint + filename once the profile form saves OK.
    if (form) {
      form.addEventListener('htmx:afterRequest', function (e) {
        if (e.detail && e.detail.successful) {
          if (pending) pending.hidden = true;
          if (filename) filename.textContent = noFile;
          clearError();
          try { input.value = ''; } catch (err) {}
        }
      });
    }
  }

  // LC-439: never-silent guarantee. If any settings form's htmx request errors
  // (non-2xx like a 413 body cap, a 500, or a network drop), htmx would
  // otherwise swap nothing. Surface a generic error in that form's status slot
  // plus a toast so a Save can never complete with no feedback.
  function errorNetInit() {
    function onErr(e) {
      var src = (e.detail && e.detail.elt) || e.target;
      var form = src && src.closest && src.closest('form');
      var slot = form && form.querySelector('.lc-set-status');
      var msg = (window.__lcS && window.__lcS('settingsSaveError', 'Could not save. Please try again.'))
        || 'Could not save. Please try again.';
      if (slot) {
        slot.replaceChildren();
        var wrap = document.createElement('span');
        wrap.className = 'lc-status lc-status--err';
        var ico = document.createElement('span');
        ico.className = 'lc-status-ico';
        ico.setAttribute('aria-hidden', 'true');
        ico.textContent = '!';
        wrap.appendChild(ico);
        wrap.appendChild(document.createTextNode(msg));
        slot.appendChild(wrap);
      }
      if (window.__lcToast) window.__lcToast('err', msg);
    }
    document.body.addEventListener('htmx:responseError', onErr);
    document.body.addEventListener('htmx:sendError', onErr);
  }

  // LC-432: the inline per-form status should not linger. Auto-clear it a few
  // seconds after it lands, and clear it the moment the user edits the form
  // again. (The toast already self-dismisses.)
  function statusInit() {
    var TIMEOUT = 4500;
    document.body.addEventListener('htmx:afterSwap', function (e) {
      var slot = e.target;
      if (!slot || !slot.classList || !slot.classList.contains('lc-set-status')) return;
      if (!slot.textContent.trim()) return;
      clearTimeout(slot._lcClear);
      slot._lcClear = setTimeout(function () { slot.replaceChildren(); }, TIMEOUT);
    });
    document.addEventListener('input', function (e) {
      var form = e.target.closest && e.target.closest('form');
      if (!form) return;
      var slot = form.querySelector('.lc-set-status');
      if (slot && slot.textContent) { clearTimeout(slot._lcClear); slot.replaceChildren(); }
    });
  }

  // Slow / navigating actions that can't return an htmx status fragment: give
  // immediate feedback so the click never feels ignored.
  function actionsInit(root) {
    var dl = root.querySelector('[data-lc-download-data]');
    if (dl) {
      dl.addEventListener('click', function () {
        if (window.__lcToast) {
          window.__lcToast('ok', dl.getAttribute('data-lc-downloading') || 'Preparing your data...');
        }
      });
    }
    var del = root.querySelector('[data-lc-delete-form]');
    if (del) {
      del.addEventListener('submit', function () {
        var btn = del.querySelector('button[type=submit]');
        if (btn) {
          btn.disabled = true;
          var label = btn.querySelector('[data-lc-btn-label]');
          if (label) label.textContent = btn.getAttribute('data-lc-working') || 'Deleting...';
        }
      });
    }
  }

  function init() {
    var root = document.querySelector('[data-lc-settings]');
    if (!root) return;
    tabsInit(root);
    avatarInit(root);
    actionsInit(root);
  }

  // Document-level status listeners: registered once (init() can re-run).
  statusInit();
  errorNetInit();

  if (document.readyState !== 'loading') init();
  else document.addEventListener('DOMContentLoaded', init);
  // Re-init after an htmx content swap navigates into the settings page.
  document.body.addEventListener('htmx:afterSettle', function (e) {
    if (e.target && e.target.querySelector && e.target.querySelector('[data-lc-settings]')) init();
  });
})();
