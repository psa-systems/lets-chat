# Let's Chat - mockup pixel-match overhaul (design)

Date: 2026-07-10
Status: design, pending user review
Branch: `docs/lets-chat-mockup-pixel-match`

## Background

The structural redesign shipped under epic LC-364 (Slack/Discord/Linear shell:
rail, sidebar, room list, headers, message rows, composer, thread panel). The
six-palette theme system shipped under LC-566 (`data-theme` palette x `data-mode`
light/dark/hc-light/hc-dark; tokens in `server/assets/main.css`). What is on
`main` today is that shell plus the P1 token foundation.

The user supplied six design comps (`docs/superpowers/reference/mockups/`) as the
target and asked to "completely match" them. Analysis of the comps against the
running app surfaces the real gaps:

1. **A purple/violet palette that does not exist.** The comps show two accent
   families - blue and purple. The shipped palettes (`blue-harbor`, `cobalt`,
   `ink-ice`, `arctic`, `deep-sea`, `royal-navy`; allow-list at
   `server/src/models/user.rs:139` and `server/src/routes/settings.rs:596`) are
   all blue/navy/cyan. The purple comps match no shipped palette.
2. **A Details panel that is not built.** Every comp shows a right-column Details
   block below the thread panel: Created / Members / Notifications (dropdown) /
   Pinned / Leave room. The app has a thread panel (`room/thread_panel.html`) and
   a separate full-page room info route (`room/info.html`), but no inline Details
   panel.
