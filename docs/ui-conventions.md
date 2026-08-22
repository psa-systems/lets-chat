# UI conventions

Shared front-end patterns every new surface should inherit instead of re-deriving. Born out of a recurring problem: each new surface silently re-implements a concept rather than reusing it. Keep this list short; when a pattern here changes, update the doc in the same PR.

## Typography scale (LC-365)

Use the semantic typography classes from `server/assets/main.css` instead of hand-rolling `text-2xl font-semibold` combinations. Each class fixes font-size + weight + line-height (+ tracking) so headings and labels stay consistent across surfaces. Color is left to the caller (`text-content` / `text-content-muted`), except `lc-caption` and `lc-section-label`, which already imply a muted tone.

| Class | Use for | Size / weight |
|---|---|---|
| `lc-display` | hero / empty-state heading | 1.5rem / 600, tight |
| `lc-h1` | page or panel heading | 1.125rem / 700 |
| `lc-h2` | section heading | 1rem / 600 |
| `lc-h3` | card / row title | 0.875rem / 600 |
| `lc-body` | default body text | 0.875rem / 400 |
| `lc-small` | secondary / help text | 0.75rem / 400 |
| `lc-caption` | timestamps, captions (muted) | 0.75rem / 400 |
| `lc-section-label` | uppercase group label (sidebar sections) | 0.6875rem / 600, uppercase |

The message body (`.lc-md`) keeps its own size, which the per-device text-size control scales; do not apply the chrome scale there.

### Page headings (LC-746)

A page has exactly one `<h1>`, and it takes its size from the scale, never from a raw `text-*` utility. Tailwind's preflight resets `<h1>` to `font-size: inherit`, so an `<h1>` with no class is not "the default heading size", it is body text at 1rem - which is how the audit found 36 page titles rendering at six different sizes.

- `lc-h1` on every page with a header bar (`.lc-header`, `.lc-admin-header`, `.lc-callbar`). This is the default.
- `lc-display` on a standalone centered page with no bar: `error.html`, `not_found.html`, `maintenance.html`, `auth/login.html`, `auth/login_approve.html`. That is the one deliberate second tier.
- `landing.html` is a marketing hero outside the app scale and is excluded.

A second heading in a body branch is an `<h2>`, not a second `<h1>`: `home/welcome.html` renders its page title in the header bar and its welcome hero as `<h2 class="lc-display">`, so the non-dashboard branch still has one top-level heading.

## Timestamps (LC-746)

Every rendered timestamp is a `<time>` carrying a machine-readable UTC instant:

```
<time datetime="{{ row.created_at|iso }}" title="{{ row.created_at }}">{{ row.created_at }}</time>
```

The `iso` filter (`server/src/i18n.rs`) converts SQLite's `YYYY-MM-DD HH:MM:SS`, which a browser would otherwise parse as local time, into `YYYY-MM-DDTHH:MM:SSZ`. Do not reformat in the template and do not add a per-struct accessor for it.

Add `data-lc-ts` where a relative time reads better than an absolute one. The LC-314 upgrade in `layout.html` replaces the element's text with "3 minutes ago" (falling back to an absolute date past ~26 days) and leaves the exact stamp in the `title`:

- **With `data-lc-ts`:** the feed-like surfaces - activity, inbox, pins (the pins page, the pinned strip, the room info panel), saved, related, search results, the transcripts list.
- **Without:** the admin audit tables (modlog, quarantine, invites, deliveries, bots, link filter, bridges, ...), where the exact stamp is the point, and the settings / room integration tables that read the same way.

That split is a decision, not an omission: an audit row is evidence, and "2 days ago" is not.

## Color tokens (LC-735)

Every color comes from the semantic tokens in `server/tailwind.config.js` (`text-content`, `text-content-muted`, `text-content-subtle`, `bg-surface{,-elevated,-sunken}`, `border-border`, `text-danger`, ...), which resolve to the CSS vars in `server/assets/main.css` and so recolor across all four modes (`light`, `dark`, `hc-light`, `hc-dark`) and every palette. A raw numbered utility like `text-slate-700` is fixed for all four, so it survives review in the mode it was written for and goes unreadable in the others.

`tailwind.config.js` uses `extend`, not an override, so the raw palette still compiles: nothing stops a raw shade except the review. One default IS overridden: `borderColor.DEFAULT` is `var(--border)` (LC-744), because a bare `border` / `border-t` / `divide-y` otherwise resolves to `colors.gray[200]`, a fixed light grey that reads brighter than the `#0b2542` panel it outlines in every dark mode. That is a backstop, not a licence: the gate below still requires the call site to name its border color, so a later config change cannot repaint an element silently. The convention applies to every surface. `ci-build/check-asset-color-tokens.nu` (wired into `just check` and the Check workflow) enforces it mechanically in two places, both of them states that theme testing rarely reaches:

