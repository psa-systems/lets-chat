// LC-740: one delegated handler for every picker rendered by
// templates/partials/file_picker.html. Echoes the chosen filename, rejects a
// wrong type (from `accept`) or an oversized file (from data-lc-max-bytes)
// with an inline role="alert" message, and blocks the form's submit button
// while the pick is invalid, so the user never learns about a bad file only
// after a native POST re-renders the page and drops their other edits.
//
// Site-specific extras (the avatar preview in settings.js, the logo preview in
// branding.js) listen for the `lc:file-picked` event this dispatches instead of
// re-implementing the validation.
(function () {
  'use strict';

  function slotFor(attr, id) {
    return document.querySelector('[' + attr + '="' + id + '"]');
  }

  // `accept` is the source of truth for allowed types: MIME types, wildcards
  // (image/*) or extensions (.zip). An empty file.type is not proof of a wrong
  // type (the OS reported none), so only a known-bad MIME is rejected; the
  // server's byte sniff remains the real gate.
  function typeAllowed(input, file) {
    var accept = (input.getAttribute('accept') || '').trim().toLowerCase();
    if (!accept) return true;
    var name = (file.name || '').toLowerCase();
    var mime = (file.type || '').toLowerCase();
    var entries = accept.split(',');
    for (var i = 0; i < entries.length; i++) {
      var entry = entries[i].trim();
      if (!entry) continue;
      if (entry.charAt(0) === '.') {
        if (name.length > entry.length && name.slice(-entry.length) === entry) return true;
      } else if (entry.slice(-2) === '/*') {
        if (mime && mime.indexOf(entry.slice(0, -1)) === 0) return true;
      } else if (mime && mime === entry) {
        return true;
      }
    }
    return !mime;
  }

  function reason(input, file) {
    if (!typeAllowed(input, file)) {
      return input.getAttribute('data-lc-err-type') || 'Unsupported file type.';
    }
    var max = parseInt(input.getAttribute('data-lc-max-bytes'), 10);
    if (max > 0 && file.size > max) {
      return input.getAttribute('data-lc-err-size') || 'File is too large.';
    }
    return '';
  }

  // A form can hold more than one picker (admin branding has logo + favicon),
  // so its submit button reflects EVERY picker in it. Re-enabling on the one
  // that just changed would unblock a save the other one is still rejecting.
  function syncSubmit(form) {
    if (!form) return;
    var submit = form.querySelector('button[type="submit"]');
    if (!submit) return;
    var inputs = form.querySelectorAll('input[type="file"][data-lc-file-picker]');
    for (var i = 0; i < inputs.length; i++) {
      var slot = slotFor('data-lc-picker-error', inputs[i].id);
      if (slot && !slot.hidden) { submit.disabled = true; return; }
    }
    submit.disabled = false;
  }

  function apply(input) {
    var id = input.id;
    var nameEl = slotFor('data-lc-picker-filename', id);
    var errEl = slotFor('data-lc-picker-error', id);
    var form = input.form;
    var file = (input.files && input.files[0]) || null;
    var err = file ? reason(input, file) : '';

    if (nameEl) {
      nameEl.textContent = file ? file.name : input.getAttribute('data-lc-no-file') || '';
    }
    if (errEl) {
      errEl.textContent = err;
      errEl.hidden = !err;
    }
    syncSubmit(form);
    // Drop the rejected pick so a submit cannot carry it. A browser that
    // refuses the reset is not a silent failure: the message is already on
    // screen and the submit button is already disabled.
    if (err) {
      try { input.value = ''; } catch (e) {}
    }
    input.dispatchEvent(new CustomEvent('lc:file-picked', {
      bubbles: true,
      detail: { file: err ? null : file, error: err },
    }));
  }

  document.addEventListener('change', function (e) {
    var el = e.target;
    if (el && el.matches && el.matches('input[type="file"][data-lc-file-picker]')) apply(el);
  });
})();
