//! Per-scope branding (LC-96).
//!
//! See `chat/0034_branding.sql` for the schema. A row is identified
//! by `(scope_kind, scope_id)`; the global scope sits at
//! `('global', 0)` and is always present (seeded by the migration).
//! Per-enclave rows are created on first save; resolution falls back
//! to the global row when an enclave has no row of its own.

use serde::Serialize;
use sqlx::{Row, SqlitePool};

/// Default brand colors when the global row was never touched.
/// These match the Tailwind `blue-600` / `blue-700` pair the rest of
/// the UI uses, so a deployment with no operator-set branding looks
/// the same as it did before LC-96 landed.
pub const DEFAULT_PRIMARY: &str = "#2563eb";
pub const DEFAULT_ACCENT: &str = "#1d4ed8";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    Enclave(i64),
}

impl Scope {
    fn parts(self) -> (&'static str, i64) {
        match self {
            Scope::Global => ("global", 0),
            Scope::Enclave(id) => ("enclave", id),
        }
    }
}

/// Resolved branding: every field is populated. Missing logos render
/// as `None`; missing colors fall back to [`DEFAULT_PRIMARY`] /
/// [`DEFAULT_ACCENT`]; missing text fields render as empty strings
/// (the layout/login template handles the empty case).
#[derive(Debug, Clone, Serialize)]
pub struct Branding {
    pub scope_kind: String,
    pub scope_id: i64,
    pub logo_upload_id: Option<i64>,
    /// LC-142: uploads.id of the custom favicon, or `None` to use the
    /// static `/assets/favicon.svg`. Global-only in v1.
    pub favicon_upload_id: Option<i64>,
    pub primary_color: String,
    pub accent_color: String,
    pub login_heading: String,
    pub login_body: String,
    pub updated_at: String,
}

impl Branding {
    pub fn defaults_for_global() -> Self {
        Self {
            scope_kind: "global".to_string(),
            scope_id: 0,
            logo_upload_id: None,
            favicon_upload_id: None,
            primary_color: DEFAULT_PRIMARY.to_string(),
            accent_color: DEFAULT_ACCENT.to_string(),
            login_heading: String::new(),
            login_body: String::new(),
            updated_at: String::new(),
        }
    }
}

fn map_row(r: sqlx::sqlite::SqliteRow) -> Branding {
    Branding {
        scope_kind: r.get("scope_kind"),
        scope_id: r.get("scope_id"),
        logo_upload_id: r.get("logo_upload_id"),
        favicon_upload_id: r.get("favicon_upload_id"),
        primary_color: r.get("primary_color"),
        accent_color: r.get("accent_color"),
        login_heading: r.get("login_heading"),
        login_body: r.get("login_body"),
        updated_at: r.get("updated_at"),
    }
}

/// Fetch a specific row by scope. `None` when no row exists.
pub async fn get(pool: &SqlitePool, scope: Scope) -> Result<Option<Branding>, sqlx::Error> {
    let (kind, id) = scope.parts();
    let row = sqlx::query(
        "SELECT scope_kind, scope_id, logo_upload_id, favicon_upload_id, primary_color, \
                accent_color, login_heading, login_body, updated_at \
         FROM branding WHERE scope_kind = ? AND scope_id = ?",
    )
    .bind(kind)
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(map_row))
}

/// Resolve the effective branding for `scope`: if the requested
/// scope has a row, return it; otherwise fall back to the global
/// row; otherwise (which should not happen given the migration
/// seeds global) fall back to in-memory defaults.
pub async fn resolve(pool: &SqlitePool, scope: Scope) -> Result<Branding, sqlx::Error> {
    if let Some(b) = get(pool, scope).await? {
        return Ok(b);
    }
    if scope != Scope::Global {
        if let Some(b) = get(pool, Scope::Global).await? {
            return Ok(b);
        }
    }
    Ok(Branding::defaults_for_global())
}

/// Upsert every editable field at once. The caller is responsible
/// for validating `primary_color` / `accent_color` (see
/// [`is_valid_hex_color`]) before calling.
#[allow(clippy::too_many_arguments)]
pub async fn upsert(
    pool: &SqlitePool,
    scope: Scope,
    logo_upload_id: Option<i64>,
    favicon_upload_id: Option<i64>,
    primary_color: &str,
    accent_color: &str,
    login_heading: &str,
    login_body: &str,
    updated_by: &str,
) -> Result<(), sqlx::Error> {
    let (kind, id) = scope.parts();
    sqlx::query(
        "INSERT INTO branding \
            (scope_kind, scope_id, logo_upload_id, favicon_upload_id, primary_color, accent_color, \
             login_heading, login_body, updated_at, updated_by) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, datetime('now'), ?) \
         ON CONFLICT(scope_kind, scope_id) DO UPDATE SET \
            logo_upload_id    = excluded.logo_upload_id, \
            favicon_upload_id = excluded.favicon_upload_id, \
            primary_color     = excluded.primary_color, \
            accent_color      = excluded.accent_color, \
            login_heading     = excluded.login_heading, \
            login_body        = excluded.login_body, \
            updated_at        = excluded.updated_at, \
            updated_by        = excluded.updated_by",
    )
    .bind(kind)
    .bind(id)
    .bind(logo_upload_id)
    .bind(favicon_upload_id)
    .bind(primary_color)
    .bind(accent_color)
    .bind(login_heading)
    .bind(login_body)
    .bind(updated_by)
    .execute(pool)
    .await?;
    Ok(())
}

/// Lightweight `#rrggbb` / `#rgb` validator. The admin form gives
/// operators a real color picker, which always emits 7-char hex; the
/// validation is here to guard against forged POSTs and to fail-fast
/// on a malformed manual submit.
pub fn is_valid_hex_color(s: &str) -> bool {
    if !s.starts_with('#') {
        return false;
    }
    let rest = &s[1..];
    matches!(rest.len(), 3 | 6) && rest.chars().all(|c| c.is_ascii_hexdigit())
}

/// Identify the active enclave id for a request path. Used by the
/// branding-injection middleware to pick which scope's colors to
/// stamp into the HTML response.
pub fn enclave_id_from_path(path: &str) -> Option<i64> {
    let mut segs = path.split('/').filter(|s| !s.is_empty());
    if segs.next()? != "enclave" {
        return None;
    }
    segs.next()?.parse::<i64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_color_validator() {
        assert!(is_valid_hex_color("#abc"));
        assert!(is_valid_hex_color("#abcdef"));
        assert!(is_valid_hex_color("#012345"));
        assert!(!is_valid_hex_color("abc"));
        assert!(!is_valid_hex_color("#abcd"));
        assert!(!is_valid_hex_color("#abcdefg"));
        assert!(!is_valid_hex_color("#zzzzzz"));
        assert!(!is_valid_hex_color(""));
    }
}
