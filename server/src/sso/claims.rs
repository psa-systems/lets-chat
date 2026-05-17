//! Per-provider attribute mapping. Resolves logical claim names
//! (`email`, `email_verified`, `name`, `username`, `groups`) onto the
//! IdP's actual wire-format names.
//!
//! Empty / missing entries in the configured `attribute_map_json` fall
//! back to the OIDC defaults so most providers need no overrides. The
//! map deserialises a partial JSON object; unknown keys are ignored
//! so future fields can be added without breaking older configs.

use serde::Deserialize;
use serde_json::{Map, Value};

/// Logical-to-wire claim-name map. One per provider row in
/// `sso_providers`. Defaults match the OIDC core spec + the de facto
/// `groups` claim used by Keycloak / Authentik / Entra.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ClaimMap {
    pub email_claim: String,
    pub email_verified_claim: String,
    pub name_claim: String,
    pub username_claim: String,
    pub groups_claim: String,
}

impl Default for ClaimMap {
    fn default() -> Self {
        Self {
            email_claim: "email".into(),
            email_verified_claim: "email_verified".into(),
            name_claim: "name".into(),
            username_claim: "preferred_username".into(),
            groups_claim: "groups".into(),
        }
    }
}

impl ClaimMap {
    /// Parse from `sso_providers.attribute_map_json`. Unknown keys are
    /// ignored. Empty-string overrides fall back to the default; this
    /// matches the admin-UI behaviour where leaving the input blank
    /// means "use the default for this row" rather than "configure an
    /// empty claim name and break sign-in."
    pub fn from_json(s: &str) -> Self {
        // Parse to a generic JSON value first so we can apply the
        // empty-string-means-default rule on a per-field basis. A
        // direct serde derive can't express that.
        #[derive(Deserialize, Default)]
        struct Partial {
            #[serde(default)]
            email_claim: Option<String>,
            #[serde(default)]
            email_verified_claim: Option<String>,
            #[serde(default)]
            name_claim: Option<String>,
            #[serde(default)]
            username_claim: Option<String>,
            #[serde(default)]
            groups_claim: Option<String>,
        }
        let parsed: Partial = serde_json::from_str(s).unwrap_or_default();
        let mut out = ClaimMap::default();
        if let Some(v) = parsed.email_claim.filter(|s| !s.is_empty()) {
            out.email_claim = v;
        }
        if let Some(v) = parsed.email_verified_claim.filter(|s| !s.is_empty()) {
            out.email_verified_claim = v;
        }
        if let Some(v) = parsed.name_claim.filter(|s| !s.is_empty()) {
            out.name_claim = v;
        }
        if let Some(v) = parsed.username_claim.filter(|s| !s.is_empty()) {
            out.username_claim = v;
        }
        if let Some(v) = parsed.groups_claim.filter(|s| !s.is_empty()) {
            out.groups_claim = v;
        }
        out
    }
}

/// Result of applying a [`ClaimMap`] to a raw id_token claim set.
/// Fields are `None` when the configured claim name was absent in the
/// payload or had the wrong JSON type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtractedClaims {
    pub email: Option<String>,
    pub email_verified: Option<bool>,
    pub name: Option<String>,
    pub username: Option<String>,
    pub groups: Option<Vec<String>>,
}

/// Project a configured claim map onto a raw id_token payload. Only
/// reads top-level claim names; nested-path lookups are deferred (see
/// doc 10 section 2).
pub fn extract(raw: &Map<String, Value>, map: &ClaimMap) -> ExtractedClaims {
    ExtractedClaims {
        email: raw
            .get(&map.email_claim)
            .and_then(|v| v.as_str())
            .map(str::to_string),
        email_verified: raw.get(&map.email_verified_claim).and_then(|v| v.as_bool()),
        name: raw
            .get(&map.name_claim)
            .and_then(|v| v.as_str())
            .map(str::to_string),
        username: raw
            .get(&map.username_claim)
            .and_then(|v| v.as_str())
            .map(str::to_string),
        groups: raw
            .get(&map.groups_claim)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|g| g.as_str().map(str::to_string))
                    .collect()
            }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn raw(body: serde_json::Value) -> Map<String, Value> {
        body.as_object().unwrap().clone()
    }

    #[test]
    fn defaults_apply_when_attribute_map_is_empty() {
        let m = ClaimMap::from_json("{}");
        assert_eq!(m, ClaimMap::default());
    }

    #[test]
    fn malformed_json_falls_back_to_defaults() {
        let m = ClaimMap::from_json("not json");
        assert_eq!(m, ClaimMap::default());
    }

    #[test]
    fn overrides_resolve_to_non_default_names() {
        let m = ClaimMap::from_json(
            r#"{"email_claim":"mail","groups_claim":"roles","name_claim":"display"}"#,
        );
        assert_eq!(m.email_claim, "mail");
        assert_eq!(m.groups_claim, "roles");
        assert_eq!(m.name_claim, "display");
        // Unset fields stay at default.
        assert_eq!(m.email_verified_claim, "email_verified");
        assert_eq!(m.username_claim, "preferred_username");
    }

    #[test]
    fn empty_string_override_falls_back_to_default() {
        let m = ClaimMap::from_json(r#"{"email_claim":""}"#);
        assert_eq!(m.email_claim, "email");
    }

    #[test]
    fn extract_reads_defaults_when_map_is_default() {
        let r = raw(json!({
            "email": "a@x",
            "email_verified": true,
            "name": "Alice",
            "preferred_username": "alice",
            "groups": ["admins", "users"],
        }));
        let claims = extract(&r, &ClaimMap::default());
        assert_eq!(claims.email.as_deref(), Some("a@x"));
        assert_eq!(claims.email_verified, Some(true));
        assert_eq!(claims.name.as_deref(), Some("Alice"));
        assert_eq!(claims.username.as_deref(), Some("alice"));
        assert_eq!(
            claims.groups,
            Some(vec!["admins".to_string(), "users".to_string()])
        );
    }

    #[test]
    fn extract_honours_overrides() {
        let r = raw(json!({
            "mail": "a@x",
            "display": "Alice",
            "roles": ["admins"],
        }));
        let m = ClaimMap {
            email_claim: "mail".into(),
            name_claim: "display".into(),
            groups_claim: "roles".into(),
            ..ClaimMap::default()
        };
        let claims = extract(&r, &m);
        assert_eq!(claims.email.as_deref(), Some("a@x"));
        assert_eq!(claims.name.as_deref(), Some("Alice"));
        assert_eq!(claims.groups, Some(vec!["admins".to_string()]));
        // Default-named claims missing => None.
        assert_eq!(claims.username, None);
    }

    #[test]
    fn extract_returns_none_for_wrong_json_types() {
        let r = raw(json!({
            "email": 42,
            "email_verified": "not-a-bool",
            "groups": "not-an-array",
        }));
        let claims = extract(&r, &ClaimMap::default());
        assert_eq!(claims.email, None);
        assert_eq!(claims.email_verified, None);
        assert_eq!(claims.groups, None);
    }

    #[test]
    fn extract_skips_non_string_group_entries() {
        let r = raw(json!({
            "groups": ["admins", 42, "users"],
        }));
        let claims = extract(&r, &ClaimMap::default());
        assert_eq!(
            claims.groups,
            Some(vec!["admins".to_string(), "users".to_string()])
        );
    }
}
