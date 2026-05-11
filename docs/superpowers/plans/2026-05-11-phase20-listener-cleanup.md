# Plan Phase 20: Fix listener accumulation on page navigation

Bug discovered after phase 19: inline `<script>` blocks rendered as part of HTMX-swapped fragments leave behind listeners and observers each time their fragment is re-rendered. The result is duplicate handlers, slow memory growth, and eventually multi-fire bugs (one click triggers N copies of an action).

This phase audits every inline script in the templates, applies the htmx-native `htmx:beforeCleanupElement` teardown pattern to the leaky ones, and writes the rule down in CLAUDE.md so future inline scripts follow it.

## Background: where listeners actually accumulate

The brief assumed "every HTMX navigation" causes the leak. Reading the templates first shows a narrower picture.

There is **no `hx-boost` anywhere in this app** (`grep "hx-boost" server/templates/` returns nothing). Sidebar links (`/room/{id}`, `/dm/{peer}`) are plain `<a href="...">`. Clicking them does a **full browser navigation**, which throws away all DOM and JS state. So the most common "navigation" does NOT accumulate.

The real sources of in-place swaps that re-render inline scripts:

1. **The reconnect soft-refresh in `layout.html`** (lines 266-274 from phase 18). After a WS drop-and-recover, the IIFE does:
   ```javascript
   htmx.ajax('GET', location.pathname + location.search, {
     target: '#main', swap: 'outerHTML', select: '#main'
   });
   ```
   This replaces the entire `#main` container with a freshly-fetched copy, which re-runs every inline script under `#main` (composer, page WS-subscribe, auto-scroll). The layout's three scripts (notification bus, nav toggle, reconnect IIFE) live OUTSIDE `#main` and do NOT re-run.

2. **Targeted fragment swaps** that contain inline scripts:
   - `room/notify_dropdown.html`: form change triggers `hx-target="#lc-room-header" hx-swap="outerHTML"`, which replaces the dropdown's parent and re-renders its `<script>` (one new `document.click` listener per mute-mode change).

3. **The composer mention-popover refresh**: `htmx.ajax('GET', url, {target:'#lc-mention-popover', swap:'innerHTML'})`. This swaps inside `#lc-mention-popover`, NOT the composer-IIFE root. The composer IIFE script itself doesn't re-run here, so this path doesn't accumulate composer listeners. The popover content itself is innerHTML-only (no scripts inside it).

In short: the leak surface is real but smaller than the brief implies. The fixes are still worth making both for the cases that exist today and because if anyone ever adds `hx-boost` to the sidebar, the leak would suddenly cover every navigation.

## Audit table

Posted in the conversation alongside this plan. Reproduced here so the plan is self-contained.

