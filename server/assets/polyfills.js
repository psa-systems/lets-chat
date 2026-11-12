// LC-1162: HTMLFormElement.requestSubmit() polyfill.
//
// Safari < 16 (WebKit < 614) has no `HTMLFormElement.prototype.requestSubmit`.
// That includes the system WebKit on macOS <= 12 (e.g. 12.7.4 ships Safari
// 15.6 / WebKit 613.3.9), which is what the Let's Chat desktop app's WKWebView
// uses there. The composer's Enter-to-send handler (and thread replies, the
// Cmd/Ctrl+Enter edit save, and in-call chat) call `event.preventDefault()`
// and then `form.requestSubmit()`; on those engines the call throws, so after
// the preventDefault the keypress does nothing at all - no send, no newline -
// and the user has to click Send. In a modern browser it just works, which is
// why it looked desktop-only.
//
// Define a spec-shaped fallback when the native method is missing: dispatch a
// real, cancelable `submit` event (which htmx's hx-post listens for), exactly
// as clicking the Send button does. Native `requestSubmit` is left untouched
// wherever it exists, so modern engines keep their built-in behaviour.
//
// Loaded first (before htmx and the app bundle) so the method is present before
// any inline handler can reach for it. This file is served straight off disk by
// the /assets ServeDir, like every other asset.
(function () {
  'use strict';
  if (typeof HTMLFormElement === 'undefined') return;
  if (typeof HTMLFormElement.prototype.requestSubmit === 'function') return;

  HTMLFormElement.prototype.requestSubmit = function (submitter) {
    if (submitter) {
      // Spec: the submitter must be a submit control owned by this form.
      if (submitter.form !== this) {
        var Err = window.DOMException || Error;
        throw new Err(
          "Failed to execute 'requestSubmit' on 'HTMLFormElement': " +
            'The specified element is not owned by this form element.',
          'NotFoundError'
        );
      }
      submitter.click();
      return;
    }
    // No explicit submitter: fire the form's `submit` event via a temporary,
    // hidden submit control. Clicking it runs constraint validation and
    // dispatches a cancelable `submit` event (which htmx handles and
    // preventDefault()s, so there is no full-page navigation), matching native
    // requestSubmit() closely enough for every call site in this app.
    var temp = document.createElement('input');
    temp.type = 'submit';
    temp.hidden = true;
    this.appendChild(temp);
    try {
      temp.click();
    } finally {
      this.removeChild(temp);
    }
  };
})();
