# Phase 25 - Accessibility (keyboard nav, ARIA, focus rings, live regions, call dialogs)

## Goal

LC-101 ("Chore(Client): Accessibility audit (keyboard nav, ARIA, focus rings)") delivered. The phase moves the existing uneven baseline onto a single consistent floor: every interactive element keyboard-reachable with a visible focus indicator; every icon-only button has an accessible name; every disclosure / dropdown / popover follows the same ARIA pattern; WebSocket-delivered UI announces itself where (and only where) announcement is useful; the two call-surface modals get real `role="dialog"` semantics with focus trapping. An axe-core baseline is captured and committed so the next pass can measure progress.

The codebase already has two reference-quality widgets - `server/templates/room/notify_dropdown.html` and `server/templates/status/picker.html`. The phase's job is to bring everything else to the same standard, not to invent a new pattern.

## Phase shape: hybrid (predict-first, axe at the end)

Most of the work is mechanically predictable from reading templates. Three categories of task:

- **Mechanical sweeps** (one task each): global `:focus-visible` rule, icon-button `aria-label` sweep, mobile-nav `aria-expanded` wiring, HTMX-swap focus restoration. Single fix shape applied uniformly across files.
- **Per-widget rewires** (one task each): mention autocomplete combobox wiring, call-dialog `role="dialog"` + focus trap, live-region annotations driven by the per-container strategy below.
- **Axe baseline** (one task): run axe-core against a running dev stack, commit the JSON report, fix anything axe surfaces that the predictions missed.

