// LC-469: Enclave branding page interactivity. Self-contained (gated on
// [data-lc-branding]) so it does not depend on the enclave-settings script.
//
//  - Color pickers: two-way sync between the native swatch and an editable hex
//    field, with a live preview applying the chosen colors.
//  - Logo upload: profile-avatar-style validation (PNG/JPEG/WebP/GIF <=1 MiB)
//    + live preview + filename, surfacing an inline error before submit.
//  - Save feedback: a success toast on the post-save reload (?saved=1).
//  - Loading state: the Save button shows a working state on submit.
(function () {
  'use strict';

  function colorInit(root) {
    var pairs = root.querySelectorAll('[data-lc-color-pair]');
    Array.prototype.forEach.call(pairs, function (pair) {
      var swatch = pair.querySelector('[data-lc-color-swatch]');
      var hex = pair.querySelector('[data-lc-color-hex]');
      if (!swatch || !hex) return;
      var preview = root.querySelector('[data-lc-brand-preview]');
      var cssVar = swatch.getAttribute('data-lc-color-var');
      function applyPreview() {
        if (preview && cssVar) preview.style.setProperty(cssVar, swatch.value);
      }
      swatch.addEventListener('input', function () {
        hex.value = swatch.value.toUpperCase();
        applyPreview();
      });
      hex.addEventListener('input', function () {
        var v = hex.value.trim();
        if (/^#?[0-9a-fA-F]{6}$/.test(v)) {
          if (v[0] !== '#') v = '#' + v;
          swatch.value = v.toLowerCase();
          applyPreview();
        }
      });
      applyPreview();
    });
  }

  // LC-740: validation and the filename echo come from the shared
  // file_picker.js handler; only the live logo preview is specific here.
  function logoInit(root) {
    var input = root.querySelector('#lc-logo-input');
    if (!input) return;
    var preview = root.querySelector('[data-lc-logo-preview]');
    if (!preview) return;
    input.addEventListener('lc:file-picked', function (e) {
      var file = e.detail && e.detail.file;
      if (!file) return;
      preview.src = URL.createObjectURL(file);
      preview.classList.remove('hidden');
    });
  }

  function flashToast(root) {
    var el = root.querySelector('[data-lc-flash-toast]');
    if (!el) return;
    var msg = el.textContent.trim();
    if (msg && window.__lcToast) window.__lcToast('ok', msg);
    el.parentNode && el.parentNode.removeChild(el);
  }

  function submitInit(root) {
    var form = root.querySelector('[data-lc-branding-form]');
    if (!form) return;
    form.addEventListener('submit', function () {
      var btn = form.querySelector('button[type="submit"]');
      if (btn && !btn.disabled) {
        var label = btn.querySelector('[data-lc-btn-label]');
        if (label) label.textContent = btn.getAttribute('data-lc-working') || label.textContent;
      }
    });
  }

  function init() {
    var root = document.querySelector('[data-lc-branding]');
    if (!root) return;
    colorInit(root);
    logoInit(root);
    submitInit(root);
    flashToast(root);
  }

  if (document.readyState !== 'loading') init();
  else document.addEventListener('DOMContentLoaded', init);
  document.body.addEventListener('htmx:afterSettle', function (e) {
    if (e.target && e.target.querySelector && e.target.querySelector('[data-lc-branding]')) init();
  });
})();
