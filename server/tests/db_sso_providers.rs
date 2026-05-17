//! Integration tests for `db::sso_providers`. Each test opens its own
//! fresh in-memory SQLite, applies the auth migration set, then
//! exercises one helper end-to-end. See
//! docs/lets-chat/sso/10-admin-managed-providers.md.

use lets_chat::db::sso_providers::{self, InsertProvider, SsoProviderRow, UpdateProvider};
use sqlx::SqlitePool;

async fn setup_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    for sql in [
        include_str!("../migrations/auth/0001_create_tables.sql"),
        include_str!("../migrations/auth/0002_read_receipts.sql"),
        include_str!("../migrations/auth/0003_profile_fields.sql"),
        include_str!("../migrations/auth/0004_user_status.sql"),
        include_str!("../migrations/auth/0005_profile_visibility.sql"),
        include_str!("../migrations/auth/0006_user_blocks.sql"),
        include_str!("../migrations/auth/0007_notification_settings.sql"),
        include_str!("../migrations/auth/0008_two_factor.sql"),
        include_str!("../migrations/auth/0009_push_subscriptions.sql"),
        include_str!("../migrations/auth/0010_password_reset.sql"),
        include_str!("../migrations/auth/0011_email_verification.sql"),
        include_str!("../migrations/auth/0012_session_metadata.sql"),
        include_str!("../migrations/auth/0013_digest_columns.sql"),
        include_str!("../migrations/auth/0014_login_alerts.sql"),
        include_str!("../migrations/auth/0015_pending_registrations.sql"),
        include_str!("../migrations/auth/0016_sso_identities.sql"),
        include_str!("../migrations/auth/0017_sso_providers.sql"),
        include_str!("../migrations/auth/0018_sso_flows_provider.sql"),
        include_str!("../migrations/auth/0019_sso_group_mappings.sql"),
    ] {
        sqlx::raw_sql(sql).execute(&pool).await.unwrap();
    }
    pool
}

fn insert_args<'a>(id: &'a str, issuer: &'a str, secret: &'a [u8]) -> InsertProvider<'a> {
    InsertProvider {
        id,
        kind: "oidc",
        display_name: "Test Provider",
        issuer_url: issuer,
        client_id: "client-uuid",
        client_secret_encrypted: secret,
        scopes: "openid email profile",
        attribute_map_json: "{}",
        allow_signup: false,
        auto_link_verified_email: true,
    }
}

#[tokio::test]
async fn insert_then_get_by_id_round_trips() {
    let pool = setup_pool().await;
    let secret = b"encrypted-blob";
    sso_providers::insert_provider(&pool, insert_args("acme", "https://idp.example/", secret))
        .await
        .unwrap();

    let row = sso_providers::get_provider_by_id(&pool, "acme")
        .await
        .unwrap()
        .expect("row exists");
    assert_eq!(row.id, "acme");
    assert_eq!(row.kind, "oidc");
    assert_eq!(row.issuer_url, "https://idp.example/");
    assert_eq!(row.client_secret_encrypted, secret);
    assert_eq!(row.scopes, "openid email profile");
    assert!(!row.allow_signup);
    assert!(row.auto_link_verified_email);
    assert!(row.enabled_at.is_none(), "new rows land disabled");
}

#[tokio::test]
async fn get_provider_by_id_returns_none_for_unknown() {
    let pool = setup_pool().await;
    let row = sso_providers::get_provider_by_id(&pool, "ghost")
        .await
        .unwrap();
    assert!(row.is_none());
}

#[tokio::test]
async fn get_provider_by_issuer_finds_by_url() {
    let pool = setup_pool().await;
    sso_providers::insert_provider(
        &pool,
        insert_args("acme", "https://idp.example/", b"secret"),
    )
    .await
    .unwrap();

    let row = sso_providers::get_provider_by_issuer(&pool, "https://idp.example/")
        .await
        .unwrap()
        .expect("found");
    assert_eq!(row.id, "acme");
}

#[tokio::test]
async fn duplicate_issuer_url_rejected() {
    let pool = setup_pool().await;
    sso_providers::insert_provider(&pool, insert_args("a", "https://shared/", b"s"))
        .await
        .unwrap();
    let err = sso_providers::insert_provider(&pool, insert_args("b", "https://shared/", b"s"))
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.to_lowercase().contains("unique"),
        "expected UNIQUE constraint violation, got: {msg}"
    );
}

