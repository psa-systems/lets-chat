//! Integration tests for `sso::discovery::discover` and the
//! `ProviderEntry` cache. Each test stands up a tiny axum server bound
//! to a random port to act as a stub IdP, then points `discover` at it
//! and checks the parsed metadata + caching behaviour.

use std::sync::Arc;

use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use lets_chat::db::sso_providers::SsoProviderRow;
use lets_chat::sso::cache::ProviderEntry;
use lets_chat::sso::discovery::{self, DiscoveryError};
use tokio::sync::Mutex;
use url::Url;

/// Spawn a stub IdP. Binds first so the issuer URL is known before
/// the handler returns it. Returns `(issuer_string, hits)` where
/// `hits` counts discovery-doc GETs (used by the caching test).
async fn spawn_stub(include_userinfo: bool) -> (String, Arc<Mutex<u32>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let issuer = format!("http://127.0.0.1:{}", addr.port());

    let userinfo_line = if include_userinfo {
        format!(r#""userinfo_endpoint": "{issuer}/oauth2/userinfo","#)
    } else {
        String::new()
    };
    let discovery_body = format!(
        r#"{{
            "issuer": "{issuer}",
            "authorization_endpoint": "{issuer}/oauth2/authorize",
            "token_endpoint": "{issuer}/oauth2/token",
            {userinfo_line}
            "jwks_uri": "{issuer}/jwks.json"
        }}"#
    );
    let jwks_body = jwks_doc().to_string();
    let hits = Arc::new(Mutex::new(0u32));
    let hits_route = hits.clone();

    let app = Router::new()
        .route(
            "/.well-known/openid-configuration",
            get(move || {
                let body = discovery_body.clone();
                let hits = hits_route.clone();
                async move {
                    *hits.lock().await += 1;
                    (
                        StatusCode::OK,
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        body,
                    )
                }
            }),
        )
        .route(
            "/jwks.json",
            get(move || {
                let body = jwks_body.clone();
                async move {
                    (
                        StatusCode::OK,
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        body,
                    )
                }
            }),
        );

    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (issuer, hits)
}

fn jwks_doc() -> &'static str {
    r#"{"keys":[{"kty":"RSA","kid":"k1","n":"x","e":"AQAB"}]}"#
}

fn row(issuer: &str) -> SsoProviderRow {
    SsoProviderRow {
        id: "stub".into(),
        kind: "oidc".into(),
        display_name: "Stub".into(),
        issuer_url: issuer.into(),
        client_id: "client".into(),
        client_secret_encrypted: vec![],
        scopes: "openid email".into(),
        attribute_map_json: "{}".into(),
        allow_signup: false,
        auto_link_verified_email: true,
        enabled_at: Some(100),
        disabled_at: None,
        created_at: 0,
        updated_at: 0,
    }
}

async fn spawn_failing_stub(status: StatusCode, body: &'static str) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let issuer = format!("http://127.0.0.1:{}", addr.port());
    let app = Router::new().route(
        "/.well-known/openid-configuration",
        get(move || async move { (status, body) }),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    issuer
}

#[tokio::test]
async fn happy_path_parses_all_endpoints() {
    let (issuer, _hits) = spawn_stub(true).await;
    let http = reqwest::Client::new();
    let md = discovery::discover(&Url::parse(&issuer).unwrap(), &http)
        .await
        .unwrap();
    assert_eq!(md.issuer, issuer);
    assert_eq!(md.authorization_endpoint.path(), "/oauth2/authorize");
    assert_eq!(md.token_endpoint.path(), "/oauth2/token");
    assert_eq!(
        md.userinfo_endpoint.as_ref().unwrap().path(),
        "/oauth2/userinfo"
    );
    assert_eq!(md.jwks_uri.path(), "/jwks.json");
    assert!(md.jwks_json.contains("\"kid\":\"k1\""));
}

#[tokio::test]
async fn missing_userinfo_endpoint_is_ok() {
    let (issuer, _hits) = spawn_stub(false).await;
    let http = reqwest::Client::new();
    let md = discovery::discover(&Url::parse(&issuer).unwrap(), &http)
        .await
        .unwrap();
    assert!(md.userinfo_endpoint.is_none());
}

#[tokio::test]
async fn cache_first_call_fetches_second_reuses() {
    let (issuer, hits) = spawn_stub(true).await;
    let entry = ProviderEntry::new(row(&issuer));
    let http = reqwest::Client::new();

    let md1 = entry.discovery(&http).await.unwrap();
    let md2 = entry.discovery(&http).await.unwrap();
    assert!(Arc::ptr_eq(&md1, &md2));
    assert_eq!(*hits.lock().await, 1, "second call must not refetch");
}

#[tokio::test]
async fn bad_status_surfaces_typed_error() {
    let issuer = spawn_failing_stub(StatusCode::INTERNAL_SERVER_ERROR, "").await;
    let http = reqwest::Client::new();
    let err = discovery::discover(&Url::parse(&issuer).unwrap(), &http)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        DiscoveryError::BadStatus {
            what: "discovery",
            status: StatusCode::INTERNAL_SERVER_ERROR,
        }
    ));
}

#[tokio::test]
async fn malformed_discovery_json_errors() {
    let issuer = spawn_failing_stub(StatusCode::OK, "not json").await;
    let http = reqwest::Client::new();
    let err = discovery::discover(&Url::parse(&issuer).unwrap(), &http)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        DiscoveryError::BadJson {
            what: "discovery",
            ..
        }
    ));
}

#[tokio::test]
async fn discovery_cached_flag_flips_after_first_resolve() {
    let (issuer, _hits) = spawn_stub(true).await;
    let entry = ProviderEntry::new(row(&issuer));
    assert!(entry.discovery_cached().is_none());
    let http = reqwest::Client::new();
    entry.discovery(&http).await.unwrap();
    assert!(entry.discovery_cached().is_some());
}
