// LC-550: Mermaid diagram rendering (progressive enhancement).
//
// Server-side markdown turns a ```mermaid fence into
//   <div class="lc-mermaid" data-lc-mermaid><pre class="lc-mermaid-src">SOURCE</pre></div>
// This loader finds those containers and swaps in an SVG. The heavy vendored
// mermaid bundle (~3.5MB) is fetched LAZILY - only once a page actually contains
// a diagram - so pages without one pay nothing. Framework-free, same posture as
// the other vendored client enhancements. With JS off or the bundle/parse
// failing, the <pre> source stays visible as a readable fallback.
(function () {
  'use strict';

  var loading = null; // Promise<mermaid>, created on first need.
  function loadMermaid() {
    if (window.mermaid) return Promise.resolve(window.mermaid);
    if (loading) return loading;
    loading = new Promise(function (resolve, reject) {
      var s = document.createElement('script');
      s.src = '/assets/vendor/mermaid.min.js';
      s.onload = function () {
        window.mermaid ? resolve(window.mermaid) : reject(new Error('mermaid missing'));
      };
      s.onerror = function () {
        reject(new Error('mermaid failed to load'));
      };
      document.head.appendChild(s);
    });
    return loading;
  }

  // Match the app mode so diagrams read correctly in light and dark. The
  // bootstrap stamps data-mode on <html> (data-theme now holds the palette,
  // not the mode); fall back to the OS preference.
  function mermaidTheme() {
    var mode = document.documentElement.getAttribute('data-mode');
    if (!mode && window.matchMedia) {
      mode = window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
    }
    var isDark = mode === 'dark' || mode === 'hc-dark';
    return isDark ? 'dark' : 'default';
  }

  var initialized = false;
  var seq = 0;

  function renderAll() {
    var blocks = document.querySelectorAll('[data-lc-mermaid]:not([data-lc-mermaid-done])');
    if (!blocks.length) return;
    loadMermaid()
      .then(function (mermaid) {
        if (!initialized) {
          // securityLevel:'strict' sanitizes labels and disables click/HTML in
          // diagrams - diagram source arrives from other users' messages.
          mermaid.initialize({
            startOnLoad: false,
            securityLevel: 'strict',
            theme: mermaidTheme(),
          });
          initialized = true;
        }
        blocks.forEach(function (block) {
          // Mark first, so a throwing render never retries in a loop.
          block.setAttribute('data-lc-mermaid-done', '');
          var src = block.querySelector('.lc-mermaid-src');
          var code = (src ? src.textContent : block.textContent) || '';
          if (!code.trim()) return;
          var id = 'lc-mmd-' + seq++;
          try {
            mermaid
              .render(id, code)
              .then(function (res) {
                block.innerHTML = res.svg;
              })
              .catch(function () {
                block.setAttribute('data-lc-mermaid-error', '');
              });
          } catch (e) {
            block.setAttribute('data-lc-mermaid-error', '');
          }
        });
      })
      .catch(function () {
        /* bundle failed to load; leave the <pre> source visible. */
      });
  }

  if (document.readyState !== 'loading') renderAll();
  else document.addEventListener('DOMContentLoaded', renderAll);
  // New messages (and any swapped-in content) arrive over HTMX; re-scan.
  document.addEventListener('htmx:afterSwap', renderAll);
})();