| File | What it does | Leaky? | Notes |
|---|---|---|---|
| `templates/base.html` | Loads htmx + extensions via `<script src defer>` | NO | External scripts loaded once at first page paint. |
| `templates/layout.html` (script #1, lines 31-151) | Notification bus: MutationObserver on `#lc-notify-bus`, lazy push registration | NO (persistent shell) | The observer is bound to an element inside layout.html which is never replaced in-place (only re-rendered on full browser navigation). Document explicitly. |
| `templates/layout.html` (script #2, lines 152-171) | Mobile nav open/close + `document.body.addEventListener('click', ...)` for auto-close | NO (persistent shell) | Same reason. Document explicitly. |
| `templates/layout.html` (script #3, lines 172-312) | WS reconnect IIFE: 4 body listeners, watchdog timers, banner state machine | NO (persistent shell) | Lives outside `#main`, never re-runs across the session. Document explicitly. |
| `templates/partials/auto_scroll.html` | Maintains scroll stickiness via 4 `document.body` listeners + 1 `#messages` scroll listener | **YES** | Included from `room/page.html` and `dm/page.html`, both inside `#main`. Each soft-refresh adds 4 more body listeners. |
| `templates/ws/user_status_update.html` | One-shot DOM update on each WS status frame | NO | The script tag itself is consumed by the OOB swap and discarded; the IIFE attaches no listeners. |
| `templates/status/picker.html` | Disclosure handlers for the status picker popover | NO (already self-cleaning) | Has its own `cleanup()` driven by a `MutationObserver` that watches for the picker being removed. Works. Could be migrated to `htmx:beforeCleanupElement` for consistency but the existing implementation is correct. |
| `templates/room/page.html` | `document.body.addEventListener('htmx:wsOpen', ...)` to emit a `subscribe` frame | **YES** | One new body listener per soft-refresh. After 3 reconnects, the subscribe frame fires 3 times per wsOpen. |
| `templates/room/notify_dropdown.html` | Disclosure pattern; binds `document.addEventListener('click', ...)` for outside-click dismissal | **YES** | One new document click listener per mute-mode form submission (each submission swaps `#lc-room-header` outerHTML). |
| `templates/room/composer.html` (script #1) | Mention autocomplete; adds `ta.addEventListener` + 1 `document.body.addEventListener('htmx:afterSwap', ...)` | **YES** | The `ta`-bound listeners go with the textarea on re-render (fine). The body `htmx:afterSwap` listener leaks one per soft-refresh. |
| `templates/room/composer.html` (script #2) | File upload + drag-drop overlay | NO (already self-guarded) | Uses a `window.__lcDropAttached` global flag to make the document drag-drop attachment idempotent. Works without teardown. |
| `templates/dm/page.html` | Same `htmx:wsOpen` body listener as `room/page.html` | **YES** | Same fix as `room/page.html`. |
| `templates/settings/blocked.html` | Username search popover + `document.addEventListener('click', ...)` to dismiss the popover | **YES** | Each visit to `/settings/blocked` (rare path, but real soft-refresh target if a reconnect lands while on that page) adds one document click listener. |

**Summary: 6 scripts to fix, 3 persistent-shell scripts to document as intentional exceptions, 1 already-clean script left alone, 2 not-leaky scripts (one external-only, one one-shot).**

## Architecture

### Cleanup mechanism: option A (`htmx:beforeCleanupElement`)

htmx fires `htmx:beforeCleanupElement` on every element it's about to remove from the DOM during a swap. Inline scripts register their teardown by listening for that event on their root.

Canonical template:

```javascript
(function() {
  // ... setup ...
  const onSwap = function(evt) { /* body listener body */ };
  document.body.addEventListener('htmx:afterSwap', onSwap);
  // ... more setup ...

  const teardown = function() {
    document.body.removeEventListener('htmx:afterSwap', onSwap);
    // clearTimeout(...), observer.disconnect(), etc.
  };
  const root = document.currentScript.closest('[data-lc-cleanup-root]')
            || document.currentScript.parentElement;
  root.addEventListener('htmx:beforeCleanupElement', teardown, { once: true });
})();
```

Default root is the script's parent element (which is whatever the template wraps the script in - usually `#main` for page-level scripts, or the fragment's wrapper div for fragment-level scripts). Explicit override via `data-lc-cleanup-root="1"` on a different ancestor when the default is wrong.

### Persistent-shell exceptions

Three scripts in `layout.html` intentionally have no teardown because the elements they attach to (`document.body`, `#lc-notify-bus`) outlive every in-place swap in this app. Mark each with a comment:

```javascript
// Persistent-shell script: lives in layout.html, runs once per full
// browser page load, never re-rendered by in-place swaps. No teardown
// needed; listeners on document.body / document are intentional.
```

This convention surfaces in CLAUDE.md alongside the canonical template so future inline scripts in layout.html follow the same pattern.

## Tasks

1. **Apply the teardown pattern to `partials/auto_scroll.html`** (4 body listeners, 1 scroll listener on `#messages`).
   - Save the four handler references at attach time, remove them all in teardown.
   - The `messages` scroll listener can stay implicit (the `#messages` element gets removed with `#main` and takes its listener with it) but adding it to teardown is cheap and consistent.
   - Root: `document.currentScript.parentElement` (which is `#main` via the page template that includes this partial).

2. **Apply the teardown pattern to `room/page.html`** (one `htmx:wsOpen` body listener).
   - Save the handler reference, remove on teardown.
   - Root: `document.currentScript.parentElement` (which is `#main`).

3. **Apply the teardown pattern to `dm/page.html`** (mirror of task 2).

4. **Apply the teardown pattern to `room/notify_dropdown.html`** (one `document.click` listener).
   - Save the handler, remove on teardown.
   - Root: the dropdown's wrapper `<div class="relative shrink-0">`. The `<script>` tag is a sibling of that div, so use `document.currentScript.previousElementSibling` or wrap the div with `data-lc-cleanup-root="1"` for explicitness. Prefer the data-attribute approach so future readers don't have to count siblings.

5. **Apply the teardown pattern to `room/composer.html` script #1** (mention autocomplete; one `htmx:afterSwap` body listener).
   - The textarea-bound listeners (`input`, `blur`, `keydown`) go with the textarea on swap. No teardown needed for those.
   - The body listener is what leaks. Save handler, remove on teardown.
   - Root: the composer form. Tag the `<form id="composer">` with `data-lc-cleanup-root="1"` since the `<script>` follows the form but is not its child.

6. **Apply the teardown pattern to `settings/blocked.html`** (one `document.click` listener, one debounce timer).
   - Save handler, remove on teardown. Clear `debounceTimer` in teardown so a pending fetch doesn't fire after the page is gone.
   - Root: `document.currentScript.parentElement`.

7. **Mark the three layout.html scripts as persistent-shell exceptions** with the documented comment. No behavior change. This is one comment per script.

8. **Document the pattern in CLAUDE.md.** Add a new section "Inline script teardown" (sibling of the Tailwind CSS section) with:
   - The canonical template
   - The persistent-shell exception
   - A short rationale: explicitly call out that this codebase does NOT use `hx-boost` today, so the only in-place re-render triggers are the reconnect soft-refresh and targeted fragment swaps. Flag that the teardown pattern becomes **load-bearing** if anyone ever enables `hx-boost`, because at that point every sidebar click would re-render `#main` and accumulate one listener per script per navigation.

9. **Verify.** `just check` (server + clippy + fmt) + `just test` standalone + `just test-saas` + `just verify`. No Rust changed, so this is mainly a "did we break a template" check via the test suite (which renders most templates as part of route tests).

10. **Manual smoke** (cannot automate without a browser harness):
    - **Reconnect-path single-fire smoke (highest signal — this is the path the bug bites today):** open `#general`. Use DevTools "Offline" toggle to drop the WS connection. Wait for the reconnect banner to flip back to Connected. Repeat 5 times in a row. Then from a second tab (or a second user) send one message to `#general`. The message should render **exactly once** in tab 1. Before the fix, the `htmx:wsOpen` subscribe-frame body listener would have accumulated 5 copies of itself, causing the room subscribe to fire 5 times. After the fix, it should fire exactly once.
    - In tab 1, throttle network in DevTools to force a WS drop and reconnect. After reconnect, the soft-refresh should run. Check DevTools "Memory" → "Listeners" count on `document.body` before and after; it should be stable, not growing.
    - Open the notify-dropdown, change mute mode 5 times. Click outside between changes. The dropdown should still close on outside-click (no missing-listener regression).
    - Type `@` in the composer, verify mention popover still works after the soft-refresh has happened at least once.
    - Settings → Blocked users page: open, leave, return. Username search should still work.

## Out of scope

- Moving inline JS to external files. Trivial inline scripts are fine; refactoring is its own future phase.
- Refactoring scripts for style, modernizing, or rewriting. If a script has a bug other than the leak, flag it in a separate note.
- Adding a Playwright/test harness for client-side testing.
- Migrating `status/picker.html` from its existing self-cleaning pattern to `htmx:beforeCleanupElement`. The existing pattern is correct.
- Removing the `window.__lcDropAttached` self-guard in `composer.html` script #2. It works; replacing it with the new pattern is a style choice with no behavior gain.
- Per-tab WS connection deduplication (mentioned out-of-scope in phase 18; still is).

## Deliverables

- 5 templates modified to add teardown: `partials/auto_scroll.html`, `room/page.html`, `dm/page.html`, `room/notify_dropdown.html`, `room/composer.html`, `settings/blocked.html` (6 files; 5 distinct scripts in 5 templates plus 1 in the partial - that's 6 templates, listed correctly above).
- 3 layout.html scripts each gain a one-line "persistent-shell" comment.
- New "Inline script teardown" section in CLAUDE.md.
- All automated checks pass.
- Staged with `git add`. No commits, no pushes - the user reviews and commits per task.

## Constraints reminder

- No Rust/Bun on host: `./dev/cargo` and `./dev/bun` only.
- All work in `server/`. Desktop untouched.
- Server-rendered HTML + HTMX; WS payloads stay pre-rendered HTML fragments.
- **Claude does not commit or push.** Stage with `git add`, stop.
