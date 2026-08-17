# UI conventions

Shared front-end patterns every new surface should inherit instead of re-deriving. Born out of a recurring problem: each new surface silently re-implements a concept rather than reusing it. Keep this list short; when a pattern here changes, update the doc in the same PR.

## Typography scale (LC-365)

Use the semantic typography classes from `server/assets/main.css` instead of hand-rolling `text-2xl font-semibold` combinations. Each class fixes font-size + weight + line-height (+ tracking) so headings and labels stay consistent across surfaces. Color is left to the caller (`text-content` / `text-content-muted`), except `lc-caption` and `lc-section-label`, which already imply a muted tone.

| Class | Use for | Size / weight |
|---|---|---|
| `lc-display` | hero / empty-state heading | 1.5rem / 600, tight |
| `lc-h1` | page or panel heading | 1.125rem / 600 |
| `lc-h2` | section heading | 1rem / 600 |
| `lc-h3` | card / row title | 0.875rem / 600 |
| `lc-body` | default body text | 0.875rem / 400 |
| `lc-small` | secondary / help text | 0.75rem / 400 |
| `lc-caption` | timestamps, captions (muted) | 0.75rem / 400 |
| `lc-section-label` | uppercase group label (sidebar sections) | 0.6875rem / 600, uppercase |

The message body (`.lc-md`) keeps its own size, which the per-device text-size control scales; do not apply the chrome scale there.

## Color tokens (LC-735)

Every color comes from the semantic tokens in `server/tailwind.config.js` (`text-content`, `text-content-muted`, `text-content-subtle`, `bg-surface{,-elevated,-sunken}`, `border-border`, `text-danger`, ...), which resolve to the CSS vars in `server/assets/main.css` and so recolor across all four modes (`light`, `dark`, `hc-light`, `hc-dark`) and every palette. A raw numbered utility like `text-slate-700` is fixed for all four, so it survives review in the mode it was written for and goes unreadable in the others.

`tailwind.config.js` uses `extend`, not an override, so the raw palette still compiles: nothing stops a raw shade except the review. The convention applies to every surface. `ci-build/check-asset-color-tokens.nu` (wired into `just check` and the Check workflow) enforces it mechanically for `server/assets/**/*.js` outside `vendor/`, where markup is built in JS and injected in transient states (offline banner, failed send, active search row) that theme testing rarely reaches.

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
| `onsubmit="return confirm(...)"` / `onclick="return confirm(...)"` | The same low-stakes confirmation on a **plain `<form>` POST** that is not htmx-driven (e.g. block user, transfer enclave ownership, delete a custom emoji). | Native `confirm()` wired directly to the DOM event. |
| Typed / re-auth verification | Irreversible, high-blast-radius actions: delete account, delete an entire enclave, stage a backup restore. | A real form gate the user must satisfy (password re-entry, or typing a confirmation phrase) handled server-side. A single `confirm()` OK click is **not** sufficient here. |

Decision rules:

- **Default to `hx-confirm`** for htmx-driven destructive actions. It is the least code and is already the majority pattern.
- **`hx-confirm` only guards htmx requests.** It does nothing for a plain `<form>` submit. Do not add `hx-confirm` to a non-htmx form expecting it to fire; use `onsubmit="return confirm(...)"` instead, or convert the action to htmx first. Converting a plain form to htmx is behavior-changing (response handling, redirect vs. fragment) and must be audited per site, not done blindly.
- **Irreversible + wide blast radius gets a stronger gate than a single OK click.** Prefer server-side re-auth (password) or a typed confirmation phrase. The native dialog is dismissible by reflex; data you cannot get back deserves friction the server enforces.

When adding a new destructive action, copy the closest existing example of the matching tier rather than inventing a fourth style.

## Form-error rendering

Two contexts, one visual + accessibility contract.

- **Server-rendered pages** (auth flows, full-page forms): the view struct carries `error: Option<String>` (or `Option<&str>`) and the template includes `auth/form_errors.html`. That partial is the single source of the error element's markup (`<p role="alert" class="text-red-600 text-sm">`). Do not hand-roll the `{% if let Some(error) = error %}` block inline; include the partial so every page announces errors identically.
- **In-place forms** (modals, the composer) that stay open after a failed submit and cannot re-render the whole page: use a pre-rendered, initially-`hidden` slot (`role="alert"`, `text-red-600`) that JS un-hides and fills from the response. Existing slots: `.composer-error`, `.thread-error`, `#lc-upload-error`.

Both contexts must share the same visual treatment (`text-red-600`) and carry `role="alert"` so assistive tech announces the error regardless of which rendering path produced it.

A field the server rejected also gets `input-error` (defined in `server/assets/tailwind.css`) on the control itself, alongside the message. The class is a red border and nothing else, so it is never the only signal: the reason always stays readable as text. See `auth/login_approve.html` and `settings/blocked.html`.

## Required fields (LC-750)

Every `required` control carries three things, and a form that has one without the others is incomplete:

1. `required` for the browser.
2. `aria-required="true"` alongside it.
3. `{% include "partials/required_mark.html" %}` inside the control's label.

The partial renders an `aria-hidden` `*` plus an `sr-only` " (required)". The asterisk is decoration; the text is what conveys the requirement, so the meaning never rests on a glyph or on color alone. Style comes from `.lc-req` in `main.css`.

A control with no visible label has nowhere to put the marker, so give it one rather than settling for a placeholder: a placeholder is not a label, disappears on the first keystroke, and cannot host the marker. Several compact single-field forms (the sidebar category rename, the enclave join-code row) grew a label for exactly this reason.

`partials/file_picker.html` handles all three itself when its caller binds `{% let required = true %}`.

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
