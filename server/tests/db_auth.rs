use sqlx::SqlitePool;

mod common;

async fn setup_pool() -> SqlitePool {
    common::auth_pool().await
}

#[tokio::test]
async fn test_create_user_and_find_by_username() {
    let pool = setup_pool().await;
    let user_id = lets_chat::db::auth::create_user(&pool, "alice", "hashed_pw_placeholder")
        .await
        .expect("Failed to create user");
    assert!(!user_id.is_empty());

    let found = lets_chat::db::auth::find_user_by_username(&pool, "alice")
        .await
        .expect("Failed to find user");
    assert!(found.is_some());
    let user = found.unwrap();
    assert_eq!(user.username, "alice");
    assert_eq!(user.role, "user");
    assert!(!user.is_banned);
}

#[tokio::test]
async fn mark_email_verified_if_unset_is_idempotent() {
    // LC-627: SSO users are provisioned with a NULL email_verified_at; the
    // callback stamps it on login so the remote-control gate stops refusing them.
    let pool = setup_pool().await;
    let user_id = lets_chat::db::auth::create_user(&pool, "sso-user", "")
        .await
        .expect("create user");
    lets_chat::db::auth::set_user_email(&pool, &user_id, Some("sso@example.com"))
        .await
        .expect("set email");

    // Fresh user: verified stamp is absent.
    assert!(
        lets_chat::db::auth::get_user_email_verified_at(&pool, &user_id)
            .await
            .expect("read")
            .is_none()
    );

    // First stamp writes the row.
    let n = lets_chat::db::auth::mark_email_verified_if_unset(&pool, &user_id, "sso@example.com")
        .await
        .expect("stamp");
    assert_eq!(n, 1, "first stamp updates one row");
    let first = lets_chat::db::auth::get_user_email_verified_at(&pool, &user_id)
        .await
        .expect("read")
        .expect("now verified");

    // Second stamp is a no-op (IS NULL guard), so the timestamp does not churn.
    let n2 = lets_chat::db::auth::mark_email_verified_if_unset(&pool, &user_id, "sso@example.com")
        .await
        .expect("stamp again");
    assert_eq!(n2, 0, "already-verified user is not re-stamped");
    let second = lets_chat::db::auth::get_user_email_verified_at(&pool, &user_id)
        .await
        .expect("read")
        .expect("still verified");
    assert_eq!(first, second, "timestamp is unchanged on the no-op path");
}

#[tokio::test]
async fn sync_bunyip_display_name_mirrors_idp_over_local_and_skips_noops() {
    // LC-762: an SSO user's display name is provisioned from the OIDC `name`
    // claim, but the resolver returns an existing linked row untouched on
    // re-login, so a name changed at the IdP stayed stale. The callback now
    // mirrors the fresh claim. Bunyip is the authority for identity, so the IdP
    // name wins - even over a display name the user edited locally.
    let pool = setup_pool().await;
    let user_id = lets_chat::db::auth::create_user_from_bunyip(
        &pool,
        "sso-user",
        "sub-123",
        Some("Old Name"),
        Some("sso@example.com"),
    )
    .await
    .expect("provision");

    async fn read_display_name(pool: &SqlitePool, id: &str) -> Option<String> {
        lets_chat::db::auth::find_user_by_id(pool, id)
            .await
            .expect("read")
            .expect("user exists")
            .display_name
    }

    // Provisioned with the first claim.
    assert_eq!(
        read_display_name(&pool, &user_id).await.as_deref(),
        Some("Old Name")
    );

    // Re-login with the SAME name is a no-op: no write, no `updated_at` churn.
    let n = lets_chat::db::auth::sync_bunyip_display_name(&pool, &user_id, "Old Name")
        .await
        .expect("sync");
    assert_eq!(n, 0, "unchanged name writes no row");

    // The IdP name changed: the next login mirrors it.
    let n = lets_chat::db::auth::sync_bunyip_display_name(&pool, &user_id, "New Name")
        .await
        .expect("sync");
    assert_eq!(n, 1, "changed name updates one row");
    assert_eq!(
        read_display_name(&pool, &user_id).await.as_deref(),
        Some("New Name")
    );

    // Policy (LC-762): the IdP name wins even over a local settings edit.
    sqlx::query("UPDATE users SET display_name = 'Custom Local' WHERE id = ?")
        .bind(&user_id)
        .execute(&pool)
        .await
        .expect("simulate a local display-name edit");
    let n = lets_chat::db::auth::sync_bunyip_display_name(&pool, &user_id, "Directory Name")
        .await
        .expect("sync");
    assert_eq!(n, 1, "a local edit does not shield the row from the IdP");
    assert_eq!(
        read_display_name(&pool, &user_id).await.as_deref(),
        Some("Directory Name"),
        "the IdP name wins over a local edit"
    );
}

