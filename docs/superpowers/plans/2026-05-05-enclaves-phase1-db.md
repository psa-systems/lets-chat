# Enclaves — Phase 1: DB Foundation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the chat-DB schema for enclaves, the `db::enclave` module, the cross-DB membership backfill, and the new room-access predicate. No UI change.

**Architecture:** New tables `enclaves`, `enclave_members`, `enclave_invitations` plus an `enclave_id` column on `rooms`. Three-tier role enum stored as a CHECK string. Startup-time idempotent backfill turns existing auth-DB users into General-enclave members.

**Tech Stack:** Rust, SQLx (SQLite), Axum, Tokio. Tests use in-memory SQLite pools.

---

## File Structure (Phase 1)

| File | Purpose |
|---|---|
| `server/migrations/chat/0009_enclaves.sql` | Schema + chat-side data move (creates `General`, sets `enclave_id`). |
| `server/src/models/enclave.rs` | `Enclave`, `EnclaveRole`, `EnclaveMembership`, `EnclaveInvitation`. |
| `server/src/models/mod.rs` | Re-export `enclave`. |
| `server/src/db/enclave.rs` | All enclave DB helpers + `backfill_general_membership`. |
| `server/src/db/mod.rs` | Re-export `enclave`. |
| `server/src/db/chat.rs` | `create_room` gains `enclave_id`; new `is_room_accessible`; new `list_rooms_in_enclave`; unread-counts gain enclave filter. |
| `server/src/perms.rs` | Permission helpers (owner/admin/member + site-admin god-mode short-circuit). |
| `server/src/lib.rs` | Re-export `perms`. |
| `server/src/main.rs` | Call `backfill_general_membership` after both pools migrate. |
| `server/src/routes/auth.rs` | Call backfill after first-user auto-promotion. |
| `server/tests/db_enclave.rs` | DB-layer tests for the new module. |
| `server/tests/migration_enclaves.rs` | Schema-migration test + room/membership invariants. |
| `server/tests/perms.rs` | Permission-helper unit tests. |

---

## Task 1: Schema migration (no data step beyond chat-side)

**Files:**
- Create: `server/migrations/chat/0009_enclaves.sql`
- Test: `server/tests/migration_enclaves.rs`

- [ ] **Step 1: Write the failing test**

