# Phase 11 — Message Search

**Date:** 2026-04-15  
**Status:** Done

---

## Goal

Full-text search across messages the caller has access to. Depends on Phase 9 (private room membership gates search results). Returns up to 50 results ordered by relevance, respecting all access rules.

---

## Tasks

- [x] 1. DB migration `0008_search.sql` — FTS5 virtual table + 3 sync triggers
- [x] 2. `src/models/search_result.rs` — `SearchResult` model
- [x] 3. `src/models/mod.rs` — register `SearchResult`
- [x] 4. `src/db/chat.rs` — `sanitize_fts_query` + `search_messages` DB functions
- [x] 5. `src/server_fns/chat.rs` — `search_messages` server fn with access control and author name resolution
- [x] 6. `src/components/sidebar.rs` — search bar UI with live results panel
- [x] 7. `tests/db_search.rs` — 7 integration tests
- [x] 8. `justfile` — add `--test db_search` to test recipe
- [x] 9. `docs/superpowers/TODO.md` — mark Phase 11 Done

---

## Schema

```sql
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    body,
    content=messages,
    content_rowid=id
);

INSERT INTO messages_fts(rowid, body)
    SELECT id, body FROM messages WHERE deleted_at IS NULL;

CREATE TRIGGER messages_fts_insert AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts(rowid, body) VALUES (new.id, new.body);
END;

CREATE TRIGGER messages_fts_delete AFTER UPDATE OF deleted_at ON messages
    WHEN new.deleted_at IS NOT NULL BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, body) VALUES ('delete', old.id, old.body);
END;

CREATE TRIGGER messages_fts_update AFTER UPDATE OF body ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, body) VALUES ('delete', old.id, old.body);
    INSERT INTO messages_fts(rowid, body) VALUES (new.id, new.body);
END;
```

`content=messages` means FTS5 reads body values back from the real table — no duplication. Triggers keep the index in sync on insert, soft-delete, and edit.

---

## Query Sanitization

FTS5 special characters (`"`, `*`, `(`, `)`, `+`, `-`, `^`, `:`) are stripped from each token. Each remaining token is wrapped in double quotes so FTS5 treats it as a literal term rather than a keyword. Empty queries (nothing left after sanitization) are rejected before hitting the DB.

```rust
pub fn sanitize_fts_query(raw: &str) -> Option<String>
```

---

## Access Control

Built entirely into the SQL query — no application-level filtering over result sets.

| Caller | Sees |
|--------|------|
| Regular user | Public rooms + rooms they are a member of (`room_members` subquery) |
| Admin | All non-DM rooms |

DM rooms are always excluded from search results regardless of role.

---

## Author Name Resolution

The `messages` table stores `user_id`, not display names. The server fn resolves each result's `user_id` to a display name via the auth pool (same pattern as `list_messages`). The `SearchResult` model carries both `user_id` and the resolved `author_name`.

---

## UI

Search bar sits between the sidebar header and the rooms list. No new route needed.

- Typing updates `search_input` signal locally.
- Pressing **Enter** (or clicking ↵) copies `search_input` to `search_query`, which triggers the `use_server_future`.
- While `search_query` is non-empty, the rooms/DMs list is replaced by a results panel showing room name, author, timestamp, and message body (up to 2 lines).
- Clicking a result navigates to the room and clears the search.
- **Escape** or the **×** button clears both signals and restores the normal sidebar.

---

## Integration Tests (`tests/db_search.rs`)

| Test | Covers |
|------|--------|
| `test_search_finds_matching_message` | Basic FTS5 match |
| `test_search_does_not_return_soft_deleted` | Soft-delete exclusion |
| `test_search_private_room_excluded_for_non_member` | Access control — blocked |
| `test_search_private_room_visible_to_member` | Access control — allowed |
| `test_search_fts_special_chars_do_not_panic` | Sanitizer safety |
| `test_search_edited_body_is_reindexed` | Update trigger correctness |
| `test_admin_can_search_private_rooms` | Admin bypass |

---

## Risks / Gotchas

- **FTS5 and SQLx**: the `query!` macro cannot introspect virtual table columns. All FTS5 queries use `sqlx::query` (raw) with manual `row.get(...)` extraction.
- **Content table sync**: `content=messages` means FTS5 does not store body text itself — it defers to the real table. This means the update trigger must delete the old entry before inserting the new one (FTS5 `'delete'` command), not just insert a new row.
- **Room filter clause ordering**: the `room_id_filter` parameter is bound before the access-control `caller_user_id` parameter. The order must match exactly how the SQL string is assembled.