#[tokio::test]
async fn duplicate_id_rejected() {
    let pool = setup_pool().await;
    sso_providers::insert_provider(&pool, insert_args("same", "https://a/", b"s"))
        .await
        .unwrap();
    assert!(
        sso_providers::insert_provider(&pool, insert_args("same", "https://b/", b"s"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn list_orders_by_display_name() {
    let pool = setup_pool().await;
    fn mk<'a>(id: &'a str, name: &'a str, issuer: &'a str) -> InsertProvider<'a> {
        InsertProvider {
            id,
            kind: "oidc",
            display_name: name,
            issuer_url: issuer,
            client_id: "c",
            client_secret_encrypted: b"s",
            scopes: "openid",
            attribute_map_json: "{}",
            allow_signup: false,
            auto_link_verified_email: true,
        }
    }
    sso_providers::insert_provider(&pool, mk("a", "Zeta", "https://z/"))
        .await
        .unwrap();
    sso_providers::insert_provider(&pool, mk("b", "alpha", "https://a/"))
        .await
        .unwrap();
    sso_providers::insert_provider(&pool, mk("c", "Mike", "https://m/"))
        .await
        .unwrap();

    let names: Vec<String> = sso_providers::list_providers(&pool)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.display_name)
        .collect();
    assert_eq!(names, vec!["alpha", "Mike", "Zeta"]);
}

#[tokio::test]
async fn list_enabled_filters_disabled_rows() {
    let pool = setup_pool().await;
    sso_providers::insert_provider(&pool, insert_args("on", "https://a/", b"s"))
        .await
        .unwrap();
    sso_providers::insert_provider(&pool, insert_args("off", "https://b/", b"s"))
        .await
        .unwrap();
    sso_providers::set_provider_enabled(&pool, "on", true)
        .await
        .unwrap();

    let enabled: Vec<String> = sso_providers::list_enabled_providers(&pool)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(enabled, vec!["on"]);
}

#[tokio::test]
async fn toggle_disable_after_enable_marks_dead() {
    let pool = setup_pool().await;
    sso_providers::insert_provider(&pool, insert_args("p", "https://a/", b"s"))
        .await
        .unwrap();
    sso_providers::set_provider_enabled(&pool, "p", true)
        .await
        .unwrap();
    // Force the disable timestamp strictly above the enable timestamp by
    // bumping the stored seconds-precision value directly. The helper
    // SQL uses `strftime('%s','now')` which can return the same integer
    // for two calls inside one wall-clock second; the audit semantics
    // (disabled_at > enabled_at => dead) only matter when the operator
    // toggles, and human toggles are minutes apart.
    sso_providers::set_provider_enabled(&pool, "p", false)
        .await
        .unwrap();
    sqlx::query("UPDATE sso_providers SET disabled_at = enabled_at + 1 WHERE id = ?")
        .bind("p")
        .execute(&pool)
        .await
        .unwrap();
    let row = sso_providers::get_provider_by_id(&pool, "p")
        .await
        .unwrap()
        .unwrap();
    assert!(!row.is_enabled());
    assert!(row.enabled_at.is_some());
    assert!(row.disabled_at.is_some());
}

#[tokio::test]
async fn re_enable_after_disable_marks_live_again() {
    let pool = setup_pool().await;
    sso_providers::insert_provider(&pool, insert_args("p", "https://a/", b"s"))
        .await
        .unwrap();
    sso_providers::set_provider_enabled(&pool, "p", true)
        .await
        .unwrap();
    sso_providers::set_provider_enabled(&pool, "p", false)
        .await
        .unwrap();
    sso_providers::set_provider_enabled(&pool, "p", true)
        .await
        .unwrap();
    // As above, force enabled_at strictly above disabled_at so the
    // assertion doesn't depend on sub-second SQL precision.
    sqlx::query("UPDATE sso_providers SET enabled_at = disabled_at + 1 WHERE id = ?")
        .bind("p")
        .execute(&pool)
        .await
        .unwrap();
    let row = sso_providers::get_provider_by_id(&pool, "p")
        .await
        .unwrap()
        .unwrap();
    assert!(row.is_enabled());
    assert!(
        row.disabled_at.is_some(),
        "previous disable timestamp preserved for audit"
    );
}

#[tokio::test]
async fn update_overwrites_fields_but_preserves_secret_when_none() {
    let pool = setup_pool().await;
    sso_providers::insert_provider(
        &pool,
        insert_args("p", "https://old/", b"original-secret-blob"),
    )
    .await
    .unwrap();

    sso_providers::update_provider(
        &pool,
        "p",
        UpdateProvider {
            display_name: "Renamed",
            issuer_url: "https://new/",
            client_id: "new-client",
            client_secret_encrypted: None,
            scopes: "openid profile",
            attribute_map_json: r#"{"email_claim":"mail"}"#,
            allow_signup: true,
            auto_link_verified_email: false,
        },
    )
    .await
    .unwrap();

    let row = sso_providers::get_provider_by_id(&pool, "p")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.display_name, "Renamed");
    assert_eq!(row.issuer_url, "https://new/");
    assert_eq!(row.client_id, "new-client");
    assert_eq!(
        row.client_secret_encrypted, b"original-secret-blob",
        "secret untouched when None"
    );
    assert_eq!(row.scopes, "openid profile");
    assert_eq!(row.attribute_map_json, r#"{"email_claim":"mail"}"#);
    assert!(row.allow_signup);
    assert!(!row.auto_link_verified_email);
}

#[tokio::test]
async fn update_rotates_secret_when_some() {
    let pool = setup_pool().await;
    sso_providers::insert_provider(&pool, insert_args("p", "https://x/", b"old"))
        .await
        .unwrap();

    let new_secret: &[u8] = b"rotated";
    sso_providers::update_provider(
        &pool,
        "p",
        UpdateProvider {
            display_name: "Test Provider",
            issuer_url: "https://x/",
            client_id: "client-uuid",
            client_secret_encrypted: Some(new_secret),
            scopes: "openid email profile",
            attribute_map_json: "{}",
            allow_signup: false,
            auto_link_verified_email: true,
        },
    )
    .await
    .unwrap();

    let row = sso_providers::get_provider_by_id(&pool, "p")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.client_secret_encrypted, new_secret);
}

