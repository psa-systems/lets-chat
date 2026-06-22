// LC-426: minimal toast notifications.
//
// Server form handlers return a small fragment that appends a toast into
// #lc-toast-region via hx-swap-oob="beforeend:#lc-toast-region". This module
// watches that region and auto-dismisses each toast after a few seconds, with a
// manual close button. No dependency on htmx internals beyond the DOM insert.
(function () {
  'use strict';

  var REGION_ID = 'lc-toast-region';
  var TIMEOUT_MS = 4500;

  function dismiss(toast) {
    if (!toast || toast.getAttribute('data-lc-leaving')) return;
    toast.setAttribute('data-lc-leaving', '1');
    toast.classList.add('lc-toast--leaving');
    var done = function () { if (toast.parentNode) toast.parentNode.removeChild(toast); };
    var fired = false;
    toast.addEventListener('animationend', function () { if (!fired) { fired = true; done(); } }, { once: true });
    // Fallback in case the animation is suppressed (reduced motion) or skipped.
    setTimeout(function () { if (!fired) { fired = true; done(); } }, 240);
  }

  function arm(toast) {
    if (!toast || toast.getAttribute('data-lc-armed')) return;
    toast.setAttribute('data-lc-armed', '1');
    var timer = setTimeout(function () { dismiss(toast); }, TIMEOUT_MS);
    // Keep it up while hovered, so a user reading it is not rushed.
    toast.addEventListener('mouseenter', function () { clearTimeout(timer); });
    toast.addEventListener('mouseleave', function () { timer = setTimeout(function () { dismiss(toast); }, 1500); });
  }

  function scan(root) {
    var region = document.getElementById(REGION_ID);
    if (!region) return;
    var nodes = (root && root.querySelectorAll) ? root.querySelectorAll('[data-lc-toast]') : region.querySelectorAll('[data-lc-toast]');
    Array.prototype.forEach.call(nodes, arm);
  }

  // Manual close (event-delegated so it survives re-renders).
  document.addEventListener('click', function (e) {
    if (!e.target.closest) return;
    var btn = e.target.closest('[data-lc-toast-close]');
    if (!btn) return;
    dismiss(btn.closest('[data-lc-toast]'));
  });

  function init() {
    var region = document.getElementById(REGION_ID);
    if (!region) return;
    scan(region);
    if (window.MutationObserver) {
      new MutationObserver(function (muts) {
        for (var i = 0; i < muts.length; i++) {
          Array.prototype.forEach.call(muts[i].addedNodes, function (n) {
            if (n.nodeType !== 1) return;
            if (n.matches && n.matches('[data-lc-toast]')) arm(n);
            scan(n);
          });
        }
      }).observe(region, { childList: true });
    }
  }

  // Client-side toast, for actions that are not a plain htmx form (a file
  // download, an avatar preview confirmation, a JS-side error). Mirrors the
  // server fragment markup so styling/auto-dismiss are identical.
  window.__lcToast = function (kind, msg) {
    var region = document.getElementById(REGION_ID);
    if (!region) return;
    var ok = kind !== 'err';
    var toast = document.createElement('div');
    toast.className = 'lc-toast ' + (ok ? 'lc-toast--ok' : 'lc-toast--err');
    toast.setAttribute('role', 'status');
    toast.setAttribute('data-lc-toast', '');
    var ico = document.createElement('span');
    ico.className = 'lc-toast-ico';
    ico.setAttribute('aria-hidden', 'true');
    ico.textContent = ok ? '✓' : '!';
    var text = document.createElement('span');
    text.className = 'lc-toast-msg';
    text.textContent = msg;
    var close = document.createElement('button');
    close.type = 'button';
    close.className = 'lc-toast-close';
    close.setAttribute('data-lc-toast-close', '');
    close.setAttribute('aria-label', (window.__lcS && window.__lcS('dismiss', 'Dismiss')) || 'Dismiss');
    close.innerHTML = '&times;';
    toast.appendChild(ico);
    toast.appendChild(text);
    toast.appendChild(close);
    region.appendChild(toast);
    arm(toast);
  };

  if (document.readyState !== 'loading') init();
  else document.addEventListener('DOMContentLoaded', init);
})();