#[tokio::test]
async fn test_create_user_duplicate_username_fails() {
    let pool = setup_pool().await;
    lets_chat::db::auth::create_user(&pool, "alice", "hash1")
        .await
        .expect("First create should succeed");
    let result = lets_chat::db::auth::create_user(&pool, "alice", "hash2").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_username_case_insensitive() {
    let pool = setup_pool().await;
    lets_chat::db::auth::create_user(&pool, "Alice", "hash1")
        .await
        .expect("Create should succeed");
    let found = lets_chat::db::auth::find_user_by_username(&pool, "alice")
        .await
        .expect("Lookup should succeed");
    assert!(found.is_some());
    assert_eq!(found.unwrap().username, "Alice");
}

#[tokio::test]
async fn test_count_users() {
    let pool = setup_pool().await;
    let count = lets_chat::db::auth::count_users(&pool)
        .await
        .expect("Count should work");
    assert_eq!(count, 0);
    lets_chat::db::auth::create_user(&pool, "alice", "hash1")
        .await
        .unwrap();
    let count = lets_chat::db::auth::count_users(&pool)
        .await
        .expect("Count should work");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn test_set_user_role() {
    let pool = setup_pool().await;
    let user_id = lets_chat::db::auth::create_user(&pool, "alice", "hash1")
        .await
        .unwrap();
    lets_chat::db::auth::set_user_role(&pool, &user_id, "admin")
        .await
        .expect("Set role should work");
    let user = lets_chat::db::auth::find_user_by_username(&pool, "alice")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.role, "admin");
}

#[tokio::test]
async fn test_create_and_validate_session() {
    let pool = setup_pool().await;
    let user_id = lets_chat::db::auth::create_user(&pool, "alice", "hash1")
        .await
        .unwrap();
    let session_id = lets_chat::db::auth::create_session(&pool, &user_id)
        .await
        .expect("Create session should work");
    assert!(!session_id.is_empty());
    let session_user = lets_chat::db::auth::get_user_by_session(&pool, &session_id)
        .await
        .expect("Get session user should work");
    assert!(session_user.is_some());
    assert_eq!(session_user.unwrap().username, "alice");
}

#[tokio::test]
async fn test_delete_session() {
    let pool = setup_pool().await;
    let user_id = lets_chat::db::auth::create_user(&pool, "alice", "hash1")
        .await
        .unwrap();
    let session_id = lets_chat::db::auth::create_session(&pool, &user_id)
        .await
        .unwrap();
    lets_chat::db::auth::delete_session(&pool, &session_id)
        .await
        .expect("Delete session should work");
    let session_user = lets_chat::db::auth::get_user_by_session(&pool, &session_id)
        .await
        .unwrap();
    assert!(session_user.is_none());
}

#[tokio::test]
async fn test_expired_session_returns_none() {
    let pool = setup_pool().await;
    let user_id = lets_chat::db::auth::create_user(&pool, "alice", "hash1")
        .await
        .unwrap();
    let session_id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO sessions (id, user_id, expires_at) VALUES (?, ?, datetime('now', '-1 hour'))",
    )
    .bind(&session_id)
    .bind(&user_id)
    .execute(&pool)
    .await
    .unwrap();
    let session_user = lets_chat::db::auth::get_user_by_session(&pool, &session_id)
        .await
        .unwrap();
    assert!(session_user.is_none());
}

#[tokio::test]
async fn test_search_users_matches_username_substring_case_insensitive() {
    let pool = setup_pool().await;
    lets_chat::db::auth::create_user(&pool, "Alice", "h")
        .await
        .unwrap();
    lets_chat::db::auth::create_user(&pool, "bob", "h")
        .await
        .unwrap();
    lets_chat::db::auth::create_user(&pool, "carol", "h")
        .await
        .unwrap();

    let hits = lets_chat::db::auth::search_users(&pool, "ali", "viewer", 50)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].username, "Alice");
}

