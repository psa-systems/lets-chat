// LC-1195: the unified call dock for the voice-channel page.
//
// It replaces three separate floating panels - the in-call chat (#lc-call-chat),
// the live transcript, and the admin diagnostics panel - with ONE right-edge
// dock that has a tab strip (Chat / Transcript / Diagnostics). The three panels
// used to fight for the same corners and overlap; one dock gives them a single
// predictable home and makes overlap structurally impossible.
//
// Ownership:
//   - The chat and diagnostics BODIES live inside the dock (voice/page.html);
//     they are voice-page-only, so moving them is safe. call_chat.js still owns
//     the message behaviour (Enter-to-send + the new-message cue) and asks this
//     module whether the chat tab is showing.
//   - The transcript body stays SHARED in layout.html (1:1 calls + huddles use it
//     too). Its tab drives that drawer in place via the transcribe.js API
//     (showPanel / isPanelOpen); CSS (body.lc-has-call-dock) aligns the shared
//     drawer to the dock column while its tab is active. We never move or
//     duplicate it, so the 1:1-call transcript is untouched.
//
// The single source of truth for "is the transcript showing" is transcribe.js
// (body.lc-transcript-open); a MutationObserver mirrors that into the tab state,
// so the transcript auto-opening when a session starts selects its tab for free.
//
// Everything is delegated off the document and no-ops on pages without a dock, so
// it survives htmx boosted navigation into the voice room like the other call JS.
(function () {
  'use strict';
  if (typeof document === 'undefined') return;

  var TAB_KEY = 'lc-call-dock-tab'; // remembers the last chat/diag body tab

  // The strip's markup follows the shared tablist controller's contract, but
  // those markup attributes are reserved to that controller's own script
  // alone (check-single-tab-controller.nu); every button also carries a
  // stable id, so this module finds and orders them by id instead of ever
  // touching the attributes itself.
  var TAB_ORDER = ['chat', 'transcript', 'diag'];

  function dock() { return document.querySelector('[data-lc-call-dock]'); }
  function tabBtn(name) { return document.getElementById('lc-dock-tab-' + name); }
  function hasTab(name) { return !!tabBtn(name); }

  // The tabs currently present (diag only when the flag/admin gate rendered
  // it), in strip order, each paired with its name for the keyboard/click
  // handlers below.
  function dockTabs() {
    return TAB_ORDER.map(function (name) { return { name: name, el: tabBtn(name) }; })
      .filter(function (t) { return !!t.el; });
  }

  // The transcript is owned by transcribe.js; read/drive it through its API, with
  // a body-class fallback so a missing API never wedges the dock.
  function transcriptApi() { return window.LetsChatTranscribe; }
  function isTranscriptOpen() {
    var api = transcriptApi();
    if (api && typeof api.isPanelOpen === 'function') return api.isPanelOpen();
    return document.body.classList.contains('lc-transcript-open');
  }
  function showTranscript(on) {
    var api = transcriptApi();
    if (api && typeof api.showPanel === 'function') api.showPanel(on);
  }

  function isDockOpen() { var d = dock(); return !!d && !d.classList.contains('hidden'); }

  // lastBodyTab is the non-transcript tab (chat or diag) that shows when the
  // transcript is closed. Restored from storage, clamped to a tab that exists.
  var lastBodyTab = 'chat';
  try {
    var saved = localStorage.getItem(TAB_KEY);
    if (saved === 'chat' || saved === 'diag') lastBodyTab = saved;
  } catch (e) { /* storage blocked: keep the default */ }
  function setLastBodyTab(name) {
    lastBodyTab = name;
    try { localStorage.setItem(TAB_KEY, name); } catch (e) {}
  }

  function activeTab() {
    if (isTranscriptOpen()) return 'transcript';
    if (lastBodyTab === 'diag' && hasTab('diag')) return 'diag';
    return 'chat';
  }

  function scrollChatToBottom() {
    // LC-1214: the in-call chat list is room-scoped (lc-call-messages-{room}),
    // found by its stable attribute within the dock, not the generic #messages.
    var m = document.querySelector('#lc-call-chat [data-lc-messages]');
    if (m) m.scrollTop = m.scrollHeight;
  }

  // Reflect the current state onto the dock UI. Pure mirror: safe to call from
  // either a user action or the transcript observer, and idempotent. `focus`
  // only matters for the tab strip's own aria-selected/roving-tabindex
  // bookkeeping (driven through the shared controller's exported
  // lcSelectTab): true moves the browser focus onto the newly active tab
  // button, for arrow-key navigation across the strip.
  function render(focus) {
    var d = dock();
    if (!d) return;
    var transcript = isTranscriptOpen();
    // The transcript opening on its own (a session starting) must reveal the dock.
    if (transcript && d.classList.contains('hidden')) ensureOpen();
    var active = activeTab();
    // Tab buttons (aria-selected + roving tabindex) and the chat/diag body
    // panels (visible only when their tab is active; "transcript" never
    // matches either panel's key, so both are hidden together while it shows,
    // same as the explicit transcript check this replaced).
    window.lcSelectTab(d, active, focus);
    // CSS hides the dock body + aligns the shared transcript to the column.
    d.classList.toggle('lc-dock--transcript', transcript);
    if (!transcript && active === 'chat') { clearUnread(); scrollChatToBottom(); }
  }

  function ensureOpen() {
    var d = dock();
    if (!d) return;
    d.classList.remove('hidden');
    document.body.classList.add('lc-dock-open'); // CSS reserves the column on #main
  }

  // `focus` is set only by the tab strip's own arrow-key navigation below: it
  // moves the browser focus onto the newly active tab button (the roving-
  // tabindex contract every other tablist in the app follows), and for that
  // reason must NOT also steal focus into the chat composer, which a plain
  // click/toolbar-toggle activation still does.
  function openTo(tab, focus) {
    if (!dock()) return;
    ensureOpen();
    if (tab === 'transcript') {
      showTranscript(true); // observer -> render; call render now for immediacy
      render(focus);
      return;
    }
    if (tab === 'diag' && !hasTab('diag')) tab = 'chat';
    setLastBodyTab(tab);
    if (isTranscriptOpen()) showTranscript(false); // leaving the transcript tab closes the drawer
    render(focus);
    if (tab === 'chat' && !focus) { var t = document.querySelector('#lc-call-chat textarea[name="body"]'); if (t) try { t.focus(); } catch (e) {} }
  }

  function collapse() {
    var d = dock();
    if (!d) return;
    if (isTranscriptOpen()) showTranscript(false);
    d.classList.add('hidden');
    document.body.classList.remove('lc-dock-open');
  }

  // A toolbar toggle (Chat / Transcript): open the dock to that tab, or collapse
  // if it is already the showing tab.
  function toggleTab(tab) {
    if (isDockOpen() && activeTab() === tab) collapse();
    else openTo(tab);
  }

  // LC-1210: the unread cue on the ALWAYS-VISIBLE callbar Chat toggle (the bubble
  // button in the call toolbar), not only the dock's Chat tab. The tab lives
  // inside the dock, which is hidden while the dock is collapsed - the common
  // case - so before this a new in-call message gave no visible sign (regression
  // from the LC-1195 dock move of the LC-1181 cue). Now the bubble lights up and
  // shows HOW MANY messages are unread: each one bumps the count, opening the
  // chat clears it.
  var unreadCount = 0;

  function renderUnread() {
    var on = unreadCount > 0;
    var toggle = document.querySelector('[data-lc-chat-toggle]');
    if (toggle) {
      if (on) toggle.setAttribute('data-lc-chat-unread', '');
      else toggle.removeAttribute('data-lc-chat-unread');
    }
    var count = document.querySelector('[data-lc-chat-unread-count]');
    if (count) {
      // Cap the display so a long absence doesn't widen the pill off the button.
      count.textContent = unreadCount > 99 ? '99+' : String(unreadCount);
      if (on) {
        count.removeAttribute('hidden');
        count.setAttribute('aria-hidden', 'false');
      } else {
        count.setAttribute('hidden', '');
        count.setAttribute('aria-hidden', 'true');
      }
    }
    // Keep the in-dock tab dot in step for when the dock is open.
    var dot = document.querySelector('[data-lc-dock-unread]');
    if (dot) {
      if (on) dot.removeAttribute('hidden');
      else dot.setAttribute('hidden', '');
    }
  }

  function bumpUnread() { unreadCount += 1; renderUnread(); }
  function clearUnread() { unreadCount = 0; renderUnread(); }
  // Back-compat shim: on(true) counts as a single new message, on(false) clears.
  function setUnread(on) { if (on) bumpUnread(); else clearUnread(); }

  // ---- events -----------------------------------------------------------

  // Tab strip + collapse (inside the dock). Tab buttons are found by role and
  // id, never by the shared tablist markup attributes (reserved to tabs.js).
  document.addEventListener('click', function (e) {
    if (!e.target || !e.target.closest) return;
    var d = dock();
    if (!d) return;
    var tabEl = e.target.closest('[role="tab"]');
    var tab = tabEl && d.contains(tabEl) && dockTabs().filter(function (t) { return t.el === tabEl; })[0];
    if (tab) { openTo(tab.name); return; }
    if (e.target.closest('[data-lc-dock-collapse]')) { collapse(); return; }
    var chatToggle = e.target.closest('[data-lc-chat-toggle]');
    if (chatToggle) { e.preventDefault(); toggleTab('chat'); return; }
  });

  // Roving-tabindex arrow-key navigation across the strip, matching the
  // contract used by settings/enclave/room-info - the dock can't call
  // window.lcInitTabs itself (its selection has to route through
  // showTranscript/diag-fallback/unread, which the shared controller has no
  // concept of), so this reimplements only the key-to-next-tab mapping, then
  // drives the actual activation through the same openTo() a click uses.
  document.addEventListener('keydown', function (e) {
    if (!e.target || !e.target.closest) return;
    var d = dock();
    if (!d) return;
    var current = e.target.closest('[role="tab"]');
    if (!current || !d.contains(current)) return;
    var tabs = dockTabs();
    var idx = -1;
    for (var i = 0; i < tabs.length; i++) { if (tabs[i].el === current) { idx = i; break; } }
    if (idx < 0) return;
    var nextIdx = null;
    if (e.key === 'ArrowDown' || e.key === 'ArrowRight') nextIdx = (idx + 1) % tabs.length;
    else if (e.key === 'ArrowUp' || e.key === 'ArrowLeft') nextIdx = (idx - 1 + tabs.length) % tabs.length;
    else if (e.key === 'Home') nextIdx = 0;
    else if (e.key === 'End') nextIdx = tabs.length - 1;
    if (nextIdx === null) return;
    e.preventDefault();
    openTo(tabs[nextIdx].name, true);
  });

  // The transcript toolbar toggle is also handled by transcribe.js (it toggles
  // the shared drawer). On the voice page the dock owns it, so intercept in the
  // CAPTURE phase and stop propagation BEFORE transcribe.js's bubble-phase handler
  // runs - otherwise both would toggle and cancel out. Only when a dock exists;
  // elsewhere (1:1 call, huddle) this is a no-op and transcribe.js handles it.
  document.addEventListener('click', function (e) {
    if (!e.target || !e.target.closest || !dock()) return;
    var pt = e.target.closest('[data-lc-transcript-panel-toggle]');
    if (!pt) return;
    e.stopPropagation();
    e.preventDefault();
    toggleTab('transcript');
  }, true);

  // Mirror the transcript's own open/close (auto-open on session start, its close
  // button, session end) into the tab state.
  if (typeof MutationObserver !== 'undefined') {
    var mo = new MutationObserver(function () { if (dock()) render(); });
    mo.observe(document.body, { attributes: true, attributeFilter: ['class'] });
  }

  // Call teardown: a left call collapses the dock (the toggles hide on leave, so an
  // open dock would otherwise linger into the lobby).
  document.addEventListener('lc:rtc-session-ended', function () { collapse(); clearUnread(); });

  // The shared transcript drawer (layout.html) only has a dock tab to pair
  // with on pages that render one; elsewhere (1:1 call, huddle) pairing it
  // would leave a dangling aria-labelledby and an orphaned tabpanel role, so
  // this module - the only script that knows whether a dock is on the page -
  // adds and removes both attributes itself instead of layout.html setting
  // them unconditionally.
  function syncTranscriptPanelRole() {
    var drawer = document.getElementById('lc-transcript-drawer');
    if (!drawer) return;
    if (dock() && hasTab('transcript')) {
      drawer.setAttribute('role', 'tabpanel');
      drawer.setAttribute('aria-labelledby', 'lc-dock-tab-transcript');
    } else {
      drawer.removeAttribute('role');
      drawer.removeAttribute('aria-labelledby');
    }
  }

  // Mark the page so CSS can align the shared transcript to the dock column, and
  // set the initial (closed) state. Re-run after boosted navigation swaps #main.
  function sync() {
    syncTranscriptPanelRole();
    if (dock()) {
      document.body.classList.add('lc-has-call-dock');
      if (isDockOpen()) render();
    } else {
      document.body.classList.remove('lc-has-call-dock', 'lc-dock-open');
    }
  }
  sync();
  document.addEventListener('htmx:afterSettle', sync);

  // Exposed for call_chat.js (the new-message cue): is the chat tab actually
  // showing, and the unread dot on the Chat tab.
  window.LetsChatDock = {
    openTo: openTo,
    collapse: collapse,
    activeTab: activeTab,
    isChatVisible: function () { return isDockOpen() && !isTranscriptOpen() && activeTab() === 'chat'; },
    setUnread: setUnread,
    bumpUnread: bumpUnread,
    clearUnread: clearUnread,
    // LC-1195: pure tab resolver, exported for unit tests.
    resolveTab: function (transcriptOpen, bodyTab, diagPresent) {
      if (transcriptOpen) return 'transcript';
      if (bodyTab === 'diag' && diagPresent) return 'diag';
      return 'chat';
    },
  };
})();
