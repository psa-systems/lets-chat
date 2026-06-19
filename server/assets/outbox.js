// lets-chat PWA / offline-outbox page client.
//
// Registers the service worker unconditionally (independent of Web Push),
// which is what makes the app installable and gives it an offline shell.
// Then it acts as the UI half of the offline outbox: it relays drain /
// retry / discard commands to the SW and renders the SW's queue state into
// a global banner (#lc-outbox) plus an inline composer hint
// (#lc-composer-net) when present.
//
// The SW owns the IndexedDB queue and is the single source of truth; this
// script holds no persistent state and simply re-renders whatever
// `lc-outbox-state` message the SW broadcasts.
(function () {
  if (!('serviceWorker' in navigator)) return;

  function sw() {
    return navigator.serviceWorker.controller;
  }
  function send(msg) {
    var c = sw();
    if (c) c.postMessage(msg);
  }
  function drain() {
    send({ type: 'lc-outbox-drain' });
  }

  navigator.serviceWorker
    .register('/sw.js', { scope: '/' })
    .then(function () {
      send({ type: 'lc-outbox-get' });
      if (navigator.onLine) drain();
    })
    .catch(function (e) {
      console.warn('service worker registration failed', e);
    });

  // First load: the SW activates and claims the page, firing
  // controllerchange. Re-request state + drain once it is controlling us.
  navigator.serviceWorker.addEventListener('controllerchange', function () {
    send({ type: 'lc-outbox-get' });
    if (navigator.onLine) drain();
  });

  navigator.serviceWorker.addEventListener('message', function (ev) {
    var d = ev.data || {};
    if (d.type === 'lc-outbox-state') render(d.pending || [], d.failed || []);
  });

  window.addEventListener('online', function () {
    paintOnline(true);
    drain();
  });
  window.addEventListener('offline', function () {
    paintOnline(false);
  });

  // Retry / discard via event delegation so the buttons survive HTMX swaps.
  document.addEventListener('click', function (e) {
    var retry = e.target.closest('[data-lc-outbox-retry]');
    if (retry) {
      send({ type: 'lc-outbox-retry', id: Number(retry.getAttribute('data-lc-outbox-retry')) });
      return;
    }
    var discard = e.target.closest('[data-lc-outbox-discard]');
    if (discard) {
      send({ type: 'lc-outbox-discard', id: Number(discard.getAttribute('data-lc-outbox-discard')) });
    }
  });

  function escapeHtml(s) {
    return String(s).replace(/[&<>"']/g, function (c) {
      return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
    });
  }

  var offline = false;

  function paintOnline(on) {
    offline = !on;
    // Re-render with current banner contents; ask SW for fresh state too.
    send({ type: 'lc-outbox-get' });
  }

  function render(pending, failed) {
    var banner = document.getElementById('lc-outbox');
    var hint = document.getElementById('lc-composer-net');

    if (hint) {
      if (offline && pending.length) {
        hint.textContent = window.__lcS('outboxOfflineQueued', 'Offline - %n% message(s) queued').replace('%n%', pending.length);
        hint.classList.remove('hidden');
      } else if (offline) {
        hint.textContent = window.__lcS('outboxOfflineIdle', 'Offline - messages will send when you reconnect');
        hint.classList.remove('hidden');
      } else if (pending.length) {
        hint.textContent = window.__lcS('outboxSending', 'Sending %n% queued message(s)…').replace('%n%', pending.length);
        hint.classList.remove('hidden');
      } else {
        hint.textContent = '';
        hint.classList.add('hidden');
      }
    }

    if (!banner) return;
    if (!failed.length && !pending.length && !offline) {
      banner.classList.add('hidden');
      banner.innerHTML = '';
      return;
    }

    var html = '';
    if (offline || pending.length) {
      var msg = offline
        ? (pending.length
            ? window.__lcS('outboxQueuedOffline', '%n% message(s) queued while offline').replace('%n%', pending.length)
            : window.__lcS('outboxYouAreOffline', 'You are offline'))
        : window.__lcS('outboxDelivering', 'Delivering %n% queued message(s)…').replace('%n%', pending.length);
      html +=
        '<div class="px-3 py-2 text-xs text-slate-700">' + escapeHtml(msg) + '</div>';
    }
    if (failed.length) {
      html += '<ul class="divide-y border-t border-slate-200">';
      for (var i = 0; i < failed.length; i++) {
        var f = failed[i];
        html +=
          '<li class="px-3 py-2 text-xs flex items-center gap-2">' +
          '<span class="text-danger">' + window.__lcS('outboxFailed', 'Failed (%status%):').replace('%status%', function () { return escapeHtml(f.status); }) + '</span> ' +
          '<span class="flex-1 truncate text-slate-700">' + escapeHtml(f.label) + '</span>' +
          '<button data-lc-outbox-retry="' + escapeHtml(f.id) + '" class="underline hover:no-underline">' + window.__lcS('outboxRetry', 'Retry') + '</button>' +
          '<button data-lc-outbox-discard="' + escapeHtml(f.id) + '" class="underline hover:no-underline text-slate-500">' + window.__lcS('outboxDiscard', 'Discard') + '</button>' +
          '</li>';
      }
      html += '</ul>';
    }
    banner.innerHTML = html;
    banner.classList.remove('hidden');
  }

  // Reflect initial connectivity.
  if (!navigator.onLine) offline = true;
})();
