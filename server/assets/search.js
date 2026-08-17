// LC-157: shared keyboard navigation + ARIA for the HTMX-driven search bars
// (sidebar people / messages, enclave invite, group add-members).
//
// HTMX still owns the debounced fetch + result swap; this module only layers a
// consistent interaction on top of whatever the results contain, so each
// search keeps its own result semantics (a link to navigate to, or an inline
// form to submit). An input opts in with:
//
//   data-lc-search  data-lc-search-results="#results-container-id"
//
// and each result item carries role="option" with a unique id. The module is
// loaded once and uses delegated listeners, so it survives HTMX swaps without
// re-binding.
(function () {
  // LC-735: token, not a raw palette shade, so the highlight recolors per
  // mode. Matches `.lc-search-row[aria-selected="true"]` in main.css.
  var ACTIVE_CLASS = 'bg-surface-sunken';

  function resultsContainer(input) {
    var sel = input.getAttribute('data-lc-search-results');
    return sel ? document.querySelector(sel) : null;
  }

  function options(container) {
    if (!container) return [];
    return Array.prototype.slice.call(container.querySelectorAll('[role="option"]'));
  }

  function activeIndex(opts) {
    for (var i = 0; i < opts.length; i++) {
      if (opts[i].getAttribute('aria-selected') === 'true') return i;
    }
    return -1;
  }

  function setActive(input, opts, idx) {
    opts.forEach(function (o) {
      o.setAttribute('aria-selected', 'false');
      o.classList.remove(ACTIVE_CLASS);
    });
    if (idx >= 0 && idx < opts.length) {
      var o = opts[idx];
      o.setAttribute('aria-selected', 'true');
      o.classList.add(ACTIVE_CLASS);
      if (o.id) input.setAttribute('aria-activedescendant', o.id);
      if (o.scrollIntoView) o.scrollIntoView({ block: 'nearest' });
    } else {
      input.removeAttribute('aria-activedescendant');
    }
  }

  // Activate the option: click it if it is itself a link/button, else the first
  // link or button inside it (the per-row Invite/Add form submit, or the row's
  // navigation anchor). Non-actionable rows (already-member pills) no-op.
  function activate(opt) {
    if (!opt) return false;
    var target =
      opt.matches('a, button') ? opt : opt.querySelector('a, button');
    if (target) {
      target.click();
      return true;
    }
    return false;
  }

  document.body.addEventListener('keydown', function (evt) {
    var input = evt.target;
    if (!input || !input.matches || !input.matches('[data-lc-search]')) return;
    var container = resultsContainer(input);
    var opts = options(container);

    if (evt.key === 'ArrowDown') {
      if (!opts.length) return;
      evt.preventDefault();
      var i = activeIndex(opts);
      setActive(input, opts, (i + 1) % opts.length);
    } else if (evt.key === 'ArrowUp') {
      if (!opts.length) return;
      evt.preventDefault();
      var j = activeIndex(opts);
      setActive(input, opts, j <= 0 ? opts.length - 1 : j - 1);
    } else if (evt.key === 'Enter') {
      var k = activeIndex(opts);
      // Only intercept Enter when an option is highlighted; otherwise let the
      // input's own keyup[Enter] HTMX trigger re-run the search.
      if (k >= 0) {
        evt.preventDefault();
        activate(opts[k]);
      }
    } else if (evt.key === 'Escape') {
      if (opts.length) {
        evt.preventDefault();
        setActive(input, opts, -1);
        input.setAttribute('aria-expanded', 'false');
      }
    }
  });

  // After each result swap, reflect whether the listbox is populated and clear
  // any stale highlight (the previous active option no longer exists).
  document.body.addEventListener('htmx:afterSettle', function (evt) {
    document.querySelectorAll('[data-lc-search]').forEach(function (input) {
      var container = resultsContainer(input);
      if (!container || !container.contains(evt.target)) return;
      var opts = options(container);
      input.setAttribute('aria-expanded', opts.length ? 'true' : 'false');
      input.removeAttribute('aria-activedescendant');
    });
  });
})();
