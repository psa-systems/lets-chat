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

  function dock() { return document.querySelector('[data-lc-call-dock]'); }
  function tabBtn(name) { return document.querySelector('[data-lc-dock-tab="' + name + '"]'); }
  function panel(name) { return document.querySelector('[data-lc-dock-panel="' + name + '"]'); }
  function hasTab(name) { return !!tabBtn(name); }

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
    var m = document.getElementById('messages');
    if (m) m.scrollTop = m.scrollHeight;
  }

  // Reflect the current state onto the dock UI. Pure mirror: safe to call from
  // either a user action or the transcript observer, and idempotent.
  function render() {
    var d = dock();
    if (!d) return;
    var transcript = isTranscriptOpen();
    // The transcript opening on its own (a session starting) must reveal the dock.
    if (transcript && d.classList.contains('hidden')) ensureOpen();
    var active = activeTab();
    // Tab buttons.
    ['chat', 'transcript', 'diag'].forEach(function (name) {
      var b = tabBtn(name);
      if (b) b.setAttribute('aria-selected', name === active ? 'true' : 'false');
    });
    // Body panels: visible only when their tab is active AND the transcript (which
    // overlays the dock body via CSS) is not showing.
    ['chat', 'diag'].forEach(function (name) {
      var p = panel(name);
      if (p) p.classList.toggle('hidden', transcript || name !== active);
    });
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

  function openTo(tab) {
    if (!dock()) return;
    ensureOpen();
    if (tab === 'transcript') {
      showTranscript(true); // observer -> render; call render now for immediacy
      render();
      return;
    }
    if (tab === 'diag' && !hasTab('diag')) tab = 'chat';
    setLastBodyTab(tab);
    if (isTranscriptOpen()) showTranscript(false); // leaving the transcript tab closes the drawer
    render();
    if (tab === 'chat') { var t = document.querySelector('#lc-call-chat textarea[name="body"]'); if (t) try { t.focus(); } catch (e) {} }
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

  // Tab strip + collapse (inside the dock).
  document.addEventListener('click', function (e) {
    if (!e.target || !e.target.closest) return;
    if (!dock()) return;
    var tab = e.target.closest('[data-lc-dock-tab]');
    if (tab) { openTo(tab.getAttribute('data-lc-dock-tab')); return; }
    if (e.target.closest('[data-lc-dock-collapse]')) { collapse(); return; }
    var chatToggle = e.target.closest('[data-lc-chat-toggle]');
    if (chatToggle) { e.preventDefault(); toggleTab('chat'); return; }
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

  // Mark the page so CSS can align the shared transcript to the dock column, and
  // set the initial (closed) state. Re-run after boosted navigation swaps #main.
  function sync() {
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