- `server/assets/**/*.js` outside `vendor/`, where markup is built in JS and injected in transient states (offline banner, failed send, active search row).
- Every `[aria-selected="true"]` rule in `server/assets/main.css`, which must paint its background from a `var(--...)` token. That is the selection highlight, only visible mid-keyboard-navigation; LC-736 found it hard-coded to `rgb(241 245 249)` there, outranking the tokenized `.lc-search-row` rule.

The template half of the same rule lives in `ci-build/check-ui-conventions.nu` (see "Convention gates" below), reporting rather than failing until LC-741 clears the call overlays in `layout.html`.

One surface is deliberately outside the token layer: `server/assets/offline.html`, the service worker's navigation fallback. It is precached and served with the network down, so it can neither link `main.css` nor run the layout's bootstrap. It therefore carries its own copy of the six colors it needs and its own `lc-mode` resolver, mirroring `server/templates/base.html`. Keep the copied values in step with `main.css` when the light or dark ramp moves; the gate below only checks that the page still resolves a mode, not that the hexes still match (LC-748).

## Page top bars (LC-742)

Every page-level top bar is `<header class="lc-header">` (`server/assets/main.css`). The class fixes the bar's height floor (`min-height: 3.5rem`), padding (`0.875rem 1.25rem`) and bottom border in one place, and its 20px gutter is what lines the bar's content up with the `px-5` message timeline below it. Hand-rolling the bar with utilities is what gave the app four different top-bar heights that jumped as the user navigated; `server/tests/lc742_page_header_shape.rs` now fails if a `<header>` re-creates the skeleton or a converted bar loses the class. The admin console's `.lc-admin-header` keeps the same height floor and padding.

`.lc-header` is `display: flex` with `align-items: center` and `justify-content: space-between`, so it wants exactly two children: the title block and the action cluster. A bar whose content needs a different alignment gets its own wrapper element (see `room/info.html` and `settings/blocked.html`) rather than a Tailwind override: `main.css` loads AFTER `tailwind-built.css`, so an equal-specificity utility such as `items-start` or `gap-3` does NOT win against the class. Utilities that `.lc-header` does not set (`flex-wrap` on `transcripts/index.html`) still compose normally.

The right-hand panel headers (`px-3 py-2`) and the compose panels (`px-2 py-1.5`) are their own consistent groups and stay as they are.

## Spacing rhythm (LC-365)

Spacing stays on Tailwind's default 4px scale - compose from it rather than inventing new gaps. Conventions for the chat chrome:

- Inter-element gaps: `gap-1` (4px) inside a control, `gap-2` (8px) between related items, `gap-4` (16px) between groups.
- Section padding: `p-3` (12px) for dense list regions, `p-4` (16px) for panels, `p-6` (24px) for the main content pane.
- Separate sections with a `border-t border-border` divider plus an `lc-section-label`, not blank space alone.
- Row height for nav/list items: `px-2 py-1.5` with `gap-2`, so the left sidebar and enclave rail read at one rhythm.

## Live updates

A new page that shows mutable data should update over the WebSocket without a manual reload. The recipe, end to end:

1. **Extract the mutable region into an Askama partial** (e.g. `enclave/members_items.html`, `saved/items.html`) and include it from the full page wrapped in a stable id (`<div id="lc-saved-list">{% include ... %}</div>`). If the page handler builds rows with non-trivial logic, factor that into a `pub(crate)` row-builder so the page and the live render share it.
2. **Add a live OOB fragment** (`server/templates/ws/<name>_live.html`) that includes the SAME partial inside `<... id="<same-id>" hx-swap-oob="outerHTML">`, with a matching `#[derive(Template)]` struct in `views::ws_fragments` (or the surface's view module).
3. **Broadcast on the mutation.** For data scoped to one user (own profile, saved, invitations) use `hub.broadcast_to_user` - no client subscription needed. For data shared by a group (enclave lists, admin lists) use `hub.broadcast_to_topic` and add `data-lc-live-topic="<topic>"` to the page so `live.js` subscribes; authorize the topic in `ws.rs::topic_subscribe_allowed`.
4. **Render per recipient in the WS send task** (`routes/ws.rs`), gating on identity where the event is per-user (`if user_id == &send_user.id`) and rendering with the recipient's own state so per-viewer controls (`can_manage`, unread counts) are correct.

Why id-keyed OOB regions instead of swapping the whole `#sidebar` or re-rendering the page: htmx silently drops an OOB swap whose id is absent from the current DOM, so the fragment lands **only** on connections actually viewing that region and is a no-op everywhere else. This means the server never has to know which page a connection is on, and a stale subscription cannot corrupt an unrelated view. Scope ids that vary by entity (enclave room nav -> `#sidebar-nav-{enclave_id}`) so one enclave's update can't swap into another's.

Exceptions / gotchas:

- **Paginated or filtered lists** (infinite-scroll `/inbox`, tab-filtered `/activity`) carry per-connection view-state the server can't see; a full-list swap would clobber it. Use a **refresh affordance** instead: a hidden bar revealed over the WS that reloads the current URL on click (LC-179).
- **Admin-only surfaces**: `routes::admin` is `#[cfg(standalone)]`, so gate the WS arm + renderer `#[cfg(feature = "standalone")]` (the event falls to `render_event` -> None in saas). Skipping this breaks `just test-saas`.
- **Access loss**: if losing access to a topic's data should stop its events, call `Hub::unsubscribe_user_from_topic` from the access-loss handler (see kick/leave/enclave-delete, LC-176).

## Confirmation dialogs

Three confirmation styles exist in the codebase. Pick by blast radius, not by convenience.

| Style | Use for | Mechanism |
|-------|---------|-----------|
| `hx-confirm` attribute | Reversible or low-stakes htmx-driven mutations: delete a message, cancel a scheduled message, revoke a moderator role, delete a sidebar category. | Native browser `confirm()` fired by htmx **before it issues the request**. Only works on elements that issue an htmx request (`hx-post`/`hx-delete`/...). |
| `onsubmit="return confirm(...)"` / `onclick="return confirm(...)"` | The same low-stakes confirmation on a **plain `<form>` POST** that is not htmx-driven (e.g. block user, transfer enclave ownership). | Native `confirm()` wired directly to the DOM event. |
| Typed / re-auth verification | Irreversible, high-blast-radius actions: delete account, delete an entire enclave, stage a backup restore. | A real form gate the user must satisfy (password re-entry, or typing a confirmation phrase) handled server-side. A single `confirm()` OK click is **not** sufficient here. |

Decision rules:

- **Default to `hx-confirm`** for htmx-driven destructive actions. It is the least code and is already the majority pattern.
- **`hx-confirm` only guards htmx requests.** It does nothing for a plain `<form>` submit. Do not add `hx-confirm` to a non-htmx form expecting it to fire; use `onsubmit="return confirm(...)"` instead, or convert the action to htmx first. Converting a plain form to htmx is behavior-changing (response handling, redirect vs. fragment) and must be audited per site, not done blindly.
- **`onsubmit="return confirm(...)"` is not a guard on an htmx form (LC-739).** htmx issues the request from the `submit` event without checking `defaultPrevented`, so cancelling the dialog cancels nothing and the mutation still runs. When a form gains `hx-post`, its `onsubmit` confirm must become `hx-confirm`. An `onclick="return confirm(...)"` on the submit button *is* safe on an htmx form: cancelling the click means the `submit` event htmx listens for never fires (this is how the custom-emoji delete still guards itself on `enclave/settings.html`).
- **Irreversible + wide blast radius gets a stronger gate than a single OK click.** Prefer server-side re-auth (password) or a typed confirmation phrase. The native dialog is dismissible by reflex; data you cannot get back deserves friction the server enforces.

When adding a new destructive action, copy the closest existing example of the matching tier rather than inventing a fourth style.

**Revoking a credential always asks first (LC-738).** A revoke cannot be undone: the token or secret is not recoverable and every integration using it breaks on the next request. Five of the six revoke surfaces (API tokens, room webhooks, room feeds, room email inboxes, sessions) had no confirmation at all, and four of them were a text link in a table row, one click away with nothing separating them from the row's other cells. All six now confirm, and the four text links are `class="btn btn-sm btn-danger-outline"`, the treatment the admin tables already use. The blast radius is one credential, so the single OK click is the right tier here; it is the account and enclave deletes that need the typed or re-auth gate.

- The confirmation string is shared per object type, not per surface: `room-table-revoke-confirm` covers all three room integration tables (they already share the `room-table-revoke` label), with `api-tokens-revoke-confirm`, `settings-session-revoke-confirm` and `admin-invites-revoke-confirm` for the others.
- The session revoke is an htmx form with a plain-POST fallback, so it uses `onclick="return confirm(...)"` on the submit button rather than `hx-confirm` on the form: per the LC-739 rule above the `onclick` form is safe on an htmx form, and it is the only one of the two that also guards the no-htmx submit.
- `ci-build/check-revoke-confirm.nu` (wired into `just check` and the Check workflow) rejects any `<form>` or `<button>` that targets a `/revoke` endpoint without a `confirm` inside it. Email templates are excluded: they render in a mail client with no scripting.

**No apostrophe in a string interpolated into an inline `confirm('...')` (LC-753).** Askama escapes every `{{ }}` through `askama_escape`, which maps `'` to `&#x27;`, and the HTML parser decodes that back to a bare `'` before the handler attribute is compiled as JavaScript. The apostrophe therefore ends the JS string literal early, the handler never compiles, and the destructive action runs on the first click with no dialog and no error the operator can see. `admin-webhooks-rotate-confirm` ("Rotate this webhook's...") disabled the webhook secret rotation confirmation that way, and `partials-dm-block-confirm-suffix` ("They won't...") the DM block one; both are now written without an apostrophe. `hx-confirm="{{ "key"|t }}"` is unaffected: htmx reads the attribute as text, so the escape round-trips and never reaches a JS parser.

- `ci-build/check-confirm-apostrophe.nu` (wired into `just check` and the Check workflow) collects every catalog key interpolated into an inline `confirm('...')` and fails if that key's value carries a `'` (ends the literal) or a `\` (escapes the next character) in any locale under `server/locales/`, naming the catalog line and the template site. It also fails on a `confirm('` whose literal does not close on the same line (it could hide a key from the scan) and on a key no catalog defines. A `"` is safe and is not rejected: the HTML parser decodes `&quot;` after the attribute is tokenized, and a double quote inside a single-quoted JS literal is just text.
- The guard reads catalog values only, so it cannot see a confirmation that interpolates runtime data (a display name, a username) into the same literal. Those sites break the same way and are tracked in LC-771, which converts every inline literal to the `data-lc-confirm` attribute pattern already used at `room/details_panel.html` and `room/manage.html`.

## Form-error rendering

Two contexts, one visual + accessibility contract.

- **Server-rendered pages** (auth flows, full-page forms): the view struct carries `error: Option<String>` (or `Option<&str>`) and the template includes `auth/form_errors.html`. That partial is the single source of the error element's markup (`<p role="alert" class="text-red-600 text-sm">`). Do not hand-roll the `{% if let Some(error) = error %}` block inline; include the partial so every page announces errors identically.
- **In-place forms** (modals, the composer) that stay open after a failed submit and cannot re-render the whole page: use a pre-rendered, initially-`hidden` slot (`role="alert"`, `text-red-600`) that JS un-hides and fills from the response. Existing slots: `.composer-error`, `.thread-error`, `#lc-upload-error`.

Both contexts must share the same visual treatment (`text-red-600`) and carry `role="alert"` so assistive tech announces the error regardless of which rendering path produced it.

A field the server rejected also gets `input-error` (defined in `server/assets/tailwind.css`) on the control itself, alongside the message. The class is a red border and nothing else, so it is never the only signal: the reason always stays readable as text. See `auth/login_approve.html` and `settings/blocked.html`.

## Settings save feedback (LC-429, LC-739)

Every form on a `.lc-set-*` settings surface (`admin/settings.html`, `settings/page.html`, `room/manage.html`, `enclave/settings.html`) carries the same three things, and a form missing any of them is incomplete:

1. `hx-post` at the same URL as its `action`, with `hx-target="find .lc-set-status"`, `hx-swap="innerHTML"` and `hx-disabled-elt="find button[type=submit]"`. `method="post"` and `action` stay, so a no-JS submit still posts and redirects.
2. `<span class="lc-spinner htmx-indicator" aria-hidden="true"></span><span>Label</span>` inside the submit button.
3. A `<span class="lc-set-status" role="status" aria-live="polite">` inside the form, next to that button.

The handler answers both callers: `views::settings::SettingsFeedback` (inline status plus an out-of-band toast) when `HX-Request` is set, its existing redirect otherwise. Where a save changes page content the status fragment cannot patch (a rotated invite code, a removed row, a renamed enclave in the header), answer `routes::redirect_or_hx` instead: htmx navigates to the same URL the no-JS redirect uses, so the reload renders the new content and fires that URL's `?ok=` flash toast.

Failures need no per-form work. A non-2xx leaves the swap untouched and the global net in `settings.js` writes the message into that form's `.lc-set-status` plus a toast, which is why the slot is mandatory even on forms whose success path reloads. Never answer a failed save with a 2xx.

A boolean setting is a checkbox plus a Save button (`.lc-toggle`), not a one-click switch that posts the opposite value: the checkbox already holds the new state, so the inline status can land without a reload. Bind the field `#[serde(default)]`, since an unchecked box posts nothing. The one-click `partials/settings_toggle.html` switch is for surfaces that re-render the whole toggle from the response; it takes the post target as a caller-provided `action` plus the form field `name`, so it is not tied to any one route shape.

Either way the control announces as a switch, because the two look identical and a setting must not be described differently depending on which page it is on (LC-747). The partial's button carries `role="switch"` with a server-set `aria-checked`; a `.lc-toggle` checkbox carries `role="switch"` on the `<input>` itself, where the native checked state supplies `aria-checked`. Never add a literal `aria-checked` to a checkbox: it goes stale the moment the user clicks. `ci-build/check-boolean-settings.nu` (wired into `just check` and the Check workflow) rejects a `.lc-toggle` checkbox with no `role="switch"`, and rejects `.lc-switch` markup anywhere but the partial.

The `.lc-toggle` row itself is written once, in `partials/toggle_row.html` (LC-751). It renders the label, the checkbox with its `role="switch"`, the track and the title, plus the description when `desc` is non-empty. It has no `<form>` of its own: the caller owns the form and the Save button. Bind these names with `{% let %}` and include it:

```jinja
{% let name = "read_receipts_enabled" %}
{% let checked = user.read_receipts_enabled %}
{% let disabled = false %}
{% let title = "settings-pref-read-receipts"|t %}
{% let desc = "" %}
{% include "partials/toggle_row.html" %}
```

- Where the description depends on a condition, bind `desc` inside the `{% if %}` branch and include the partial in each branch: askama scopes a `{% let %}` to its block, so a binding made inside the branch is not visible after it (the three email rows in `settings/page.html` are the example).
- A description carrying inline markup cannot come through the binding, because askama escapes it, and a `|safe` binding would put translated content on an unescaped path. Those rows stay hand-written and are named, with their reason, in `EXEMPT_ROWS` in `ci-build/check-boolean-settings.nu`: the push row in `settings/page.html` (a `<code>` naming `LETS_CHAT_SECRET_KEY`) and the link-filter row in `admin/anti_spam.html` (a link to `/admin/link-filter`). The same check rejects `.lc-toggle` markup spelled out in any other template, and fails if an exempt file grows a second hand-written row.

A checkbox that is one option among several (an API-token scope list, a webhook event list, the LLM audience) is not a switch and correctly stays a plain checkbox; it lives inside a `<fieldset>` with a `<legend>` naming the group, and carries no `.lc-toggle` class.

## Required fields (LC-750)

Every `required` control carries three things, and a form that has one without the others is incomplete:

1. `required` for the browser.
2. `aria-required="true"` alongside it.
3. `{% include "partials/required_mark.html" %}` inside the control's label.

The partial renders an `aria-hidden` `*` plus an `sr-only` " (required)". The asterisk is decoration; the text is what conveys the requirement, so the meaning never rests on a glyph or on color alone. Style comes from `.lc-req` in `main.css`.

A control with no visible label has nowhere to put the marker, so give it one rather than settling for a placeholder: a placeholder is not a label, disappears on the first keystroke, and cannot host the marker. Several compact single-field forms (the sidebar category rename, the enclave join-code row) grew a label for exactly this reason.

`partials/file_picker.html` handles all three itself when its caller binds `{% let required = true %}`.

## Accessible names on form controls (LC-746)

Every `<input>`, `<select>` and `<textarea>` resolves an accessible name, from a `<label for>`, a wrapping `<label>`, `aria-label` or `aria-labelledby`. A `placeholder` is not a name: a screen reader announces it only as a fallback, and it disappears the moment the user types.

- A row editor or a filter field in a dense table takes `aria-label` with the key the column header already uses.
- Where voice control should be able to say the field's name, use an `sr-only` `<label for>` instead, so the name is a real label. The `admin/branding.html` hex fields do this.
- `type="hidden"`, `submit`, `button`, `reset` and `image` are exempt: they are not announced, or they take their name from `value` / `alt`.

`server/tests/template_a11y.rs` sweeps every template and fails with the file, line and tag of anything unnamed. It is a test rather than a rule in `check-ui-conventions.nu` because a wrapping `<label>` can only be resolved from element ranges, not from a single line.

## File pickers (LC-740)

There is exactly **one** file picker: `partials/file_picker.html` plus the delegated handler in `server/assets/file_picker.js` (loaded in `base.html`). It renders a styled `<label class="btn btn-secondary btn-sm">` trigger, a filename echo, an `sr-only` `<input type="file">` and a `hidden role="alert"` error slot. The handler echoes the chosen filename, rejects a wrong type (matched against `accept`) or an oversized file (against `data-lc-max-bytes`) before submit, and disables the form's submit button while the pick is invalid.

Bind these names with `{% let %}` and include the partial; put any help text after the include so the error sits directly under the control:

```jinja
{% let input_id = "lc-avatar-input" %}
{% let name = "avatar" %}
{% let accept = "image/png,image/jpeg,image/webp" %}
{% let max_bytes = "1048576" %}
{% let choose_label = "settings-choose-image"|t %}
{% let no_file_label = "settings-no-file"|t %}
{% let err_type = "settings-avatar-err-type"|t %}
{% let err_size = "settings-avatar-err-size"|t %}
{% let required = false %}
{% include "partials/file_picker.html" %}
```

- `max_bytes` MUST equal the cap the handler behind `name` enforces (`MAX_AVATAR_BYTES`, `MAX_EMOJI_BYTES`, `persist_brand_file`'s 1 MiB, the route's `DefaultBodyLimit`). Read it from the handler rather than picking a round number, and cite it in a template comment.
- The input stays `sr-only`, so the browser's own "no file chosen" chrome never renders next to our echo. `ci-build/check-file-pickers.nu` (wired into `just check` and the Check workflow) rejects any `<input type="file">` in a template that is neither `sr-only` nor `class="hidden"`; the one `class="hidden"` exemption is the composer's programmatically-driven attachment input. `server/tests/routes_file_pickers.rs` pins the rendered shape and every `data-lc-max-bytes`.
- Per-site extras (the avatar preview in `settings.js`, the logo preview in `branding.js`) listen for the `lc:file-picked` event the handler dispatches (`detail.file` is null when the pick was rejected). Never re-implement the filename echo or the validation.

## Data tables (LC-510, LC-737, LC-745)

Every record list is `<div class="card lc-table-wrap">` around `<table class="lc-table">`, with bare `<th>` and `<td>`. `.lc-table` (`main.css`) owns the cell padding, the header color, the row rules and the hover, and it is declared in exactly one place: `tailwind.css` used to declare the same class name a second time, with explicit head / row / cell sub-classes, and because `base.html` loads `main.css` second the sub-classes styled nothing outside the dev gallery (deleted in LC-744); the actions column is `class="lc-table-actions"` on both the header cell and the body cell (right-aligned, `white-space: nowrap`). `admin/invites.html` is the reference. A list scoped to a room reads the same as the server-wide one because both are the same component, not because someone matched the utilities by eye.

`.lc-table-wrap` is `overflow-x: auto` in `main.css`, so the rows still go edge to edge and the card's rounded corners still clip them (a scroll container on one axis is a clipping context on the other), but a table wider than the viewport scrolls instead of hiding its trailing columns.

- Never wrap a table in `overflow-hidden`. The widest admin tables carry 5 to 6 columns plus a `white-space: nowrap` `.lc-table-actions` cell with up to four buttons, and clipping made those buttons unreachable at 375px: no scrollbar, no drag, no way to get to them. `ci-build/check-ui-conventions.nu` rejects a `card overflow-hidden` wrapper anywhere under `server/templates/admin/`.
- Never put a padding utility on a `<th>` or `<td>`. The three room integration tables hand-rolled `py-2` headers and `py-1` cells, which is a row 6px shorter than every admin table; the component is the one place row height is decided.
- An empty list is `partials/empty_state.html` under the page heading, not a `colspan` row inside a table with no data in it.
- `ci-build/check-table-scroll.nu` rejects any `<table>` whose wrapper does not carry `lc-table-wrap` or `overflow-x-auto`. `ci-build/check-table-shape.nu` rejects a `<table>` that is not `.lc-table`, an `.lc-table` outside a `.card` wrapper, and a padding utility on any cell inside one. Both are wired into `just check` and the Check workflow; both exclude the email templates, which are inline-styled layout tables in a mail client.
- Two tables are named in `check-table-shape.nu`'s exemption list: the cohort-retention heat grid (`admin/analytics.html`) and the IMAP ingress drop log (`admin/settings.html`). Both are real data tables that were outside LC-745's file set, and moving them onto the shared component is tracked in LC-756. The component gallery (`dev/theme_gallery.html`) is no longer exempt: LC-744 put it on the production markup, because a gallery that demonstrates a different contract from the one the 13 real tables use is worse than no gallery.

## Empty states (LC-454, LC-557, LC-744)

An empty collection renders `partials/empty_state.html`: the caller binds the message key and includes it.

```
{% let empty_text_key = "room-webhooks-empty" %}
{% include "partials/empty_state.html" %}
```

That gives one icon, one size and one color everywhere, instead of the six treatments the 2026-08-11 audit found (a `<p>` at four sizes, an italic variant, a padding-less variant, and a `colspan` row at `text-xs text-content-subtle`, the codebase's quietest color at its smallest size on the surface where an operator most needs to see that nothing is configured yet).

For an empty state that carries a call to action, bind two more values and include the block directly; the action then sits inside the same centered column rather than under it.

```
{% let empty_text_key = "room-webhooks-empty" %}
{% let empty_action_href = "#room-webhooks-create" %}
{% let empty_action_key = "room-webhooks-create-button" %}
{% include "partials/empty_state_block.html" %}
```

`partials/empty_state.html` is the text-only entry point: it binds the two action values to the empty string and includes the same block. Askama has no optional include parameter, so a value the block reads has to be bound on every path.

## Page widths (LC-562, LC-744)

A content column takes its width from one of three helpers in `tailwind.css`, chosen by role, never from a per-page `max-w-*`:

| Helper | Width | Use for |
|---|---|---|
| `lc-page-narrow` | `max-w-lg` | login, error, maintenance, short standalone forms |
| `lc-page-medium` | `max-w-3xl` | settings-card and content pages |
| `lc-page-wide` | `max-w-5xl` | admin table pages |

All three centre the column and go full-width on mobile. `lc-page-pad` (`px-4 py-6 sm:px-6`) and `lc-page-stack` (`flex flex-col gap-6`) go with them on the pages that used to hand-write those. Picking the width per page is what left two single-column settings pages in the same sibling group at `max-w-3xl` and `max-w-2xl`. `landing.html` is the one exclusion: it is a marketing page with a deliberately wider grid, and the gate skips it by name.

## Callouts and secret reveals (LC-555, LC-744)

An inline callout is `.alert` plus one of `alert-success` / `alert-warning` / `alert-danger` / `alert-info`, with `role="alert"` on the ones that report a failure or a one-time value. The base class owns the radius, the padding and the `flex items-start gap-2.5` layout; the modifier owns the tokens. Hand-rolled copies of it drifted by 2px and 4px of padding and by one radius step, and three admin templates used the component for the error branch of an `if` and hand-rolled the success branch of the same `if`.

The seven "here is your new secret, copy it now" surfaces (room webhooks / feeds / email inboxes, personal API tokens, admin bots / bridges / outgoing webhooks) all render `partials/secret_reveal.html`, which is that alert plus the monospace value. The caller binds `secret_heading_key` and `secret_value`, and passes `""` for the `secret_subject` / `secret_suffix_key` / `secret_note_key` it does not use.

## Tabs (LC-747)

There is exactly **one** tablist controller: `server/assets/tabs.js`, loaded in `base.html` before its consumers and precached by `sw.js`. It exposes `window.lcInitTabs(root, storageKey)`; the settings page, the enclave settings page and the room-info page each call it with their own root and key (`lc-settings-tab`, `lc-enclave-tab`, `lc-roominfo-tab`) so the three remember their tab independently.

Markup contract: `[data-lc-tab="<key>"]` triggers with `role="tab"`, `[data-lc-tabpanel="<key>"]` panels with `role="tabpanel"`, all inside the root. Panels stay in the DOM so their htmx wiring survives; the controller only toggles `hidden`, `aria-selected` and the roving tabindex.

- The initial tab is the URL hash, then the `sessionStorage` value, then whichever tab the server pre-rendered as `aria-selected="true"`, then the first.
- On `hashchange`, only a hash that names a tab moves the panel. Following an in-page anchor to anything else leaves the visible panel alone. The three copies this replaced had already drifted on exactly this point: `settings.js` re-ran its full fallback chain on every `hashchange`, so an unrelated anchor silently reset the user to the remembered or first tab.
- Arrow keys wrap in both directions, `Home` and `End` jump to the ends, and the moved-to tab takes focus.
- `ci-build/check-single-tab-controller.nu` (wired into `just check` and the Check workflow) rejects any `[data-lc-tab]` reference in an asset other than `tabs.js`, and checks that each consumer calls `window.lcInitTabs`, that `base.html` loads the file, and that `sw.js` precaches it.

## Result listboxes (LC-736)

Every combobox result list is a `role="listbox"` whose rows carry `role="option"`, a unique `id` and `aria-selected`. `server/assets/search.js` (sidebar people/messages, room search, quick switcher, enclave invite, group add-member) and the composer's own popover nav (`@mention`, `/slash`, `:emoji:`, `#channel`) both drive selection by toggling `aria-selected` and pointing `aria-activedescendant` at the active row. Neither toggles a class.

The keyboard highlight is owned by exactly **one** rule, `[role="option"][aria-selected="true"]` in `main.css`, which paints `var(--surface-sunken)`. It sits in the base layer above the component rules, so `.lc-search-row` and `.lc-ac-option` can still specialize at equal specificity, and a listbox added later is highlighted without touching CSS. A companion `[data-mode="hc-*"]` rule adds an inset `var(--ring)` outline, because the high-contrast palettes collapse `--surface-sunken` onto `--surface-elevated` and a wash alone is invisible there.

- Never set the highlight from JavaScript. A class toggled in JS is a second owner of an appearance the CSS already owns via the attribute, and the two drift apart (`search.js` shipped a `bg-slate-100` toggle that outranked the tokenized `.lc-search-row` rule).
- Panel chrome comes from `.lc-search-panel` with the scrolling `role="listbox"` nested inside it, and rows from `.lc-search-row` (add `.lc-search-row--center` for single-line rows; the default top alignment is for the two-line message snippet). Do not hand-roll `bg-surface-elevated border border-border-strong rounded shadow-lg`.
- A row swapped in by HTMX (`enclave/invite_row_result.html`, `enclave/group_member_row_result.html`) uses the same row classes, so the chrome does not change under the swap.

## Avatars and presence badges

There is exactly **one** avatar renderer: `partials/avatar.html`. It renders the image-or-initial circle plus the presence status dot, driven by these caller-bound names:

```
avatar_user_id, avatar_username, avatar_ext, avatar_status, avatar_custom_status, avatar_size
```

To render an avatar inside a `{% for %}` loop, bind the row's fields to those names and include the partial (see `users/search.html`, `enclave/invite_search.html`, `partials/mention_popover.html`):

```jinja
{% let avatar_user_id = r.id.clone() %}
{% let avatar_username = r.username.clone() %}
{% let avatar_ext = r.avatar_ext.clone() %}
{% let avatar_status = r.status.clone() %}
{% let avatar_custom_status = r.custom_status.clone() %}
{% let avatar_size = "h-6 w-6 text-xs" %}
{% include "partials/avatar.html" %}
```

Never re-implement the avatar/badge markup inline. Presence (`avatar_status`) should be resolved with `routes::effective_status` so a disconnected user shows offline consistently with every other surface. Glyph-only badges that are not user avatars (the `@here`/`@channel` broadcast token, the `#group` token in the mention popover) legitimately render their own small glyph and do not use this partial.

## Tooltips (LC-370)

Styled tooltips come from one shared helper: `server/assets/tooltip.js` (loaded in `base.html`) plus the `#lc-tooltip` rule in `main.css`. Drive a tooltip declaratively with attributes on the trigger - never hand-roll a positioned tooltip element, and prefer this over the native `title=` attribute (which is unstyled, theme-blind, and clipped by `overflow:auto` ancestors like the enclave rail).

```html
<a href="..." aria-label="Settings" data-lc-tip="Settings" data-lc-tip-pos="right">...</a>
```

- `data-lc-tip="<text>"` is the visible tooltip text. `data-lc-tip-pos` is `top` (default) | `right` | `bottom` | `left`; the helper clamps into the viewport, so `pos` is a preference, not a guarantee.
- The tooltip is a single `position:fixed` element appended to `<body>`, so it escapes `overflow` clipping. It shows after a 400ms hover delay, immediately on keyboard focus, and hides on leave/blur/scroll/resize/Escape.
- **Accessibility:** the tooltip element is `aria-hidden`; it is decoration, not the accessible name. The trigger must carry its own accessible name. For an icon-only control add an explicit `aria-label`; a control with visible text already has one, so do not add a redundant `aria-label` (it would override the visible text). Do not leave a `title=` alongside `data-lc-tip` - it double-renders (native + styled).
- Placement convention: vertical rails (the enclave switcher) use `data-lc-tip-pos="right"`; top bars (the room header) use `data-lc-tip-pos="bottom"`, so the tooltip opens away from the chrome edge.

## Convention gates (LC-749)

`ci-build/check-ui-conventions.nu` (`just check-ui-conventions`, and a step of the Check workflow) holds closed the classes that reopen the moment someone adds a file. Each is one pattern with its own allowlist, and each failure message names the convention and the issue that established it.

| Rule | Scope | Allowed exceptions |
|---|---|---|
| No palette literals in templates | `server/templates/**/*.html` | a line (or the line above it) carrying an `lc-allow-palette` comment naming why, for the deliberately-dark call stage |
| No fake link buttons | `<button ... hover:underline>` in templates | none |
| No open-coded danger outline | the literal `.btn-danger-outline` expansion | none |
| No untokenized borders | a `border` / `border-t` / `divide-y` class attribute with no `border-`/`divide-` color | none; a numbered palette color counts as named here and is the palette rule's to reject |
| No superseded component classes | the class names LC-744 deleted, over the templates and both stylesheets | none |
| Page width from a helper | `mx-auto` next to a `max-w-*` in a template | `landing.html`, a marketing page with a wider grid |
| No hand-rolled callouts | an all-sides `border` with a matching `bg-*-surface` + `border-*-border` pair and no `alert` | a pill (`rounded-full`) and an edge rule (`border-b` alone), which are different components |
| No clipping table wrappers | `card overflow-hidden` under `server/templates/admin/` | none |
| No raw h1 sizes | `<h1 ... text-lg/xl/2xl/...>` in templates | `landing.html`, a marketing hero outside the app scale |
| h1 on the scale | an `<h1>` whose `class` carries neither `lc-h1` nor `lc-display` | `landing.html`, as above |
| One h1 per template | a second `<h1>` in the same template file | none |
| Timestamps are time elements | `{{ ..._at }}` on a line with no `<time>` | none; `email/` is already out of scope |
| Offline page brand name | `lets-chat` in `server/assets/offline.html` and `server/assets/sw.js` | a comment line, which may keep the repo name |
| Offline page follows the mode | a light-only `color-scheme` in `server/assets/offline.html` | none |
| No raw NUL bytes | a literal U+0000 in every tracked text file | vendored assets (`server/assets/vendor/`); binary assets carry no text extension and are never read |
| One ellipsis glyph, no em dash | U+2026 in the locale catalogs; U+2014 in every tracked text file | vendored assets (`server/assets/vendor/`) |

Email templates are excluded from every template rule: they render in a mail client with no stylesheet, so a Tailwind class there is inert. The U+2026 half lives in `ci-build/check-locale-ellipsis.nu` and the `server/assets/**/*.js` palette rule in `ci-build/check-asset-color-tokens.nu`; both run in the same job.

Two things to know before editing the script:

- A rule whose class is not clear yet carries a `pending: "<issue>"` marker. It still runs and prints its hits on every run, but does not fail the build; the issue named in the marker deletes it as part of its own change, which is why each of those issues carries "add the CI check" in its own acceptance criteria. The script is the source of which rules are live; do not restate that here.
- Read files with `open --raw`, never `grep -r`, so no rule depends on grep's binary heuristic: a single raw control byte makes every grep-family tool skip the whole file silently. A NUL written as a raw byte in `layout.html` did exactly that until LC-757 respelled it as the `\u0000` JS escape; the `no-raw-nul-bytes` rule now fails the build if one comes back.
