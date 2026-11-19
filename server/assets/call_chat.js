// LC-1127: in-call chat message behaviour (Enter-to-send + the new-message cue).
//
// The chat lives inside the unified call dock now (LC-1195); voice_dock.js owns
// the container - opening, closing, which tab shows - so this file no longer
// toggles or drags a standalone drawer. It keeps the two things that are about
// the MESSAGES rather than the container: Enter submits the composer, and a new
// message that arrives while the chat tab is not showing flashes, plays a soft
// sound, and badges the Chat tab (the unread cue, LC-1181).
//
// External (not an inline <script>) so its delegated document listeners survive
// htmx boosted navigation into the voice room. Everything no-ops on pages without
// the chat panel (#lc-call-chat).
(function () {
  'use strict';
  if (typeof document === 'undefined') return;

  function panel() { return document.getElementById('lc-call-chat'); }
  function messagesEl() { return document.getElementById('messages'); }
  function setMessagesBottom() { var m = messagesEl(); if (m) m.scrollTop = m.scrollHeight; }

  // "Is the chat actually showing?" The dock is the source of truth; fall back to
  // the panel's own visibility if the dock JS is somehow absent.
  function chatVisible() {
    if (window.LetsChatDock && typeof window.LetsChatDock.isChatVisible === 'function') {
      return window.LetsChatDock.isChatVisible();
    }
    var p = panel();
    return !!p && !p.classList.contains('hidden');
  }
  function setUnread(on) {
    if (window.LetsChatDock && typeof window.LetsChatDock.setUnread === 'function') {
      window.LetsChatDock.setUnread(on);
    }
  }

  // LC-1210: a new message while the chat is hidden bumps the unread COUNT on the
  // Chat bubble (how many), rather than just lighting a dot.
  function bumpUnread() {
    if (window.LetsChatDock && typeof window.LetsChatDock.bumpUnread === 'function') {
      window.LetsChatDock.bumpUnread();
    } else {
      setUnread(true);
    }
  }

  // Enter submits / Shift+Enter newline, scoped to the chat form's body.
  document.addEventListener('keydown', function (e) {
    if (e.key !== 'Enter' || e.shiftKey) return;
    var t = e.target;
    if (!t || t.name !== 'body' || !t.closest) return;
    var form = t.closest('[data-lc-call-chat-form]');
    if (!form) return;
    e.preventDefault();
    form.requestSubmit();
  });

  // Keep the log pinned to newest while the chat is showing.
  document.addEventListener('htmx:wsAfterMessage', function () { if (chatVisible()) setMessagesBottom(); });

  // ---- LC-1181: new-message cue -----------------------------------------
  // During a call the chat is easy to miss (another tab showing, or you are
  // watching the video), so a new message gave no sign. Flash the row (your own
  // send included - the confirmation the report asked for), and when the message
  // arrives while the chat tab is NOT showing play a soft sound and badge the Chat
  // tab. All scoped to the in-call chat: the MutationObserver only attaches when
  // #lc-call-chat is on the page, so regular rooms are untouched.
  var SEEN = Object.create(null); // msg id -> true; skip edit re-appends
  var observed = null;            // the #messages node currently observed

  function flash(node) {
    if (!node || !node.classList) return;
    node.classList.add('lc-msg-flash'); // reduced-motion safe (main.css)
    setTimeout(function () { node.classList.remove('lc-msg-flash'); }, 2000);
  }

  function onNewMessage(node) {
    flash(node);
    var visible = chatVisible();
    // "Not looking": chat tab not showing, tab hidden, or window unfocused.
    var away = !visible || document.hidden ||
      (typeof document.hasFocus === 'function' && !document.hasFocus());
    if (away && window.LetsChatSfx && window.LetsChatSfx.playMessage) {
      window.LetsChatSfx.playMessage(); // self-gates on the sound preference
    }
    if (!visible) bumpUnread();
  }

  function handleMutations(records) {
    for (var i = 0; i < records.length; i++) {
      var added = records[i].addedNodes;
      for (var j = 0; j < added.length; j++) {
        var n = added[j];
        if (!n || n.nodeType !== 1 || !n.id || n.id.indexOf('msg-') !== 0) continue;
        if (SEEN[n.id]) continue; // an edit re-appended the same id: not new
        SEEN[n.id] = true;
        onNewMessage(n);
      }
    }
  }

  function observeMessages() {
    if (typeof MutationObserver === 'undefined') return;
    if (!panel()) return; // in-call chat only
    var m = messagesEl();
    if (!m || m === observed) return;
    if (window.__lcCallChatObs) window.__lcCallChatObs.disconnect();
    observed = m;
    window.__lcCallChatObs = new MutationObserver(handleMutations);
    window.__lcCallChatObs.observe(m, { childList: true });
  }

  observeMessages();
  // #messages can be replaced by a boosted navigation into the voice room.
  document.addEventListener('htmx:afterSettle', observeMessages);
})();
