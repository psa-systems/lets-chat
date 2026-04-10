use dioxus::prelude::*;

use crate::server_fns::chat::{get_room, list_messages, send_message};

#[component]
pub fn RoomViewPage(room_id: String) -> Element {
    let parsed_id: i64 = match room_id.parse() {
        Ok(id) => id,
        Err(_) => {
            return rsx! {
                div { class: "flex-1 flex items-center justify-center text-red-500",
                    "Invalid room id"
                }
            };
        }
    };

    let room = use_server_future(move || async move { get_room(parsed_id).await })?;

    let mut messages_version = use_signal(|| 0u32);
    let messages = use_server_future(move || {
        let _v = messages_version();
        async move { list_messages(parsed_id).await }
    })?;

    let mut draft = use_signal(String::new);
    let mut error = use_signal(|| Option::<String>::None);

    let room_name = match room() {
        Some(Ok(Some(r))) => r.name,
        _ => format!("room {}", parsed_id),
    };
    let room_topic = match room() {
        Some(Ok(Some(r))) => r.topic,
        _ => None,
    };

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

    rsx! {
        // Header
        header { class: "px-6 py-3 border-b border-gray-200 bg-white",
            div { class: "flex items-baseline gap-3",
                h2 { class: "text-lg font-semibold text-gray-800", "# {room_name}" }
                if let Some(topic) = room_topic {
                    span { class: "text-sm text-gray-500", "{topic}" }
                }
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
        form {
            class: "px-6 py-3 border-t border-gray-200 bg-white",
            onsubmit: move |evt: Event<FormData>| {
                evt.prevent_default();
                let body = draft();
                if body.trim().is_empty() {
                    return;
                }
                spawn(async move {
                    match send_message(parsed_id, body).await {
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