// LC-533: update_user_profile persists and later clears the three profile
// extras (pronouns / links / timezone) alongside display_name + bio.
#[tokio::test]
async fn test_update_user_profile_round_trips_extras() {
    let pool = setup_pool().await;
    let id = lets_chat::db::auth::create_user(&pool, "prof", "h")
        .await
        .unwrap();

    lets_chat::db::auth::update_user_profile(
        &pool,
        &id,
        Some("Prof Ile"),
        Some("hi"),
        Some("they/them"),
        Some("https://a.example\nhttps://b.example"),
        Some("America/New_York"),
    )
    .await
    .unwrap();

    let rec = lets_chat::db::auth::find_user_by_id(&pool, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rec.pronouns.as_deref(), Some("they/them"));
    assert_eq!(
        rec.profile_links.as_deref(),
        Some("https://a.example\nhttps://b.example")
    );
    assert_eq!(rec.timezone.as_deref(), Some("America/New_York"));

    // Clearing every extra nulls the columns (does not leave stale values).
    lets_chat::db::auth::update_user_profile(&pool, &id, Some("Prof Ile"), None, None, None, None)
        .await
        .unwrap();
    let rec = lets_chat::db::auth::find_user_by_id(&pool, &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rec.pronouns, None);
    assert_eq!(rec.profile_links, None);
    assert_eq!(rec.timezone, None);
}

#[tokio::test]
async fn test_search_users_matches_display_name() {
    let pool = setup_pool().await;
    let id = lets_chat::db::auth::create_user(&pool, "u1", "h")
        .await
        .unwrap();
    lets_chat::db::auth::update_user_profile(&pool, &id, Some("Jane Doe"), None, None, None, None)
        .await
        .unwrap();
    lets_chat::db::auth::create_user(&pool, "other", "h")
        .await
        .unwrap();

    let hits = lets_chat::db::auth::search_users(&pool, "jane", "viewer", 50)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, id);
}

#[tokio::test]
async fn test_search_users_excludes_banned() {
    let pool = setup_pool().await;
    let id = lets_chat::db::auth::create_user(&pool, "alice", "h")
        .await
        .unwrap();
    lets_chat::db::auth::ban_user(&pool, &id, Some("spam"))
        .await
        .unwrap();

    let hits = lets_chat::db::auth::search_users(&pool, "ali", "viewer", 50)
        .await
        .unwrap();
    assert!(hits.is_empty());
}

#[tokio::test]
async fn test_search_users_escapes_like_wildcards() {
    let pool = setup_pool().await;
    lets_chat::db::auth::create_user(&pool, "alice", "h")
        .await
        .unwrap();
    lets_chat::db::auth::create_user(&pool, "bob", "h")
        .await
        .unwrap();

    // `%` would match every row if not escaped; with escaping it matches nothing.
    let hits = lets_chat::db::auth::search_users(&pool, "%", "viewer", 50)
        .await
        .unwrap();
    assert!(hits.is_empty());

    let hits = lets_chat::db::auth::search_users(&pool, "_", "viewer", 50)
        .await
        .unwrap();
    assert!(hits.is_empty());
}

#[tokio::test]
async fn test_search_users_respects_limit() {
    let pool = setup_pool().await;
    for n in 0..5 {
        lets_chat::db::auth::create_user(&pool, &format!("user{n}"), "h")
            .await
            .unwrap();
    }
    let hits = lets_chat::db::auth::search_users(&pool, "user", "viewer", 3)
        .await
        .unwrap();
    assert_eq!(hits.len(), 3);
}

#[tokio::test]
async fn test_search_users_excludes_private_profiles() {
    let pool = setup_pool().await;
    let alice_id = lets_chat::db::auth::create_user(&pool, "alice", "h")
        .await
        .unwrap();
    let _bob_id = lets_chat::db::auth::create_user(&pool, "alicia", "h")
        .await
        .unwrap();
    lets_chat::db::auth::set_profile_public(&pool, &alice_id, false)
        .await
        .unwrap();

    let hits = lets_chat::db::auth::search_users(&pool, "ali", "viewer", 50)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].username, "alicia");
}

