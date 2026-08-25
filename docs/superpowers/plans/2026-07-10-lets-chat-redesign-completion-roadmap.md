# Let's Chat Redesign - Completion Roadmap (preliminary)

> **Archived plan (LC-760):** `docs/superpowers/plans/` is an archive. This document records the state and reasoning as of its date (2026-07-10) for LC-553; it is not a live tracker. Issue status is read from YouTrack, not from this file. The live UI conventions are in `docs/ui-conventions.md`.

Date: 2026-07-10
Status: Preliminary roadmap. Each genuinely-remaining item gets its own spec/plan at kickoff.
Tracking: LC-553 (the active LC-UI-01..12 follow-up epic). See ticket-hygiene notes for LC-541 / LC-364 / LC-566.

## Why this document exists

The "Let's Chat UI redesign" is not a greenfield program. The structural redesign already shipped under epic **LC-364** (professional Slack/Discord/Linear UI): rail, sidebar, room list, headers, message rows, composer, voice/video, welcome/empty states, Settings, room info, enclave settings, branding, admin - roughly 30 sub-tickets, all Done. That is the app shown in the target mockups.

Two further tracks then ran:
- **LC-553 / LC-UI-01..12** (tickets LC-554..565): a Blue Harbor token + component polish pass. Mostly done; a few tail items remain.
- **LC-566** (the six-palette theme system, merged PR #528): genuinely new - the app had one Blue Harbor theme, now it has six selectable palettes on the `data-theme`(palette) + `data-mode`(light/dark/hc-light/hc-dark) model.

An earlier "LC-541 six-phase P2-P6 pixel-match program" was drafted before LC-364 was known to be complete; it is largely redundant. This roadmap replaces it with the honest, small set of work that actually remains, and maps the old P2-P6 against reality so nothing is lost.

## 1. Reconciliation record (2026-07-10)

Verified each LC-UI tail ticket against its merged PR and the code on `origin/main` (`14103395`).

The per-ticket status columns this table once carried were removed (LC-760): live state is read from YouTrack, not restated here. What remains is the durable finding for each ticket as of this date.

| Ticket | LC-UI | Finding |
| --- | --- | --- |
| LC-554 | 01 tokens | Blue Harbor semantic tokens. |
| LC-555 | 02 components | All requested classes exist in `tailwind.css` @layer components; `.alert` adoption broad (28 boxes, PR #534). `.lc-toolbar` / `.lc-empty` are unused dead CSS. Adopt-or-close. |
| LC-556 | 03 feeds/webhooks/email | Migrated to the settings-card shell. |
| LC-557 | 04 empty/error states | Empty-state residuals tokenized (PR #530). |
| LC-558 | 05 timeline polish | Only the edit-form token fix landed. Subjective hierarchy/hover/mention-highlight polish genuinely unstarted. |
| LC-559 | 06 composer | Composer lifted onto elevated card (PR #529). |
| LC-560 | 07 sidebar/rail | Navy sidebar + rail token remap done (`f604320e`) + pre-existing LC-365/366/367. One real gap: `.lc-rail-tile` has no `:focus-visible`. |
| LC-561 | 08 admin | Status chips done (PR #531). Shared admin page-header (title/description/action/filters) never built - `admin_layout.html` is title-only. |
| LC-562 | 09 width/layout | `.lc-page-*` helpers. |
| LC-563 | 10 typography | Keep system UI font; personality via tokens. |
| LC-564 | 11 landing/login/welcome | Verified fully done: entry surfaces token-built, errors via `.alert` (PR #532). Stale state corrected 2026-07-10. |
| LC-565 | 12 a11y regression | Focus ring + phantom-token fix (PR #533), 28 alerts (#534), 44px coarse-pointer rule (#535). Remaining: `.lc-cbtn` omitted from #535's selector (still 40px); dense-row 44x44 pending device QA. |

Comments recording the above were posted to LC-555 / LC-561 / LC-565; LC-564 was flipped to Done.

## 2. Genuinely-remaining redesign work (with preliminary plans)

Small and concrete. Each is its own spec/plan at kickoff.

### R1. LC-561 - shared admin page-header component
- Scope: one reusable Askama header pattern (title, optional description, optional primary action, optional filter/toolbar slot) adopted across `server/templates/admin/*`.
- Files: `server/templates/admin_layout.html` (extend the `admin_header` block), the admin pages that hand-roll intros (`admin/bridges.html`, `admin/invites.html`, `admin/rooms.html`, others), `server/assets/tailwind.css` if a new `.lc-admin-header` class is needed.
- Approach: define the block/partial once, migrate pages onto it, reuse `.lc-action-row` (finally giving that dead class a real home - see R5). No new colors; tokens only.
- ACs: every admin page uses the shared header; intro/action rows are consistent; `just check` + `just test` green; no raw hex.

### R2. LC-558 - room timeline hierarchy polish
- Scope: strengthen author/timestamp hierarchy; soften the hover action chip; retune the mentioned-message highlight via `accent-surface`; verify day/unread divider contrast; keep grouping + keyboard reachability + OOB ids.
- Files: `server/templates/room/message.html`, `server/templates/room/followup_block.html`, related `.lc-msg-*` / `.lc-mentioned` / `.lc-day-divider` rules in `main.css`.
- Approach: token/class tuning only, no structural change; preserve every `hx-swap-oob` id (`msg-{id}`, `reactions-{id}`, `seen-{id}`, `followup-{id}`, `poll-{id}`).
- ACs: dense chat still fits the same message count; mention highlight obvious but not loud; edit form unchanged; hover menus stable under htmx swaps; all 24 palette/mode combos legible (re-run the gallery + contrast script).
- Note: this is the one genuinely subjective item; it should start with a quick visual pass in `just dev-web-local` before coding.

### R3. LC-560 - enclave rail focus-visible
- Scope: add a tokenized `:focus-visible` treatment to `.lc-rail-tile` (the only real gap; navy remap + hover/active already done).
- Files: `server/assets/main.css` (`.lc-rail-tile` rule).
- Approach: mirror the composer focus-ring pattern LC-565 established; use `--ring` so it themes per palette.
- ACs: keyboard focus on the rail is clearly visible in all 6 palettes x light/dark/hc; no visual change to mouse hover/active.

### R4. LC-565 tail - call-button + dense-row tap targets
- Scope: add `.lc-cbtn` to the coarse-pointer 44px rule; decide 44x44 on dense composer/header rows (needs real-device QA).
- Files: `server/assets/main.css` (the `@media (pointer: coarse)` block from PR #535).
- Approach: extend the existing selector list; the dense-row decision is a QA call, not code-first.
- ACs: call buttons are >=44px on coarse pointers; dense-row decision recorded.

### R5. LC-555 - adopt or close the dead helpers
- Scope: `.lc-toolbar` and `.lc-empty` have zero adoption; `.lc-action-row` only in the dev gallery.
- Approach: prefer adopting them via R1 (admin header uses `.lc-action-row`/`.lc-toolbar`) and the empty-state partial (`.lc-empty` via `partials/empty_state.html`), then close LC-555 as satisfied. If not adopted, close as satisfied anyway (the literal "classes exist" AC is met).
- ACs: either the helpers have a real usage, or LC-555 is closed with a note.

### R6. The Details panel - the one genuine mockup delta (needs a decision)
- The mockups show a right-side room Details panel (Created date, Members roster/count, Notifications, Pinned, Leave room). This does NOT exist as a column: Notifications lives in a header dropdown, Pinned/Files/About are a full-page tabbed About (`room/info.html`), "Leave" is enclave-level, and Created-date + a per-room members roster are not surfaced at all.
- Building it is net-new UI plus backend data (room created_at, per-room members query, per-room leave semantics vs enclave leave). This is the only part of the old "P2 pixel-match" that is genuinely unbuilt.
- Decision required before planning: (a) build the Details panel as a real feature (new backend + column), (b) restyle-only the existing fragmented About/notify surfaces to look closer to the mockup, or (c) drop it (the mockup diverges from the shipped IA, like the LC-368 account-menu call). Recommend deciding at kickoff; if (a), it gets a full spec/plan of its own.

## 3. Six-palette follow-on (LC-566 downstream - genuinely new)

The palettes recolor via tokens, so they only look right on surfaces that are fully tokenized. LC-364/LC-UI have been tokenizing steadily; this is the palette-specific verification + gap sweep.

- V1. Hardcoded-color sweep: grep templates for raw hex / Tailwind literal colors (`slate-`, `blue-`, `bg-white`, etc.) that would not recolor under cobalt/ink-ice/arctic/deep-sea/royal-navy; convert to tokens. (The LC-UI passes did this generally; this pass is palette-specific and covers surfaces they did not touch.)
- V2. Sidebar/rail per palette: confirm the `.lc-sidebar` / `.lc-rail-*` token remap resolves correctly for all 6 palettes (each palette defines its own `--sidebar-*` / `--rail-*`); eyeball on `/dev/theme-gallery` and in the room view.
- V3. Regression: re-run `server/scripts/contrast-check.mjs` (expect 72/72) after the LC-UI token churn (e.g. the phantom-token fix touched tokens); confirm the gallery still renders all 24 combos.
- V4. Accent decision (open from P1): arctic + deep-sea LIGHT accents were darkened one shade (sky-700 / cyan-700) for WCAG AA with white button text. Confirm keep, or switch to the brighter cyan with near-black text. Update the two blocks + re-run the contrast script.
- ACs: no un-tokenized color breaks a non-default palette on any primary surface; 72/72 contrast holds; accent decision recorded.

## 4. Old P2-P6 program, mapped to reality

The earlier six-phase pixel-match plan, annotated. It is retained here only to show nothing was dropped; it is NOT executed as written.

| Old phase | Reality | Genuine remaining |
| --- | --- | --- |
| P1 Foundation (6 palettes) | Done - LC-566, merged #528 | V1-V4 verification (section 3) |
| P2 Core chat pixel-match | Done - LC-364 (LC-366/367/371/373/374/375-381) + LC-UI-06 composer | Details panel (R6); LC-558 timeline (R2); LC-560 rail focus (R3) |
| P3 Utility-page migration | Done - LC-UI-03 (feeds/webhooks/email, LC-556) + LC-UI-04 (empty states, LC-557) | LC-561 admin header (R1) |
| P4 Remaining-surface sweep | Done - LC-364 (voice/video LC-402/405/406/416, Settings LC-426, room info LC-449, enclave LC-463/469, admin LC-510) | token gaps only -> covered by the palette sweep (V1) |
| P5 Public/brand | Done - LC-UI-11 (LC-564) | none |
| P6 A11y + density + regression | Done - LC-UI-12 (LC-565) + #534/#535 | LC-565 tail (R4); regression re-run (V3) |

Net: the entire redesign reduces to R1-R6 + V1-V4. No new multi-phase program is warranted.

## 5. Ticket hygiene recommendations (for the owner to action)

- LC-364: the real completed redesign epic. Leave Done.
- LC-553: the LC-UI-01..12 epic, still "To do" though most children are done. After R1-R5 land, reconcile and close.
- LC-541: the original design-mandate ticket, split and delivered via LC-364 + LC-553 + LC-566. Its only undelivered AC is the Discord-style bottom account menu, which was deliberately diverged (LC-368 folded the account menu into the profile header). Recommend: drop that AC (or fork it into its own ticket), then close LC-541 as superseded. Note: LC-566 currently relates to LC-541; consider re-pointing it to LC-553 or LC-364.
- LC-566: six-palette system, merged. After the section-3 verification, it is complete.

## 6. Suggested order

1. Quick wins: R3 (rail focus), R4 (call-button 44px), R5 (adopt/close helpers).
2. R1 (admin header) - self-contained, gives R5's helpers a home.
3. V1-V4 (palette verification + accent decision) - protects the LC-566 investment as other surfaces change.
4. R2 (timeline polish) - start with a visual pass.
5. R6 (Details panel) - only after the build/restyle/drop decision.
6. Ticket hygiene (section 5) throughout.
