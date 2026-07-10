# Let's Chat Mockup Pixel-Match Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the running Let's Chat app match the six design comps pixel-for-pixel in all four states (`blue-harbor` + new `amethyst` palette, light + dark), add the missing Details panel, and propagate the styling app-wide, preserving WCAG contrast and en+es i18n parity.

**Architecture:** Reference-first. The static reference (`docs/superpowers/reference/2026-07-10-mockup-reference.html`, Phase 0, already in this branch) is the locked target and uses the same semantic token names as `server/assets/main.css`, so palette values port 1:1. Phase 1 adds the `amethyst` palette through every layer that enumerates palettes; Phase 2 builds the Details panel; Phase 3 reconciles chat-surface layout region-by-region against the reference via screenshot diff; Phase 4 sweeps the rest of the app.

**Tech Stack:** Rust (Axum + Askama templates), hand-authored `main.css` (static, not Tailwind-compiled), Tailwind component layer (`tailwind.css`), Node contrast script (`contrast-check.mjs`), docker dev wrappers (`./dev/cargo`, `./dev/bun`, `just dev-web-local` on :18080).

**Verification model:** Palette work is gated by `contrast-check.mjs` (a real pass/fail test) and screenshot diff. UI strings are gated by `tests/i18n_catalog.rs`. Layout work is gated by screenshot diff against `mockups/` + the static reference (no unit test exists for pixel layout). `just check` + `just test` gate every phase.

---

## Reference material (read before starting)

