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

### Phase 3 scope revision (2026-07-10 - verified gap assessment, LITERAL pixel-match decision)

A read-only region-by-region gap assessment against the six comps + the static reference was run on 2026-07-10 (branch history: this supersedes the "fine nudging" framing above). **Two things changed the scope:**

1. **The gaps are not all "nudges."** Four regions have STRUCTURAL / archetype mismatches that require markup restructuring, not just token/spacing tweaks. Only the timeline and details panel are mostly spacing-level.
2. **Direction chosen: LITERAL pixel-match.** The running app is deliberately feature-richer than the comps (multi-enclave switcher, huddles, catch-me-up, polls, scheduling, semantic search, 7-row nav, per-message overflow menus). A literal match means these extra affordances must be HIDDEN or RELOCATED out of the chat surface so the rendered result matches the comps. Each task below calls out what must be hidden/removed. Feature-hiding must be reversible (prefer a flag / CSS `hidden` / config gate over deletion) and NEEDS ITS OWN YT SUB-TICKET under LC-553, because hiding shipped functionality is a product decision beyond a CSS pass. Flag any feature-hide to the user before deleting server routes.

**Gap severity scorecard (verified 2026-07-10):**

| Region | Verdict | Nature of gap |
|---|---|---|
| Shell widths (rail 64 / sidebar 256) | MATCH | correct |
| Amethyst palette (4 blocks, contrast 84/84) | MATCH | shipped Phase 1 |
| Connection banner (tokenized) | MATCH | shipped Phase 4 |
| Rail | STRUCTURAL | enclave switcher vs fixed single-org nav rail |
| Right column | STRUCTURAL | two columns (384+288) vs one 360px stacked-card column |
| Composer | STRUCTURAL | inverted/split 2-row vs input + single toolbar |
| Header | MAJOR | 6-9 labeled actions vs 3 icons + avatar stack + star |
| Sidebar | MAJOR | own-profile head + extra nav/tabs; active-row token blocked |
| Timeline | MIXED | geometry off + unread banner missing + wrong divider color |
| Details panel | CLOSE | exists (Phase 2) but wrong button/layout/chevron/visibility |

Task numbering below is unchanged; each Task's Step 1 now carries the VERIFIED current-vs-target deltas (exact px / token / weight) so the implementer does not re-derive them. Screenshots still gate acceptance in all four states {blue-harbor, amethyst} x {light, dark}.

### Task 11: Rail region

**Files:** Modify `server/templates/partials/enclave_switcher.html`, `partials/sidebar_self.html` (avatar relocation), `.lc-rail-*` rules in `main.css`. **STRUCTURAL - needs LC-553 sub-ticket (archetype change + feature relocation).**

**Verified gap (2026-07-10):** the current rail is a DYNAMIC multi-enclave switcher (Discord-style workspace list); the comp is a FIXED single-org nav rail. Token wiring is correct (dark values match the reference exactly); the gap is structural/geometric.
- Container MATCHES: `w-16` (64px), `py-3` (12px), `gap-2` (8px), bg `--rail-surface`. Keep.
- MISSING entirely (comp has, rail lacks): accent logo tile (40x40, radius 12px, bg `--accent`, top); ACME org tile with green presence dot; the 5 global nav icons (people / calendar / files / pins / globe, each 40x40, radius 10px, `--rail-content-muted`); bottom help "?"; user avatar-in-rail (34px round + green dot - currently lives in the sidebar footer via `sidebar_self.html`, must move here).
- Tile geometry off: current tiles `w-11 h-11` (44px) radius ~15px vs target 40px / radius 12px; hover/active add a radius-morph (0.95rem->1.375rem) + accent bg + Discord left-indicator pill (`.lc-rail-tile::before`) that the flat comp tiles do NOT have.
- Badge token wrong: unread badge uses `--danger`/`--danger-content`; comp uses `--accent`/`--accent-content` (min 16px).
- LITERAL-MATCH -> HIDE/RELOCATE (flag to user first): the dynamic enclave list, Home house-tile, per-enclave settings gear, Discover "+" tile, `.lc-rail-divider`, sidebar-collapse chevron, and the left-indicator pill. Replace with the fixed logo + org tile + 5 nav icons + messages tile + help + avatar layout.
- [ ] **Step 2:** Rebuild the rail markup to the fixed-nav layout; set tile sizing/radius/gap and active/hover (color-only active per comp). Keep colors on `--rail-*` tokens; switch the badge to `--accent`.
- [ ] **Step 3:** Re-screenshot in all four states; confirm match.
- [ ] **Step 4:** Commit: `style(rail): match comp spacing and states` + `#LC-553`.