Audit-first (phase 24's shape) is the wrong shape here because the gaps are predictable from reading templates. Running axe before changing anything would slow the phase down without changing the conclusions. Axe runs at the end to catch what predictions missed.

## Hard constraints

- No new framework, no JS bundler change. Stays on Askama + HTMX + inline IIFEs.
- All work in `server/`. Do not touch `desktop/`.
- Listener-cleanup discipline (phase 20): every new `addEventListener` outside the swapped element's lifetime gets a paired `removeEventListener` on teardown. The new call-dialog focus trap registers when the dialog opens and tears down when it closes; the same shape, just with `open` / `close` as the lifecycle events instead of `htmx:beforeCleanupElement` (the call shells live in the persistent layout, not in an htmx-swapped subtree).
- Do not change behavior. This is annotation, focus, and labeling work. Composer Enter-to-send keeps working; mention autocomplete arrow keys keep working; HTMX swaps keep their current shape. The user-visible diff is "more announcements, more visible focus, more keyboard reachable" - not "different layout or interaction model."
- Do not introduce a new CSS preprocessor or Tailwind plugin. Global `:focus-visible` lives in `server/assets/main.css`; per-component overrides stay Tailwind utilities where the design needs different styling.
- Claude does NOT commit or push. Stage with `git add` and stop. The user commits per task during execution as a review step.

## Out of scope (named explicitly in the PR description so the "we did an accessibility pass" claim stays honest)

- WCAG 2.1 AA compliance certification. Legal artifact, not engineering.
- Color contrast remediation. Touches the theme; separate phase.
- `prefers-reduced-motion` honor. Reconnect spinner, mention-chip animations, auto-scroll all play regardless today. Separate phase.
- Screen reader walkthrough (VoiceOver / NVDA). Deferred follow-up with budgeted time; this phase ships without it.
- Voice-message seekbar keyboard alternative (`layout.html` canvas-based player). Structurally hard - the canvas has no `role="slider"`, no `<input type="range">` fallback, no arrow-key handling. Half-fixing is worse than not touching.
- Uploaded-image alt text. The user does not supply alt text at upload time, so every image renders with empty alt. Real fix requires a composer UI change; that is its own feature, not an accessibility annotation pass.
- Touch-target sizes / mobile-specific accessibility. Separate concern.
- Voice control (Dragon, macOS Voice Control) / cognitive accessibility / i18n. Distinct audits.

The PR description names every deferral above so reviewers know what is and is not in this phase.

## Background

### Existing baseline (read of `server/templates/`)

| Surface | State today |
|---|---|
| `room/notify_dropdown.html` | Reference-quality. `aria-haspopup`, `aria-expanded`, `aria-controls`, `role="menu"`, `aria-labelledby`, Escape closes + restores focus, `focus:ring-2 focus:ring-blue-500` on the trigger. |
| `status/picker.html` | Reference-quality. `<fieldset>` + `<legend class="sr-only">`, real `<input type="radio">`, Escape closes + restores focus, clear-button has `aria-label`. |
| `partials/sidebar.html` | Has explicit focus-ring utilities. |
| `partials/mention_popover.html` | Half-wired. `role="listbox"`, `role="option"` correct on the list. Textarea side is not declared as a combobox; no `aria-controls`, no `aria-activedescendant`, no `aria-expanded`. `aria-selected` is set on a nested `<button>` instead of the `role="option"` element. |
| `room/composer.html` | Send button + Record button have `aria-label`. Attach (`+`) button has only `title`. Composer Enter-to-send works for keyboard users. Voice-recording state ("● Recording 0:00 Stop Cancel") is visual-only - no live region. |
| `partials/connection_status.html` | No `aria-live`, no `role="status"`, no `role="alert"`. State changes via JS (`bannerEl.setAttribute('data-state', ...)`) are silent for screen readers. |
| `layout.html` `lc-notify-bus` (line 24) | Hidden `<div>`. WS mention events appended as children. Mutation-observer drains them. No live region; the inferred user-visible signal is the title flash + favicon dot + browser Notification API. SR users get nothing inside the document. |
| `layout.html` `lc-mention-counts` / `lc-broadcast-count` | Hidden config div + composer slot. No live region. Broadcast-count refresh on every keystroke is fine without a live region (announcing on every keystroke would be hostile). |
| `layout.html` incoming-call (line 28-37) and active-call (line 38-55) | `<div>` shells that show/hide via class toggle. No `role="dialog"`, no `aria-modal`, no focus trap. Mute/Camera buttons toggle without `aria-pressed`. Active-call status pill changes text without a live region. |
| `layout.html` mobile-nav button (line 13) | Has `aria-label="Open navigation"`. `aria-expanded` is never set or toggled by `lcOpenNav` / `lcCloseNav`. |
| `admin/*.html` | Zero ARIA. Mostly plain `<form>` + `<input>`; labels mostly wired via `<label>`. Likely just needs the global focus-ring rule and a sweep for icon-button labels (admin "delete" / "promote" / "kick" buttons). |
| WS-fragment templates (`ws/*.html`, mostly `hx-swap-oob` payloads) | Mention chips, typing indicators, seen indicators, etc. Land in the DOM via OOB swap. They are document content already; should NOT also be in a live region, or screen readers will hear the same thing twice. |

### Focus-ring coverage today

`grep -rl 'focus-visible\|focus:ring' server/templates/` returns three files: `notify_dropdown.html`, `sidebar.html`, `own_avatar_oob.html`. Everything else relies on Tailwind preflight + the browser's default `outline`, which is technically visible but inconsistent against the app's blue-accent palette and near-invisible on dark hover backgrounds (`hover:bg-slate-800` etc.).

## Live region strategy (per-container, not mechanical)

A global "add `aria-live='polite'` to every status element" would produce a hostile screen-reader experience. Per-container decisions, made up front so implementation does not re-litigate:

| Container | Live region | Reasoning |
|---|---|---|
| `partials/connection_status.html` `#lc-conn-status-text` | `role="status" aria-live="polite"` for `connected-flash` / `reconnecting`; **escalate to `role="alert"` (implicit `aria-live="assertive"`) for `failed-long`**. The state machine in `layout.html` already distinguishes these states; the script swaps the `role` attribute on the text element when entering `failed-long` and back to `status` when reconnecting. | Reconnecting briefly: polite is fine, the user can finish what they are reading. Five-minute-plus failure: interrupt, this is an action item. |
| `layout.html` `#lc-notify-bus` | `aria-live="polite" aria-relevant="additions"`. Children stay; the mutation-observer no longer immediately calls `bus.replaceChildren()`. Children are summarized into a single text node ("Mention from Bob in #general") that the SR reads, then drained on the next tick. | The visible signal today (title flash, favicon dot, browser notification) does not reach the SR user inside the document. Polite because mentions arrive at unpredictable rates and assertive would interrupt every read. |
| `layout.html` `[data-lc-call-status]` (active-call status pill) | `aria-live="polite"`. | "Connecting...", "Connected", "Reconnecting", "Ended" - the SR user needs to hear state transitions for a call they are in. Polite is correct - assertive would step on the speaker. |
| `room/composer.html` `#lc-broadcast-count` | **No live region.** | Refreshes on every keystroke while the user types `@here` or `@channel`; announcing on every keystroke is hostile. The count is visible inline next to the composer; SR users encounter it on Tab. Acceptable underannouncement. |
| `room/composer.html` `#lc-staged` (file/voice attachment status) | `aria-live="polite"`. | One-shot status changes ("Uploading file.png...", "Attached: file.png", "Voice message attached"). Reasonable to announce. |
| `room/composer.html` `#lc-upload-error` and `.composer-error` | `role="alert"`. | Error conditions that the user must notice to recover. Assertive is correct. |
| WS-fragment templates (`ws/mentioned.html`, `ws/new_message.html`, `ws/typing.html`, `ws/seen_indicator.html`, `ws/edited_message.html`, etc.) | **No live region.** | These render content into the document flow (sidebar badges, message rows, typing strips). They become part of the page; SR users encounter them by navigating. A live region would double-announce. The one exception is mention notifications, which we route through `lc-notify-bus` above (the bus IS the live region, the individual `ws/mentioned.html` fragment is not). |
| `room/composer.html` voice-recording state ("● Recording 0:00 Stop Cancel" inside `#lc-staged`) | Covered by `#lc-staged`'s `aria-live="polite"`. The first paint of "Recording" announces; the ticking timer does NOT announce because the staged container will be `aria-atomic="false"` and only the wrapper text changes. | Avoid announcing the timer every 250ms. |
| Mention autocomplete popover (`partials/mention_popover.html`) | **No live region.** | The combobox + listbox + `aria-activedescendant` pattern is what SRs announce; a live region on top would double-announce. Wired in Task 5. |
| Reconnect soft-refresh content | **No live region.** | The full `<main>` swap is a navigation event; SR users will start reading from the new content. No "page reloaded" announcement needed (which would be wrong anyway - it is a partial swap). |

The strategy is recorded here so implementation tasks copy from it rather than deciding per-template.

## File Map

| Action | Path | Responsibility |
|---|---|---|
| Add | `docs/superpowers/plans/2026-05-16-phase25-accessibility.md` | This plan. |
| Add | `docs/superpowers/plans/2026-05-16-phase25-accessibility-axe-baseline.json` | Raw axe-core report from Task 8, committed for future regression comparison. Not a CI artifact (no CI for it yet). |
| Edit | `server/assets/main.css` | Global `:focus-visible` rule. Colors / outline-offset match `notify_dropdown.html` (`ring-2 ring-blue-500` ~ `outline: 2px solid #2563eb; outline-offset: 2px`) so the visual stays consistent. |
| Edit | `server/templates/layout.html` | Call-dialog `role="dialog"` + `aria-modal` + `aria-labelledby`. Mobile-nav button `aria-expanded` toggling. `lc-notify-bus` becomes a live region with the redesigned drain logic. Call-status pill gets `aria-live`. New HTMX afterSwap focus-restoration handler (one persistent IIFE in the layout shell). |
| Edit | `server/templates/partials/connection_status.html` | `role="status" aria-live="polite"` on the text element by default. The state-machine JS in `layout.html` flips it to `role="alert"` on `failed-long` and back. |
| Edit | `server/templates/room/composer.html` | Attach (`+`) button gets `aria-label="Attach file"`. `#lc-staged` gets `aria-live="polite" aria-atomic="false"`. `.composer-error` and `#lc-upload-error` get `role="alert"`. Textarea becomes `role="combobox"` with combobox attributes; popover-driven swap updates `aria-expanded` and `aria-activedescendant`. |
| Edit | `server/templates/partials/mention_popover.html` | Move `aria-selected` from inner `<button>` to the `role="option"` `<li>`. Add stable `id` to each option for `aria-activedescendant`. The buttons stay as click targets (no role change). |
| Edit | various icon-only `<button>` sites | Sweep for `<button>` whose only content is an SVG; add `aria-label`. Inventory in Task 3 derives the file list mechanically. |
| Edit | `server/templates/admin/*.html` | Whatever the focus-ring + icon-label sweep catches. No new patterns. |

## Tasks

### Task 1 - Global `:focus-visible` in `main.css`

Add one rule at the top of `server/assets/main.css`:

```css
:focus-visible {
    outline: 2px solid #2563eb;
    outline-offset: 2px;
    border-radius: 2px;
}
```

`#2563eb` is `blue-600`, matching the existing `focus:ring-blue-500` used in `notify_dropdown.html`. Color choice is intentionally NOT `blue-500` for the ring: a 2px solid outline of the same blue would be visually heavier than the ring; dropping one shade keeps perceived weight similar.

Hunt down `focus:outline-none` utilities that drop the outline without pairing with a Tailwind ring utility, and remove them:

- [ ] `./dev/cargo` not relevant here.
- [ ] `bunx tailwindcss --input server/assets/tailwind.css --output server/assets/tailwind-built.css` (or `just build-css`) to regenerate Tailwind output.
- [ ] `grep -rn 'focus:outline-none' server/templates/` and review each match. Keep it when paired with `focus:ring-*` (`notify_dropdown.html`); remove it otherwise so the global outline shows through.
- [ ] Manual check: load `/login`, `/register`, `/`, a room, `/settings`, an admin page, Tab through each. Every interactive element must show the new outline.
- [ ] `git add server/assets/main.css <any template files where focus:outline-none was removed>` and stop.

### Task 2 - Live regions, per-container

Apply the table from "Live region strategy" above. One commit, covers every container.

- [ ] `partials/connection_status.html`: add `role="status" aria-live="polite"` to `#lc-conn-status-text`. Default state.
- [ ] `layout.html` connection-banner IIFE (the `setBanner(next, msg)` function): when `next === 'failed-long'`, set `textEl.setAttribute('role', 'alert')`. When transitioning out of `failed-long`, restore `textEl.setAttribute('role', 'status')`. The `aria-live` attribute can stay polite throughout; `role="alert"` carries the implicit assertive behavior.
- [ ] `layout.html` `#lc-notify-bus`: change from `class="hidden"` to `class="sr-only" aria-live="polite" aria-relevant="additions" aria-atomic="false"`. Tailwind's `sr-only` keeps it accessible-but-invisible. Drain logic: replace the current `bus.replaceChildren()` (which removes children before the SR can read them) with: read child's `data-event` summary into a transient `<p>` text node, leave it in the bus for 2000ms, then remove. The summary text is "Mention from {{ author }} in {{ roomLabel }}" for `mentioned` events, and nothing for `mention_cleared` (no announcement needed when a chip clears). Title flash / favicon dot / browser Notification / push-subscribe paths stay unchanged - they are independent of the live region.
- [ ] `layout.html` `[data-lc-call-status]` element (inside active-call dialog, currently line 48): add `aria-live="polite"`.
- [ ] `room/composer.html`: add `aria-label="Attach file"` to the `+` button (line 44-46); `aria-live="polite" aria-atomic="false"` to `#lc-staged`; `role="alert"` to `.composer-error` and `#lc-upload-error`.
- [ ] No live region on `#lc-broadcast-count`, WS-fragment templates, mention popover, or reconnect soft-refresh content. Per strategy table.
- [ ] Manual check with a screen reader (VoiceOver on macOS or NVDA on Windows): connect, disconnect (kill the server briefly), reconnect, receive a mention while focus is in the composer, trigger an upload error. Confirm the announcements match the intent and do not double-announce.
- [ ] `git add` the touched templates + `layout.html`. Stop.

### Task 3 - Icon-only button labels sweep

- [ ] Inventory: `grep -rn '<button[^>]*>' server/templates/ | grep -v 'aria-label\|>[A-Za-z]'` to find every button without an `aria-label` whose content does not start with text. Hand-filter the false positives (buttons whose first non-tag content is a Tailwind utility class string, etc.).
- [ ] For each surface, add `aria-label` matching the visual intent: "Reply", "Delete message", "Pin message", "Edit message", "Block user", "Unblock user", "Mute room", "Unmute room", "Promote to moderator", "Demote moderator", "Kick user", "Ban user", "Open reactions picker", "Close thread", etc. Use the surrounding text or the SVG meaning as the source.
- [ ] Buttons whose only content is `&#10005;` ("x" close glyph) get `aria-label="Close"` or `aria-label="Clear <field-name>"` depending on context.
- [ ] Buttons that contain text already (`Reply`, `Save`, `Cancel`, `Send`, `Block`, `Unblock`) do not need a label.
- [ ] Manual check: navigate the chat surface, reactions bar, message hover actions, settings, admin user/room rows. Every actionable button must read meaningfully when focused.
- [ ] `git add` the touched templates. Stop.

### Task 4 - Mobile-nav `aria-expanded`

- [ ] `layout.html` line 13 button: change `aria-label="Open navigation"` to a label that does not lie about state. Two options - either `aria-label="Toggle navigation"` with `aria-expanded="false"` synced by JS, or keep the static "Open navigation" label and add `aria-expanded`. Go with `aria-label="Toggle navigation" aria-expanded="false" aria-controls="lc-nav-panel"` for symmetry with the notify-dropdown pattern.
- [ ] Update `lcOpenNav` to set `aria-expanded="true"` on the toggle button. `lcCloseNav` flips it back to `"false"`. The button is at `#main > button:first-child` in current layout; if needed, give it `id="lc-nav-toggle"` and reference by id.
- [ ] Manual check: load the app at mobile width (DevTools, or `window.matchMedia('(min-width: 768px)')` false), Tab to the menu button, Enter/Space toggles. SR announces "Toggle navigation, button, expanded" / "collapsed."
- [ ] `git add server/templates/layout.html`. Stop.

### Task 5 - HTMX afterSwap focus restoration

Lands before the mention-combobox task on purpose: once the global handler exists with a documented opt-out, the combobox in Task 7 is designed knowing the carve-out mechanism is already there. Otherwise the popover slot ends up retrofitting around a handler the combobox author has to discover.

- [ ] New IIFE in `layout.html` (persistent-shell script). Single `document.body.addEventListener('htmx:afterSwap', ...)` handler:
  - If the swap target has `[autofocus]` inside, focus it.
  - Else, if `event.detail.requestConfig.elt` is still in the document and is focusable, focus it.
  - Else, do nothing (browser default - focus stays where it was, which for OOB-only swaps is correct).
- [ ] Document the opt-out attribute `[data-lc-skip-focus]` in a comment on the IIFE. The handler checks for it on the swap target and bails. Used by Task 7's mention-popover slot; reserved for any future swap target where the global default would be wrong.
- [ ] Carve-out: composer Enter-submit already calls `ta.focus()` in its `hx-on::after-request`; leave that alone. The handler runs after that callback, so the carve-out is "the target is the composer textarea's ancestor" - explicitly skip targets that contain `#composer textarea[name=body]` so the ordering does not fight.
- [ ] Manual check: room navigation (sidebar link click - was the link focused or did focus disappear?), open a thread panel, close it, edit a message inline, reactions picker open/close. Focus should land somewhere sensible after each swap.
- [ ] `git add server/templates/layout.html`. Stop.

### Task 6 - Call-dialog `role="dialog"` + focus trap

- [ ] `layout.html` incoming-call shell (line 28-37): outer `<div data-lc-call-incoming class="hidden ...">` becomes `<div data-lc-call-incoming class="hidden ..." role="dialog" aria-modal="true" aria-labelledby="lc-call-incoming-title" aria-hidden="true">`. Add `id="lc-call-incoming-title"` to the existing "Incoming call" `<div>` (line 30).
- [ ] Active-call shell (line 38-55): same treatment. `role="dialog" aria-modal="true" aria-labelledby="lc-call-active-title"`. Add a visually-hidden `<h2 id="lc-call-active-title" class="sr-only">Active call</h2>` inside the shell (the existing status pill is too dynamic to use as a stable label).
- [ ] Mute / Camera buttons (line 51-52): toggle `aria-pressed="true"|"false"` via `call.js` whenever the underlying state flips. The script already tracks these states; adding the attribute is one line per toggle.
- [ ] When a dialog is `hidden`, set `aria-hidden="true"` and ensure no focusable descendant. When it opens, remove `aria-hidden` and move focus into the dialog (Accept button on incoming; Hang up on active).
- [ ] Focus trap helper. New IIFE in `layout.html` (one of the persistent-shell scripts):
  - Exposes `window.__lcDialogTrap(rootElement)` which installs a `keydown` listener on `rootElement` filtering Tab / Shift+Tab against `rootElement.querySelectorAll(<focusable selectors>)`, wraps focus, and returns a `dispose()` function that removes the listener.
  - Selectors: `'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])'`.
- [ ] **Close-path enumeration in `call.js`.** Listener-cleanup discipline (phase 20) applies here even though the lifecycle is open/close instead of `htmx:beforeCleanupElement`. Every `addEventListener` MUST have a paired `removeEventListener` on EVERY lifetime-end path. Enumerate, in code, every code path that closes a call dialog and confirm each disposes the trap and restores focus:

  Incoming dialog close paths:
  1. User clicks **Accept** -> dialog hides, active dialog opens (trap migrates from incoming to active).
  2. User clicks **Decline** -> dialog hides, no successor.
  3. Caller hangs up before answer (remote-side end via WS) -> dialog hides, no successor.
  4. WS disconnect while incoming dialog is open -> the connection-banner script reloads `#main`; the persistent shell survives, but the call shell may be left visible. Decide: either tear down the trap explicitly on `htmx:wsClose`, or accept the dialog stays open and the trap follows it (defensible since the user can still Tab through it). Document the choice in the IIFE comment.
  5. Page navigation while incoming dialog is open -> beforeunload teardown not normally needed (the page is going away), but if there is in-app navigation that does NOT unload (htmx swap that replaces `#main`), the trap stays installed on a shell that is no longer reachable. Audit `call.js` for any `show()` path that does not also have a paired `hide()` and add one.

  Active dialog close paths:
  1. User clicks **Hang up**.
  2. Remote peer hangs up (WS message).
  3. RTC connection state -> `failed` or `disconnected` for a sustained period.
  4. WS disconnect during active call (same handling as incoming case 4 above).
  5. Page navigation.

  Discipline: each branch ends with `if (currentTrap) { currentTrap.dispose(); currentTrap = null; }` and `if (previouslyFocused) { previouslyFocused.focus(); previouslyFocused = null; }`. A single `closeIncoming()` / `closeActive()` helper that every branch funnels through is the safest shape - prevents "remember to dispose" being a checkbox at every call site. Risk pattern to avoid: trap-installed-listener stays bound on a forgotten close path, and subsequent Tab presses on unrelated UI get hijacked. Verify by inducing each close path and confirming Tab works normally afterwards on the surrounding app.
- [ ] `call.js`: at each `show(incoming|active)` entry point, install the trap and move focus (Accept button on incoming, Hang up on active). Record `document.activeElement` before opening so the close helpers can restore it.
- [ ] Manual check (with a real audio device or DevTools' fake media): trigger an incoming call signal, confirm focus lands on Accept and Tab cycles only between Accept and Decline. Active-call dialog: Tab cycles Mute / Camera / Hang up. Decline / Hang up dismisses and returns focus to the previously-focused element. Then induce each close path from the enumeration above and confirm Tab on the surrounding app is not hijacked after.
- [ ] `git add server/templates/layout.html server/assets/call.js`. Stop.

### Task 7 - Mention autocomplete combobox wiring

Now that Task 5's global handler exists with `[data-lc-skip-focus]`, mark the popover slot with that attribute up front rather than retrofitting.

- [ ] `room/composer.html` `<textarea>` (line 55-79): add `role="combobox" aria-autocomplete="list" aria-expanded="false" aria-controls="lc-mention-list" aria-haspopup="listbox"`. Optionally `aria-owns="lc-mention-list"` for broader SR compatibility; `aria-controls` is the modern attribute.
- [ ] Popover slot `<div id="lc-mention-popover">`: add `data-lc-skip-focus` so Task 5's global handler does not try to focus it after a swap.
- [ ] Composer mention IIFE (the one starting at line 96): when `refresh()` triggers a popover swap, after the `htmx:afterSwap` callback wires up the first option, set the textarea's `aria-expanded="true"`. When `close()` runs, set `aria-expanded="false"`. The `aria-activedescendant` of the textarea must point at the currently-highlighted option's `id`; update it in `select(li)` alongside the `aria-selected` move.
- [ ] `partials/mention_popover.html`:
  - Move `aria-selected="true"` from the inner `<button>` to the `role="option"` `<li>`. Update the composer IIFE accordingly - `items()` becomes the LIs not the buttons; `select(li)` flips `aria-selected` on the LI and the inner button stays as the click target.
  - Add stable `id="lc-mention-option-{{ loop.index0 }}"` to each `<li>`. The textarea's `aria-activedescendant` will be set to this id.
  - `<button data-username>` keeps its existing role implicit; `aria-selected` no longer belongs on it.
- [ ] Composer IIFE `onAfterSwap`: after the popover lands, the first `<li role="option">` gets `aria-selected="true"`, and the textarea's `aria-activedescendant` is set to that LI's id.
- [ ] Manual check: focus the textarea, type `@b`, popover appears, ArrowDown/ArrowUp moves selection, Enter inserts. With a screen reader, navigating with arrows announces each suggested username (the SR follows `aria-activedescendant`).
- [ ] `git add server/templates/room/composer.html server/templates/partials/mention_popover.html`. Stop.

### Task 8 - Axe baseline + gap fills

- [ ] Run `bunx @axe-core/cli https://${USER}-chat.a8n.run/login https://${USER}-chat.a8n.run/register https://${USER}-chat.a8n.run/ https://${USER}-chat.a8n.run/settings --save docs/superpowers/plans/2026-05-16-phase25-accessibility-axe-baseline.json`. If the dev stack is not running, start it first (`just dev-web` or `just dev-web-local`).
- [ ] If the stack URL has self-signed certs, pass `--no-sandbox` and use the local http URL (`http://localhost:18080`).
- [ ] Read the resulting JSON. For every issue at severity `serious` or `critical` that is NOT in the "Out of scope" list above, fix it as part of this task. Do not expand scope - low-contrast text, missing `lang` attribute on `<html>`, and similar items that the brainstorm deferred stay deferred. The PR description names them explicitly so reviewers know.
- [ ] Commit the raw axe JSON at the documented path so the next pass has a baseline to compare against.
- [ ] `git add docs/superpowers/plans/2026-05-16-phase25-accessibility-axe-baseline.json <any template files patched in this task>`. Stop.

### Final task - PR

- [ ] `just check` clean (clippy + fmt for standalone and saas).
- [ ] `just test` and `just test-saas` clean. No test in `server/tests/` exercises accessibility annotations directly, so the diff should not move test outcomes; verify it does not regress them.
- [ ] `just verify` still passes (release binary serves `/login`).
- [ ] Manual end-to-end check by a **literal** keyboard-only user. Disconnect the mouse for the duration of the walkthrough - "mostly used Tab, occasionally clicked" is not what shipped. Cover: register, login, enter a room, send a message, mention a user via autocomplete, react to a message, open the notify dropdown, change a setting, open and close an admin user row. Every step must be reachable and reversible. This is the load-bearing verification for the phase; the user-visible improvement is felt by users who cannot use a mouse, so the test must be what they actually do.
- [ ] PR title: `feat(a11y): phase 25 - keyboard nav, ARIA, focus rings, live regions, call dialogs`.
- [ ] PR body (single long lines per bullet, per the project commit-style rule):
  - Summary: brings the codebase onto a single accessibility baseline. Global focus-visible. Live regions per the strategy in the plan. Mention autocomplete combobox wiring. Call dialogs get role="dialog" + focus trap. Icon-only buttons get accessible names. Mobile nav button toggles aria-expanded. HTMX swaps restore focus.
  - Deferred (named explicitly, with rationale): WCAG 2.1 AA compliance certification; color contrast; prefers-reduced-motion; screen reader walkthrough; voice-message seekbar keyboard alternative; uploaded-image alt text; mobile touch targets; voice-message Play/Pause aria-label toggle (surfaced during Task 3 - button label stays "Play voice message" when paused/playing); reaction-chip viewer-reacted state via `aria-pressed` (surfaced during Task 3 - chip background communicates state visually but not to SR). The last two are candidates for a follow-up "toggle-state ARIA" mini-phase.
  - Axe baseline committed at `docs/superpowers/plans/2026-05-16-phase25-accessibility-axe-baseline.json`. Summary of issues found / fixed / deferred from that report.

## Things to confirm during implementation

1. **`sr-only` is available in the compiled Tailwind output.** It is a default Tailwind utility, but the project's `tailwind.config.js` extends nothing; confirm a regeneration includes it before relying on it in `lc-notify-bus`. If somehow missing, define it inline in `main.css` (the standard 1px clip pattern).
2. **`@axe-core/cli` works through `bunx`.** Bun resolves npm packages; `bunx @axe-core/cli` should work without a global install. If it does not, use a one-off Docker container (`docker run --rm -v $(pwd):/work -w /work node:lts npx @axe-core/cli ...`).
3. **The call dialogs' lifecycle in `call.js` actually has discrete open/close functions.** The plan assumes there are stable insertion points to wire the focus trap. If `call.js` toggles classes from many different branches, the focus-trap install/dispose may need a small helper inside `call.js` rather than per-call-site changes.
4. **Mention-popover IIFE `aria-activedescendant` and `aria-selected` move.** The current IIFE selects via `items()` returning buttons; moving `aria-selected` to the LI means `items()` must return LIs instead, and `select(btn)` is renamed `select(li)`. Confirm the keyboard handler at line 178 still sees the correct list when refactored.
5. **HTMX afterSwap focus handler does not break composer behavior.** The composer's `hx-on::after-request` already calls `ta.focus()`. After-swap is a different event but they can race on rapid sequential submits. Verify the composer Enter-to-send + the global focus handler do not fight each other; the carve-out is the safety net.
6. **Live-region drain timing in `lc-notify-bus`.** The current drain happens synchronously in the mutation observer callback (`bus.replaceChildren()`). SR engines typically need 100-300ms with a node in the DOM to announce it. The 2000ms hold proposed in Task 2 is conservative; tune down only after confirming announcements still land on the SR you test against.

## Summary plan

Eight tasks plus a Final task. Hybrid shape: mechanical sweeps (focus-visible, icon labels, mobile-nav aria-expanded, HTMX focus restoration) and per-widget rewires (live regions, mention combobox, call dialogs), then axe-core at the end to catch what the predictions missed. The per-container live-region strategy is baked into the plan, not deferred to implementation. Deferrals (contrast, reduced motion, SR walkthrough, voice-seekbar keyboard, image-alt) are named in the PR description so the "we did an accessibility pass" claim stays honest. Axe baseline committed at a stable path for future regression comparison.
