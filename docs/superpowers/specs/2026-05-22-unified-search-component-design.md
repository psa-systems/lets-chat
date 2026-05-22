# Design: unified search / typeahead component (LC-157)

Status: proposed. Parent audit: `docs/audit/2026-05-22-lc148-audit-report.md` (gap C2).

## Problem

Six distinct search/typeahead implementations, three tiers of quality for one concept:

| Surface | Trigger | Result render | Keyboard nav | a11y |
|---|---|---|---|---|
| Sidebar message search | `input delay:200ms, keyup Enter` -> `/search` | HTML fragment into `#search-msg-results` | none | none |
| Sidebar people search | same -> `/users/search` | fragment into `#search-people-results` | none | none |
| Enclave invite search | same -> `/enclave/{id}/invite/search` | fragment | none | none |
| Group add-members search | same -> `/enclave/{id}/groups/{gid}/members/search` | fragment | none | none |
| Mention autocomplete | `input` + JS token parse -> `/users/mentions` | popover via `htmx.ajax` | arrows/enter/tab/escape | full `aria-combobox` |
| Slash-command autocomplete | `input` + regex -> `/api/slash-commands` | popover | none (click only) | none |

The four generic searches are copy-paste of the same trigger + fragment pattern with per-instance endpoint/container/empty-string, none keyboard-navigable. The two composer comboboxes diverge sharply (one fully accessible, one not). Adding a new search means re-deciding debounce, nav, rendering, and empty state from scratch.

## Proposal

One reusable typeahead, two layers:

### 1. A server-side result-fragment contract

Define a single Askama partial shape every search endpoint renders: a `<ul role="listbox">` of `<li role="option" data-lc-search-item data-lc-value="...">` rows, with a standard empty-state row and a standard item layout (avatar + label + optional action). Each existing endpoint (`/search`, `/users/search`, invite/group searches) re-renders into this shape. Result rendering stops being per-surface markup.

### 2. A client-side `data-lc-search` controller (one shared JS module)

Declarative wiring on the input:

```html
<input data-lc-search
       data-lc-search-url="/users/search"
       data-lc-search-target="#search-people-results"
       data-lc-search-min="1"
       data-lc-search-debounce="200"
       aria-controls="search-people-results"
       role="combobox" aria-expanded="false" aria-autocomplete="list">
```

The controller provides, for every search, by construction:
- Debounced fetch (default 200ms) via htmx `ajax` into the target, with a min-length gate.
- Keyboard navigation: Up/Down move `aria-activedescendant` across `[role=option]`, Enter/Tab activate the focused item, Escape closes/clears. (Lifts the mention combobox's existing logic into the shared module.)
- a11y: manages `aria-expanded`, `aria-activedescendant`, `aria-selected`, and announces result counts.
- Activation semantics: an item either navigates (anchor href), inserts text (composer), or submits an inline form (invite/add) - selectable via `data-lc-search-action="navigate|insert|submit"` so the same controller serves all current uses.
- Empty/loading/error states from the standard fragment.

### 3. Fold the composer comboboxes in

The mention and slash autocompletes become instances of the same controller (`action="insert"`), differing only in URL + token-detection trigger. This removes the slash combobox's a11y gap for free and collapses three keyboard-handling code paths into one.

## Migration

1. Land the shared fragment partial + `data-lc-search` controller; migrate the 4 generic sidebar/enclave searches first (they are nearly identical, lowest risk).
2. Migrate the mention combobox (port its keyboard logic into the controller as the reference implementation, then delete the inline version).
3. Migrate the slash combobox last (gains keyboard nav).

## Acceptance

- One JS module + one fragment shape; a new search is an input with `data-lc-search-*` attributes plus an endpoint that renders the standard fragment.
- All searches share debounce, keyboard nav, and a11y; the slash combobox gains keyboard nav.
- No per-surface typeahead JS remains.

## Out of scope

- Changing what each search queries or its ranking. This unifies the interaction shell, not the search backends.
