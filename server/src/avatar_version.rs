//! LC-781 (F11): in-process avatar version mirror.
//!
//! Server-rendered avatar URLs are `/avatars/{id}?v={token}` (the `avatar_url`
//! Askama filter). The token is the avatar file's mtime, so it changes on every
//! upload - preserving LC-348's freshness guarantee - while letting
//! `routes::avatar` answer `Cache-Control: public, max-age=31536000, immutable`
//! and the browser skip the per-navigation revalidation a bare `/avatars/{id}`
//! forces (one conditional request + a `find_user_by_id` + a `stat` per distinct
//! author, per navigation).
//!
//! Computing the token needs a filesystem `stat`, but the filter is synchronous
//! and hot (every rendered avatar). So tokens are cached here after the first
//! stat and invalidated on upload, rather than stat'd on every render.

use std::sync::OnceLock;
use std::time::UNIX_EPOCH;

use dashmap::DashMap;

use crate::db;

fn mirror() -> &'static DashMap<String, String> {
    static M: OnceLock<DashMap<String, String>> = OnceLock::new();
    M.get_or_init(DashMap::new)
}

/// Avatar file extensions the upload path accepts (`sniff_image_ext` in
/// `routes::settings`). `version_of` probes these because the render sites carry
/// only a user id, not the stored extension.
const EXTS: [&str; 3] = ["png", "jpg", "webp"];

/// The version token for `user_id`'s avatar, cached after the first stat.
///
/// A user WITH a custom avatar gets the file's mtime, which changes on every
/// upload (LC-348 freshness) and lets `routes::avatar` answer `immutable`. A
/// user WITHOUT one gets `"0"` - harmless, because the route serves their
/// username-derived default SVG with `no-cache` regardless of the `?v`, so a
/// later username change still reflects immediately. Either way a matching
/// [`invalidate`] on the next avatar write re-probes and moves the token.
pub fn version_of(user_id: &str) -> String {
    if let Some(v) = mirror().get(user_id) {
        return v.clone();
    }
    let token = compute(user_id);
    mirror().insert(user_id.to_string(), token.clone());
    token
}

fn compute(user_id: &str) -> String {
    let dir = db::avatars_dir();
    for ext in EXTS {
        let mtime = std::fs::metadata(dir.join(format!("{user_id}.{ext}")))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok());
        if let Some(d) = mtime {
            return d.as_nanos().to_string();
        }
    }
    "0".to_string()
}

/// Drop the cached token for `user_id` so the next render re-stats the file and
/// picks up the new mtime. Call after any write to the user's avatar (upload or
/// removal), so a changed image busts the immutable cache on the next paint.
pub fn invalidate(user_id: &str) {
    mirror().remove(user_id);
}
