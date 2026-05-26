//! LC-77-DEAD-LETTER (#203): round-trip test for the new
//! `dead_letter_folder` column on `imap_inbox_config`.
//!
//! The COPY-before-Seen wiring itself runs over the live async-imap
//! session and cannot be unit-tested without a greenmail-style fixture;
//! that wiring's correctness is verified by manual smoke per the
//! Test plan in the PR description.

use lets_chat::db::imap_config::{self, ImapConfig};

mod common;

const SECRET: [u8; 32] = [11u8; 32];

fn base_cfg(dead_letter: Option<&str>) -> ImapConfig {
    ImapConfig {
        host: "imap.example.com".into(),
        port: 993,
        tls: true,
        username: "mailer".into(),
        password: "secret".into(),
        folder: "INBOX".into(),
        ingress_domain: Some("mail.example.com".into()),
        enabled: false,
        dead_letter_folder: dead_letter.map(str::to_string),
    }
}

#[tokio::test]
async fn write_then_read_round_trips_dead_letter_folder_when_set() {
    let pool = common::settings_pool().await;
    imap_config::write(&pool, &SECRET, &base_cfg(Some("INBOX/lets-chat-rejected")))
        .await
        .unwrap();
    let cfg = imap_config::read(&pool, &SECRET)
        .await
        .unwrap()
        .expect("row");
    assert_eq!(
        cfg.dead_letter_folder.as_deref(),
        Some("INBOX/lets-chat-rejected"),
    );
}

#[tokio::test]
async fn write_then_read_preserves_none_when_unset() {
    let pool = common::settings_pool().await;
    imap_config::write(&pool, &SECRET, &base_cfg(None))
        .await
        .unwrap();
    let cfg = imap_config::read(&pool, &SECRET)
        .await
        .unwrap()
        .expect("row");
    assert!(cfg.dead_letter_folder.is_none());
}

#[tokio::test]
async fn upsert_clears_previously_set_dead_letter_folder_when_re_saved_as_none() {
    // Operator turned the feature on, then turned it off again. The
    // upsert path must overwrite the previously-set value with NULL,
    // not preserve the old folder name. (Asymmetric with the password
    // field, which DOES preserve on empty input; the dead-letter
    // folder is operator-visible and an empty input is an explicit
    // "off" intent rather than a "preserve" intent.)
    let pool = common::settings_pool().await;
    imap_config::write(&pool, &SECRET, &base_cfg(Some("INBOX/rejected")))
        .await
        .unwrap();
    imap_config::write(&pool, &SECRET, &base_cfg(None))
        .await
        .unwrap();
    let cfg = imap_config::read(&pool, &SECRET)
        .await
        .unwrap()
        .expect("row");
    assert!(
        cfg.dead_letter_folder.is_none(),
        "saving with None must clear a previously-set folder",
    );
}
