//! LC-260: quick switcher (Ctrl/Cmd+K). `GET /switcher?q=&enclave_id=` returns
//! a flat, access-correct, capped list of rooms (current enclave), the viewer's
//! DM conversations, and people, for the palette modal. Reuses the existing
//! room/DM/people queries the sidebar and people-search already use.

use std::collections::HashSet;

use axum::extract::{Query, State};
use serde::Deserialize;

use crate::auth::AuthUser;
use crate::db;
use crate::error::AppError;
use crate::state::AppState;
use crate::views::switcher::{SwitcherItem, SwitcherResults};
use crate::views::{html, Html};

/// Per-group cap so one busy group cannot crowd out the others.
const PER_GROUP: usize = 8;

#[derive(Deserialize)]
pub struct SwitcherQuery {
    pub q: Option<String>,
    /// String (not `i64`) so an empty `enclave_id=` from the home view (no
    /// active enclave) parses cleanly instead of 400-ing on an empty integer.
    pub enclave_id: Option<String>,
}

fn label_for(rec: &crate::models::user::UserRecord) -> String {
    match rec.display_name.as_deref() {
        Some(n) if !n.trim().is_empty() => n.to_string(),
        _ => rec.username.clone(),
    }
}

/// `GET /switcher`
pub async fn get_switcher(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(SwitcherQuery { q, enclave_id }): Query<SwitcherQuery>,
) -> Result<Html, AppError> {
    let query = q.unwrap_or_default();
    let needle = query.trim().to_lowercase();
    let has_q = !needle.is_empty();
    let enclave_id = enclave_id.and_then(|s| s.trim().parse::<i64>().ok());
    let is_admin = user.role == "admin";

    let mut items: Vec<SwitcherItem> = Vec::new();

    // Rooms in the current enclave the viewer can see (same query the sidebar
    // uses), name-filtered by the query.
    if let Some(eid) = enclave_id {
        let rooms = db::chat::list_rooms_in_enclave(&state.chat, eid, &user.id, is_admin).await?;
        for r in rooms
            .into_iter()
            .filter(|r| !has_q || r.name.to_lowercase().contains(&needle))
            .take(PER_GROUP)
        {
            let opt_id = format!("lc-switch-opt-{}", items.len());
            items.push(SwitcherItem {
                href: format!("/room/{}", r.id),
                glyph: if r.is_voice {
                    "\u{1F50A}".into()
                } else {
                    "#".into()
                },
                label: r.name,
                sublabel: String::new(),
                opt_id,
            });
        }
    }

    // The viewer's DM conversations. Track every DM peer id (even ones the
    // query filtered out) so the people group below never re-lists a peer the
    // viewer already DMs - both rows would point at the same `/dm/{id}`.
    let dm_rooms = db::chat::list_user_dm_rooms(&state.chat, &user.id).await?;
    let mut dm_peer_ids: HashSet<String> = HashSet::new();
    let mut dm_shown = 0usize;
    for (_room, peer_id) in &dm_rooms {
        if db::auth::is_blocked_either_way(&state.auth, &user.id, peer_id).await? {
            continue;
        }
        dm_peer_ids.insert(peer_id.clone());
        if dm_shown >= PER_GROUP {
            continue;
        }
        let Some(rec) = db::auth::find_user_by_id(&state.auth, peer_id).await? else {
            continue;
        };
        let label = label_for(&rec);
        if has_q
            && !label.to_lowercase().contains(&needle)
            && !rec.username.to_lowercase().contains(&needle)
        {
            continue;
        }
        let opt_id = format!("lc-switch-opt-{}", items.len());
        items.push(SwitcherItem {
            href: format!("/dm/{}", rec.id),
            glyph: "@".into(),
            label,
            sublabel: format!("@{}", rec.username),
            opt_id,
        });
        dm_shown += 1;
    }

    // People (only when querying) so the viewer can start a NEW DM. Excludes
    // self and anyone already shown as a DM.
    if has_q {
        let people =
            db::auth::search_users(&state.auth, query.trim(), &user.id, (PER_GROUP as i64) * 2)
                .await?;
        let mut shown = 0usize;
        for rec in people {
            if shown >= PER_GROUP {
                break;
            }
            if rec.id == user.id || dm_peer_ids.contains(&rec.id) {
                continue;
            }
            if db::auth::is_blocked_either_way(&state.auth, &user.id, &rec.id).await? {
                continue;
            }
            let opt_id = format!("lc-switch-opt-{}", items.len());
            items.push(SwitcherItem {
                href: format!("/dm/{}", rec.id),
                glyph: "@".into(),
                label: label_for(&rec),
                sublabel: format!("@{}", rec.username),
                opt_id,
            });
            shown += 1;
        }
    }

    html(&SwitcherResults { items })
}