3. **Layout/spacing/typography differences** between the shipped shell and the
   comps across rail, sidebar, timeline (reaction pills, hover action bar, "New
   messages" divider, unread banner), composer, thread panel, and header.
4. **App-wide consistency.** The user asked that existing components (settings,
   admin, onboarding, modals) also adopt the refined style.

## Goal

Make the running app match the six comps pixel-for-pixel, in all four states
(`blue-harbor` and the new `amethyst` palette, each in light and dark), and
propagate the refined styling app-wide. WCAG AA/AAA contrast and en+es i18n
parity are preserved throughout.

## Decisions (locked with user 2026-07-10)

- **Purple is an additive 7th palette**, not a replacement, not the new default.
  `blue-harbor` stays the default.
- **Palette name: `amethyst`** (fits the gem/mineral side of the existing
  taxonomy alongside `cobalt`). Alternative `wine-dark` was offered; `amethyst`
  chosen for clarity. User may override at spec review.
- **Reach: whole app.** Chat surface first, then settings/admin/onboarding/modals.
- **Details panel coexists** with the existing full-page `room/info.html` route
  rather than replacing it. The inline panel is additive; the full page stays as
  a fallback/deep-link surface. (User may override at spec review.)
- **Verification: user screenshots, agent diffs.** The user runs
  `just dev-web-local` and pastes screenshots; the agent compares against the
  comps and the static reference, and corrects.

## Proposed approach: reference-first

Lock the visual target as code before touching the app, then port.

### Phase 0 - Static reference (DONE in this branch)

`docs/superpowers/reference/2026-07-10-mockup-reference.html` reproduces the room
view in all four states, self-contained, using the **same semantic token names**
as `main.css` so values port 1:1. It doubles as the spec's source of truth and,
mirrored into `/dev/theme-gallery`, a regression fixture. The six comps are
archived beside it in `mockups/` with a mapping README.

### Phase 1 - `amethyst` palette, end to end

Add the palette across every layer that enumerates palettes:

- **CSS** (`server/assets/main.css`): four blocks - `[data-theme="amethyst"]`
  (light), `[data-theme="amethyst"][data-mode="dark"]`,
  `[data-theme="amethyst"][data-mode="hc-light"]`,
  `[data-theme="amethyst"][data-mode="hc-dark"]` - following the cobalt block
  structure exactly. Only the palette-VARYING tokens are defined (surface,
  content, border, accent, accent-surface, ring, sidebar-*, rail-*); the
  palette-CONSTANT status/actor-badge tokens are inherited from the
  `[data-mode="..."]` blocks. hc-light/hc-dark inherit blue-harbor's
  contrast-first neutral surfaces; only the accent family carries violet
  identity. Draft values are in the static reference; final accent values are
  pinned by the contrast script.
- **Backend allow-list**: add `"amethyst"` to the match arms at
  `server/src/models/user.rs:139` and `server/src/routes/settings.rs:596`.
- **Picker**: add the swatch to `server/templates/settings/page.html` (~207-244)
  and its preview color to `server/assets/tailwind.css` (~164-179).
- **Dev gallery**: add to the palette list at `server/src/routes/dev.rs:29-36`.
- **No DB migration**: `users.theme_palette` is free `TEXT` validated in Rust.
- **Contrast**: extend `server/scripts/contrast-check.mjs` to cover amethyst; all
  accent/accent-content pairs pass AA (AAA for hc). Nudge accents if any pair
  fails, same posture as LC-541 Task 11.

### Phase 2 - Details panel

New right-column component below the thread panel, matching the comp:

- Rows: Created (formatted timestamp), Members (count), Notifications (dropdown:
  All messages / Mentions / Nothing), Pinned (Yes/No), then Leave room (danger).
- New partial (e.g. `room/details_panel.html`) rendered in the right column slot;
  view data sourced the same way `room/info.html` sources it, factored so both
  the inline panel and the full page share one data path.
- Notifications dropdown wires to the existing per-room notification preference if
  one exists; otherwise it is display-only in this phase and the write path is a
  follow-on ticket (stated assumption - confirm during implementation).
- en+es locale keys for every label (or `tests/i18n_catalog.rs` fails).

### Phase 3 - Chat-surface pixel pass

Reconcile spacing/radii/typography to the comp across rail (icon tiles, active
state, unread badge), sidebar (section headers, unread count pills, active row
tint, hash glyphs, DM avatars), timeline (message row padding, reaction pills,
hover action bar reply/emoji/bookmark/more, pinned-message accent bar, "New
messages" divider, unread banner), composer (formatting toolbar + emoji/@/send),
thread panel (reply count, reply rows, reply box), and header (title + star,
avatar stack +23, search/add-member/overflow). Driven off the reference; verified
by screenshot diff per region.

### Phase 4 - App-wide sweep

Settings, admin, onboarding, and modals adopt the retuned radii/surface/spacing
tokens. Most falls out of the token layer; targeted fixes where colors/spacing
are hardcoded (the completion roadmap already flags a hardcoded-color sweep).
Each surface verified against the palette matrix.

## Alternatives considered

- **Direct-edit-and-iterate** (skip the static reference): rejected - each
  iteration needs a docker build + screenshot, the feedback loop is slow, and
  there is no locked target so "done" is fuzzy.
- **Token-only match** (colors only, defer layout + Details panel): rejected -
  the user explicitly wants the Details panel, pixel layout, and app-wide reach.
- **Purple replaces an existing palette / becomes default**: rejected by user;
  additive palette with blue-harbor default chosen.

## Toolchain notes

- Host has no cargo; `just` recipes run via `./dev/cargo` / `./dev/bun` docker
  wrappers. `just dev-web-local` serves the debug build on :18080.
- `main.css` is a static asset (not Tailwind-compiled) - validate CSS by hand.
  Component classes live in `tailwind.css` (`@layer components`, compiled +
  gitignored).
- Askama has no literal-array `{% for %}` and no dynamic `|t` keys - unroll and
  use static locale keys.

## Acceptance criteria

- [ ] `amethyst` palette defined in `main.css` across all four modes, structured
      like the cobalt blocks; only palette-varying tokens redefined.
- [ ] `amethyst` added to both backend allow-lists; selectable in the settings
      picker with a swatch; listed in `/dev/theme-gallery`.
- [ ] `contrast-check.mjs` covers amethyst and passes (AA body / AAA hc) with no
      regressions to the other six palettes.
- [ ] Details panel renders in the right column below the thread panel with
      Created / Members / Notifications / Pinned / Leave room, matching the comp
      in all four states; the full-page `room/info.html` route still works.
- [ ] All new UI strings have en+es locale keys; `tests/i18n_catalog.rs` passes.
- [ ] Rail, sidebar, timeline, composer, thread panel, and header match the comps
      pixel-for-pixel in `blue-harbor` and `amethyst`, light and dark (verified by
      screenshot diff against `mockups/` + the static reference).
- [ ] Settings, admin, onboarding, and modals render consistently under all seven
      palettes x four modes with no hardcoded-color breakage.
- [ ] `just check` and `just test` pass; no new clippy/fmt failures.
- [ ] Each phase maps to a YouTrack ticket under the LC-553 polish epic (filed at
      execution time, per the tracked-issue rule).

## Open assumptions (resolved at implementation, not blocking)

- Final amethyst accent hex values may shift from the reference drafts if the
  contrast script flags a pair; the darker/lighter nudge follows the LC-541 Task
  11 precedent.
- The Notifications dropdown write path may be deferred to a follow-on ticket if
  no per-room preference store exists yet; the panel renders display-only until
  then.
