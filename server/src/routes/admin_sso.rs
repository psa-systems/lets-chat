//! Admin CRUD for SSO providers. AdminUser-gated.
//!
//! Routes:
//!   GET  /admin/sso              list
//!   GET  /admin/sso/new          new-row form
//!   POST /admin/sso              create row from new-row form
//!   GET  /admin/sso/:id          edit form (pre-filled)
//!   POST /admin/sso/:id          save edits (action=save) OR run discovery test (action=test)
//!   POST /admin/sso/:id/enable   flip enabled_at
//!   POST /admin/sso/:id/disable  flip disabled_at
//!   POST /admin/sso/:id/delete   delete row (refuses when sso_identities references it)
//!
//! Per doc 10 section 4. Group-mapping CRUD is L17.

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use serde::Deserialize;

use crate::auth::AdminUser;
use crate::db::sso_providers::{self, InsertProvider, UpdateProvider};
use crate::error::AppError;
use crate::sso::discovery;
use crate::sso::secret;
use crate::state::AppState;
use crate::version;
use crate::views::admin_sso::{
    AdminSsoProviderView, AttributeMapForm, DiscoveryTestResult, SsoEditPage, SsoListPage,
};
use crate::views::{html, Html};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/sso", get(get_list).post(post_create))
        .route("/admin/sso/new", get(get_new))
        .route("/admin/sso/{id}", get(get_edit).post(post_edit))
        .route("/admin/sso/{id}/enable", post(post_enable))
        .route("/admin/sso/{id}/disable", post(post_disable))
        .route("/admin/sso/{id}/delete", post(post_delete))
}

#[derive(Deserialize)]
pub struct ListFlash {
    pub flash: Option<String>,
}

pub async fn get_list(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    Query(q): Query<ListFlash>,
) -> Result<Html, AppError> {
    let (sidebar_rooms, sidebar_peers, switcher) = super::load_chrome(&state, &user, None).await?;
    let rows = sso_providers::list_providers(&state.auth).await?;
    let mut providers = Vec::with_capacity(rows.len());
    for r in &rows {
        let linked_users =
            sso_providers::count_identities_for_issuer(&state.auth, &r.issuer_url).await?;
        providers.push(AdminSsoProviderView {
            id: r.id.clone(),
            display_name: r.display_name.clone(),
            issuer_url: r.issuer_url.clone(),
            enabled: r.is_enabled(),
            allow_signup: r.allow_signup,
            linked_users,
        });
    }
    let page = SsoListPage {
        user: &user,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        git_version: version::GIT_VERSION,
        build_date: version::BUILD_DATE,
        section: "sso",
        providers: &providers,
        flash: q.flash.as_deref(),
        secret_key_missing: state.secret_key.is_none(),
    };
    html(&page)
}

pub async fn get_new(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
) -> Result<Response, AppError> {
    if state.secret_key.is_none() {
        return Ok(Redirect::to("/admin/sso?flash=set+LETS_CHAT_SECRET_KEY+first").into_response());
    }
    let (sidebar_rooms, sidebar_peers, switcher) = super::load_chrome(&state, &user, None).await?;
    let page = SsoEditPage {
        user: &user,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        git_version: version::GIT_VERSION,
        build_date: version::BUILD_DATE,
        section: "sso",
        editing: false,
        id: String::new(),
        display_name: String::new(),
        issuer_url: String::new(),
        client_id: String::new(),
        scopes: "openid email profile".into(),
        allow_signup: false,
        auto_link_verified_email: true,
        attribute_map: AttributeMapForm::default(),
        enabled: false,
        test_result: None,
        error: None,
        flash: None,
    };
    Ok(html(&page)?.into_response())
}

fn extract_attribute_map(f: &CreateForm) -> AttributeMapForm {
    AttributeMapForm {
        email_claim: f.email_claim.clone().unwrap_or_default(),
        email_verified_claim: f.email_verified_claim.clone().unwrap_or_default(),
        name_claim: f.name_claim.clone().unwrap_or_default(),
        username_claim: f.username_claim.clone().unwrap_or_default(),
        groups_claim: f.groups_claim.clone().unwrap_or_default(),
    }
}

fn serialize_attribute_map(m: &AttributeMapForm) -> String {
    let mut obj = serde_json::Map::new();
    if !m.email_claim.is_empty() {
        obj.insert("email_claim".into(), m.email_claim.clone().into());
    }
    if !m.email_verified_claim.is_empty() {
        obj.insert(
            "email_verified_claim".into(),
            m.email_verified_claim.clone().into(),
        );
    }
    if !m.name_claim.is_empty() {
        obj.insert("name_claim".into(), m.name_claim.clone().into());
    }
    if !m.username_claim.is_empty() {
        obj.insert("username_claim".into(), m.username_claim.clone().into());
    }
    if !m.groups_claim.is_empty() {
        obj.insert("groups_claim".into(), m.groups_claim.clone().into());
    }
    serde_json::Value::Object(obj).to_string()
}

