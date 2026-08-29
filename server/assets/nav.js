// LC-837: boosted navigation in the persistent shell.
//
// Links in the nav panel (enclave switcher, sidebar, account menu) carry
// hx-boost targeting #main (partials/nav_boost.html), so a page move swaps
// only <main id="main"> and the ws-connect wrapper, the sidebar and every
// running script survive it. This file is the glue htmx does not provide:
//
// - A boosted response that is not one of our pages (the login page after the
//   session expired, an error page, a 4xx/5xx) has no #main to select, and
//   htmx would swap nothing or the wrong thing. Cancel the swap and make it a
//   real navigation to the URL the response came from.
// - The branding <style> lives in <head>, which a #main swap never touches.
//   Carry it over from the response so a move into a scope with its own
//   branding recolors without a page load.
// - The mobile nav panel is an overlay; a full load closed it for free.
// - History: the cache is 0, so back/forward re-fetch the page (htmx sends
//   HX-History-Restore-Request and swaps the response's hx-history-elt into
//   #main). A cached snapshot would show a room as it was when the user left
//   it, and none of the messages since.
//
// The two decisions that need no DOM are exported on window.LetsChatNav so
// nav.test.js can pin them.
(function () {
  function hasMain(html) {
    return /<main\b[^>]*\bid="main"/.test(String(html || ''));
  }

  function brandCss(html) {
    var m = /<style data-lc-brand>([\s\S]*?)<\/style>/.exec(String(html || ''));
    return m ? m[1] : null;
  }

  // Where to send the browser when a boosted response cannot be swapped: the
  // URL the response actually came from (after redirects), then the request.
  function fallbackUrl(detail) {
    var xhr = detail && detail.xhr;
    if (xhr && xhr.responseURL) return xhr.responseURL;
    var p = detail && detail.pathInfo;
    if (p && (p.responsePath || p.finalRequestPath)) return p.responsePath || p.finalRequestPath;
    return detail && detail.requestConfig && detail.requestConfig.path;
  }

  function isBoosted(detail) {
    return !!(detail && (detail.boosted || (detail.requestConfig && detail.requestConfig.boosted)));
  }

  function isMainSwap(evt) {
    var swapped = evt.target;
    var requested = evt.detail && evt.detail.target;
    return (swapped && swapped.id === 'main') || (requested && requested.id === 'main');
  }

  if (window.htmx && window.htmx.config) {
    window.htmx.config.historyCacheSize = 0;
  }

  document.body.addEventListener('htmx:beforeSwap', function (evt) {
    var d = evt.detail;
    if (!isBoosted(d)) return;
    var xhr = d.xhr;
    var html = xhr ? xhr.responseText : '';
    if ((xhr && xhr.status >= 400) || !hasMain(html)) {
      d.shouldSwap = false;
      var url = fallbackUrl(d);
      if (url) window.location.assign(url);
      return;
    }
    var css = brandCss(html);
    if (css !== null) {
      var cur = document.head.querySelector('style[data-lc-brand]');
      if (cur) {
        cur.textContent = css;
      } else {
        var el = document.createElement('style');
        el.setAttribute('data-lc-brand', '');
        el.textContent = css;
        document.head.appendChild(el);
      }
    }
  });

  document.body.addEventListener('htmx:afterSwap', function (evt) {
    if (isMainSwap(evt) && typeof window.lcCloseNav === 'function') window.lcCloseNav();
  });
  document.body.addEventListener('htmx:historyRestore', function () {
    if (typeof window.lcCloseNav === 'function') window.lcCloseNav();
  });

  window.LetsChatNav = { hasMain: hasMain, brandCss: brandCss, fallbackUrl: fallbackUrl };
})();