#[tokio::test]
async fn test_block_and_unblock_round_trip() {
    let pool = setup_pool().await;
    let a = lets_chat::db::auth::create_user(&pool, "alice", "h")
        .await
        .unwrap();
    let b = lets_chat::db::auth::create_user(&pool, "bob", "h")
        .await
        .unwrap();

    assert!(!lets_chat::db::auth::did_block(&pool, &a, &b).await.unwrap());
    assert!(!lets_chat::db::auth::is_blocked_either_way(&pool, &a, &b)
        .await
        .unwrap());

    lets_chat::db::auth::block_user(&pool, &a, &b)
        .await
        .unwrap();
    assert!(lets_chat::db::auth::did_block(&pool, &a, &b).await.unwrap());
    assert!(lets_chat::db::auth::is_blocked_either_way(&pool, &a, &b)
        .await
        .unwrap());
    assert!(lets_chat::db::auth::is_blocked_either_way(&pool, &b, &a)
        .await
        .unwrap());

    // Idempotent.
    lets_chat::db::auth::block_user(&pool, &a, &b)
        .await
        .unwrap();
    let blocked = lets_chat::db::auth::list_blocked_users(&pool, &a)
        .await
        .unwrap();
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0].id, b);

    lets_chat::db::auth::unblock_user(&pool, &a, &b)
        .await
        .unwrap();
    assert!(!lets_chat::db::auth::did_block(&pool, &a, &b).await.unwrap());
    assert!(!lets_chat::db::auth::is_blocked_either_way(&pool, &a, &b)
        .await
        .unwrap());
}

#[tokio::test]
async fn test_list_blocked_ids_either_way_includes_both_directions() {
    let pool = setup_pool().await;
    let viewer = lets_chat::db::auth::create_user(&pool, "viewer", "h")
        .await
        .unwrap();
    let blocked_by_viewer = lets_chat::db::auth::create_user(&pool, "alice", "h")
        .await
        .unwrap();
    let blocks_viewer = lets_chat::db::auth::create_user(&pool, "bob", "h")
        .await
        .unwrap();
    let unrelated = lets_chat::db::auth::create_user(&pool, "carol", "h")
        .await
        .unwrap();

    lets_chat::db::auth::block_user(&pool, &viewer, &blocked_by_viewer)
        .await
        .unwrap();
    lets_chat::db::auth::block_user(&pool, &blocks_viewer, &viewer)
        .await
        .unwrap();

    let ids = lets_chat::db::auth::list_blocked_ids_either_way(&pool, &viewer)
        .await
        .unwrap();
    assert!(ids.contains(&blocked_by_viewer));
    assert!(ids.contains(&blocks_viewer));
    assert!(!ids.contains(&unrelated));
    assert_eq!(ids.len(), 2);
}

#[tokio::test]
async fn test_search_excludes_blocked_either_direction() {
    let pool = setup_pool().await;
    let viewer = lets_chat::db::auth::create_user(&pool, "viewer", "h")
        .await
        .unwrap();
    let blocked_by_viewer = lets_chat::db::auth::create_user(&pool, "alice", "h")
        .await
        .unwrap();
    let blocks_viewer = lets_chat::db::auth::create_user(&pool, "alicia", "h")
        .await
        .unwrap();
    let _other = lets_chat::db::auth::create_user(&pool, "alma", "h")
        .await
        .unwrap();

    lets_chat::db::auth::block_user(&pool, &viewer, &blocked_by_viewer)
        .await
        .unwrap();
    lets_chat::db::auth::block_user(&pool, &blocks_viewer, &viewer)
        .await
        .unwrap();

    let hits = lets_chat::db::auth::search_users(&pool, "al", &viewer, 50)
        .await
        .unwrap();
    let names: Vec<&str> = hits.iter().map(|u| u.username.as_str()).collect();
    assert_eq!(names, vec!["alma"]);
}

#[tokio::test]
async fn test_search_users_self_visible_when_private() {
    let pool = setup_pool().await;
    let alice_id = lets_chat::db::auth::create_user(&pool, "alice", "h")
        .await
        .unwrap();
    lets_chat::db::auth::set_profile_public(&pool, &alice_id, false)
        .await
        .unwrap();

    let hits = lets_chat::db::auth::search_users(&pool, "ali", &alice_id, 50)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, alice_id);
}

/// Strip a row's `bunyip_sub` so it looks like a pre-SSO (LC-588) account. The
/// `create_user` test helper synthesizes a placeholder sub, which the LC-698
/// guard correctly treats as already linked. Unlinked is the empty string: the
/// column is `NOT NULL DEFAULT ''` (migration 0029).
async fn unlink(pool: &SqlitePool, user_id: &str) {
    sqlx::query("UPDATE users SET bunyip_sub = '' WHERE id = ?")
        .bind(user_id)
        .execute(pool)
        .await
        .unwrap();
}

