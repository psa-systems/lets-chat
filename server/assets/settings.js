// LC-426: Settings page interactivity.
//
//  - Real tabs: LC-747 moved the controller to the shared assets/tabs.js; this
//    only supplies the root and the storage key.
//  - Avatar live preview: show the chosen image immediately and a "not applied
//    until you save" hint; clear it once the profile form saves successfully.
//  - Loading affordances for the non-htmx actions (data download, account
//    delete) that cannot return a status fragment.
(function () {
  'use strict';

  // LC-740: the filename echo, the type/size rejection and the Save block all
  // live in the shared file_picker.js handler now. This only adds what is
  // specific to the avatar: the LC-432 single preview <img> and the LC-439
  // "not applied yet" hint.
  function avatarInit(root) {
    var input = root.querySelector('#lc-avatar-input');
    if (!input) return;
    var preview = root.querySelector('[data-lc-avatar-preview]');
    var pending = root.querySelector('[data-lc-avatar-pending]');
    var form = input.form;

    input.addEventListener('lc:file-picked', function (e) {
      var file = e.detail && e.detail.file;
      if (!file) { if (pending) pending.hidden = true; return; }
      if (preview) preview.src = URL.createObjectURL(file);
      if (pending) pending.hidden = false;
    });

    // Clear the "not applied yet" hint + filename once the profile form saves OK.
    if (form) {
      form.addEventListener('htmx:afterRequest', function (e) {
        if (!e.detail || !e.detail.successful) return;
        if (pending) pending.hidden = true;
        try { input.value = ''; } catch (err) {}
        // Re-run the shared handler so the filename echo and error slot reset.
        input.dispatchEvent(new Event('change', { bubbles: true }));
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

  // LC-553: dismiss the settings page by clicking outside it (on the rail /
  // sidebar background) or pressing Escape - it fills the #main pane with no
  // close control, so "click out to leave" is the expected exit. Registered
  // once at document level; a guard flag survives init() re-runs.
  var DISMISS_INTERACTIVE = 'a[href],button,input,select,textarea,label,[role="menuitem"],[role="tab"],[contenteditable="true"]';
  function leaveSettings() {
    // Return to whatever was behind settings (the room / home); fall back home
    // when there is no in-app history to step back to.
    if (window.history.length > 1) window.history.back();
    else window.location.assign('/');
  }
  function dismissInit() {
    if (window.__lcSettingsDismissWired) return;
    window.__lcSettingsDismissWired = true;
    document.addEventListener('pointerdown', function (e) {
      // Only while the settings page is actually shown.
      if (!document.querySelector('[data-lc-settings]')) return;
      var main = document.getElementById('main');
      if (!main) return;
      // Clicks inside the settings pane (including its header) never dismiss.
      if (main.contains(e.target)) return;
      // Let real controls (sidebar room links, rail buttons) act normally; a
      // room link already navigates away. Only empty outside space dismisses.
      if (e.target.closest && e.target.closest(DISMISS_INTERACTIVE)) return;
      leaveSettings();
    });
    document.addEventListener('keydown', function (e) {
      if (e.key === 'Escape' && document.querySelector('[data-lc-settings]')) leaveSettings();
    });
  }

  function init() {
    var root = document.querySelector('[data-lc-settings]');
    if (!root) return;
    window.lcInitTabs(root, 'lc-settings-tab');
    avatarInit(root);
    actionsInit(root);
    dismissInit();
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