### Task 12: Sidebar region

**Files:** Modify `server/templates/partials/sidebar.html`, `partials/sidebar_nav.html`, `partials/sidebar_room_row.html`, `partials/sidebar_peer_row.html`, `partials/unread_badge.html`; `.lc-sidebar` + `.lc-room-row-active` rules in `main.css`. **MAJOR - needs LC-553 sub-ticket (head restructure + feature-hide).**

**Verified gap (2026-07-10):**
- Container MATCHES: `w-64` (256px), `.lc-sidebar` bg/color/border. Keep.
- Head is the WRONG content: comp head = "Let's Chat" brand title (18px/700) + collapse `<<`, then an "Acme Corp" org row (15px/600 + chevron, padding 6px 16px 12px). Current head holds the own-user profile block + account-menu (`sidebar_self.html`) + `#status-picker`. Add the brand title + org row; relocate own-profile to the rail (Task 11).
- BLOCKER token bug: `.lc-sidebar` remaps `--accent-surface` -> `--sidebar-elevated` and `--accent-surface-content` -> `--sidebar-content` (main.css ~1836). This makes the comp's SOLID `--accent` active-row fill impossible - active rows render as a subtle elevated tint. Also `.lc-room-row-active` currently uses `--surface-elevated` bg + accent text + a left inset bar (weight 600). Target: bg solid `--accent-surface` (the real blue), color `--accent-surface-content`, weight 700, no left bar. Fix requires NOT remapping accent-surface inside `.lc-sidebar`.
- Count pills wrong: `unread_badge.html` is always `bg-accent` at rest, radius 4px, no min-size. Target: neutral `--sidebar-elevated` pill (min 20px, radius 10px) at rest, flipping to solid `--accent` only on the active row.
- Search off: `#sidebar-search-input` radius 4px, no sunken bg, no `⌘K` kbd; comp wants sunken bg, radius 8px, `⌘K` hint, plus a 34px accent "+" square beside it (currently only a plain-text `+` in the Rooms header).
- Minor: section header padding `px-2 py-0.5` (8/2) vs 4/8; room row `rounded-md` (6px) / `gap-2.5` (10px) vs 8px/8px; DM avatar 24px vs 22px.
- MISSING: bordered "Invite people" footer (person+ icon, padding 12px 16px) - was moved into the account menu; re-add.
- LITERAL-MATCH -> HIDE (flag to user): the Messages/People segmented search tabs; the entire 7-row "Navigation" section (Inbox/Activity/Saved/Scheduled/Kudos/Stats/Transcripts); the unread-only filter + mark-all-read buttons; per-row hover controls (mark-read/star/move-category); draft/mention/voice badges; collapsible category chevrons + rename/delete/drag. Rename sections to static Favorites / Team / Projects / Direct Messages (comp) or keep the app's category names as a lower-fidelity compromise (user's call).
- [ ] **Step 2:** Reconcile to match; keep colors on `--sidebar-*` / `--accent*` tokens; stop the `--accent-surface` remap so the active row can render solid.
- [ ] **Step 3:** Re-screenshot all four states.
- [ ] **Step 4:** Commit: `style(sidebar): match comp rows, sections, unread pills` + `#LC-553`.

### Task 13: Timeline region

**Files:** Modify `server/templates/room/messages.html`, `room/message.html`, `partials/reaction_bar.html` (+ `.lc-day-*`, `.lc-msg-*`, `.lc-react-*` rules in `main.css`). **MIXED - mostly nudges + 2 missing elements + 1 wrong-color; LC-553 sub-ticket.**