// LC-698 (restores the LC-588 guard that LC-618 dropped): a row already linked
// to a DIFFERENT subject is never relinked. The UPDATE must match zero rows, so
// an email string - which any user can set on their own profile, unverified -
// can never re-point an incoming identity onto someone else's account.
#[tokio::test]
async fn test_link_bunyip_sub_refuses_already_linked_row() {
    let pool = setup_pool().await;
    let id = lets_chat::db::auth::create_user_from_bunyip(
        &pool,
        "mallory",
        "sub-mallory",
        Some("Mallory"),
        Some("newhire@example.com"),
    )
    .await
    .unwrap();

    let linked = lets_chat::db::auth::link_bunyip_sub(&pool, &id, "sub-newhire")
        .await
        .unwrap();
    assert!(!linked, "an already-linked row must not be relinked");

    // The row still answers to its original sub only: nothing was written.
    assert_eq!(
        lets_chat::db::auth::get_user_auth_flags_by_bunyip_sub(&pool, "sub-mallory")
            .await
            .unwrap(),
        Some((id, false, false)),
    );
    assert!(
        lets_chat::db::auth::get_user_auth_flags_by_bunyip_sub(&pool, "sub-newhire")
            .await
            .unwrap()
            .is_none(),
        "the incoming sub was not bound to the existing row",
    );
}

// LC-588 (must not regress under LC-698): an UNLINKED row is still linked, so a
// pre-SSO account can be claimed by its owner's first SSO login.
#[tokio::test]
async fn test_link_bunyip_sub_links_unlinked_row() {
    let pool = setup_pool().await;
    let id = lets_chat::db::auth::create_user(&pool, "alice", "h")
        .await
        .unwrap();
    unlink(&pool, &id).await;

    let linked = lets_chat::db::auth::link_bunyip_sub(&pool, &id, "sub-alice")
        .await
        .unwrap();
    assert!(linked, "an unlinked row is still adoptable");
    assert_eq!(
        lets_chat::db::auth::get_user_auth_flags_by_bunyip_sub(&pool, "sub-alice")
            .await
            .unwrap(),
        Some((id, false, false)),
    );
}

// LC-698: linking a row deletes its sessions, so a link can never leave a
// previous holder's cookie live on the account.
#[tokio::test]
async fn test_link_bunyip_sub_revokes_sessions() {
    let pool = setup_pool().await;
    let id = lets_chat::db::auth::create_user(&pool, "alice", "h")
        .await
        .unwrap();
    unlink(&pool, &id).await;
    let token = lets_chat::db::auth::create_session(&pool, &id)
        .await
        .unwrap();
    assert!(
        lets_chat::db::auth::get_user_by_session(&pool, &token)
            .await
            .unwrap()
            .is_some(),
        "precondition: the session is live",
    );

    assert!(
        lets_chat::db::auth::link_bunyip_sub(&pool, &id, "sub-alice")
            .await
            .unwrap()
    );

    assert!(
        lets_chat::db::auth::get_user_by_session(&pool, &token)
            .await
            .unwrap()
            .is_none(),
        "the pre-link session must be revoked",
    );
}

// LC-698: the admin relink path. Clearing the sub drops the row's sessions and
// makes it linkable again, so the next login adopts it through link_bunyip_sub.
#[tokio::test]
async fn test_clear_bunyip_sub_unlinks_and_revokes_sessions() {
    let pool = setup_pool().await;
    let id = lets_chat::db::auth::create_user_from_bunyip(
        &pool,
        "alice",
        "sub-old",
        Some("Alice"),
        Some("alice@example.com"),
    )
    .await
    .unwrap();
    let token = lets_chat::db::auth::create_session(&pool, &id)
        .await
        .unwrap();

    assert!(lets_chat::db::auth::clear_bunyip_sub(&pool, &id)
        .await
        .unwrap());

    assert!(
        lets_chat::db::auth::get_user_auth_flags_by_bunyip_sub(&pool, "sub-old")
            .await
            .unwrap()
            .is_none(),
        "the old sub no longer resolves",
    );
    assert!(
        lets_chat::db::auth::get_user_by_session(&pool, &token)
            .await
            .unwrap()
            .is_none(),
        "clearing the sub revokes the row's sessions",
    );

    // The next login links the rotated sub onto the same row.
    assert!(lets_chat::db::auth::link_bunyip_sub(&pool, &id, "sub-new")
        .await
        .unwrap());
    assert_eq!(
        lets_chat::db::auth::get_user_auth_flags_by_bunyip_sub(&pool, "sub-new")
            .await
            .unwrap(),
        Some((id, false, false)),
    );

    // A second clear is a no-op, so the caller can report "nothing to unlink".
    let repeat = lets_chat::db::auth::clear_bunyip_sub(&pool, "nobody")
        .await
        .unwrap();
    assert!(!repeat);
}