```rust
// server/tests/migration_enclaves.rs
use sqlx::{Row, SqlitePool};

async fn fresh_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    for sql in [
        include_str!("../migrations/chat/0001_create_tables.sql"),
        include_str!("../migrations/chat/0002_moderation.sql"),
        include_str!("../migrations/chat/0003_dms.sql"),
        include_str!("../migrations/chat/0004_message_editing.sql"),
        include_str!("../migrations/chat/0005_private_rooms.sql"),
        include_str!("../migrations/chat/0006_read_receipts.sql"),
        include_str!("../migrations/chat/0007_reactions.sql"),
        include_str!("../migrations/chat/0008_search.sql"),
        include_str!("../migrations/chat/0009_enclaves.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

#[tokio::test]
async fn migration_creates_general_and_moves_rooms() {
    let pool = fresh_pool().await;

    let general_id: i64 = sqlx::query("SELECT id FROM enclaves WHERE name='General'")
        .fetch_one(&pool).await.unwrap().get("id");

    let row = sqlx::query(
        "SELECT name, created_by FROM enclaves WHERE id=?",
    ).bind(general_id).fetch_one(&pool).await.unwrap();
    assert_eq!(row.get::<String,_>("name"), "General");
    assert_eq!(row.get::<String,_>("created_by"), "system");

    let n: i64 = sqlx::query("SELECT COUNT(*) AS c FROM rooms WHERE enclave_id IS NULL AND room_type != 'dm'")
        .fetch_one(&pool).await.unwrap().get("c");
    assert_eq!(n, 0, "every non-DM room must be in an enclave after migration");

    let m: i64 = sqlx::query("SELECT COUNT(*) AS c FROM rooms WHERE enclave_id = ? AND name IN ('general','random')")
        .bind(general_id).fetch_one(&pool).await.unwrap().get("c");
    assert_eq!(m, 2);

    let members: i64 = sqlx::query("SELECT COUNT(*) AS c FROM enclave_members")
        .fetch_one(&pool).await.unwrap().get("c");
    assert_eq!(members, 0, "membership backfill is a separate step");
}

#[tokio::test]
async fn migration_partial_unique_owner_index_enforced() {
    let pool = fresh_pool().await;
    let general_id: i64 = sqlx::query("SELECT id FROM enclaves WHERE name='General'")
        .fetch_one(&pool).await.unwrap().get("id");

    sqlx::query("INSERT INTO enclave_members (enclave_id, user_id, role) VALUES (?, 'u1', 'owner')")
        .bind(general_id).execute(&pool).await.unwrap();
    let dup = sqlx::query("INSERT INTO enclave_members (enclave_id, user_id, role) VALUES (?, 'u2', 'owner')")
        .bind(general_id).execute(&pool).await;
    assert!(dup.is_err(), "two owners per enclave must be rejected");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `./dev/cargo test -p lets-chat-server --test migration_enclaves`
Expected: FAIL with "no such file or table" pointing at `0009_enclaves.sql`.

- [ ] **Step 3: Write the migration**

```sql
-- server/migrations/chat/0009_enclaves.sql
CREATE TABLE enclaves (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL UNIQUE,
    description TEXT,
    is_public   INTEGER NOT NULL DEFAULT 0,
    invite_code TEXT,
    created_by  TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE UNIQUE INDEX idx_enclaves_invite_code
    ON enclaves(invite_code) WHERE invite_code IS NOT NULL;

CREATE TABLE enclave_members (
    enclave_id  INTEGER NOT NULL REFERENCES enclaves(id) ON DELETE CASCADE,
    user_id     TEXT NOT NULL,
    role        TEXT NOT NULL CHECK (role IN ('owner','admin','member')),
    joined_at   TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (enclave_id, user_id)
);
CREATE UNIQUE INDEX idx_enclaves_one_owner
    ON enclave_members(enclave_id) WHERE role = 'owner';
CREATE INDEX idx_enclave_members_user ON enclave_members(user_id);

CREATE TABLE enclave_invitations (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    enclave_id  INTEGER NOT NULL REFERENCES enclaves(id) ON DELETE CASCADE,
    invitee_id  TEXT NOT NULL,
    invited_by  TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (enclave_id, invitee_id)
);
CREATE INDEX idx_enclave_invitations_invitee ON enclave_invitations(invitee_id);

ALTER TABLE rooms ADD COLUMN enclave_id INTEGER REFERENCES enclaves(id) ON DELETE CASCADE;
CREATE INDEX idx_rooms_enclave ON rooms(enclave_id);

INSERT INTO enclaves (name, description, created_by) VALUES ('General', 'Default enclave', 'system');
UPDATE rooms SET enclave_id = (SELECT id FROM enclaves WHERE name='General') WHERE room_type != 'dm';
```

- [ ] **Step 4: Run test to verify it passes**

Run: `./dev/cargo test -p lets-chat-server --test migration_enclaves`
Expected: PASS for both tests.

- [ ] **Step 5: Commit**

```bash
git add server/migrations/chat/0009_enclaves.sql server/tests/migration_enclaves.rs
git commit -m "$(cat <<'EOF'
feat(enclaves): add chat-DB schema migration

Adds enclaves, enclave_members, enclave_invitations, and a rooms.enclave_id column. Inserts a default General enclave and moves every non-DM room into it. Membership backfill is intentionally deferred to a Rust startup step that can read auth.db.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `EnclaveRole` enum + struct models

**Files:**
- Create: `server/src/models/enclave.rs`
- Modify: `server/src/models/mod.rs`
- Test: `server/tests/db_enclave.rs` (will grow across Phase 1)

- [ ] **Step 1: Write the failing test**

```rust
// server/tests/db_enclave.rs
use lets_chat::models::enclave::EnclaveRole;

#[test]
fn role_round_trips_via_str() {
    for r in [EnclaveRole::Owner, EnclaveRole::Admin, EnclaveRole::Member] {
        let s = r.as_str();
        assert_eq!(EnclaveRole::from_str(s).unwrap(), r);
    }
    assert!(EnclaveRole::from_str("nope").is_err());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `./dev/cargo test -p lets-chat-server --test db_enclave`
Expected: FAIL with "module enclave not found".

- [ ] **Step 3: Write the model file**

```rust
// server/src/models/enclave.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnclaveRole { Owner, Admin, Member }

impl EnclaveRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            EnclaveRole::Owner => "owner",
            EnclaveRole::Admin => "admin",
            EnclaveRole::Member => "member",
        }
    }
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "owner" => Ok(EnclaveRole::Owner),
            "admin" => Ok(EnclaveRole::Admin),
            "member" => Ok(EnclaveRole::Member),
            other => Err(format!("invalid enclave role: {other}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Enclave {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub is_public: bool,
    pub invite_code: Option<String>,
    pub created_by: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnclaveMembership {
    pub enclave_id: i64,
    pub user_id: String,
    pub role: EnclaveRole,
    pub joined_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnclaveInvitation {
    pub id: i64,
    pub enclave_id: i64,
    pub invitee_id: String,
    pub invited_by: String,
    pub created_at: String,
}
```

- [ ] **Step 4: Re-export from `models::mod`**

Open `server/src/models/mod.rs` and add `pub mod enclave;` near the other `pub mod` lines, then add `pub use enclave::{Enclave, EnclaveInvitation, EnclaveMembership, EnclaveRole};` near the other re-exports.

- [ ] **Step 5: Run test to verify it passes**

Run: `./dev/cargo test -p lets-chat-server --test db_enclave`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add server/src/models/enclave.rs server/src/models/mod.rs server/tests/db_enclave.rs
git commit -m "$(cat <<'EOF'
feat(enclaves): add Enclave, EnclaveRole, EnclaveMembership, EnclaveInvitation models

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `db::enclave` skeleton + `create_enclave` (transactional)

**Files:**
- Create: `server/src/db/enclave.rs`
- Modify: `server/src/db/mod.rs`
- Test: `server/tests/db_enclave.rs`

- [ ] **Step 1: Add a setup helper + the failing test**

Append to `server/tests/db_enclave.rs`:

```rust
use sqlx::{Row, SqlitePool};

async fn chat_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    for sql in [
        include_str!("../migrations/chat/0001_create_tables.sql"),
        include_str!("../migrations/chat/0002_moderation.sql"),
        include_str!("../migrations/chat/0003_dms.sql"),
        include_str!("../migrations/chat/0004_message_editing.sql"),
        include_str!("../migrations/chat/0005_private_rooms.sql"),
        include_str!("../migrations/chat/0006_read_receipts.sql"),
        include_str!("../migrations/chat/0007_reactions.sql"),
        include_str!("../migrations/chat/0008_search.sql"),
        include_str!("../migrations/chat/0009_enclaves.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

#[tokio::test]
async fn create_enclave_inserts_owner_membership() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "rust", Some("rustaceans"), "u-creator")
        .await.unwrap();
    let row = sqlx::query("SELECT name, description, created_by FROM enclaves WHERE id=?")
        .bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(row.get::<String,_>("name"), "rust");
    assert_eq!(row.get::<Option<String>,_>("description").unwrap(), "rustaceans");
    assert_eq!(row.get::<String,_>("created_by"), "u-creator");

    let role: String = sqlx::query("SELECT role FROM enclave_members WHERE enclave_id=? AND user_id=?")
        .bind(id).bind("u-creator").fetch_one(&pool).await.unwrap().get("role");
    assert_eq!(role, "owner");
}

#[tokio::test]
async fn create_enclave_duplicate_name_errors() {
    let pool = chat_pool().await;
    lets_chat::db::enclave::create_enclave(&pool, "dup", None, "u").await.unwrap();
    let err = lets_chat::db::enclave::create_enclave(&pool, "dup", None, "u2").await;
    assert!(err.is_err());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `./dev/cargo test -p lets-chat-server --test db_enclave`
Expected: FAIL with "module enclave not found in db".

- [ ] **Step 3: Write the module skeleton + `create_enclave`**

```rust
// server/src/db/enclave.rs
use sqlx::{Row, SqlitePool};

use crate::models::enclave::{Enclave, EnclaveInvitation, EnclaveMembership, EnclaveRole};

fn map_enclave(row: &sqlx::sqlite::SqliteRow) -> Enclave {
    Enclave {
        id: row.get("id"),
        name: row.get("name"),
        description: row.get("description"),
        is_public: row.get::<i64, _>("is_public") != 0,
        invite_code: row.get("invite_code"),
        created_by: row.get("created_by"),
        created_at: row.get("created_at"),
    }
}

pub async fn create_enclave(
    pool: &SqlitePool,
    name: &str,
    description: Option<&str>,
    creator_id: &str,
) -> Result<i64, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let res = sqlx::query("INSERT INTO enclaves (name, description, created_by) VALUES (?, ?, ?)")
        .bind(name).bind(description).bind(creator_id)
        .execute(&mut *tx).await?;
    let id = res.last_insert_rowid();
    sqlx::query("INSERT INTO enclave_members (enclave_id, user_id, role) VALUES (?, ?, 'owner')")
        .bind(id).bind(creator_id)
        .execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(id)
}
```

- [ ] **Step 4: Re-export from `db::mod`**

Open `server/src/db/mod.rs` and add `pub mod enclave;` near the other `pub mod` lines.

- [ ] **Step 5: Run tests**

Run: `./dev/cargo test -p lets-chat-server --test db_enclave`
Expected: PASS for both.

- [ ] **Step 6: Commit**

```bash
git add server/src/db/enclave.rs server/src/db/mod.rs server/tests/db_enclave.rs
git commit -m "$(cat <<'EOF'
feat(enclaves): add db::enclave::create_enclave with owner-membership tx

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Lookups (`get_enclave`, `get_enclave_by_invite_code`, `get_membership`)

**Files:**
- Modify: `server/src/db/enclave.rs`
- Test: `server/tests/db_enclave.rs`

- [ ] **Step 1: Add failing tests**

Append:

```rust
#[tokio::test]
async fn get_enclave_round_trip() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "u").await.unwrap();
    let e = lets_chat::db::enclave::get_enclave(&pool, id).await.unwrap().unwrap();
    assert_eq!(e.name, "x");
    assert!(!e.is_public);
    assert_eq!(e.invite_code, None);
    assert!(lets_chat::db::enclave::get_enclave(&pool, 9999).await.unwrap().is_none());
}

#[tokio::test]
async fn get_membership_returns_role() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "u").await.unwrap();
    let m = lets_chat::db::enclave::get_membership(&pool, id, "u").await.unwrap().unwrap();
    assert_eq!(m.role, lets_chat::models::enclave::EnclaveRole::Owner);
    assert!(lets_chat::db::enclave::get_membership(&pool, id, "nobody").await.unwrap().is_none());
}

