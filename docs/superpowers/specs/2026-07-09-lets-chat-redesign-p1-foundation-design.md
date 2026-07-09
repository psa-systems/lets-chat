# Let's Chat UI/UX Redesign - Phase 1 (Foundation) Design

Date: 2026-07-09
Status: Approved design, pending implementation plan
Tracking: LC-541 (redesign umbrella)

## Program context

The redesign makes every surface of Let's Chat consistent with a new set of mockups (a polished Blue-Harbor-style chat UI) while keeping all existing functionality. Two scope decisions are locked:

- Scope: full pixel-match. The ~10 surfaces the mockups actually show (enclave rail, room sidebar, room header, message timeline, reactions, unread banner + divider, thread panel, the new Details panel, composer) are matched literally. The other ~195 surfaces have no mockup to match, so there "consistent" means: apply the design language and shared components defined in this foundation. That is what makes the sweep mechanical rather than 195 judgment calls.
- Themes: six selectable palettes (Blue Harbor, Cobalt Workspace, Ink + Ice, Arctic Messenger, Deep Sea Cyan, Royal Navy), each with light, dark, and high-contrast variants.

Because full pixel-match + 6 palettes + the net-new Details panel across 205 templates is too large for one spec, the work is a phased program under LC-541, each phase its own spec -> plan -> build -> review:

- P1 Foundation (this document): 6-palette token system, theme selection model, appearance picker, component-base retune, proof gallery.
- P2 Core chat pixel-match: rail, sidebar, header, timeline, reactions, composer, thread panel, and the new Details panel (Created / Members / Leave).
- P3 Utility-page migration: feeds, webhooks, email-inboxes, pins, api-tokens, blocked, discover, invitations onto the component idiom.
- P4 Remaining-surface sweep: the ~180 other surfaces (calls, polls, events, wiki, bridges, settings, admin polish, transcripts) to the design language.
- P5 Public/brand surfaces: landing, login, welcome, error.
- P6 Accessibility + density + regression pass across all 6 palettes x light/dark/HC, both densities, mobile drawer, htmx/WS swaps.

## Current architecture (verified against source 2026-07-09)

- Rust + axum 0.8 + Askama 0.12 templates, htmx, small vanilla JS. Real-time via WebSocket delivering pre-rendered Askama fragments swapped with `hx-swap-oob`. No SPA framework. Not to be rewritten.
- Semantic CSS-variable token layer in `server/assets/main.css` (~40 tokens). Tokens are mapped to Tailwind utilities (`bg-surface`, `text-content`, `bg-accent`, etc.) in `server/tailwind.config.js` `theme.extend.colors`. Component classes (`.btn*`, `.input`, `.card`, `.alert*`, `.lc-page-*`) live in `server/assets/tailwind.css` `@layer components`, compiled to the gitignored `tailwind-built.css` via bun (`justfile`).
- Four themes today: `light` (`:root`, main.css:187), `dark` (main.css:296), `hc-light` (main.css:388), `hc-dark` (main.css:451), selected by a single flat `data-theme` attribute.
- No-flash theme bootstrap: synchronous script in `server/templates/base.html:26-102`, before stylesheets, resolving `lc-theme` cookie -> localStorage -> OS (`prefers-color-scheme` + `prefers-contrast`). Tailwind `darkMode: ["selector", '[data-theme="dark"]']` (tailwind.config.js:7).
- Persistence: `users.theme TEXT` (migration `0025_user_theme.sql`), synced to the `lc-theme` cookie by the locale middleware (`server/src/auth.rs:209`); routes `post_theme` (`server/src/routes/settings.rs:618`), db `set_user_theme` (`server/src/db/auth.rs:778`), model `theme_or_system` (`server/src/models/user.rs:121`).
- The `--rail-*` and `--sidebar-*` vars are NOT mapped to Tailwind utilities; they are consumed by raw `.lc-rail-*` / `.lc-sidebar` rules in main.css. Each palette must therefore author its own rail/sidebar colors.

## P1 goal

Establish the theming and component foundation the entire program depends on: six palettes, each in light/dark/hc-light/hc-dark, selectable via a live appearance picker, with the shared component vocabulary retuned to the mockup spec and proven by a theme/component gallery. After P1 the app looks identical by default (Blue Harbor) but gains 6 selectable palettes and the foundation for phases 2-6.

