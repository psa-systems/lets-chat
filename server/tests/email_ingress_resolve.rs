//! LC-77-REPLY commit 2a: unit tests for the namespace fork in
//! `email_ingress::resolve::resolve_address`. The two-namespace surface
//! (per-room inbox secret vs. reply-by-email token) is the highest-
//! consequence change in stage 2; covering it directly lets a refactor
//! catch a mis-routing before the full poll path is exercised.
//!
//! Full-path tests for the ReplyMatch arm (parse -> resolve -> actor
//! -> post) land in commit 2c once the actor wiring is in.

use lets_chat::db;
use lets_chat::email_ingress::resolve::{resolve_address, ResolveOutcome, REPLY_PREFIX};
use mail_parser::MessageParser;

mod common;

const SECRET: [u8; 32] = [17u8; 32];
const INGRESS_DOMAIN: &str = "mail.example.com";

fn parse_with_to(addr: &str) -> Vec<u8> {
    format!(
        "From: outside@example.com\r\n\
         To: {addr}\r\n\
         Subject: hello\r\n\
         Date: Mon, 25 May 2026 12:00:00 +0000\r\n\
         Message-ID: <{}@spike.test>\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         hi\r\n",
        uuid::Uuid::new_v4()
    )
    .into_bytes()
}

async fn fresh_room(chat: &sqlx::SqlitePool) -> i64 {
    db::chat::create_room(chat, "ops", None, "public", None, None)
        .await
        .unwrap()
}

async fn insert_message(chat: &sqlx::SqlitePool, room_id: i64, user_id: &str) -> i64 {
    db::chat::insert_message(chat, room_id, user_id, "original")
        .await
        .unwrap()
}

#[tokio::test]
async fn reply_namespace_routes_to_reply_match_for_active_token() {
    let chat = common::chat_pool().await;
    let room_id = fresh_room(&chat).await;
    let message_id = insert_message(&chat, room_id, "alice").await;
    let token = db::reply_tokens::mint_token();
    db::reply_tokens::insert(&chat, &token, "alice", message_id, "2099-01-01 00:00:00")
        .await
        .unwrap();

    let raw = parse_with_to(&format!("{REPLY_PREFIX}{token}@{INGRESS_DOMAIN}"));
    let msg = MessageParser::default().parse(&raw).unwrap();
    let outcome = resolve_address(&chat, &SECRET, &msg, INGRESS_DOMAIN)
        .await
        .unwrap();
    let ResolveOutcome::ReplyMatch(row) = outcome else {
        panic!("expected ReplyMatch, got {outcome:?}");
    };
    assert_eq!(row.user_id, "alice");
    assert_eq!(row.message_id, message_id);
}

#[tokio::test]
async fn reply_namespace_routes_to_reply_expired_for_past_token() {
    let chat = common::chat_pool().await;
    let room_id = fresh_room(&chat).await;
    let message_id = insert_message(&chat, room_id, "alice").await;
    let token = db::reply_tokens::mint_token();
    db::reply_tokens::insert(&chat, &token, "alice", message_id, "2000-01-01 00:00:00")
        .await
        .unwrap();

    let raw = parse_with_to(&format!("{REPLY_PREFIX}{token}@{INGRESS_DOMAIN}"));
    let msg = MessageParser::default().parse(&raw).unwrap();
    let outcome = resolve_address(&chat, &SECRET, &msg, INGRESS_DOMAIN)
        .await
        .unwrap();
    let ResolveOutcome::ReplyExpired(row) = outcome else {
        panic!("expected ReplyExpired, got {outcome:?}");
    };
    assert_eq!(row.user_id, "alice");
}

#[tokio::test]
async fn reply_namespace_unknown_token_drops_to_not_found_without_inbox_fallback() {
    // The `reply-` namespace is reserved: an unknown token must NOT be
    // HMAC-hashed and tried against the email_inboxes table, even if it
    // happens to match a row's hash by accident. This test asserts the
    // fall-through is to NotFound rather than to Match/Revoked.
    let chat = common::chat_pool().await;
    let raw = parse_with_to(&format!(
        "{REPLY_PREFIX}DEFINITELY_NOT_A_REAL_TOKEN@{INGRESS_DOMAIN}"
    ));
    let msg = MessageParser::default().parse(&raw).unwrap();
    let outcome = resolve_address(&chat, &SECRET, &msg, INGRESS_DOMAIN)
        .await
        .unwrap();
    let ResolveOutcome::NotFound { tried_addresses } = outcome else {
        panic!("expected NotFound for unknown reply-token, got {outcome:?}");
    };
    assert!(
        tried_addresses.iter().any(|a| a.starts_with(REPLY_PREFIX)),
        "tried_addresses must surface the reply-token shape so an operator can diagnose",
    );
}

#[tokio::test]
async fn inbox_namespace_unaffected_by_reply_fork() {
    // A normal per-room inbox secret address (no `reply-` prefix) still
    // resolves through the HMAC path. This is the regression test that
    // the namespace fork didn't break the existing surface.
    let chat = common::chat_pool().await;
    let room_id = fresh_room(&chat).await;
    let admin_id = "admin";
    sqlx::query(
        "INSERT OR IGNORE INTO users (id, username, password_hash, role) VALUES (?, ?, ?, ?)",
    )
    .bind(admin_id)
    .bind("admin")
    .bind("h")
    .bind("admin")
    .execute(&chat)
    .await
    .ok(); // best-effort; the chat pool may not have a users table
    let secret_plain = "lc_normal_inbox_secret_token_value_xxxxxxxxxxxxx";
    let hash = lets_chat::auth::hash_api_token(&SECRET, secret_plain);
    let inbox_id = db::email_inbox::insert(&chat, room_id, "Inbox", None, &hash, "admin")
        .await
        .unwrap();

    let raw = parse_with_to(&format!("{secret_plain}@{INGRESS_DOMAIN}"));
    let msg = MessageParser::default().parse(&raw).unwrap();
    let outcome = resolve_address(&chat, &SECRET, &msg, INGRESS_DOMAIN)
        .await
        .unwrap();
    let ResolveOutcome::Match(inbox) = outcome else {
        panic!("expected Match for valid inbox secret, got {outcome:?}");
    };
    assert_eq!(inbox.id, inbox_id);
}

#[tokio::test]
async fn reply_prefix_is_case_insensitive() {
    let chat = common::chat_pool().await;
    let room_id = fresh_room(&chat).await;
    let message_id = insert_message(&chat, room_id, "alice").await;
    let token = db::reply_tokens::mint_token();
    db::reply_tokens::insert(&chat, &token, "alice", message_id, "2099-01-01 00:00:00")
        .await
        .unwrap();

    // Some MTAs lower-case the local part on rewrite, but `reply-` is
    // already lowercase. Belt-and-braces: any case must route to the
    // reply namespace so the inbox table is never consulted.
    let raw = parse_with_to(&format!("REPLY-{token}@{INGRESS_DOMAIN}"));
    let msg = MessageParser::default().parse(&raw).unwrap();
    let outcome = resolve_address(&chat, &SECRET, &msg, INGRESS_DOMAIN)
        .await
        .unwrap();
    assert!(
        matches!(outcome, ResolveOutcome::ReplyMatch(_)),
        "uppercase REPLY- prefix must still route to the reply namespace, got {outcome:?}",
    );
}