#[tokio::test]
async fn get_enclave_by_invite_code_finds_match() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "u").await.unwrap();
    sqlx::query("UPDATE enclaves SET invite_code='abc' WHERE id=?").bind(id).execute(&pool).await.unwrap();
    let e = lets_chat::db::enclave::get_enclave_by_invite_code(&pool, "abc").await.unwrap().unwrap();
    assert_eq!(e.id, id);
    assert!(lets_chat::db::enclave::get_enclave_by_invite_code(&pool, "missing").await.unwrap().is_none());
}
```

- [ ] **Step 2: Run; expected FAIL**

Run: `./dev/cargo test -p lets-chat-server --test db_enclave`

- [ ] **Step 3: Implement**

Append to `server/src/db/enclave.rs`:

```rust
pub async fn get_enclave(pool: &SqlitePool, id: i64) -> Result<Option<Enclave>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, name, description, is_public, invite_code, created_by, created_at \
         FROM enclaves WHERE id=?",
    ).bind(id).fetch_optional(pool).await?;
    Ok(row.as_ref().map(map_enclave))
}

pub async fn get_enclave_by_invite_code(
    pool: &SqlitePool,
    code: &str,
) -> Result<Option<Enclave>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, name, description, is_public, invite_code, created_by, created_at \
         FROM enclaves WHERE invite_code = ?",
    ).bind(code).fetch_optional(pool).await?;
    Ok(row.as_ref().map(map_enclave))
}

pub async fn get_membership(
    pool: &SqlitePool,
    enclave_id: i64,
    user_id: &str,
) -> Result<Option<EnclaveMembership>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT enclave_id, user_id, role, joined_at \
         FROM enclave_members WHERE enclave_id=? AND user_id=?",
    ).bind(enclave_id).bind(user_id).fetch_optional(pool).await?;
    let Some(r) = row else { return Ok(None) };
    let role_str: String = r.get("role");
    let role = EnclaveRole::from_str(&role_str)
        .map_err(|e| sqlx::Error::Decode(e.into()))?;
    Ok(Some(EnclaveMembership {
        enclave_id: r.get("enclave_id"),
        user_id: r.get("user_id"),
        role,
        joined_at: r.get("joined_at"),
    }))
}
```

- [ ] **Step 4: Run; expected PASS**

Run: `./dev/cargo test -p lets-chat-server --test db_enclave`

- [ ] **Step 5: Commit**

```bash
git add server/src/db/enclave.rs server/tests/db_enclave.rs
git commit -m "feat(enclaves): get_enclave, get_enclave_by_invite_code, get_membership

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: List helpers (`list_enclaves_for_user`, `list_public_enclaves`, `list_members`)

**Files:**
- Modify: `server/src/db/enclave.rs`
- Test: `server/tests/db_enclave.rs`

- [ ] **Step 1: Add failing tests**

Append:

```rust
#[tokio::test]
async fn list_enclaves_for_user_returns_only_member_enclaves() {
    let pool = chat_pool().await;
    let a = lets_chat::db::enclave::create_enclave(&pool, "a", None, "u1").await.unwrap();
    let _b = lets_chat::db::enclave::create_enclave(&pool, "b", None, "u2").await.unwrap();
    let mine = lets_chat::db::enclave::list_enclaves_for_user(&pool, "u1").await.unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].id, a);
}

#[tokio::test]
async fn list_public_enclaves_filters_on_is_public() {
    let pool = chat_pool().await;
    let _ = lets_chat::db::enclave::create_enclave(&pool, "private", None, "u").await.unwrap();
    let pub_id = lets_chat::db::enclave::create_enclave(&pool, "open", None, "u").await.unwrap();
    sqlx::query("UPDATE enclaves SET is_public=1 WHERE id=?").bind(pub_id).execute(&pool).await.unwrap();
    let pubs = lets_chat::db::enclave::list_public_enclaves(&pool).await.unwrap();
    assert_eq!(pubs.len(), 1);
    assert_eq!(pubs[0].id, pub_id);
}

#[tokio::test]
async fn list_members_returns_all_with_roles() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "owner1").await.unwrap();
    sqlx::query("INSERT INTO enclave_members (enclave_id, user_id, role) VALUES (?, 'admin1', 'admin')")
        .bind(id).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO enclave_members (enclave_id, user_id, role) VALUES (?, 'member1', 'member')")
        .bind(id).execute(&pool).await.unwrap();
    let mut members = lets_chat::db::enclave::list_members(&pool, id).await.unwrap();
    members.sort_by(|a,b| a.user_id.cmp(&b.user_id));
    assert_eq!(members.len(), 3);
    assert_eq!(members[0].user_id, "admin1");
    assert_eq!(members[0].role, lets_chat::models::enclave::EnclaveRole::Admin);
}
```

- [ ] **Step 2: Run; FAIL**

- [ ] **Step 3: Implement**

Append:

```rust
pub async fn list_enclaves_for_user(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<Enclave>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT e.id, e.name, e.description, e.is_public, e.invite_code, e.created_by, e.created_at \
         FROM enclaves e \
         JOIN enclave_members m ON m.enclave_id = e.id AND m.user_id = ? \
         ORDER BY e.name COLLATE NOCASE",
    ).bind(user_id).fetch_all(pool).await?;
    Ok(rows.iter().map(map_enclave).collect())
}

pub async fn list_public_enclaves(pool: &SqlitePool) -> Result<Vec<Enclave>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, name, description, is_public, invite_code, created_by, created_at \
         FROM enclaves WHERE is_public = 1 ORDER BY name COLLATE NOCASE",
    ).fetch_all(pool).await?;
    Ok(rows.iter().map(map_enclave).collect())
}

pub async fn list_members(
    pool: &SqlitePool,
    enclave_id: i64,
) -> Result<Vec<EnclaveMembership>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT enclave_id, user_id, role, joined_at FROM enclave_members WHERE enclave_id = ? ORDER BY joined_at",
    ).bind(enclave_id).fetch_all(pool).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let role_str: String = r.get("role");
        let role = EnclaveRole::from_str(&role_str)
            .map_err(|e| sqlx::Error::Decode(e.into()))?;
        out.push(EnclaveMembership {
            enclave_id: r.get("enclave_id"),
            user_id: r.get("user_id"),
            role,
            joined_at: r.get("joined_at"),
        });
    }
    Ok(out)
}
```

- [ ] **Step 4: Run; PASS**

- [ ] **Step 5: Commit**

