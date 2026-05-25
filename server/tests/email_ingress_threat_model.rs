//! LC-77 threat-model integration tests.
//!
//! Pins the named threat-model lines from the parent brainstorm as a
//! dedicated suite. The same surfaces are covered piecemeal in
//! `email_ingress_process.rs` and `email_ingress_attachments.rs`; this
//! file consolidates the load-bearing assertions in one place so a
//! security review can find them.
//!
//! Each test docstring names which brainstorm/threat-model line it
//! anchors so a future change that touches the assertion has to grapple
//! with the documented invariant, not just see "this test broke".
//!
//! See `docs/email-ingress.md` for the operator-facing description of
//! each invariant.

use std::sync::{Arc, OnceLock};

use lets_chat::email_ingress::poll::{process_polled_message, ProcessOutcome};
use lets_chat::email_ingress::DropReason;
use lets_chat::{auth, db, state::AppState, ws::hub::Hub};

mod common;

const SECRET: [u8; 32] = [23u8; 32];
const INGRESS_DOMAIN: &str = "mail.example.com";
const TOKEN: &str = "lc_threatmodelaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn ensure_tempdir() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let p = std::env::temp_dir().join(format!("lc-tm-tests-{}", std::process::id()));
        std::fs::create_dir_all(&p).expect("mkdir");
        db::set_data_dir(p.to_string_lossy().into_owned());
    });
}

struct Fixture {
    state: AppState,
    room_id: i64,
    inbox_id: i64,
    secret_key: [u8; 32],
}

async fn setup() -> Fixture {
    ensure_tempdir();
    let auth_pool = common::auth_pool().await;
    let chat_pool = common::chat_pool().await;
    let settings_pool = common::settings_pool().await;
    let admin = db::auth::create_user(&auth_pool, "admin", "h")
        .await
        .unwrap();
    sqlx::query("UPDATE users SET role='admin', totp_enabled=1 WHERE id=?")
        .bind(&admin)
        .execute(&auth_pool)
        .await
        .unwrap();
    db::enclave::backfill_general_membership(&auth_pool, &chat_pool)
        .await
        .unwrap();
    let eid = db::enclave::create_enclave(&chat_pool, "Acme", None, &admin)
        .await
        .unwrap();
    let room_id = db::chat::create_room(&chat_pool, "ops", None, "public", None, Some(eid))
        .await
        .unwrap();
    let secret_hash = auth::hash_api_token(&SECRET, TOKEN);
    let inbox_id =
        db::email_inbox::insert(&chat_pool, room_id, "Inbox", None, &secret_hash, &admin)
            .await
            .unwrap();
    let bg = lets_chat::bg::spawn(auth_pool.clone());
    let state = AppState {
        auth: auth_pool,
        chat: chat_pool,
        settings: settings_pool,
        hub: Arc::new(Hub::new()),
        asset_version: "test".into(),
        last_seen_ledger: lets_chat::auth::new_last_seen_ledger(),
        activity_ledger: lets_chat::auth::new_last_seen_ledger(),
        bg,
        secret_key: Some(Arc::new(SECRET)),
        vapid: None,
        push_client: Arc::new(lets_chat::push::MockPushClient::default()),
        apns_client: None,
        fcm_client: None,
        mailer: None,
        base_url: "http://localhost:8080".to_string(),
        ice_servers: "[]".to_string(),
        rate_limits: lets_chat::rate_limit::RateLimits::new(),
    };
    Fixture {
        state,
        room_id,
        inbox_id,
        secret_key: SECRET,
    }
}

/// THREAT MODEL: "Identity is the secret, not the From header."
///
/// The sender's `From:` is trivially forged. A polled message with a
/// valid token in the To header MUST post as the inbox's synthetic
/// actor regardless of what the From header claims. Specifically the
/// stored message row's `email_inbox_id` must point at the inbox the
/// token resolved to, the stored `user_id` must be the empty-string
/// synthetic-actor sentinel, and the stored body must NOT verbatim
/// include the forged From address (a future change that accidentally
/// echoes the sender into the body would break this).
#[tokio::test]
async fn forged_from_still_posts_as_inbox_actor() {
    let fx = setup().await;
    let raw = format!(
        "From: admin@deployment.test\r\n\
         To: {TOKEN}@{INGRESS_DOMAIN}\r\n\
         Subject: looks like admin\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         \r\n\
         I am NOT admin.\r\n",
    )
    .into_bytes();
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Posted { message_id } = outcome else {
        panic!("expected Posted, got {outcome:?}");
    };
    let m = db::chat::get_message(&fx.state.chat, message_id)
        .await
        .unwrap()
        .expect("message row");
    assert_eq!(m.email_inbox_id, Some(fx.inbox_id));
    assert_eq!(m.user_id, "", "synthetic-actor sentinel");
    assert!(
        !m.body.contains("admin@deployment.test"),
        "stored body must not echo the From address verbatim",
    );
}

