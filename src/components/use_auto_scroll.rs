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
    let mut show_new_pill = use_signal(|| false);
    let mut first_unseen_id = use_signal(|| Option::<i64>::None);

    let mut initialized_for_room = use_signal(|| 0i64);
    let mut last_len = use_signal(|| 0usize);
    let mut last_max_id = use_signal(|| 0i64);

    use_effect(move || {
        let rid = room_id();
        let list = messages();

        if *initialized_for_room.peek() != rid {
            show_new_pill.set(false);
            first_unseen_id.set(None);
            last_len.set(0);
            last_max_id.set(0);
            initialized_for_room.set(rid);
        }

        let prev_len = *last_len.peek();
        let new_len = list.len();
        let newest_id = list.last().map(|m| m.id).unwrap_or(0);

        // First non-empty render for this room: decide initial scroll target.
        if prev_len == 0 && new_len > 0 {
            #[cfg(target_arch = "wasm32")]
            {
                let last_seen = read_last_seen(rid);
                let unseen = match last_seen {
                    Some(seen) => list.iter().find(|m| m.id > seen).map(|m| m.id),
                    None => None,
                };
                first_unseen_id.set(unseen);

                let container_id = format!("chat-scroll-{rid}");
                match unseen {
                    Some(id) => scroll_message_into_view(id),
                    None => {
                        scroll_container_to_bottom(&container_id);
                        write_last_seen(rid, newest_id);
                    }
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = (rid, newest_id);
                first_unseen_id.set(None);
            }

            last_len.set(new_len);
            last_max_id.set(newest_id);
            return;
        }

        // Appended messages (new message arrived) for this room.
        if new_len > prev_len && newest_id > *last_max_id.peek() {
            #[cfg(target_arch = "wasm32")]
            {
                let container_id = format!("chat-scroll-{rid}");
                let was_near_bottom = get_container(&container_id)
                    .map(|el| is_near_bottom(&el))
                    .unwrap_or(true);

                if was_near_bottom {
                    scroll_container_to_bottom(&container_id);
                    write_last_seen(rid, newest_id);
                    show_new_pill.set(false);
                } else {
                    show_new_pill.set(true);
                }
            }
            last_max_id.set(newest_id);
        }

        last_len.set(new_len);
    });

    let scroll_to_bottom = use_callback(move |_: ()| {
        let rid = *room_id.peek();
        let newest_id = messages.peek().last().map(|m| m.id).unwrap_or(0);
        #[cfg(target_arch = "wasm32")]
        {
            let container_id = format!("chat-scroll-{rid}");
            scroll_container_to_bottom(&container_id);
            if newest_id > 0 {
                write_last_seen(rid, newest_id);
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (rid, newest_id);
        }
        show_new_pill.set(false);
    });

    let container_id = {
        let id = *room_id.peek();
        format!("chat-scroll-{id}")
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

#[cfg(target_arch = "wasm32")]
const NEAR_BOTTOM_PX: f64 = 50.0;

#[cfg(target_arch = "wasm32")]
fn get_container(id: &str) -> Option<web_sys::HtmlElement> {
    use wasm_bindgen::JsCast;
    let doc = web_sys::window()?.document()?;
    let el = doc.get_element_by_id(id)?;
    el.dyn_into::<web_sys::HtmlElement>().ok()
}

#[cfg(target_arch = "wasm32")]
fn is_near_bottom(el: &web_sys::HtmlElement) -> bool {
    let scroll_top = el.scroll_top() as f64;
    let client_height = el.client_height() as f64;
    let scroll_height = el.scroll_height() as f64;
    scroll_top + client_height >= scroll_height - NEAR_BOTTOM_PX
}

#[cfg(target_arch = "wasm32")]
fn scroll_container_to_bottom(id: &str) {
    if let Some(el) = get_container(id) {
        el.set_scroll_top(el.scroll_height());
    }
}

#[cfg(target_arch = "wasm32")]
fn scroll_message_into_view(message_id: i64) {
    use wasm_bindgen::JsCast;
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let selector = format!("[data-msg-id=\"{message_id}\"]");
    let Ok(Some(el)) = doc.query_selector(&selector) else {
        return;
    };
    if let Ok(html) = el.dyn_into::<web_sys::HtmlElement>() {
        html.scroll_into_view_with_bool(true);
    }
}