**Verified gap (2026-07-10):**
- MISSING: the unread BANNER ("x 2 unread messages" + "Mark as read >" on `--accent-surface`, margin 16px 20px 0, padding 12px 16px, radius 10px) has NO counterpart - the app has a floating jump-pill + a mid-stream inline divider instead. Build it new at top of timeline.
- WRONG COLOR FAMILY: the "New messages" divider renders as a DANGER-red pill (`.lc-day-chip-danger`/`.lc-day-rule-danger`, label "Unread messages") - comp wants `--accent` plain text 12px/600 + accent hairlines at opacity .5, label "New messages".
- Day divider: current is a bordered UPPERCASE pill (`.lc-day-chip`, 11px); comp wants bare 12px `--content-subtle` text with plain hairlines (no chip).
- Row geometry: `px-4 py-2` (16/8) no radius; comp wants padding 8px 12px, radius 10px, flex gap 12px. Avatar `h-6 w-6` (24px) vs 36px. Name `font-medium` (~500) vs 700. Time uses `--content-muted` vs `--content-subtle`.
- MISSING: pinned/selected message state (comp: bg `--surface-sunken` + `box-shadow: inset 3px 0 0 --accent`). Only `.lc-mentioned` has an accent wash today; `is_pinned` just flips a menu label.
- Reaction pills: resting bg `--surface-sunken` vs comp `--surface-elevated`; container gap 4px vs 6px, `mt-1` vs `mt-2`, radius-full vs 14px. "reacted/on" state MATCHES. Add-reaction chip present (minor bg diff).
- Bulleted list padding 24px vs 18px; inline mention color OK but not weight-600.
- LITERAL-MATCH -> HIDE (flag to user): the comp's clean rows have none of the app's per-message chrome - system-event rows, "edited"/history button, seen caption, reply-count chip, ack bar, quote/reply chip, quick-react bar, thread+flag icons in the hover bar, and the large overflow "..." menu. Reduce the hover action bar to reply/emoji/bookmark/more (28x26, radius 8px) per comp; move the rest behind "more" or hide.
- [ ] **Step 2:** Reconcile each element; build the unread banner; re-tone the New-messages divider to accent.
- [ ] **Step 3:** Re-screenshot all four states.
- [ ] **Step 4:** Commit: `style(timeline): match comp message rows, reactions, dividers` + `#LC-553`.

### Task 14: Composer region

**Files:** Modify `server/templates/room/composer.html` (composer chrome ~L115-427), `.lc-fmt-btn`/`.lc-composer-*`/`.lc-mdinput` rules in `main.css`. **STRUCTURAL - inverted/split layout; needs LC-553 sub-ticket + feature-hide.**

**Verified gap (2026-07-10):**
- Structure inverted+split: comp = borderless input on top, ONE toolbar row below. App = a formatting row ABOVE a separately-bordered/rounded textarea that is flanked inline by a SECOND action row. Rebuild to input-flush-in-wrapper + single toolbar.
- Wrapper MATCHES radius (`rounded-xl` 12px) + bg (`bg-surface-elevated`); margins wrong: `mx-2 mb-2` (8/8/0) vs 12px 20px 20px; drop the extra `shadow-sm`.
- Input padding 8/12 vs 14/16/6; placeholder token `--content-muted` vs `--content-subtle` (textarea is its own bordered box today - make it flush).
- Toolbar composition: MISSING the vertical separator, the ordered-list button, and the `@`-mention button; strike/code/link order differs; attach + emoji live in the wrong rows. Add the `ml-auto` right cluster (emoji / @ / send).
- Sizes: toolbar buttons `.lc-fmt-btn` 28px vs 30px; send `.lc-composer-send` 36px vs 34px (tokens/glyph already correct: accent bg, accent-content, paper-plane, radius 8px). Toolbar gap 4px vs 2px.
- LITERAL-MATCH -> HIDE (flag to user): blockquote btn, Write/Preview toggle, live char-counter, record-voice, record-clip/video, poll, AI writing-assist, RSVP event, GIF picker, schedule-send, and the TTL `<select>`. Comp toolbar is only B I S | code link ul ol attach ... emoji @ send.
- [ ] **Step 2:** Rebuild to input-on-top + single toolbar; reconcile spacing, icon set, separator, right cluster, and the send button.
- [ ] **Step 3:** Re-screenshot all four states.
- [ ] **Step 4:** Commit: `style(composer): match comp toolbar and send button` + `#LC-553`.

### Task 15: Right column (Thread + Details) region