// LC-618: bot rows are never linked. link_bunyip_sub returns false so the
// callback raises an identity conflict rather than hijacking a bot identity.
// clear_bunyip_sub refuses them for the same reason (a bot's placeholder sub is
// what keeps UNIQUE(bunyip_sub) satisfied).
#[tokio::test]
async fn test_link_bunyip_sub_refuses_bot_row() {
    let pool = setup_pool().await;
    let bot_id = lets_chat::db::auth::create_bot(&pool, "botty")
        .await
        .unwrap();

    let linked = lets_chat::db::auth::link_bunyip_sub(&pool, &bot_id, "sub-x")
        .await
        .unwrap();
    assert!(!linked);
    assert!(!lets_chat::db::auth::clear_bunyip_sub(&pool, &bot_id)
        .await
        .unwrap());

    // "sub-x" was not written anywhere: the bot's placeholder sub is untouched.
    assert!(
        lets_chat::db::auth::get_user_auth_flags_by_bunyip_sub(&pool, "sub-x")
            .await
            .unwrap()
            .is_none()
    );
}

// LC-698: find_email_owner reports the flags the SSO resolver classifies on -
// unlike the LC-588 lookup it replaces, a banned row is returned (so the caller
// can refuse) rather than filtered (which let the caller fall through to a
// colliding INSERT).
#[tokio::test]
async fn test_find_email_owner_reports_link_and_verified_state() {
    let pool = setup_pool().await;
    let id = lets_chat::db::auth::create_user_from_bunyip(
        &pool,
        "alice",
        "sub-alice",
        Some("Alice"),
        Some("alice@example.com"),
    )
    .await
    .unwrap();

    // Provisioning does not stamp the verified flag, and the match is
    // case-insensitive.
    let owner = lets_chat::db::auth::find_email_owner(&pool, "ALICE@example.com")
        .await
        .unwrap()
        .expect("row owns the address");
    assert_eq!(owner.id, id);
    assert_eq!(owner.bunyip_sub.as_deref(), Some("sub-alice"));
    assert!(!owner.email_verified);
    assert!(!owner.is_bot);
    assert!(!owner.is_banned);

    lets_chat::db::auth::mark_email_verified_if_unset(&pool, &id, "alice@example.com")
        .await
        .unwrap();
    unlink(&pool, &id).await;
    let owner = lets_chat::db::auth::find_email_owner(&pool, "alice@example.com")
        .await
        .unwrap()
        .expect("row owns the address");
    assert!(owner.email_verified);
    assert!(
        owner.bunyip_sub.is_none(),
        "NULL and empty both read as unlinked",
    );

    assert!(
        lets_chat::db::auth::find_email_owner(&pool, "nobody@example.com")
            .await
            .unwrap()
            .is_none()
    );
}

// LC-698: the verified stamp asserts that THIS address was vouched for, so it
// never lands on a row holding a different (self-service, unverified) address.
#[tokio::test]
async fn test_mark_email_verified_if_unset_requires_matching_email() {
    let pool = setup_pool().await;
    let id = lets_chat::db::auth::create_user(&pool, "mallory", "h")
        .await
        .unwrap();
    lets_chat::db::auth::set_user_email(&pool, &id, Some("newhire@example.com"))
        .await
        .unwrap();

    // Mallory's own login (her bunyip address is mallory@example.com) must not
    // stamp the stranger's address she typed into her profile.
    let n = lets_chat::db::auth::mark_email_verified_if_unset(&pool, &id, "mallory@example.com")
        .await
        .unwrap();
    assert_eq!(n, 0, "a non-matching address is never stamped verified");
    assert!(lets_chat::db::auth::get_user_email_verified_at(&pool, &id)
        .await
        .unwrap()
        .is_none());
}