## P1 design

### 1. Theme selection model (architecture B)

Two orthogonal attributes on `<html>`:

- `data-theme` = palette: `blue-harbor` (default), `cobalt`, `ink-ice`, `arctic`, `deep-sea`, `royal-navy`.
- `data-mode` = mode: `light`, `dark`, `hc-light`, `hc-dark`.

Palette unset resolves to blue-harbor, so an unstyled document is today's exact look. Tailwind changes to `darkMode: ['selector', '[data-mode~="dark"]']` so `dark:` variants fire for both `dark` and `hc-dark`.

Rationale: palette and mode are genuinely independent (you pick a palette; light/dark/contrast is orthogonal). This is the clean end-state model, chosen deliberately over the additive-`data-palette` compromise because the full overhaul rewrites the theme CSS regardless, so there is no reason to bank a naming scar or a second migration later.

### 2. Token architecture

- The ~40 semantic token names are unchanged and remain the single source of truth: `--surface/-elevated/-sunken`, `--content/-muted/-subtle`, `--border/-strong`, `--accent/-hover/-content/-surface/-surface-content`, `--ring`, the `--success/-warning/-danger` trio with their `-surface/-border/-surface-content`, the actor-badge vars (`--webhook/-email/-bridge-*`), and the raw-CSS `--rail-*` / `--sidebar-*`.
- Every palette defines the full token set for every mode. 6 palettes x 4 modes = 24 blocks in main.css:
  - `[data-theme="<p>"]` -> palette light vars (default mode)
  - `[data-theme="<p>"][data-mode="dark"]` -> dark overrides
  - `[data-theme="<p>"][data-mode="hc-light"]` and `[data-theme="<p>"][data-mode="hc-dark"]` -> high-contrast overrides
- Each palette block includes its own `--rail-*` / `--sidebar-*` values (drives the dark-navy sidebar per palette).
- Downstream phases consume tokens only and never hardcode a hex. The blue-harbor light/dark values are the current production values; cobalt/ink-ice/arctic/deep-sea/royal-navy light+dark values come from the provided palette stubs; all 24 hc-light/hc-dark blocks are authored to the AAA bar (section 6, AC5).

### 3. No-flash bootstrap port

`server/templates/base.html:26-102` is ported to two independent axes, preserving the synchronous-before-stylesheets pattern:

- Palette: `lc-palette` cookie -> localStorage -> `blue-harbor`. Sets `data-theme`.
- Mode: today's resolve logic (cookie -> localStorage -> `prefers-color-scheme`/`prefers-contrast`, with the `EXPLICIT`/`FLIP`/`META` maps and the `contrast:more` -> HC upgrade) moves verbatim onto `data-mode`. Mode values are unchanged.
- JS globals: add `__lcSetPalette(p)`; rename `__lcSetTheme` -> `__lcSetMode(m)`; keep `__lcToggleTheme` (flips mode within the contrast family). Density and sidebar bootstraps are untouched.
- `<meta name="theme-color">` continues to track the active surface; the META map is keyed by mode and, where a palette's surface differs enough to matter, by palette.

### 4. Persistence + migration

- Rename `users.theme` -> `users.theme_mode` (pure column rename, data intact) and add `users.theme_palette TEXT` (NULL = blue-harbor). New migration under `server/migrations/auth/`.
- Update the Rust references mechanically: `server/src/models/user.rs` (`theme_or_system` -> mode accessor + new palette accessor), `server/src/db/auth.rs` (`set_user_theme` -> mode setter + palette setter), `server/src/routes/settings.rs` (`post_theme` keeps setting mode; add `post_palette`), `server/src/auth.rs` cookie sync (emit both `lc-mode` and `lc-palette` from the two columns).
- Existing users need zero data transform: the stored value is already a mode; palette defaults to blue-harbor, so everyone keeps their current look.
- Routes: keep `POST /settings/theme` (mode, used by the sidebar quick-toggle); add `POST /settings/palette`.

### 5. Appearance picker

In `server/templates/settings/page.html` (appearance section) + `server/assets/settings.js`:

- Palette: six swatch cards, each previewing accent + surface + sidebar; radio-select calls `__lcSetPalette(name)` -> applies `data-theme` instantly, writes localStorage, fire-and-forget `POST /settings/palette`.
- Mode: light / dark / system control plus a high-contrast toggle, calling `__lcSetMode(m)` -> applies `data-mode` instantly, persists via `POST /settings/theme`.
- Live, no reload (attributes on `<html>`); identical persistence pattern to today. The sidebar quick-toggle stays.

### 6. Component base retune + additions (YAGNI)

- Retune the existing shared classes in `server/assets/tailwind.css` `@layer components` to the mockup spec (radii, shadow, spacing, weight) via token/@layer edits only, no structural change: `.btn*`, `.input`, `.card`, `.alert*`, `.toast`, `.modal*`, `.lc-page-*`, and the `.lc-set-*` family in main.css.
- Add the one genuinely app-wide primitive that is missing and that P3 needs: a `.lc-table` family (`.lc-table`, head/row/cell). No speculative library; Details-panel-specific parts are built in P2 when first used.

### 7. Theme/component gallery (P1 proof + P6 regression harness)

One internal route (e.g. `GET /dev/theme-gallery`, gated to non-production or admin) rendering every shared component in all 6 palettes x 4 modes. Validates the foundation end-to-end without touching a product surface; reused as the standing regression surface in P6.

## Out of scope for P1 (deferred to P2+)

No product-surface redesign, no Details panel, no sidebar relabel, no utility-page migration. P1 changes tokens, selection, persistence, the picker, shared component styling, and adds the gallery.

## Backward compatibility

- Palette unset + mode unchanged = today's Blue Harbor look, so an existing session is visually identical until the user opts into another palette.
- Existing `users.theme` values map straight onto `users.theme_mode`; palette defaults in.
- All existing token utilities (`bg-surface`, etc.) keep resolving; no template needs to change to keep working.

## Acceptance criteria

1. All 6 palettes render correctly in light, dark, hc-light, hc-dark (24 combinations) on the gallery, the room view, and settings.
2. No flash-of-wrong-theme on hard reload for any palette/mode (cookie path).
3. The picker applies palette and mode instantly with no reload, persists server-side, survives logout/login and a second device, and the sidebar quick-toggle still flips mode.
4. An existing user with `users.theme='dark'` sees blue-harbor dark (their current look) with no action.
5. Every palette's hc-light and hc-dark meet the same AAA contrast bar as today's high-contrast themes (verified on text/surface, muted/surface, and accent/accent-content pairs).
6. Tailwind builds; `dark:` utilities fire on `dark` and `hc-dark`; a grep shows no new hardcoded hex in templates; all 24 blocks define the full token contract.
7. Existing automated tests pass and the app boots in the dev container.

## Files touched (map)

- `server/assets/main.css`: 24 palette token blocks (replace the 4 flat theme blocks); rail/sidebar per palette; HC AAA blocks.
- `server/tailwind.config.js`: `darkMode` selector.
- `server/templates/base.html`: two-axis no-flash bootstrap; JS globals.
- `server/migrations/auth/00NN_theme_palette.sql`: rename `theme` -> `theme_mode`, add `theme_palette`.
- `server/src/models/user.rs`, `server/src/db/auth.rs`, `server/src/routes/settings.rs`, `server/src/routes/mod.rs`, `server/src/auth.rs`: mode/palette accessors, setters, routes, cookie sync.
- `server/templates/settings/page.html`, `server/assets/settings.js`: appearance picker.
- `server/assets/tailwind.css`: component retune + `.lc-table`.
- `server/templates/dev/theme_gallery.html` (+ route): the gallery.

## Risks and open items

- Authoring 12 HC blocks (hc-light + hc-dark x 6) to AAA is the largest single effort; mitigate by starting from today's proven hc-light/hc-dark and adjusting hue per palette while holding luminance contrast.
- The column rename touches ~4 Rust files; verify the cookie-sync middleware and the quick-toggle path in the same pass.
- `theme-color` META per palette is cosmetic; acceptable to key only by mode initially if palette-specific values are not ready.

## Later phases

P2-P6 each get their own spec -> plan -> build -> review cycle under LC-541, consuming this foundation. A YouTrack issue for P1 will be filed against LC-541 once the implementation plan is complete.