**Files:** Modify `server/templates/room/page.html` (right-column shell), `room/thread_panel.html`, `room/details_panel.html`, `room/thread_reply_inner.html`; card/panel rules in `main.css`. **STRUCTURAL - right-column shell rewrite; covers BOTH Thread and Details (Phase 2 details panel does NOT fully match). Needs LC-553 sub-ticket.**

**Verified gap (2026-07-10):**
- SHELL WRONG: comp = ONE 360px right column (border-left, bg `--surface`, scrolls) holding two STACKED cards (`.panel`: margin 14px, border 1px `--border`, radius 12px, bg `--surface-elevated`). App = separate sibling flex columns: `#thread-panel` `w-96` (384px, hidden until htmx-swapped) + `#details-panel` `w-72` (288px, always visible) -> side-by-side 672px when both open, no card framing. Rebuild as one 360px column with two margin/radius/elevated cards; make the thread card persistent-slot per comp.
- Thread header: MISSING the "3 of 8" position counter and the bordered prev/next arrow group (radius 7px); title `font-semibold` (600/~14px) vs 700/15px. Extra follow/mute/Summarize buttons present (hide for literal match). Close X OK.
- "Replies to <name>": name NOT accent-colored (comp: name in `--accent`); string differs ("Replies to" vs "In reply to").
- Thread message rows: avatar 24px vs 30px; name weight 500 vs 700; time 12px `--content-muted` vs 11px `--content-subtle`; text 14px vs 13px. "N replies" label is 11px UPPERCASE with rule dividers vs comp plain 12px muted.
- Reply box: `rounded border` (~4px, no sunken bg) vs comp radius 9px + bg `--surface-sunken`; emoji picker MISSING (send only); extra composer-cue + error chrome (hide).
- DETAILS panel (Phase 2, verify - does NOT fully match): "Leave room" is a filled `btn-danger` (solid red bg) + no icon + hidden on PUBLIC rooms; comp wants a red-TEXT link (`--danger`, 600) with icon, shown always. Notifications row MISSING its dropdown chevron. Rows use `justify-between` + `px-3 py-2` vs comp fixed 110px key column + padding 10px 16px, value not weight-500, 14px vs 13px. Header "Details" 600/~14px vs 700/15px.
- [ ] **Step 2:** Rebuild the 360px stacked-card shell; reconcile Thread + Details to the deltas above.
- [ ] **Step 3:** Re-screenshot all four states.
- [ ] **Step 4:** Commit: `style(thread): match comp thread + details right column` + `#LC-553`.

### Task 16: Header region

**Files:** Modify `server/templates/partials/room_header.html`; `.lc-header*`/`.lc-h1` rules in `main.css`. **MAJOR - collapse action set + add 3 missing elements. Needs LC-553 sub-ticket + feature-hide.**

**Verified gap (2026-07-10):**
- Container: `.lc-header` uses `justify-between` gap 8px padding 8/16 min-h 48px; comp wants gap 12px padding 14px 20px with the right cluster on `margin-left:auto` gap 14px. Room name `.lc-h1` weight 600 vs 700; "#" prefix OK (subtle). Description subline size/color OK.
- MISSING three elements: the member avatar STACK (28px round, overlap -8px, 2px `--surface` border), the "+23" overflow count, and the star/favorite toggle beside the room name (member count exists only in the details panel today).
- Right cluster is the wrong model: comp = 3 compact ICON buttons (search / add-member person+ / overflow kebab) at 16px. App renders 6-9 LABELED buttons + an expanded inline search FIELD + a semantic-search toggle; action icons are 20px not 16px, gap 4px not 14px.
- LITERAL-MATCH -> HIDE/COLLAPSE (flag to user): fold huddle, jump-to-date, catch-me-up, info, highlights, manage, and notify-dropdown into the kebab overflow; replace the inline search field with a search ICON that expands on click; add the add-member person+ icon.
- [ ] **Step 2:** Add the avatar stack, "+23", and star; collapse actions to the 3-icon comp model; reconcile spacing/weights.
- [ ] **Step 3:** Re-screenshot all four states.
- [ ] **Step 4:** Commit: `style(header): match comp title bar, avatar stack, collapsed actions` + `#LC-553`.

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