fn deserialize_attribute_map(json: &str) -> AttributeMapForm {
    let obj: serde_json::Value = serde_json::from_str(json).unwrap_or(serde_json::Value::Null);
    let get = |k: &str| {
        obj.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    AttributeMapForm {
        email_claim: get("email_claim"),
        email_verified_claim: get("email_verified_claim"),
        name_claim: get("name_claim"),
        username_claim: get("username_claim"),
        groups_claim: get("groups_claim"),
    }
}

#[derive(Deserialize)]
pub struct CreateForm {
    pub id: String,
    pub display_name: String,
    pub issuer_url: String,
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default)]
    pub scopes: String,
    #[serde(default)]
    pub email_claim: Option<String>,
    #[serde(default)]
    pub email_verified_claim: Option<String>,
    #[serde(default)]
    pub name_claim: Option<String>,
    #[serde(default)]
    pub username_claim: Option<String>,
    #[serde(default)]
    pub groups_claim: Option<String>,
    #[serde(default)]
    pub allow_signup: Option<String>,
    #[serde(default)]
    pub auto_link_verified_email: Option<String>,
}

fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        && s.chars()
            .next()
            .map(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            .unwrap_or(false)
}

pub async fn post_create(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Form(f): Form<CreateForm>,
) -> Result<Response, AppError> {
    let key = state
        .secret_key
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("LETS_CHAT_SECRET_KEY not set".into()))?;
    if !valid_slug(&f.id) {
        return Err(AppError::BadRequest(
            "slug must be lowercase alphanumeric, _ or -".into(),
        ));
    }
    if f.client_secret.is_empty() {
        return Err(AppError::BadRequest(
            "client_secret is required on create".into(),
        ));
    }
    let encrypted = secret::encrypt_client_secret(key.as_ref(), &f.client_secret)
        .map_err(|e| AppError::Internal(format!("encrypt secret: {e}")))?;
    let scopes = if f.scopes.trim().is_empty() {
        "openid email profile".to_string()
    } else {
        f.scopes.trim().to_string()
    };
    let map = extract_attribute_map(&f);
    let attribute_map_json = serialize_attribute_map(&map);
    sso_providers::insert_provider(
        &state.auth,
        InsertProvider {
            id: &f.id,
            kind: "oidc",
            display_name: &f.display_name,
            issuer_url: &f.issuer_url,
            client_id: &f.client_id,
            client_secret_encrypted: &encrypted,
            scopes: &scopes,
            attribute_map_json: &attribute_map_json,
            allow_signup: f.allow_signup.as_deref() == Some("1"),
            auto_link_verified_email: f.auto_link_verified_email.as_deref() == Some("1"),
        },
    )
    .await?;
    state.sso.reload(&state.auth).await?;
    Ok(Redirect::to(&format!("/admin/sso/{}?flash=created", f.id)).into_response())
}

#[derive(Deserialize)]
pub struct EditFlash {
    pub flash: Option<String>,
    pub test: Option<String>,
}

pub async fn get_edit(
    State(state): State<AppState>,
    AdminUser(user): AdminUser,
    Path(id): Path<String>,
    Query(q): Query<EditFlash>,
) -> Result<Response, AppError> {
    let Some(row) = sso_providers::get_provider_by_id(&state.auth, &id).await? else {
        return Err(AppError::NotFound);
    };
    let (sidebar_rooms, sidebar_peers, switcher) = super::load_chrome(&state, &user, None).await?;
    let test_result = q.test.as_deref().map(|s| match s {
        "ok" => DiscoveryTestResult {
            ok: true,
            message: "Discovery succeeded.".into(),
            authorization_endpoint: None,
            token_endpoint: None,
            userinfo_endpoint: None,
            jwks_uri: None,
        },
        msg => DiscoveryTestResult {
            ok: false,
            message: format!("Discovery failed: {msg}"),
            authorization_endpoint: None,
            token_endpoint: None,
            userinfo_endpoint: None,
            jwks_uri: None,
        },
    });
    let page = SsoEditPage {
        user: &user,
        sidebar_rooms: &sidebar_rooms,
        sidebar_peers: &sidebar_peers,
        switcher: &switcher,
        asset_version: &state.asset_version,
        app_version: version::VERSION,
        git_hash: version::GIT_HASH,
        git_version: version::GIT_VERSION,
        build_date: version::BUILD_DATE,
        section: "sso",
        editing: true,
        id: row.id.clone(),
        display_name: row.display_name.clone(),
        issuer_url: row.issuer_url.clone(),
        client_id: row.client_id.clone(),
        scopes: row.scopes.clone(),
        allow_signup: row.allow_signup,
        auto_link_verified_email: row.auto_link_verified_email,
        attribute_map: deserialize_attribute_map(&row.attribute_map_json),
        enabled: row.is_enabled(),
        test_result,
        error: None,
        flash: q.flash.clone(),
    };
    Ok(html(&page)?.into_response())
}