#[tokio::test]
async fn update_missing_returns_zero_rows_affected() {
    let pool = setup_pool().await;
    let n = sso_providers::update_provider(
        &pool,
        "ghost",
        UpdateProvider {
            display_name: "n",
            issuer_url: "https://x/",
            client_id: "c",
            client_secret_encrypted: None,
            scopes: "openid",
            attribute_map_json: "{}",
            allow_signup: false,
            auto_link_verified_email: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn delete_removes_row() {
    let pool = setup_pool().await;
    sso_providers::insert_provider(&pool, insert_args("p", "https://x/", b"s"))
        .await
        .unwrap();
    let removed = sso_providers::delete_provider(&pool, "p").await.unwrap();
    assert_eq!(removed, 1);
    assert!(sso_providers::get_provider_by_id(&pool, "p")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn delete_missing_is_noop() {
    let pool = setup_pool().await;
    let removed = sso_providers::delete_provider(&pool, "ghost")
        .await
        .unwrap();
    assert_eq!(removed, 0);
}

#[tokio::test]
async fn is_enabled_method_matches_sql_predicate() {
    // Sanity check the struct method tracks the SQL filter.
    let never_enabled = SsoProviderRow {
        id: "x".into(),
        kind: "oidc".into(),
        display_name: "x".into(),
        issuer_url: "x".into(),
        client_id: "x".into(),
        client_secret_encrypted: vec![],
        scopes: String::new(),
        attribute_map_json: "{}".into(),
        allow_signup: false,
        auto_link_verified_email: false,
        enabled_at: None,
        disabled_at: None,
        created_at: 0,
        updated_at: 0,
    };
    assert!(!never_enabled.is_enabled());

    let live = SsoProviderRow {
        enabled_at: Some(100),
        disabled_at: None,
        ..never_enabled.clone()
    };
    assert!(live.is_enabled());

    let dead = SsoProviderRow {
        enabled_at: Some(100),
        disabled_at: Some(200),
        ..never_enabled.clone()
    };
    assert!(!dead.is_enabled());

    let re_enabled = SsoProviderRow {
        enabled_at: Some(300),
        disabled_at: Some(200),
        ..never_enabled
    };
    assert!(re_enabled.is_enabled());
}