```bash
git add server/src/db/enclave.rs server/tests/db_enclave.rs
git commit -m "feat(enclaves): list_enclaves_for_user, list_public_enclaves, list_members

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Member ops (`add_member`, `remove_member`, `update_role`)

**Files:**
- Modify: `server/src/db/enclave.rs`
- Test: `server/tests/db_enclave.rs`

- [ ] **Step 1: Failing tests**

Append:

```rust
use lets_chat::models::enclave::EnclaveRole;

#[tokio::test]
async fn add_remove_member_round_trip() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "owner1").await.unwrap();
    lets_chat::db::enclave::add_member(&pool, id, "u2", EnclaveRole::Member).await.unwrap();
    assert!(lets_chat::db::enclave::get_membership(&pool, id, "u2").await.unwrap().is_some());
    lets_chat::db::enclave::remove_member(&pool, id, "u2").await.unwrap();
    assert!(lets_chat::db::enclave::get_membership(&pool, id, "u2").await.unwrap().is_none());
}

#[tokio::test]
async fn add_member_idempotent_via_or_ignore() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "owner1").await.unwrap();
    lets_chat::db::enclave::add_member(&pool, id, "u2", EnclaveRole::Member).await.unwrap();
    lets_chat::db::enclave::add_member(&pool, id, "u2", EnclaveRole::Admin).await.unwrap();
    let m = lets_chat::db::enclave::get_membership(&pool, id, "u2").await.unwrap().unwrap();
    assert_eq!(m.role, EnclaveRole::Member, "second add must NOT promote silently");
}

#[tokio::test]
async fn update_role_changes_role() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "owner1").await.unwrap();
    lets_chat::db::enclave::add_member(&pool, id, "u2", EnclaveRole::Member).await.unwrap();
    lets_chat::db::enclave::update_role(&pool, id, "u2", EnclaveRole::Admin).await.unwrap();
    let m = lets_chat::db::enclave::get_membership(&pool, id, "u2").await.unwrap().unwrap();
    assert_eq!(m.role, EnclaveRole::Admin);
}
```

- [ ] **Step 2: Run; FAIL**

- [ ] **Step 3: Implement**

Append:

```rust
pub async fn add_member(
    pool: &SqlitePool,
    enclave_id: i64,
    user_id: &str,
    role: EnclaveRole,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO enclave_members (enclave_id, user_id, role) VALUES (?, ?, ?)")
        .bind(enclave_id).bind(user_id).bind(role.as_str())
        .execute(pool).await?;
    Ok(())
}

pub async fn remove_member(
    pool: &SqlitePool,
    enclave_id: i64,
    user_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM enclave_members WHERE enclave_id=? AND user_id=?")
        .bind(enclave_id).bind(user_id)
        .execute(pool).await?;
    Ok(())
}

pub async fn update_role(
    pool: &SqlitePool,
    enclave_id: i64,
    user_id: &str,
    role: EnclaveRole,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE enclave_members SET role=? WHERE enclave_id=? AND user_id=?")
        .bind(role.as_str()).bind(enclave_id).bind(user_id)
        .execute(pool).await?;
    Ok(())
}
```

- [ ] **Step 4: Run; PASS**

- [ ] **Step 5: Commit**

```bash
git add server/src/db/enclave.rs server/tests/db_enclave.rs
git commit -m "feat(enclaves): add_member, remove_member, update_role

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: `transfer_ownership` (atomic)

**Files:**
- Modify: `server/src/db/enclave.rs`
- Test: `server/tests/db_enclave.rs`

- [ ] **Step 1: Failing test**

Append:

```rust
#[tokio::test]
async fn transfer_ownership_demotes_old_promotes_new_atomically() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "owner1").await.unwrap();
    lets_chat::db::enclave::add_member(&pool, id, "u2", EnclaveRole::Admin).await.unwrap();
    lets_chat::db::enclave::transfer_ownership(&pool, id, "u2").await.unwrap();
    let prev = lets_chat::db::enclave::get_membership(&pool, id, "owner1").await.unwrap().unwrap();
    assert_eq!(prev.role, EnclaveRole::Admin);
    let next = lets_chat::db::enclave::get_membership(&pool, id, "u2").await.unwrap().unwrap();
    assert_eq!(next.role, EnclaveRole::Owner);
}

#[tokio::test]
async fn transfer_ownership_rejects_non_member() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "owner1").await.unwrap();
    let err = lets_chat::db::enclave::transfer_ownership(&pool, id, "stranger").await;
    assert!(err.is_err());
}
```

- [ ] **Step 2: Run; FAIL**

- [ ] **Step 3: Implement**

Append:

```rust
pub async fn transfer_ownership(
    pool: &SqlitePool,
    enclave_id: i64,
    new_owner_id: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    let exists = sqlx::query("SELECT 1 FROM enclave_members WHERE enclave_id=? AND user_id=?")
        .bind(enclave_id).bind(new_owner_id)
        .fetch_optional(&mut *tx).await?;
    if exists.is_none() {
        return Err(sqlx::Error::RowNotFound);
    }
    sqlx::query("UPDATE enclave_members SET role='admin' WHERE enclave_id=? AND role='owner'")
        .bind(enclave_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE enclave_members SET role='owner' WHERE enclave_id=? AND user_id=?")
        .bind(enclave_id).bind(new_owner_id)
        .execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
```

- [ ] **Step 4: Run; PASS**

- [ ] **Step 5: Commit**

```bash
git add server/src/db/enclave.rs server/tests/db_enclave.rs
git commit -m "feat(enclaves): transfer_ownership (atomic demote+promote)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Invitations (`create_invitation`, `list_invitations_for_user`, `get_invitation`, `delete_invitation`, `accept_invitation`)

**Files:**
- Modify: `server/src/db/enclave.rs`
- Test: `server/tests/db_enclave.rs`

- [ ] **Step 1: Failing tests**

Append:

```rust
#[tokio::test]
async fn create_and_list_invitation() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "owner1").await.unwrap();
    let inv_id = lets_chat::db::enclave::create_invitation(&pool, id, "u2", "owner1").await.unwrap();
    let pending = lets_chat::db::enclave::list_invitations_for_user(&pool, "u2").await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].0.id, inv_id);
    assert_eq!(pending[0].1.id, id);
}

#[tokio::test]
async fn create_invitation_unique_per_invitee_per_enclave() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "owner1").await.unwrap();
    lets_chat::db::enclave::create_invitation(&pool, id, "u2", "owner1").await.unwrap();
    let err = lets_chat::db::enclave::create_invitation(&pool, id, "u2", "owner1").await;
    assert!(err.is_err());
}