#[derive(Deserialize)]
pub struct EditForm {
    pub action: Option<String>,
    pub display_name: String,
    pub issuer_url: String,
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default)]
    pub scopes: String,
    #[serde(default)]
    pub email_claim: Option<String>,
    #[serde(default)]
    pub email_verified_claim: Option<String>,
    #[serde(default)]
    pub name_claim: Option<String>,
    #[serde(default)]
    pub username_claim: Option<String>,
    #[serde(default)]
    pub groups_claim: Option<String>,
    #[serde(default)]
    pub allow_signup: Option<String>,
    #[serde(default)]
    pub auto_link_verified_email: Option<String>,
}

pub async fn post_edit(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path(id): Path<String>,
    Form(f): Form<EditForm>,
) -> Result<Response, AppError> {
    let Some(_) = sso_providers::get_provider_by_id(&state.auth, &id).await? else {
        return Err(AppError::NotFound);
    };
    if f.action.as_deref() == Some("test") {
        let http = reqwest::Client::new();
        let url = match url::Url::parse(&f.issuer_url) {
            Ok(u) => u,
            Err(e) => {
                return Ok(Redirect::to(&format!(
                    "/admin/sso/{id}?test=bad+issuer+url+{}",
                    urlencode(&e.to_string())
                ))
                .into_response());
            }
        };
        match discovery::discover(&url, &http).await {
            Ok(_) => Ok(Redirect::to(&format!("/admin/sso/{id}?test=ok")).into_response()),
            Err(e) => Ok(Redirect::to(&format!(
                "/admin/sso/{id}?test={}",
                urlencode(&e.to_string())
            ))
            .into_response()),
        }
    } else {
        let encrypted_opt = if f.client_secret.is_empty() {
            None
        } else {
            let key = state.secret_key.as_ref().ok_or_else(|| {
                AppError::BadRequest("LETS_CHAT_SECRET_KEY not set; cannot rotate secret".into())
            })?;
            Some(
                secret::encrypt_client_secret(key.as_ref(), &f.client_secret)
                    .map_err(|e| AppError::Internal(format!("encrypt secret: {e}")))?,
            )
        };
        let scopes = if f.scopes.trim().is_empty() {
            "openid email profile".to_string()
        } else {
            f.scopes.trim().to_string()
        };
        let map = AttributeMapForm {
            email_claim: f.email_claim.unwrap_or_default(),
            email_verified_claim: f.email_verified_claim.unwrap_or_default(),
            name_claim: f.name_claim.unwrap_or_default(),
            username_claim: f.username_claim.unwrap_or_default(),
            groups_claim: f.groups_claim.unwrap_or_default(),
        };
        let attribute_map_json = serialize_attribute_map(&map);
        sso_providers::update_provider(
            &state.auth,
            &id,
            UpdateProvider {
                display_name: &f.display_name,
                issuer_url: &f.issuer_url,
                client_id: &f.client_id,
                client_secret_encrypted: encrypted_opt.as_deref(),
                scopes: &scopes,
                attribute_map_json: &attribute_map_json,
                allow_signup: f.allow_signup.as_deref() == Some("1"),
                auto_link_verified_email: f.auto_link_verified_email.as_deref() == Some("1"),
            },
        )
        .await?;
        state.sso.reload(&state.auth).await?;
        Ok(Redirect::to(&format!("/admin/sso/{id}?flash=saved")).into_response())
    }
}

pub async fn post_enable(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let n = sso_providers::set_provider_enabled(&state.auth, &id, true).await?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    state.sso.reload(&state.auth).await?;
    Ok(Redirect::to(&format!("/admin/sso/{id}?flash=enabled")).into_response())
}

pub async fn post_disable(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let n = sso_providers::set_provider_enabled(&state.auth, &id, false).await?;
    if n == 0 {
        return Err(AppError::NotFound);
    }
    state.sso.reload(&state.auth).await?;
    Ok(Redirect::to(&format!("/admin/sso/{id}?flash=disabled")).into_response())
}

pub async fn post_delete(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let Some(row) = sso_providers::get_provider_by_id(&state.auth, &id).await? else {
        return Err(AppError::NotFound);
    };
    let linked = sso_providers::count_identities_for_issuer(&state.auth, &row.issuer_url).await?;
    if linked > 0 {
        return Err(AppError::Conflict(format!(
            "refusing to delete: {linked} user(s) are still linked to this provider"
        )));
    }
    sso_providers::delete_provider(&state.auth, &id).await?;
    state.sso.reload(&state.auth).await?;
    Ok(Redirect::to("/admin/sso?flash=deleted").into_response())
}

fn urlencode(s: &str) -> String {
    // Tiny inline encoder for the test-flash query string. Replaces
    // every byte that isn't alphanumeric / `-` / `_` / `.` / `~`
    // with %XX. Sufficient for an error message round-trip; never
    // round-trips through the body of an actual request.
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
