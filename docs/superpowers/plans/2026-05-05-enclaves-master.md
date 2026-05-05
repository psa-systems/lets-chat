# Enclaves Implementation Plan (Master Index)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement each phase plan task-by-task.

**Spec:** `docs/superpowers/specs/2026-05-05-enclaves-design.md`

The feature is split into four phase plans. Phases land in order; each phase ends with a passing `just check` and a green branch. The application keeps working between phases — earlier phases land backend code that is not yet wired into the UI; the UI cutover happens in Phase 3.

| Phase | Plan | Outcome |
|------:|------|---------|
| 1 | `2026-05-05-enclaves-phase1-db.md` | Migration, models, `db::enclave` module, `is_room_accessible`, `perms`, and the cross-DB `backfill_general_membership` startup hook. No UI change. |
| 2 | `2026-05-05-enclaves-phase2-routes.md` | `routes::enclave` module (CRUD, invitations, discovery, member/room ops), `last_visited` cookie, search scoping. Routes are reachable but the sidebar still hides them. |
| 3 | `2026-05-05-enclaves-phase3-ui.md` | Two-column chrome (switcher + per-enclave sidebar), enclave/invitation/discover templates, welcome + admin/rooms updates. Full user-facing feature. |
| 4 | `2026-05-05-enclaves-phase4-realtime-cleanup.md` | New WS events + broadcasting, removal of `POST /admin/rooms`, integration smoke. |

**Branch:** `feat/enclaves` (already created, holds the spec). Each phase commits onto it. Single PR at the end of phase 4.

**Conventions all phases follow:**

- TDD: every behavior change has a failing test first.
- Each task ends with a commit on `feat/enclaves`.
- Code uses ASCII-only punctuation (no em-dash) and follows existing Askama / Axum / SQLx patterns.
- Tests live in `server/tests/<topic>.rs` using the in-memory SQLite pool pattern from `server/tests/db_auth.rs`.
- Run `./dev/cargo test -p lets-chat-server --test <name>` for a single test file.
- Run `just check` at every phase boundary.
