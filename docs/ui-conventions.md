# UI conventions

Shared front-end patterns every new surface should inherit instead of re-deriving. Born out of the LC-148 audit's C3 finding (each new surface silently re-implements a concept rather than reusing it). Keep this list short; when a pattern here changes, update the doc in the same PR.

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
