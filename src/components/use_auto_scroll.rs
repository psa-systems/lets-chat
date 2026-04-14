use dioxus::prelude::*;

use crate::models::Message;

/// Handle returned by [`use_auto_scroll`]. Components attach `container_id`
/// to their scrollable `<div>`, render the `↑ New messages` divider above
/// the message whose id equals `first_unseen_id()`, and render the
/// `↓ New messages` pill when `show_new_pill()` is true.
#[derive(Clone)]
pub struct AutoScroll {
    pub container_id: String,
    pub show_new_pill: Signal<bool>,
    pub first_unseen_id: Signal<Option<i64>>,
    pub scroll_to_bottom: Callback<()>,
}

/// Manage scroll position for a chat message list.
///
/// - On first non-empty render after a room change, scrolls to the first
///   unseen message (or to the bottom if nothing is unseen).
/// - On message append, stays pinned to the bottom if the user is already
///   near the bottom; otherwise shows the "new messages" pill.
/// - Tracks the highest-seen message id in `localStorage` keyed by room.
pub fn use_auto_scroll(room_id: Signal<i64>, messages: Signal<Vec<Message>>) -> AutoScroll {
    let _ = messages;

    let show_new_pill = use_signal(|| false);
    let first_unseen_id = use_signal(|| Option::<i64>::None);

    let scroll_to_bottom = use_callback(move |_: ()| {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = room_id;
            // Filled in by a later task.
        }
    });

    let container_id = {
        let id = *room_id.peek();
        format!("chat-scroll-{}", id)
    };

    AutoScroll {
        container_id,
        show_new_pill,
        first_unseen_id,
        scroll_to_bottom,
    }
}

const LAST_SEEN_PREFIX: &str = "lets-chat:last-seen:";

#[cfg(target_arch = "wasm32")]
fn read_last_seen(room_id: i64) -> Option<i64> {
    let storage = web_sys::window()?.local_storage().ok().flatten()?;
    let key = format!("{LAST_SEEN_PREFIX}{room_id}");
    let raw = storage.get_item(&key).ok().flatten()?;
    raw.parse::<i64>().ok()
}

#[cfg(target_arch = "wasm32")]
fn write_last_seen(room_id: i64, message_id: i64) {
    let Some(window) = web_sys::window() else { return };
    let Ok(Some(storage)) = window.local_storage() else { return };
    let key = format!("{LAST_SEEN_PREFIX}{room_id}");
    let _ = storage.set_item(&key, &message_id.to_string());
}

#[cfg(not(target_arch = "wasm32"))]
fn read_last_seen(_room_id: i64) -> Option<i64> {
    None
}

#[cfg(not(target_arch = "wasm32"))]
fn write_last_seen(_room_id: i64, _message_id: i64) {}
