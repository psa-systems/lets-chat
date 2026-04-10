use dioxus::prelude::*;

use crate::components::use_websocket::WsHandle;
use crate::models::User;
use crate::server_fns::chat::list_messages;
use crate::server_fns::dm::{get_or_create_dm, send_dm_message};
use crate::ws::events::ChatEvent;

#[component]
pub fn DmViewPage(user_id: String) -> Element {
    let current_user: Signal<User> = use_context::<Signal<User>>();
    let u = current_user();

    // Resolve or create the DM room
    let dm_room = use_server_future(move || {
        let uid = user_id.clone();
        async move { get_or_create_dm(uid).await }
    })?;

    let room = match dm_room() {
        Some(Ok(r)) => r,
        Some(Err(e)) => {
            return rsx! {
                div { class: "flex-1 flex items-center justify-center text-red-500",
                    "Error: {e}"
                }
            };
        }
        None => {
            return rsx! {
                div { class: "flex-1 flex items-center justify-center text-gray-500",
                    "Loading DM..."
                }
            };
        }
    };

    let room_id = room.id;

    let mut messages_version = use_signal(|| 0u32);
    let messages = use_server_future(move || {
        let _v = messages_version();
        async move { list_messages(room_id).await }
    })?;

    let ws = use_context::<WsHandle>();

    // Subscribe to this DM room's WS events
    let ws_sub = ws.clone();
    use_effect(move || {
        ws_sub.subscribe(room_id);
    });

    let ws_drop = ws.clone();
    use_drop(move || {
        ws_drop.unsubscribe(room_id);
    });

    // When a WS event arrives for this room, bump messages_version
    use_effect(move || {
        if let Some(ref event) = *ws.latest_event.read() {
            match event {
                ChatEvent::NewMessage { message, .. } if message.room_id == room_id => {
                    messages_version.set(messages_version() + 1);
                }
                _ => {}
            }
        }
    });

    let mut draft = use_signal(String::new);
    let mut error = use_signal(|| Option::<String>::None);

    let message_list = match messages() {
        Some(Ok(list)) => list,
        Some(Err(e)) => {
            return rsx! {
                div { class: "flex-1 flex items-center justify-center text-red-500",
                    "Failed to load messages: {e}"
                }
            };
        }
        None => vec![],
    };

    // Extract other user's name from room name (dm-<user1>-<user2>)
    let other_name = room.name
        .strip_prefix("dm-")
        .unwrap_or(&room.name)
        .split('-')
        .find(|part| *part != u.username)
        .unwrap_or("DM")
        .to_string();

    let is_muted = u.is_muted;
    let mute_message = if is_muted {
        if let Some(ref until) = u.muted_until {
            format!("You are muted until {}", until)
        } else {
            "You are muted".to_string()
        }
    } else {
        String::new()
    };

    rsx! {
        // Header
        header { class: "px-6 py-3 border-b border-gray-200 bg-white",
            div { class: "flex items-baseline gap-3",
                h2 { class: "text-lg font-semibold text-gray-800", "DM with {other_name}" }
            }
        }

        // Message list
        div { class: "flex-1 overflow-y-auto px-6 py-4 space-y-3",
            if message_list.is_empty() {
                div { class: "text-center text-gray-400 mt-12",
                    "No messages yet — say hello!"
                }
            } else {
                for msg in message_list.iter() {
                    div { key: "{msg.id}", class: "flex flex-col",
                        div { class: "flex items-baseline gap-2",
                            span { class: "font-semibold text-gray-800", "{msg.author_name}" }
                            span { class: "text-xs text-gray-400", "{msg.created_at}" }
                        }
                        p { class: "text-gray-700 whitespace-pre-wrap", "{msg.body}" }
                    }
                }
            }
        }

        // Composer
        if is_muted {
            div { class: "px-6 py-3 border-t border-gray-200 bg-yellow-50 text-center",
                span { class: "text-sm text-yellow-700", "{mute_message}" }
            }
        } else {
            form {
                class: "px-6 py-3 border-t border-gray-200 bg-white",
                onsubmit: move |evt: Event<FormData>| {
                    evt.prevent_default();
                    let body = draft();
                    if body.trim().is_empty() {
                        return;
                    }
                    spawn(async move {
                        match send_dm_message(room_id, body).await {
                            Ok(_) => {
                                draft.set(String::new());
                                error.set(None);
                                messages_version.set(messages_version() + 1);
                            }
                            Err(e) => {
                                error.set(Some(e.to_string()));
                            }
                        }
                    });
                },
                if let Some(err) = error() {
                    div { class: "mb-2 text-sm text-red-600", "{err}" }
                }
                div { class: "flex items-center gap-2",
                    input {
                        class: "flex-1 px-3 py-1.5 border border-gray-300 rounded",
                        r#type: "text",
                        placeholder: "Type a message…",
                        value: "{draft}",
                        oninput: move |evt| draft.set(evt.value()),
                    }
                    button {
                        class: "px-4 py-1.5 bg-blue-600 text-white rounded hover:bg-blue-700",
                        r#type: "submit",
                        "Send"
                    }
                }
            }
        }
    }
}