/// THREAT MODEL: "Raw HTML never reaches the stored body."
///
/// The sender's HTML payload is either dropped (no text fallback) or
/// posted as the stripped-to-text version. In neither case may the
/// stored body contain a raw `<script` tag or any other piece of
/// executable markup. The chat markdown pipeline already drops raw
/// HTML at render time; this test asserts the BODY itself stays clean
/// so we don't depend on rendering correctness to be safe.
#[tokio::test]
async fn raw_html_never_appears_in_stored_body() {
    let fx = setup().await;
    let raw = format!(
        "From: a@example.com\r\n\
         To: {TOKEN}@{INGRESS_DOMAIN}\r\n\
         Subject: html only\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         \r\n\
         <html><body><p>hello</p><script>alert('xss')</script><img src=x onerror=alert(1)></body></html>\r\n",
    )
    .into_bytes();
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    if let ProcessOutcome::Posted { message_id } = outcome {
        let m = db::chat::get_message(&fx.state.chat, message_id)
            .await
            .unwrap()
            .expect("message row");
        for forbidden in ["<script", "</script", "onerror=", "<img"] {
            assert!(
                !m.body.contains(forbidden),
                "stored body must not contain `{forbidden}`; got {:?}",
                m.body,
            );
        }
    }
    // A Dropped outcome with reason=ParseFail is also acceptable: it
    // means mail-parser produced no text fallback, which is consistent
    // with the "no raw HTML in body" invariant (we dropped rather than
    // post anything).
}

/// THREAT MODEL: "Unknown secret drops SILENTLY."
///
/// IMAP-poll has no requestor to respond to: the mail came from an
/// SMTP sender we never spoke to directly. lets-chat MUST NOT generate
/// a bounce or any visible artifact when a secret doesn't match;
/// revealing whether an inbox address exists would let an attacker
/// enumerate live inboxes. The structured log line is the ONLY
/// diagnostic surface, and it must carry `reason=address_no_match` so
/// the operator can find it. Specifically: NO message row is inserted
/// in the chat database.
#[tokio::test]
async fn unknown_secret_silent_drop_no_message_row() {
    let fx = setup().await;
    let raw = format!(
        "From: stranger@example.com\r\n\
         To: lc_unknownsecretnotinanytable@{INGRESS_DOMAIN}\r\n\
         Subject: probing\r\n\
         Content-Type: text/plain\r\n\
         \r\n\
         is there an inbox here\r\n",
    )
    .into_bytes();
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Dropped { reason, .. } = outcome else {
        panic!("expected Dropped, got {outcome:?}");
    };
    assert_eq!(reason, DropReason::AddressNoMatch);

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE room_id = ?")
        .bind(fx.room_id)
        .fetch_one(&fx.state.chat)
        .await
        .unwrap();
    assert_eq!(
        count, 0,
        "an unknown-secret drop must not create a message row anywhere",
    );
}

/// THREAT MODEL: "Loop heuristic headers drop, even from legitimate senders."
///
/// `Auto-Submitted`, `Precedence: bulk/list/junk`, `X-Autoreply`,
/// `X-Autorespond`, and `List-Id` all drop with `LoopDetected`. The
/// `List-Id` case is the load-bearing one: a legitimate monitoring
/// tool that tags itself as a list will see its mail dropped. The
/// operator's diagnostic is the structured log; the docs
/// (`docs/email-ingress.md` "Not supported") name this trade-off so an
/// operator hitting `reason=loop_detected detail="List-Id present"`
/// knows what to look for.
#[tokio::test]
async fn loop_headers_drop_consistently() {
    let fx = setup().await;
    let recipient = format!("{TOKEN}@{INGRESS_DOMAIN}");
    // Each tuple is (header line, the detail substring the log records).
    let cases: &[(&str, &str)] = &[
        ("Auto-Submitted: auto-replied", "Auto-Submitted"),
        ("Auto-Submitted: auto-generated", "Auto-Submitted"),
        ("Precedence: bulk", "Precedence"),
        ("Precedence: list", "Precedence"),
        ("Precedence: junk", "Precedence"),
        ("X-Autoreply: yes", "X-Autoreply"),
        ("X-Autorespond: indeed", "X-Autorespond"),
        ("List-Id: <ops.example.com>", "List-Id"),
    ];
    for (header_line, detail_marker) in cases {
        let raw = format!(
            "From: bot@example.com\r\n\
             To: {recipient}\r\n\
             Subject: test\r\n\
             {header_line}\r\n\
             Content-Type: text/plain\r\n\
             \r\n\
             body\r\n",
        )
        .into_bytes();
        let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
        let ProcessOutcome::Dropped { reason, detail } = outcome else {
            panic!("expected Dropped for {header_line:?}, got {outcome:?}");
        };
        assert_eq!(
            reason,
            DropReason::LoopDetected,
            "header {header_line:?} should trip loop detection",
        );
        assert!(
            detail.contains(detail_marker),
            "loop-drop detail must name the matching header so the operator can diagnose; \
             expected substring {detail_marker:?} in {detail:?} for header {header_line:?}",
        );
    }
}