- Target comps: `docs/superpowers/reference/mockups/*.png` (mapping in that folder's `README.md`).
- Static reference (source of truth for token values + layout): `docs/superpowers/reference/2026-07-10-mockup-reference.html`.
- Design spec: `docs/superpowers/specs/2026-07-10-lets-chat-mockup-pixel-match-design.md`.
- Existing palette structure to mirror: the `cobalt` blocks in `server/assets/main.css:544-587`.

---

## Phase 1: `amethyst` palette, end to end

### Task 1: Add the amethyst CSS blocks

**Files:**
- Modify: `server/assets/main.css` (append after the last palette block; the palettes run from ~`:544` through royal-navy - insert amethyst immediately after royal-navy's `hc-dark` block, keeping the same ordering convention)

- [ ] **Step 1: Add the four amethyst blocks**

Mirror the cobalt structure (light / dark / hc-light / hc-dark). Only palette-VARYING tokens; the palette-CONSTANT status + actor-badge tokens come from the `[data-mode="..."]` blocks. Paste exactly:

```css
/* Amethyst - the violet palette (LC mockup pixel-match). Same token contract as
   cobalt; hc-light/hc-dark inherit blue-harbor's contrast-first neutral surfaces
   and only carry violet identity in the accent family. */
[data-theme="amethyst"] {
  --surface:#faf8ff; --surface-elevated:#ffffff; --surface-sunken:#f3eeff;
  --content:#160f24; --content-muted:#5b5270; --content-subtle:#8a80a3;
  --border:#e6ddf7; --border-strong:#cebfec;
  --accent:#7c3aed; --accent-hover:#6d28d9; --accent-content:#ffffff;
  --accent-surface:#ede4ff; --accent-surface-content:#6d28d9; --ring:#8b5cf6;
  --sidebar-surface:#1a1030; --sidebar-elevated:#241640; --sidebar-sunken:#120a22;
  --sidebar-content:#f3eeff; --sidebar-muted:#b6a9d1; --sidebar-border:#33245a;
  --rail-surface:#120a22; --rail-tile:#241640; --rail-tile-hover:#6d28d9;
  --rail-content:#ffffff; --rail-content-muted:#b6a9d1;
}
[data-theme="amethyst"][data-mode="dark"] {
  --surface:#1a1030; --surface-elevated:#241640; --surface-sunken:#120a22;
  --content:#f3eeff; --content-muted:#b6a9d1; --content-subtle:#8776a3;
  --border:#33245a; --border-strong:#4a3680;
  --accent:#a78bfa; --accent-hover:#c4b5fd; --accent-content:#120a22;
  --accent-surface:#3a2a6b; --accent-surface-content:#ede4ff; --ring:#c4b5fd;
  --sidebar-surface:#120a22; --sidebar-elevated:#241640; --sidebar-sunken:#0d0719;
  --sidebar-content:#f3eeff; --sidebar-muted:#b6a9d1; --sidebar-border:#33245a;
  --rail-surface:#0d0719; --rail-tile:#241640; --rail-tile-hover:#c4b5fd;
  --rail-content:#ffffff; --rail-content-muted:#b6a9d1;
}
[data-theme="amethyst"][data-mode="hc-light"] {
  --surface:#ffffff; --surface-elevated:#ffffff; --surface-sunken:#f5f5f5;
  --content:#000000; --content-muted:#1f2937; --content-subtle:#374151;
  --border:#000000; --border-strong:#000000;
  --accent:#6b21a8; --accent-hover:#581c87; --accent-content:#ffffff;
  --accent-surface:#ede4ff; --accent-surface-content:#6b21a8; --ring:#000000;
  --sidebar-surface:#000000; --sidebar-elevated:#1f2937; --sidebar-sunken:#000000;
  --sidebar-content:#ffffff; --sidebar-muted:#ffffff; --sidebar-border:#ffffff;
  --rail-surface:#000000; --rail-tile:#1f2937; --rail-tile-hover:#374151;
  --rail-content:#ffffff; --rail-content-muted:#ffffff;
}
[data-theme="amethyst"][data-mode="hc-dark"] {
  --surface:#000000; --surface-elevated:#0a0a0a; --surface-sunken:#000000;
  --content:#ffffff; --content-muted:#e5e7eb; --content-subtle:#d1d5db;
  --border:#ffffff; --border-strong:#ffffff;
  --accent:#d8b4fe; --accent-hover:#e9d5ff; --accent-content:#000000;
  --accent-surface:#0a0a0a; --accent-surface-content:#d8b4fe; --ring:#ffffff;
  --sidebar-surface:#000000; --sidebar-elevated:#1f2937; --sidebar-sunken:#000000;
  --sidebar-content:#ffffff; --sidebar-muted:#ffffff; --sidebar-border:#ffffff;
  --rail-surface:#000000; --rail-tile:#1f2937; --rail-tile-hover:#374151;
  --rail-content:#ffffff; --rail-content-muted:#ffffff;
}
```

- [ ] **Step 2: Sanity-check the CSS parses**

Run: `./dev/bun x lightningcss server/assets/main.css -o /dev/null` (or `./dev/cargo run` boot, whichever the repo uses to serve static CSS). Expected: no parse error. If lightningcss is unavailable, grep-verify brace balance:

Run: `grep -c 'data-theme="amethyst"' server/assets/main.css`
Expected: `4`

- [ ] **Step 3: Commit**

```bash
git add server/assets/main.css
git commit -m "feat(theme): add amethyst violet palette tokens

#LC-566"
```

---

### Task 2: Cover amethyst in the contrast checker

**Files:**
- Modify: `server/scripts/contrast-check.mjs` (add `amethyst` to whatever palette list the script iterates)
- Test: the script itself is the test.

- [ ] **Step 1: Read the script to find the palette list**

Run: `sed -n '1,80p' server/scripts/contrast-check.mjs`
Expected: locate the array/object enumerating palette names (the other six) and the accent/accent-content pairs per mode.

- [ ] **Step 2: Add `amethyst` with its accent pairs**

Add an `amethyst` entry mirroring an existing palette's shape, using the accent/accent-content/accent-surface values from Task 1 for each of the four modes. (Exact key names follow the script's existing structure - match them; do not invent new keys.)

- [ ] **Step 3: Run the checker**

Run: `./dev/bun server/scripts/contrast-check.mjs` (or `node server/scripts/contrast-check.mjs` per the repo's convention)
Expected: PASS, all pairs green, including the new amethyst rows. Previously the memory noted "72/72"; expect the new total (e.g. 84/84) all passing.

- [ ] **Step 4: If any amethyst pair fails**

Nudge only the failing accent one step darker (light/hc-light) or lighter (dark/hc-dark), re-run. Precedent: LC-541 Task 11 nudged blue-700 -> blue-800 for the AAA floor. Update the same value in `main.css` (Task 1) to keep them in sync.

- [ ] **Step 5: Commit**

```bash
git add server/scripts/contrast-check.mjs server/assets/main.css
git commit -m "test(theme): cover amethyst palette in contrast checker

#LC-566"
```

---

### Task 3: Add amethyst to the backend allow-lists

**Files:**
- Modify: `server/src/models/user.rs:139`
- Modify: `server/src/routes/settings.rs:596`

- [ ] **Step 1: Read both match arms**

Run: `sed -n '135,145p' server/src/models/user.rs; sed -n '590,600p' server/src/routes/settings.rs`
Expected: two string-literal match arms listing `"blue-harbor" | "cobalt" | "ink-ice" | "arctic" | "deep-sea" | "royal-navy"`.

- [ ] **Step 2: Add `"amethyst"` to each arm**

In both locations, extend the pattern to include `| "amethyst"`. Example (user.rs):

```rust
"blue-harbor" | "cobalt" | "ink-ice" | "arctic" | "deep-sea" | "royal-navy"
    | "amethyst" => { /* unchanged body */ }
```

Apply the identical addition at `settings.rs:596`.

- [ ] **Step 3: Compile**

Run: `./dev/cargo check -p server`
Expected: compiles, no warnings about the new arm.

- [ ] **Step 4: Commit**

```bash
git add server/src/models/user.rs server/src/routes/settings.rs
git commit -m "feat(theme): allow amethyst palette in backend validation

#LC-566"
```

---

### Task 4: Add the amethyst swatch to the settings picker

**Files:**
- Modify: `server/templates/settings/page.html` (~207-244, the palette swatch list)
- Modify: `server/assets/tailwind.css` (~164-179, the `.lc-palette-*` swatch preview colors)

- [ ] **Step 1: Read the existing swatch markup + swatch colors**

Run: `sed -n '200,250p' server/templates/settings/page.html; sed -n '160,185p' server/assets/tailwind.css`
Expected: a repeated swatch element per palette (radio/button with `data-theme` or a palette key + a visible color chip), and a `.lc-palette-<name>` rule per palette giving the chip its color.

- [ ] **Step 2: Add the amethyst swatch entry**

Duplicate the last palette's swatch block in `settings/page.html`, changing the palette key/label/value to `amethyst` / "Amethyst". Add the matching locale label key if the labels are `|t`-translated (see Task 8 for the i18n pattern; add `settings.theme.palette.amethyst` = "Amethyst" / es "Amatista" to both catalogs).

- [ ] **Step 3: Add the amethyst swatch color**

In `tailwind.css`, add:

```css
.lc-palette-amethyst { background: linear-gradient(135deg, #7c3aed, #a78bfa); }
```

(Match the exact selector shape the other `.lc-palette-*` rules use.)

- [ ] **Step 4: Rebuild the component CSS**

Run: `just build-css`
Expected: `tailwind-built.css` regenerates without error.

- [ ] **Step 5: Commit**

```bash
git add server/templates/settings/page.html server/assets/tailwind.css server/i18n/*
git commit -m "feat(theme): add amethyst swatch to settings picker

#LC-566"
```

---

### Task 5: List amethyst in the dev theme gallery

**Files:**
- Modify: `server/src/routes/dev.rs:29-36`

- [ ] **Step 1: Read the palette list**

Run: `sed -n '25,40p' server/src/routes/dev.rs`
Expected: a Rust array/vec of the six palette names fed to the gallery template.

- [ ] **Step 2: Add `"amethyst"`**

Append `"amethyst"` to the array, preserving ordering style.

- [ ] **Step 3: Compile**

Run: `./dev/cargo check -p server`
Expected: compiles.

- [ ] **Step 4: Commit**

```bash
git add server/src/routes/dev.rs
git commit -m "feat(theme): show amethyst in dev theme gallery

#LC-566"
```

---

### Task 6: Screenshot-verify amethyst against the comps

**Files:** none (verification gate).

- [ ] **Step 1: Boot the dev build**

Run: `just dev-web-local`
Expected: server on http://localhost:18080.

- [ ] **Step 2: Capture the room view in amethyst light + dark**

Set the palette to Amethyst in settings (or visit `/dev/theme-gallery`), toggle mode light/dark, and screenshot the room view.

- [ ] **Step 3: Diff against the comps**

Compare each screenshot to `mockups/letschat-mockup-amethyst-light.png` and `mockups/letschat-mockup-amethyst-dark.png`. Note any token mismatch (surface too dark, accent too blue, sidebar wrong navy). Correct the values in `main.css` Task 1 and re-run Task 2's contrast check + re-screenshot until they match.

- [ ] **Step 4: Commit any corrections**

```bash
git add server/assets/main.css server/scripts/contrast-check.mjs
git commit -m "fix(theme): tune amethyst tokens to match comp

#LC-566"
```

---

## Phase 2: Details panel

### Task 7: Factor the room-info view data into a shared source

**Files:**
- Read first: `server/templates/room/info.html`, and the route/handler that populates it (find with `grep -rn "info.html\|RoomInfo\|room_info" server/src`)
- Modify: the room view-model/handler so the fields Created / Members / Notifications / Pinned are available to BOTH the full info page and the new inline panel

- [ ] **Step 1: Locate the info data path**

Run: `grep -rn "info.html" server/src; grep -rn "fn.*room.*info\|RoomInfo" server/src`
Expected: the handler and struct that provide created-at, member count, pinned flag, notification preference.

- [ ] **Step 2: Confirm the room page handler has (or can cheaply get) these fields**

Run: `grep -rn "room/page.html\|fn room_page\|RoomPage" server/src`
Expected: identify the struct rendered by `room/page.html`. If it already carries created-at/member-count/pinned, no change. If not, add those fields to the room-page view struct, populated from the same query/service the info handler uses (do NOT duplicate the query - call the same function).

- [ ] **Step 3: Compile**

Run: `./dev/cargo check -p server`
Expected: compiles.

- [ ] **Step 4: Commit**

```bash
git add server/src
git commit -m "refactor(room): expose room detail fields to the room page view

#LC-553"
```

---

### Task 8: Build the Details panel partial with i18n keys

**Files:**
- Create: `server/templates/room/details_panel.html`
- Modify: locale catalogs (find with `grep -rn "room.info\|room\." server/i18n` or wherever en/es live - the spec references `tests/i18n_catalog.rs`)

- [ ] **Step 1: Add locale keys to en AND es**

Add to both catalogs (keys illustrative - match the repo's existing key namespacing, e.g. `room.details.*`):

```
room.details.title      = "Details"      / es "Detalles"
room.details.created    = "Created"      / es "Creado"
room.details.members    = "Members"      / es "Miembros"
room.details.notifications = "Notifications" / es "Notificaciones"
room.details.pinned     = "Pinned"       / es "Fijado"
room.details.pinned_yes = "Yes"          / es "Sí"
room.details.pinned_no  = "No"           / es "No"
room.details.leave      = "Leave room"   / es "Salir de la sala"
room.details.notif_all      = "All messages" / es "Todos los mensajes"
room.details.notif_mentions = "Mentions"     / es "Menciones"
room.details.notif_none     = "Nothing"      / es "Nada"
```

- [ ] **Step 2: Write the partial**

Create `server/templates/room/details_panel.html`, mirroring the reference's `.panel.details` markup and using the room detail fields from Task 7 + the static locale keys (Askama needs static `|t` keys - do not build keys dynamically). Structure: panel header (title), four rows (created/members/notifications-dropdown/pinned), leave-room danger action. Use the semantic token classes already in the codebase (`bg-surface-elevated`, `text-content`, `border-border`, `text-danger`, etc.) so it themes automatically across all seven palettes.

- [ ] **Step 3: Verify no dynamic locale keys**

Run: `grep -n '|t' server/templates/room/details_panel.html`
Expected: every `|t` uses a static string literal key, none built from a variable.

- [ ] **Step 4: Commit**

```bash
git add server/templates/room/details_panel.html server/i18n/*
git commit -m "feat(room): add Details panel partial + locale keys

#LC-553"
```

---

### Task 9: Render the Details panel in the right column

**Files:**
- Modify: `server/templates/room/page.html:47-48` (the right-column slot region, currently the empty `#thread-panel` / `#history-panel` asides)

- [ ] **Step 1: Read the right-column slot**

Run: `sed -n '40,53p' server/templates/room/page.html`
Expected: the two `<aside>` slots. Determine where the Details panel should mount so it sits BELOW the thread panel (per the comp), visible whether or not a thread is open.

- [ ] **Step 2: Include the partial**

Add `{% include "room/details_panel.html" %}` in the right column, positioned after the thread-panel slot, wrapped so it shares the right column's width (the reference uses a 360px right column containing both panels stacked and scrolling together).

- [ ] **Step 3: Compile + render check**

Run: `./dev/cargo check -p server`
Expected: compiles (Askama template resolves the include and all `|t` keys).

- [ ] **Step 4: Commit**

```bash
git add server/templates/room/page.html
git commit -m "feat(room): mount Details panel below the thread panel

#LC-553"
```

---

### Task 10: Verify i18n parity + Details panel visuals

**Files:** none (verification gate).

- [ ] **Step 1: Run the i18n catalog test**

Run: `./dev/cargo test -p server i18n_catalog`
Expected: PASS (en+es have every new `room.details.*` key).

- [ ] **Step 2: Boot + screenshot**

Run: `just dev-web-local`, open a room, screenshot the right column in all four target states (blue/amethyst x light/dark).

- [ ] **Step 3: Diff against comps**

Compare the Details panel to each mockup's right column. Verify row labels, the Notifications dropdown chevron, the "Leave room" danger color, spacing, and the panel border/radius. Correct `details_panel.html` and re-screenshot until matched.

- [ ] **Step 4: Commit**

```bash
git add server/templates/room/details_panel.html
git commit -m "fix(room): align Details panel to comp

#LC-553"
```

---

## Phase 3: Chat-surface pixel pass (diff-driven)

Each task below is a visual reconciliation: read the current template, open the matching region in the static reference + comp, and adjust spacing/radii/token usage until the screenshot matches. There is no unit test for pixel layout; the gate is the screenshot diff. Work one region at a time and commit per region so regressions are bisectable. Prefer changing token-backed utility classes over hardcoding hex - if a color must change, change the token, not the element.

### Task 11: Rail region

**Files:** Modify `server/templates/partials/enclave_switcher.html` (+ `.lc-rail-*` rules in `main.css` if spacing/size is off).

- [ ] **Step 1:** Screenshot current rail vs `mockups/*` rail (icon tile size 40px, radius 12px, active-tile state, unread badge on the messages tile, green presence dot on the avatar, bottom help + avatar). Reference region: `.rail*` in the static reference.
- [ ] **Step 2:** Adjust tile sizing/radius/gap and active/hover states to match. Keep colors on `--rail-*` tokens.
- [ ] **Step 3:** Re-screenshot in all four states; confirm match.
- [ ] **Step 4:** Commit: `style(rail): match comp spacing and states` + `#LC-553`.

### Task 12: Sidebar region

**Files:** Modify `server/templates/partials/sidebar.html`, `partials/sidebar_nav.html`, `partials/sidebar_room_row.html`, `partials/sidebar_peer_row.html`.

- [ ] **Step 1:** Screenshot current sidebar vs comp: section headers (uppercase, muted, letter-spacing), room row padding/radius, active-row tint (`--accent-surface`/`--accent-surface-content`), unread count pills (right-aligned, pill shape; active row uses solid `--accent`), `#` hash glyph color, DM avatar size, "Invite people" footer with top border. Reference region: `.sidebar*` / `.sb-*`.
- [ ] **Step 2:** Reconcile to match; keep colors on `--sidebar-*` / `--accent*` tokens.
- [ ] **Step 3:** Re-screenshot all four states.
- [ ] **Step 4:** Commit: `style(sidebar): match comp rows, sections, unread pills` + `#LC-553`.

### Task 13: Timeline region

**Files:** Modify `server/templates/room/messages.html`, `room/message.html` (+ message/reaction/hover-action rules in `main.css`).

- [ ] **Step 1:** Screenshot current timeline vs comp: message row gap/padding, avatar 36px, name+time baseline, bulleted-list rendering, reaction pills (rounded, bordered; "reacted" state tinted `--accent-surface`; trailing add-reaction chip), the pinned/selected message accent bar (`box-shadow: inset 3px 0 0 var(--accent)` + `--surface-sunken`), the hover action bar (reply/emoji/bookmark/more, top-right, appears on hover), the "New messages" divider (accent text + accent hairline), the "Today" day divider, and the unread banner ("2 unread messages" / "Mark as read" on `--accent-surface`). Reference region: `.timeline`, `.msg`, `.react`, `.hoveract`, `.newdiv`, `.banner`.
- [ ] **Step 2:** Reconcile each element.
- [ ] **Step 3:** Re-screenshot all four states.
- [ ] **Step 4:** Commit: `style(timeline): match comp message rows, reactions, dividers` + `#LC-553`.

### Task 14: Composer region

**Files:** Modify `server/templates/room/composer.html`.

- [ ] **Step 1:** Screenshot current composer vs comp: outer border/radius (12px), placeholder "Message #product", formatting toolbar (B / I / S / code / link / ul / ol / attach) with a separator, right cluster (emoji / @ / send), send button as a filled `--accent` square. Reference region: `.composer`.
- [ ] **Step 2:** Reconcile spacing, icon set, and the send button.
- [ ] **Step 3:** Re-screenshot all four states.
- [ ] **Step 4:** Commit: `style(composer): match comp toolbar and send button` + `#LC-553`.

### Task 15: Thread panel region

**Files:** Modify `server/templates/room/thread_panel.html`.

- [ ] **Step 1:** Screenshot current thread panel vs comp: header ("Thread", "3 of 8", prev/next arrows in a bordered group, close X), "In reply to <name>" (name in accent), the source message, "3 replies" label, reply rows (30px avatar, name+time, text), and the "Reply in thread..." box with emoji + send. Reference region: `.panel` (thread), `.tmsg`, `.reply-box`.
- [ ] **Step 2:** Reconcile.
- [ ] **Step 3:** Re-screenshot all four states.
- [ ] **Step 4:** Commit: `style(thread): match comp thread panel` + `#LC-553`.

### Task 16: Header region

**Files:** Modify `server/templates/partials/room_header.html`.

- [ ] **Step 1:** Screenshot current header vs comp: `#` + room name + star, description subline, right cluster (overlapping member avatar stack with "+23", search, add-member, overflow). Reference region: `.hdr`.
- [ ] **Step 2:** Reconcile the avatar stack overlap and icon spacing.
- [ ] **Step 3:** Re-screenshot all four states.
- [ ] **Step 4:** Commit: `style(header): match comp title bar and avatar stack` + `#LC-553`.

---

## Phase 4: App-wide sweep

### Task 17: Inventory hardcoded colors and off-token spacing

**Files:** none (produces a work-list).

- [ ] **Step 1:** Find raw hex/util colors that bypass tokens outside the chat surface.

Run: `grep -rnE '#[0-9a-fA-F]{3,6}|(bg|text|border)-(slate|gray|blue|zinc|neutral)-[0-9]{2,3}' server/templates --include=*.html | grep -viE 'room/|partials/(sidebar|enclave|room_header)' `
Expected: a list of hardcoded colors in settings/admin/onboarding/modals. Save it as the Phase 4 work-list.

- [ ] **Step 2:** Commit the work-list as a note in the plan folder (optional) or track inline. No code change.

### Task 18: Convert non-chat surfaces to tokens

**Files:** Modify each template surfaced by Task 17 (settings, admin, onboarding, modals).

- [ ] **Step 1:** For each file, replace hardcoded colors with the semantic token utilities (`bg-surface`, `bg-surface-elevated`, `text-content`, `text-content-muted`, `border-border`, `bg-accent`, `text-danger`, etc.) so the surface recolors across all seven palettes.
- [ ] **Step 2:** After each file, `just build-css` if any new utility class was introduced; `./dev/cargo check -p server`.
- [ ] **Step 3:** Commit per surface: `style(<surface>): move off hardcoded colors onto tokens` + `#LC-553`.

### Task 19: Full-matrix verification

**Files:** none (verification gate).

- [ ] **Step 1:** Boot `just dev-web-local`. Walk settings, admin, onboarding, and one of each modal under all seven palettes x four modes (use `/dev/theme-gallery` for shared components; visit real pages for the rest).
- [ ] **Step 2:** Confirm no hardcoded-color breakage (a surface that stays light in dark mode, unreadable text, an off-brand accent).
- [ ] **Step 3:** Run the full gate:

Run: `just check && just test`
Expected: PASS (fmt, clippy, unit tests incl. `i18n_catalog` and `contrast-check` if wired into `just test`; run `contrast-check.mjs` separately if not).

- [ ] **Step 4:** Commit any final fixes: `fix(theme): resolve app-wide palette breakage` + `#LC-553`.

---

## Self-review notes

- **Spec coverage:** AC1-3 -> Tasks 1-6 (amethyst palette, contrast, picker, gallery). AC4 (Details panel) -> Tasks 7-10. AC5 (i18n parity) -> Tasks 8, 10. AC6 (chat pixel-match) -> Tasks 11-16. AC7 (app-wide) -> Tasks 17-19. AC8 (`just check`/`just test`) -> Task 19. AC9 (YT tickets) -> commit trailers reference LC-566 (palette) / LC-553 (polish epic); file the specific sub-tickets at execution start.
- **No unit tests for pixel layout** is intentional and called out; Phases 3-4 gate on screenshot diff + the existing `contrast-check` / `i18n_catalog` tests, which is the honest verification surface for CSS in this repo.
- **Line numbers** (`user.rs:139`, `settings.rs:596`, `dev.rs:29-36`, `page.html:207-244`, `tailwind.css:164-179`) are from the 2026-07-10 read of `main`; each task re-greps to confirm before editing, so they self-correct if the code has moved.
- **YouTrack:** file the palette sub-ticket under LC-566 and the Details-panel + pixel-pass + sweep sub-tickets under the LC-553 epic before starting execution, per the tracked-issue rule. Commit trailers use bare `#LC-...` references.