#[tokio::test]
async fn accept_invitation_inserts_member_and_deletes_invite() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "owner1").await.unwrap();
    let inv_id = lets_chat::db::enclave::create_invitation(&pool, id, "u2", "owner1").await.unwrap();
    let (eid, uid) = lets_chat::db::enclave::accept_invitation(&pool, inv_id).await.unwrap();
    assert_eq!(eid, id);
    assert_eq!(uid, "u2");
    let m = lets_chat::db::enclave::get_membership(&pool, id, "u2").await.unwrap().unwrap();
    assert_eq!(m.role, EnclaveRole::Member);
    assert!(lets_chat::db::enclave::list_invitations_for_user(&pool, "u2").await.unwrap().is_empty());
}
```

- [ ] **Step 2: Run; FAIL**

- [ ] **Step 3: Implement**

Append:

```rust
pub async fn create_invitation(
    pool: &SqlitePool,
    enclave_id: i64,
    invitee_id: &str,
    invited_by: &str,
) -> Result<i64, sqlx::Error> {
    let res = sqlx::query(
        "INSERT INTO enclave_invitations (enclave_id, invitee_id, invited_by) VALUES (?, ?, ?)",
    )
    .bind(enclave_id).bind(invitee_id).bind(invited_by)
    .execute(pool).await?;
    Ok(res.last_insert_rowid())
}

pub async fn list_invitations_for_user(
    pool: &SqlitePool,
    user_id: &str,
) -> Result<Vec<(EnclaveInvitation, Enclave)>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT i.id, i.enclave_id, i.invitee_id, i.invited_by, i.created_at, \
                e.id AS e_id, e.name, e.description, e.is_public, e.invite_code, e.created_by, e.created_at AS e_created_at \
         FROM enclave_invitations i \
         JOIN enclaves e ON e.id = i.enclave_id \
         WHERE i.invitee_id = ? \
         ORDER BY i.created_at DESC",
    ).bind(user_id).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|r| {
        let inv = EnclaveInvitation {
            id: r.get("id"),
            enclave_id: r.get("enclave_id"),
            invitee_id: r.get("invitee_id"),
            invited_by: r.get("invited_by"),
            created_at: r.get("created_at"),
        };
        let enc = Enclave {
            id: r.get("e_id"),
            name: r.get("name"),
            description: r.get("description"),
            is_public: r.get::<i64, _>("is_public") != 0,
            invite_code: r.get("invite_code"),
            created_by: r.get("created_by"),
            created_at: r.get("e_created_at"),
        };
        (inv, enc)
    }).collect())
}

pub async fn get_invitation(
    pool: &SqlitePool,
    id: i64,
) -> Result<Option<EnclaveInvitation>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, enclave_id, invitee_id, invited_by, created_at FROM enclave_invitations WHERE id=?",
    ).bind(id).fetch_optional(pool).await?;
    Ok(row.map(|r| EnclaveInvitation {
        id: r.get("id"),
        enclave_id: r.get("enclave_id"),
        invitee_id: r.get("invitee_id"),
        invited_by: r.get("invited_by"),
        created_at: r.get("created_at"),
    }))
}

pub async fn delete_invitation(pool: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM enclave_invitations WHERE id=?")
        .bind(id).execute(pool).await?;
    Ok(())
}

pub async fn accept_invitation(
    pool: &SqlitePool,
    id: i64,
) -> Result<(i64, String), sqlx::Error> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query("SELECT enclave_id, invitee_id FROM enclave_invitations WHERE id=?")
        .bind(id).fetch_optional(&mut *tx).await?;
    let row = row.ok_or(sqlx::Error::RowNotFound)?;
    let enclave_id: i64 = row.get("enclave_id");
    let invitee_id: String = row.get("invitee_id");
    sqlx::query("INSERT OR IGNORE INTO enclave_members (enclave_id, user_id, role) VALUES (?, ?, 'member')")
        .bind(enclave_id).bind(&invitee_id)
        .execute(&mut *tx).await?;
    sqlx::query("DELETE FROM enclave_invitations WHERE id=?")
        .bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((enclave_id, invitee_id))
}
```

- [ ] **Step 4: Run; PASS**

- [ ] **Step 5: Commit**

```bash
git add server/src/db/enclave.rs server/tests/db_enclave.rs
git commit -m "feat(enclaves): invitations CRUD + atomic accept

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: Metadata, invite-code, visibility, delete

**Files:**
- Modify: `server/src/db/enclave.rs`
- Test: `server/tests/db_enclave.rs`

- [ ] **Step 1: Failing tests**

Append:

```rust
#[tokio::test]
async fn update_metadata_and_visibility_and_invite_code() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "u").await.unwrap();
    lets_chat::db::enclave::update_metadata(&pool, id, "y", Some("hi")).await.unwrap();
    lets_chat::db::enclave::set_public(&pool, id, true).await.unwrap();
    lets_chat::db::enclave::regenerate_invite_code(&pool, id, "code123").await.unwrap();
    let e = lets_chat::db::enclave::get_enclave(&pool, id).await.unwrap().unwrap();
    assert_eq!(e.name, "y");
    assert_eq!(e.description.as_deref(), Some("hi"));
    assert!(e.is_public);
    assert_eq!(e.invite_code.as_deref(), Some("code123"));
    lets_chat::db::enclave::clear_invite_code(&pool, id).await.unwrap();
    let e2 = lets_chat::db::enclave::get_enclave(&pool, id).await.unwrap().unwrap();
    assert_eq!(e2.invite_code, None);
}

#[tokio::test]
async fn delete_enclave_cascades() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "u").await.unwrap();
    sqlx::query("INSERT INTO rooms (name, room_type, enclave_id) VALUES ('r', 'public', ?)")
        .bind(id).execute(&pool).await.unwrap();
    lets_chat::db::enclave::delete_enclave(&pool, id).await.unwrap();
    let n: i64 = sqlx::query("SELECT COUNT(*) AS c FROM rooms WHERE enclave_id=?")
        .bind(id).fetch_one(&pool).await.unwrap().get("c");
    assert_eq!(n, 0);
    let m: i64 = sqlx::query("SELECT COUNT(*) AS c FROM enclave_members WHERE enclave_id=?")
        .bind(id).fetch_one(&pool).await.unwrap().get("c");
    assert_eq!(m, 0);
}
```

- [ ] **Step 2: Run; FAIL**

- [ ] **Step 3: Implement**

Append:

```rust
pub async fn update_metadata(
    pool: &SqlitePool,
    id: i64,
    name: &str,
    description: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE enclaves SET name=?, description=? WHERE id=?")
        .bind(name).bind(description).bind(id)
        .execute(pool).await?;
    Ok(())
}

pub async fn set_public(pool: &SqlitePool, id: i64, is_public: bool) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE enclaves SET is_public=? WHERE id=?")
        .bind(if is_public { 1_i64 } else { 0_i64 }).bind(id)
        .execute(pool).await?;
    Ok(())
}

pub async fn regenerate_invite_code(
    pool: &SqlitePool,
    id: i64,
    new_code: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE enclaves SET invite_code=? WHERE id=?")
        .bind(new_code).bind(id)
        .execute(pool).await?;
    Ok(())
}

pub async fn clear_invite_code(pool: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE enclaves SET invite_code=NULL WHERE id=?")
        .bind(id).execute(pool).await?;
    Ok(())
}

pub async fn delete_enclave(pool: &SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM enclaves WHERE id=?")
        .bind(id).execute(pool).await?;
    Ok(())
}
```

- [ ] **Step 4: Run; PASS**

- [ ] **Step 5: Commit**