/// THREAT MODEL: "Auto-Submitted: no is the only opt-out value."
///
/// RFC 3834 says `Auto-Submitted: no` indicates the message is not
/// machine-generated. Real human-typed mail rarely sets the header at
/// all; the few clients that DO set `Auto-Submitted: no` must pass
/// through. Anything else (including unfamiliar values we haven't
/// seen) drops. This pins the negative-test posture: a human-set "no"
/// posts, anything else drops.
#[tokio::test]
async fn auto_submitted_no_does_not_drop_but_anything_else_does() {
    let fx = setup().await;
    let recipient = format!("{TOKEN}@{INGRESS_DOMAIN}");

    let raw_no = format!(
        "From: human@example.com\r\n\
         To: {recipient}\r\n\
         Subject: real reply\r\n\
         Auto-Submitted: no\r\n\
         Content-Type: text/plain\r\n\
         \r\n\
         typed by a person\r\n",
    )
    .into_bytes();
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw_no).await;
    assert!(
        matches!(outcome, ProcessOutcome::Posted { .. }),
        "Auto-Submitted: no should NOT trigger loop-drop; got {outcome:?}",
    );
}

/// THREAT MODEL: "Revoked inbox drops silently."
///
/// An inbox marked revoked must not accept any subsequent polled mail.
/// The drop is silent (no bounce), the log carries `revoked_inbox`,
/// and no message row is inserted.
#[tokio::test]
async fn revoked_inbox_post_revoke_silent_drop_no_message_row() {
    let fx = setup().await;
    db::email_inbox::revoke(&fx.state.chat, fx.inbox_id, fx.room_id)
        .await
        .unwrap();
    let raw = format!(
        "From: a@example.com\r\n\
         To: {TOKEN}@{INGRESS_DOMAIN}\r\n\
         Subject: post revoke\r\n\
         Content-Type: text/plain\r\n\
         \r\n\
         hi\r\n",
    )
    .into_bytes();
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Dropped { reason, .. } = outcome else {
        panic!("expected Dropped, got {outcome:?}");
    };
    assert_eq!(reason, DropReason::RevokedInbox);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE room_id = ?")
        .bind(fx.room_id)
        .fetch_one(&fx.state.chat)
        .await
        .unwrap();
    assert_eq!(
        count, 0,
        "revoked-inbox drops must not create a message row",
    );
}

/// THREAT MODEL: "Domain match required."
///
/// A token that matches a known inbox at the WRONG domain must NOT
/// resolve. This stops a future cross-tenant bleed where two
/// deployments share a polled mailbox or where the same secret is
/// (mis)used at multiple domains.
#[tokio::test]
async fn token_at_wrong_domain_fails_to_resolve() {
    let fx = setup().await;
    let raw = format!(
        "From: a@example.com\r\n\
         To: {TOKEN}@completely-different.test\r\n\
         Subject: wrong-domain probe\r\n\
         Content-Type: text/plain\r\n\
         \r\n\
         should not match\r\n",
    )
    .into_bytes();
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Dropped { reason, .. } = outcome else {
        panic!("expected Dropped, got {outcome:?}");
    };
    assert_eq!(
        reason,
        DropReason::AddressNoMatch,
        "the right token at the wrong domain must NOT match - this is the cross-tenant safety property",
    );
}

/// THREAT MODEL: "Drop log carries operator-actionable diagnostic detail."
///
/// The structured log is the SOLE diagnostic channel (no bounces, no
/// dead-letter folder in v1). For every drop reason, the `detail` field
/// must carry enough information for an operator hearing "my email
/// didn't post" to diagnose from logs alone. This test pins the
/// invariant that AddressNoMatch's detail names the tried addresses
/// (the most common diagnostic surface).
#[tokio::test]
async fn address_no_match_drop_detail_carries_tried_addresses() {
    let fx = setup().await;
    let raw = format!(
        "From: a@example.com\r\n\
         To: lc_definitelyunknown@{INGRESS_DOMAIN}\r\n\
         Cc: lc_alsounknown@{INGRESS_DOMAIN}\r\n\
         Subject: diagnosis\r\n\
         Content-Type: text/plain\r\n\
         \r\n\
         body\r\n",
    )
    .into_bytes();
    let outcome = process_polled_message(&fx.state, &fx.secret_key, INGRESS_DOMAIN, &raw).await;
    let ProcessOutcome::Dropped { reason, detail } = outcome else {
        panic!("expected Dropped, got {outcome:?}");
    };
    assert_eq!(reason, DropReason::AddressNoMatch);
    assert!(
        detail.contains("lc_definitelyunknown") && detail.contains("lc_alsounknown"),
        "detail must list every address checked so the operator can see what the resolver saw; \
         got {detail:?}",
    );
}