```bash
git add server/src/db/enclave.rs server/tests/db_enclave.rs
git commit -m "feat(enclaves): metadata, visibility, invite-code, cascade delete

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 10: Cross-DB `backfill_general_membership`

**Files:**
- Modify: `server/src/db/enclave.rs`
- Test: `server/tests/db_enclave.rs`

- [ ] **Step 1: Add an auth-pool helper to the test file**

Append to `server/tests/db_enclave.rs`:

```rust
async fn auth_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    for sql in [
        include_str!("../migrations/auth/0001_create_tables.sql"),
        include_str!("../migrations/auth/0002_read_receipts.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

async fn insert_user(pool: &SqlitePool, id: &str, username: &str, role: &str, created_at: &str) {
    sqlx::query("INSERT INTO users (id, username, password_hash, role, created_at, updated_at) VALUES (?, ?, 'h', ?, ?, ?)")
        .bind(id).bind(username).bind(role).bind(created_at).bind(created_at)
        .execute(pool).await.unwrap();
}
```

- [ ] **Step 2: Failing tests**

Append:

```rust
#[tokio::test]
async fn backfill_assigns_owner_admin_member_by_role_and_age() {
    let auth = auth_pool().await;
    let chat = chat_pool().await;
    insert_user(&auth, "ua", "alice", "admin", "2026-01-01 00:00:00").await;
    insert_user(&auth, "ub", "bob",   "moderator", "2026-01-02 00:00:00").await;
    insert_user(&auth, "uc", "carol", "user", "2026-01-03 00:00:00").await;
    insert_user(&auth, "ud", "dave",  "admin", "2026-01-04 00:00:00").await;

    lets_chat::db::enclave::backfill_general_membership(&auth, &chat).await.unwrap();

    let general_id: i64 = sqlx::query("SELECT id FROM enclaves WHERE name='General'")
        .fetch_one(&chat).await.unwrap().get("id");

    let role_of = |uid: &'static str, chat: SqlitePool| async move {
        let r: String = sqlx::query("SELECT role FROM enclave_members WHERE enclave_id=(SELECT id FROM enclaves WHERE name='General') AND user_id=?")
            .bind(uid).fetch_one(&chat).await.unwrap().get("role");
        r
    };
    assert_eq!(role_of("ua", chat.clone()).await, "owner");
    assert_eq!(role_of("ub", chat.clone()).await, "member");
    assert_eq!(role_of("uc", chat.clone()).await, "member");
    assert_eq!(role_of("ud", chat.clone()).await, "admin");

    let created_by: String = sqlx::query("SELECT created_by FROM enclaves WHERE id=?")
        .bind(general_id).fetch_one(&chat).await.unwrap().get("created_by");
    assert_eq!(created_by, "ua");
}

#[tokio::test]
async fn backfill_idempotent() {
    let auth = auth_pool().await;
    let chat = chat_pool().await;
    insert_user(&auth, "ua", "alice", "admin", "2026-01-01 00:00:00").await;
    insert_user(&auth, "ub", "bob",   "user", "2026-01-02 00:00:00").await;
    lets_chat::db::enclave::backfill_general_membership(&auth, &chat).await.unwrap();
    lets_chat::db::enclave::backfill_general_membership(&auth, &chat).await.unwrap();
    let n: i64 = sqlx::query("SELECT COUNT(*) AS c FROM enclave_members").fetch_one(&chat).await.unwrap().get("c");
    assert_eq!(n, 2);
}

#[tokio::test]
async fn backfill_skips_when_no_admin() {
    let auth = auth_pool().await;
    let chat = chat_pool().await;
    insert_user(&auth, "ua", "alice", "user", "2026-01-01 00:00:00").await;
    lets_chat::db::enclave::backfill_general_membership(&auth, &chat).await.unwrap();
    let n: i64 = sqlx::query("SELECT COUNT(*) AS c FROM enclave_members").fetch_one(&chat).await.unwrap().get("c");
    assert_eq!(n, 0);
    let cb: String = sqlx::query("SELECT created_by FROM enclaves WHERE name='General'")
        .fetch_one(&chat).await.unwrap().get("created_by");
    assert_eq!(cb, "system");
}
```

- [ ] **Step 3: Run; FAIL**

- [ ] **Step 4: Implement**

Append to `server/src/db/enclave.rs`:

```rust
/// Idempotent. No-op when General has any members. Reads users from auth pool,
/// writes membership rows + General.created_by into chat pool.
pub async fn backfill_general_membership(
    auth: &SqlitePool,
    chat: &SqlitePool,
) -> Result<(), sqlx::Error> {
    let Some(general_row) = sqlx::query("SELECT id FROM enclaves WHERE name='General'")
        .fetch_optional(chat).await?
    else {
        return Ok(());
    };
    let general_id: i64 = general_row.get("id");

    let any_member = sqlx::query("SELECT 1 FROM enclave_members WHERE enclave_id=? LIMIT 1")
        .bind(general_id).fetch_optional(chat).await?;
    if any_member.is_some() {
        return Ok(());
    }

    let users = sqlx::query("SELECT id, role FROM users ORDER BY created_at ASC, id ASC")
        .fetch_all(auth).await?;
    let mut owner_id: Option<String> = None;
    for u in &users {
        let id: String = u.get("id");
        let role: String = u.get("role");
        let target_role = if role == "admin" {
            if owner_id.is_none() {
                owner_id = Some(id.clone());
                "owner"
            } else {
                "admin"
            }
        } else {
            "member"
        };
        sqlx::query("INSERT OR IGNORE INTO enclave_members (enclave_id, user_id, role) VALUES (?, ?, ?)")
            .bind(general_id).bind(&id).bind(target_role)
            .execute(chat).await?;
    }
    if let Some(o) = owner_id {
        sqlx::query("UPDATE enclaves SET created_by=? WHERE id=? AND created_by='system'")
            .bind(&o).bind(general_id).execute(chat).await?;
    }
    Ok(())
}
```

- [ ] **Step 5: Run; PASS**

- [ ] **Step 6: Commit**

```bash
git add server/src/db/enclave.rs server/tests/db_enclave.rs
git commit -m "feat(enclaves): backfill_general_membership reads auth users

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 11: Wire backfill into startup + first-user registration

**Files:**
- Modify: `server/src/main.rs`
- Modify: `server/src/routes/auth.rs`

- [ ] **Step 1: Update `main.rs`**

In `server/src/main.rs`, after the `AppState { ... }` literal but before `let app = ...`:

```rust
    if let Err(e) = db::enclave::backfill_general_membership(&state.auth, &state.chat).await {
        tracing::warn!(error = %e, "enclave backfill failed at startup");
    }
```

- [ ] **Step 2: Update `routes/auth.rs`**

Open `server/src/routes/auth.rs`. Locate the post-register branch where the first user is auto-promoted to `admin`. Immediately after the promotion (and before the redirect), call:

```rust
    if let Err(e) = crate::db::enclave::backfill_general_membership(&state.auth, &state.chat).await {
        tracing::warn!(error = %e, "enclave backfill after first registration failed");
    }
```

- [ ] **Step 3: Run the full test suite**

Run: `./dev/cargo test -p lets-chat-server`
Expected: PASS.

- [ ] **Step 4: Run `just check`**

Run: `just check`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add server/src/main.rs server/src/routes/auth.rs
git commit -m "feat(enclaves): run backfill at startup and after first-user promotion

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 12: `chat::create_room` adds `enclave_id`; existing call sites updated

**Files:**
- Modify: `server/src/db/chat.rs`
- Modify: every caller of `db::chat::create_room` (likely `routes/admin.rs` and tests).

- [ ] **Step 1: Find call sites**

Run: `grep -rn "db::chat::create_room\|chat::create_room\|create_room(" server/src server/tests`
Note every caller; each must add an `enclave_id: Option<i64>` argument.

- [ ] **Step 2: Modify the function signature**

In `server/src/db/chat.rs`, change `create_room`:

```rust
pub async fn create_room(
    pool: &sqlx::SqlitePool,
    name: &str,
    topic: Option<&str>,
    room_type: &str,
    invite_code: Option<&str>,
    enclave_id: Option<i64>,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query(
        "INSERT INTO rooms (name, topic, room_type, invite_code, enclave_id) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(name).bind(topic).bind(room_type).bind(invite_code).bind(enclave_id)
    .execute(pool).await?;
    Ok(result.last_insert_rowid())
}
```

- [ ] **Step 3: Update every caller**

- In `server/src/routes/admin.rs`, the existing public/private room creation goes into the General enclave for now: pass the General id. Look it up once in the handler with `db::enclave::get_general_id(&state.chat)` (you'll add this helper in step 4) and pass `Some(general_id)`.
- In `server/src/db/chat.rs` itself, the DM creator already uses a separate INSERT path; no change.
- Update any test that constructs a public/private room directly via `create_room` to pass `None` for DMs and `Some(general_id)` for non-DMs. For tests that just need a room and don't care about enclave, pass `Some(1)` after creating an enclave row in setup.

- [ ] **Step 4: Add `get_general_id` helper**

Append to `server/src/db/enclave.rs`:

```rust
pub async fn get_general_id(pool: &SqlitePool) -> Result<Option<i64>, sqlx::Error> {
    let row = sqlx::query("SELECT id FROM enclaves WHERE name='General'")
        .fetch_optional(pool).await?;
    Ok(row.map(|r| r.get::<i64,_>("id")))
}
```

- [ ] **Step 5: Run all tests + check**

Run: `./dev/cargo test -p lets-chat-server`
Run: `just check`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(enclaves): create_room takes enclave_id; admin room create uses General

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 13: `is_room_accessible` predicate

**Files:**
- Modify: `server/src/db/chat.rs`
- Test: `server/tests/db_enclave.rs`

- [ ] **Step 1: Failing tests**

Append to `server/tests/db_enclave.rs`:

```rust
#[tokio::test]
async fn is_room_accessible_admin_godmode() {
    let pool = chat_pool().await;
    let general: i64 = sqlx::query("SELECT id FROM enclaves WHERE name='General'")
        .fetch_one(&pool).await.unwrap().get("id");
    let room_id = lets_chat::db::chat::create_room(&pool, "private", None, "private", None, Some(general)).await.unwrap();
    let ok = lets_chat::db::chat::is_room_accessible(&pool, room_id, "outsider", true).await.unwrap();
    assert!(ok);
}

#[tokio::test]
async fn is_room_accessible_enclave_member_open_room() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "u1").await.unwrap();
    let room = lets_chat::db::chat::create_room(&pool, "open", None, "public", None, Some(id)).await.unwrap();
    assert!(lets_chat::db::chat::is_room_accessible(&pool, room, "u1", false).await.unwrap());
    assert!(!lets_chat::db::chat::is_room_accessible(&pool, room, "stranger", false).await.unwrap());
}

#[tokio::test]
async fn is_room_accessible_private_requires_room_member() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "u1").await.unwrap();
    lets_chat::db::enclave::add_member(&pool, id, "u2", lets_chat::models::enclave::EnclaveRole::Member).await.unwrap();
    let room = lets_chat::db::chat::create_room(&pool, "secret", None, "private", None, Some(id)).await.unwrap();
    assert!(!lets_chat::db::chat::is_room_accessible(&pool, room, "u2", false).await.unwrap());
    lets_chat::db::chat::add_room_member(&pool, room, "u2").await.unwrap();
    assert!(lets_chat::db::chat::is_room_accessible(&pool, room, "u2", false).await.unwrap());
}

#[tokio::test]
async fn is_room_accessible_dm_via_room_members() {
    let pool = chat_pool().await;
    let _ = lets_chat::db::chat::create_dm_room(&pool, "dm", "u1", "u2").await.unwrap();
    let row = sqlx::query("SELECT id FROM rooms WHERE room_type='dm'").fetch_one(&pool).await.unwrap();
    let dm_id: i64 = row.get("id");
    assert!(lets_chat::db::chat::is_room_accessible(&pool, dm_id, "u1", false).await.unwrap());
    assert!(!lets_chat::db::chat::is_room_accessible(&pool, dm_id, "u3", false).await.unwrap());
}
```

- [ ] **Step 2: Run; FAIL**

- [ ] **Step 3: Implement**

Append to `server/src/db/chat.rs`:

```rust
/// Predicate combining DM, public-in-enclave, and private-room rules.
/// `is_site_admin` short-circuits to true.
pub async fn is_room_accessible(
    pool: &sqlx::SqlitePool,
    room_id: i64,
    user_id: &str,
    is_site_admin: bool,
) -> Result<bool, sqlx::Error> {
    if is_site_admin {
        return Ok(true);
    }
    let row = sqlx::query("SELECT room_type, enclave_id FROM rooms WHERE id=?")
        .bind(room_id).fetch_optional(pool).await?;
    let Some(r) = row else { return Ok(false); };
    let room_type: String = r.get("room_type");
    let enclave_id: Option<i64> = r.get("enclave_id");

    if room_type == "dm" {
        return is_room_member(pool, room_id, user_id).await;
    }

    let Some(eid) = enclave_id else { return Ok(false); };
    let in_enclave = sqlx::query("SELECT 1 FROM enclave_members WHERE enclave_id=? AND user_id=?")
        .bind(eid).bind(user_id).fetch_optional(pool).await?.is_some();
    if !in_enclave {
        return Ok(false);
    }

    if room_type == "public" {
        return Ok(true);
    }
    is_room_member(pool, room_id, user_id).await
}
```

- [ ] **Step 4: Run; PASS**

- [ ] **Step 5: Commit**

```bash
git add server/src/db/chat.rs server/tests/db_enclave.rs
git commit -m "feat(enclaves): is_room_accessible covers DM, public, private + site admin godmode

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 14: `list_rooms_in_enclave` and unread filter

**Files:**
- Modify: `server/src/db/chat.rs`
- Test: `server/tests/db_enclave.rs`

- [ ] **Step 1: Failing test**

Append:

```rust
#[tokio::test]
async fn list_rooms_in_enclave_returns_visible_rooms() {
    let pool = chat_pool().await;
    let id = lets_chat::db::enclave::create_enclave(&pool, "x", None, "u1").await.unwrap();
    let _open = lets_chat::db::chat::create_room(&pool, "open", None, "public", None, Some(id)).await.unwrap();
    let secret = lets_chat::db::chat::create_room(&pool, "secret", None, "private", None, Some(id)).await.unwrap();
    let _other_enclave = {
        let oid = lets_chat::db::enclave::create_enclave(&pool, "other", None, "stranger").await.unwrap();
        lets_chat::db::chat::create_room(&pool, "other-room", None, "public", None, Some(oid)).await.unwrap()
    };
    lets_chat::db::enclave::add_member(&pool, id, "u2", lets_chat::models::enclave::EnclaveRole::Member).await.unwrap();
    let visible = lets_chat::db::chat::list_rooms_in_enclave(&pool, id, "u2", false).await.unwrap();
    assert_eq!(visible.iter().map(|r| r.name.clone()).collect::<Vec<_>>(), vec!["open"]);
    lets_chat::db::chat::add_room_member(&pool, secret, "u2").await.unwrap();
    let visible2 = lets_chat::db::chat::list_rooms_in_enclave(&pool, id, "u2", false).await.unwrap();
    let names: Vec<String> = visible2.iter().map(|r| r.name.clone()).collect();
    assert!(names.contains(&"open".to_string()));
    assert!(names.contains(&"secret".to_string()));
}
```

- [ ] **Step 2: Run; FAIL**

- [ ] **Step 3: Implement**

Append to `server/src/db/chat.rs` (and remove the now-unused `list_rooms` body or adapt it to call the new helper for the General enclave during the transition):

```rust
pub async fn list_rooms_in_enclave(
    pool: &sqlx::SqlitePool,
    enclave_id: i64,
    user_id: &str,
    can_see_all_private: bool,
) -> Result<Vec<Room>, sqlx::Error> {
    if can_see_all_private {
        let rows = sqlx::query(
            "SELECT id, name, topic, room_type, invite_code, created_at \
             FROM rooms WHERE enclave_id=? AND room_type != 'dm' ORDER BY name",
        ).bind(enclave_id).fetch_all(pool).await?;
        return Ok(rows.iter().map(map_room).collect());
    }
    let rows = sqlx::query(
        "SELECT r.id, r.name, r.topic, r.room_type, r.invite_code, r.created_at \
         FROM rooms r \
         LEFT JOIN room_members m ON m.room_id = r.id AND m.user_id = ? \
         WHERE r.enclave_id=? AND r.room_type != 'dm' \
           AND (r.room_type='public' OR m.user_id IS NOT NULL) \
         ORDER BY r.name",
    ).bind(user_id).bind(enclave_id).fetch_all(pool).await?;
    Ok(rows.iter().map(map_room).collect())
}
```

Leave the old `list_rooms` in place for now; it stays correct because every non-DM row carries an `enclave_id` after migration. Phase 2 deletes it once callers move to the new helper.

- [ ] **Step 4: Run; PASS**

- [ ] **Step 5: Commit**

```bash
git add server/src/db/chat.rs server/tests/db_enclave.rs
git commit -m "feat(enclaves): list_rooms_in_enclave honors enclave + private-membership rule

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 15: Permission helpers

**Files:**
- Create: `server/src/perms.rs`
- Modify: `server/src/lib.rs`
- Test: `server/tests/perms.rs`

- [ ] **Step 1: Failing tests**

Create `server/tests/perms.rs`:

```rust
use lets_chat::models::enclave::EnclaveRole;
use lets_chat::perms::{
    enclave_can_add_room, enclave_can_delete, enclave_can_invite,
    enclave_can_manage, enclave_can_manage_admins,
};

#[test]
fn site_admin_godmode_short_circuits_every_check() {
    for f in [enclave_can_manage, enclave_can_delete, enclave_can_invite,
              enclave_can_add_room, enclave_can_manage_admins] {
        assert!(f(None, "admin"));
        assert!(f(Some(EnclaveRole::Member), "admin"));
    }
}

#[test]
fn member_only_reads() {
    let r = Some(EnclaveRole::Member);
    assert!(!enclave_can_manage(r, "user"));
    assert!(!enclave_can_delete(r, "user"));
    assert!(!enclave_can_invite(r, "user"));
    assert!(!enclave_can_add_room(r, "user"));
    assert!(!enclave_can_manage_admins(r, "user"));
}

#[test]
fn admin_can_manage_invite_addroom_but_not_delete_or_admin_mgmt() {
    let r = Some(EnclaveRole::Admin);
    assert!(enclave_can_manage(r, "user"));
    assert!(enclave_can_invite(r, "user"));
    assert!(enclave_can_add_room(r, "user"));
    assert!(!enclave_can_delete(r, "user"));
    assert!(!enclave_can_manage_admins(r, "user"));
}

#[test]
fn owner_can_do_everything() {
    let r = Some(EnclaveRole::Owner);
    assert!(enclave_can_manage(r, "user"));
    assert!(enclave_can_invite(r, "user"));
    assert!(enclave_can_add_room(r, "user"));
    assert!(enclave_can_delete(r, "user"));
    assert!(enclave_can_manage_admins(r, "user"));
}

#[test]
fn no_membership_no_powers() {
    assert!(!enclave_can_manage(None, "user"));
    assert!(!enclave_can_delete(None, "user"));
}
```

- [ ] **Step 2: Run; FAIL**

- [ ] **Step 3: Implement**

```rust
// server/src/perms.rs
use crate::models::enclave::EnclaveRole;

fn is_site_admin(site_role: &str) -> bool { site_role == "admin" }

pub fn enclave_can_manage(role: Option<EnclaveRole>, site_role: &str) -> bool {
    if is_site_admin(site_role) { return true; }
    matches!(role, Some(EnclaveRole::Owner | EnclaveRole::Admin))
}

pub fn enclave_can_delete(role: Option<EnclaveRole>, site_role: &str) -> bool {
    if is_site_admin(site_role) { return true; }
    matches!(role, Some(EnclaveRole::Owner))
}

pub fn enclave_can_invite(role: Option<EnclaveRole>, site_role: &str) -> bool {
    enclave_can_manage(role, site_role)
}

pub fn enclave_can_add_room(role: Option<EnclaveRole>, site_role: &str) -> bool {
    enclave_can_manage(role, site_role)
}

pub fn enclave_can_manage_admins(role: Option<EnclaveRole>, site_role: &str) -> bool {
    if is_site_admin(site_role) { return true; }
    matches!(role, Some(EnclaveRole::Owner))
}
```

- [ ] **Step 4: Re-export from `lib.rs`**

Open `server/src/lib.rs` and add `pub mod perms;` near the other `pub mod` lines.

- [ ] **Step 5: Run; PASS**

Run: `./dev/cargo test -p lets-chat-server --test perms`
Expected: PASS.

- [ ] **Step 6: Run full check**

Run: `just check`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add server/src/perms.rs server/src/lib.rs server/tests/perms.rs
git commit -m "feat(enclaves): permission helpers with site-admin godmode

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Phase 1 Done

Sanity gates:

- `./dev/cargo test -p lets-chat-server` is green.
- `just check` is green.
- The branch `feat/enclaves` carries: spec, schema migration, models, full `db::enclave`, backfill wired into startup + first registration, `is_room_accessible`, `list_rooms_in_enclave`, perms helpers.
- The running app behaves identically to before — no UI change, no new routes yet.

Next: Phase 2 (`2026-05-05-enclaves-phase2-routes.md`).
