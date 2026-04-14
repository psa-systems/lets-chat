use dioxus::prelude::*;

use crate::components::use_auto_scroll::use_auto_scroll;
use crate::components::use_websocket::WsHandle;
use crate::models::{Message, User};
use crate::server_fns::chat::{edit_message, list_messages};
use crate::server_fns::dm::{
    get_dm_peer_read_state, get_or_create_dm, mark_dm_read, send_dm_message,
};
use crate::ws::events::ChatEvent;

#[component]
pub fn DmViewPage(user_id: String) -> Element {
    let current_user: Signal<Option<User>> = use_context::<Signal<Option<User>>>();
    let u = current_user().expect("user must be authenticated");

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
    let room_id_sig = use_signal(|| room_id);

    // Initial load from server — fetched once per DM room.
    let messages_fetch =
        use_server_future(move || async move { list_messages(room_id).await })?;

    let mut messages = use_signal(Vec::<Message>::new);
    let mut load_error = use_signal(|| Option::<String>::None);

    let auto = use_auto_scroll(room_id_sig, messages);
    let mut visibility_tick = use_signal(|| 0u32);

    use_effect(move || match messages_fetch() {
        Some(Ok(list)) => {
            messages.set(list);
            load_error.set(None);
        }
        Some(Err(e)) => {
            load_error.set(Some(crate::server_fns::auth::user_facing_error(&e)));
        }
        None => {}
    });

    let mut draft = use_signal(String::new);
    let mut error = use_signal(|| Option::<String>::None);
    let mut editing_msg_id = use_signal(|| Option::<i64>::None);
    let mut edit_draft = use_signal(String::new);
    let mut edit_error = use_signal(|| Option::<String>::None);
    let mut ws_peer_read = use_signal(|| Option::<(i64, String)>::None);

    let peer_state =
        use_server_future(move || async move { get_dm_peer_read_state(room_id).await })?;

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

    // Apply WS events directly to the local messages signal — no re-fetching.
    let my_id_for_ws = u.id.clone();
    use_effect(move || {
        if let Some(ref event) = *ws.latest_event.read() {
            match event {
                ChatEvent::NewMessage { message, .. } if message.room_id == room_id => {
                    let m = message.clone();
                    messages.with_mut(|v| {
                        if !v.iter().any(|x| x.id == m.id) {
                            v.push(m);
                        }
                    });
                }
                ChatEvent::MessageEdited {
                    room_id: event_room_id,
                    message_id,
                    new_body,
                    edited_at,
                } if *event_room_id == room_id => {
                    let mid = *message_id;
                    let body = new_body.clone();
                    let at = edited_at.clone();
                    messages.with_mut(|v| {
                        if let Some(m) = v.iter_mut().find(|m| m.id == mid) {
                            m.body = body;
                            m.edited_at = Some(at);
                        }
                    });
                }
                ChatEvent::MessageDeleted {
                    room_id: event_room_id,
                    message_id,
                } if *event_room_id == room_id => {
                    let mid = *message_id;
                    messages.with_mut(|v| v.retain(|m| m.id != mid));
                }
                ChatEvent::DmRead {
                    room_id: event_room_id,
                    user_id: event_user_id,
                    last_read_message_id,
                    read_at,
                } if *event_room_id == room_id && *event_user_id != my_id_for_ws => {
                    ws_peer_read.set(Some((*last_read_message_id, read_at.clone())));
                }
                _ => {}
            }
        }
    });

    let mut typing_users = use_signal(Vec::<(String, String)>::new);
    let mut last_typing_sent = use_signal(|| 0.0f64);
    let my_user_id = u.id.clone();

    // Handle typing indicator events
    use_effect(move || {
        if let Some(ref event) = *ws.latest_event.read() {
            match event {
                ChatEvent::UserTyping {
                    room_id: event_room_id,
                    user_id,
                    username,
                } if *event_room_id == room_id && *user_id != my_user_id => {
                    let uid = user_id.clone();
                    let name = username.clone();
                    typing_users.with_mut(|v| {
                        if !v.iter().any(|(id, _)| id == &uid) {
                            v.push((uid, name));
                        }
                    });
                }
                ChatEvent::UserStoppedTyping {
                    room_id: event_room_id,
                    user_id,
                } if *event_room_id == room_id => {
                    let uid = user_id.clone();
                    typing_users.with_mut(|v| v.retain(|(id, _)| id != &uid));
                }
                ChatEvent::NewMessage { message, .. } if message.room_id == room_id => {
                    let uid = message.user_id.clone();
                    typing_users.with_mut(|v| v.retain(|(id, _)| id != &uid));
                }
                _ => {}
            }
        }
    });

    let my_id_for_read = u.id.clone();
    use_effect(move || {
        let _tick = visibility_tick();
        let list = messages();
        if list.is_empty() {
            return;
        }
        let latest_peer = list.iter().rev().find(|m| m.user_id != my_id_for_read);
        let Some(latest) = latest_peer else { return };
        let latest_id = latest.id;

        #[cfg(target_arch = "wasm32")]
        {
            let visible = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| {
                    js_sys::Reflect::get(
                        d.as_ref(),
                        &wasm_bindgen::JsValue::from_str("visibilityState"),
                    )
                    .ok()
                })
                .and_then(|v| v.as_string())
                .map(|s| s == "visible")
                .unwrap_or(true);
            if !visible {
                return;
            }
        }

        spawn(async move {
            let _ = mark_dm_read(room_id, latest_id).await;
        });
    });

    use_effect(move || {
        #[cfg(target_arch = "wasm32")]
        {
            use wasm_bindgen::closure::Closure;
            use wasm_bindgen::JsCast;
            let Some(document) = web_sys::window().and_then(|w| w.document()) else {
                return;
            };
            let cb = Closure::<dyn FnMut()>::new(move || {
                let v = *visibility_tick.peek();
                visibility_tick.set(v + 1);
            });
            let _ = document
                .add_event_listener_with_callback("visibilitychange", cb.as_ref().unchecked_ref());
            cb.forget();
        }
    });

    if let Some(e) = load_error() {
        return rsx! {
            div { class: "flex-1 flex items-center justify-center text-red-500",
                "Failed to load messages: {e}"
            }
        };
    }
    let message_list = messages();

    // Extract other user's name from room name (dm-<user1>-<user2>)
    let other_name = room
        .name
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
        div {
            id: "{auto.container_id}",
            class: "flex-1 overflow-y-auto px-6 py-4 space-y-3",
            if message_list.is_empty() {
                div { class: "text-center text-gray-400 mt-12",
                    "No messages yet — say hello!"
                }
            } else {
                for msg in message_list.iter() {
                    {
                        let msg_id = msg.id;
                        let msg_user_id = msg.user_id.clone();
                        let msg_body = msg.body.clone();
                        let is_own = msg_user_id == u.id;
                        let is_editing = editing_msg_id() == Some(msg_id);
                        let has_edited = msg.edited_at.is_some();
                        let is_first_unseen = *auto.first_unseen_id.read() == Some(msg_id);
                        rsx! {
                            div { key: "{msg.id}",
                                if is_first_unseen {
                                    div { class: "flex items-center gap-2 my-2 text-xs font-medium text-blue-600",
                                        div { class: "flex-1 h-px bg-blue-300" }
                                        span { "New messages" }
                                        div { class: "flex-1 h-px bg-blue-300" }
                                    }
                                }
                                div {
                                    "data-msg-id": "{msg.id}",
                                    class: "group flex flex-col",
                                div { class: "flex items-baseline gap-2",
                                    span { class: "font-semibold text-gray-800", "{msg.author_name}" }
                                    span { class: "text-xs text-gray-400", "{msg.created_at}" }
                                    if has_edited {
                                        span { class: "text-xs text-gray-400 italic", "(edited)" }
                                    }
                                    if is_own && !is_editing {
                                        button {
                                            class: "opacity-0 group-hover:opacity-100 text-xs text-blue-500 hover:text-blue-700 ml-2 transition-opacity",
                                            onclick: move |_| {
                                                editing_msg_id.set(Some(msg_id));
                                                edit_draft.set(msg_body.clone());
                                                edit_error.set(None);
                                            },
                                            "edit"
                                        }
                                    }
                                }
                                if is_editing {
                                    div { class: "mt-1 flex flex-col gap-1",
                                        if let Some(err) = edit_error() {
                                            div { class: "text-xs text-red-600", "{err}" }
                                        }
                                        textarea {
                                            class: "w-full px-3 py-1.5 border border-blue-400 rounded text-sm resize-none",
                                            rows: "3",
                                            value: "{edit_draft}",
                                            oninput: move |e| edit_draft.set(e.value()),
                                        }
                                        div { class: "flex gap-2",
                                            button {
                                                class: "px-3 py-1 text-xs font-medium text-white bg-blue-600 rounded hover:bg-blue-700",
                                                onclick: move |_| {
                                                    let body = edit_draft();
                                                    spawn(async move {
                                                        match edit_message(msg_id, body).await {
                                                            Ok(()) => {
                                                                editing_msg_id.set(None);
                                                                edit_draft.set(String::new());
                                                                edit_error.set(None);
                                                            }
                                                            Err(e) => {
                                                                edit_error.set(Some(crate::server_fns::auth::user_facing_error(&e)));
                                                            }
                                                        }
                                                    });
                                                },
                                                "Save"
                                            }
                                            button {
                                                class: "px-3 py-1 text-xs font-medium text-gray-700 bg-gray-100 rounded hover:bg-gray-200",
                                                onclick: move |_| {
                                                    editing_msg_id.set(None);
                                                    edit_draft.set(String::new());
                                                    edit_error.set(None);
                                                },
                                                "Cancel"
                                            }
                                        }
                                    }
                                } else {
                                    p { class: "text-gray-700 whitespace-pre-wrap", "{msg.body}" }
                                }
                                {
                                    let (last_read, read_at_str) = match ws_peer_read() {
                                        Some((id, at)) => (Some(id), Some(at)),
                                        None => match peer_state() {
                                            Some(Ok(Some(s))) => (Some(s.last_read_message_id), Some(s.read_at)),
                                            _ => (None, None),
                                        },
                                    };
                                    let show_seen = is_own && last_read.map(|lr| {
                                        msg_id <= lr &&
                                        message_list.iter().rev()
                                            .find(|m| m.user_id == u.id && m.id <= lr)
                                            .map(|m| m.id == msg_id).unwrap_or(false)
                                    }).unwrap_or(false);
                                    if show_seen {
                                        let read_at = read_at_str.unwrap_or_default();
                                        let hhmm = read_at.split(' ').nth(1).map(|t| &t[..5.min(t.len())]).unwrap_or("").to_string();
                                        rsx! { div { class: "text-xs text-gray-400 mt-0.5", "Seen {hhmm}" } }
                                    } else {
                                        rsx! {}
                                    }
                                }
                                }
                            }
                        }
                    }
                }
            }
        }

        if *auto.show_new_pill.read() {
            div { class: "relative",
                button {
                    r#type: "button",
                    class: "absolute right-6 -top-12 px-3 py-1.5 bg-blue-600 text-white text-sm rounded-full shadow-lg hover:bg-blue-700",
                    onclick: move |_| auto.scroll_to_bottom.call(()),
                    "↓ New messages"
                }
            }
        }

        // Typing indicator
        {
            let typers: Vec<String> = typing_users.read().iter().map(|(_, name)| name.clone()).collect();
            if !typers.is_empty() {
                let label = match typers.len() {
                    1 => format!("{} is typing…", typers[0]),
                    2 => format!("{} and {} are typing…", typers[0], typers[1]),
                    _ => "Several people are typing…".to_string(),
                };
                rsx! {
                    div { class: "px-6 py-1 text-xs text-gray-400 italic", "{label}" }
                }
            } else {
                rsx! {}
            }
        }

        // Composer
        if is_muted {
            div { class: "px-6 py-3 border-t border-gray-200 bg-yellow-50 text-center",
                span { class: "text-sm text-yellow-700", "{mute_message}" }
            }
        } else {
            div { class: "px-6 py-3 border-t border-gray-200 bg-white",
                if let Some(err) = error() {
                    div { class: "mb-2 text-sm text-red-600", "{err}" }
                }
                div { class: "flex items-center gap-2",
                    input {
                        class: "flex-1 px-3 py-1.5 border border-gray-300 rounded",
                        r#type: "text",
                        placeholder: "Type a message…",
                        value: "{draft}",
                        oninput: move |evt| {
                            draft.set(evt.value());
                            #[cfg(target_arch = "wasm32")]
                            {
                                let now = js_sys::Date::now();
                                if now - *last_typing_sent.peek() > 1000.0 {
                                    last_typing_sent.set(now);
                                    ws.send_typing(room_id);
                                }
                            }
                        },
                        onkeydown: move |evt| {
                            if evt.key() == Key::Enter {
                                let body = draft();
                                if body.trim().is_empty() {
                                    return;
                                }
                                spawn(async move {
                                    match send_dm_message(room_id, body).await {
                                        Ok(_) => {
                                            draft.set(String::new());
                                            error.set(None);
                                        }
                                        Err(e) => error.set(Some(crate::server_fns::auth::user_facing_error(&e))),
                                    }
                                });
                            }
                        },
                    }
                    button {
                        class: "px-4 py-1.5 bg-blue-600 text-white rounded hover:bg-blue-700",
                        r#type: "button",
                        onclick: move |_| {
                            let body = draft();
                            if body.trim().is_empty() {
                                return;
                            }
                            spawn(async move {
                                match send_dm_message(room_id, body).await {
                                    Ok(_) => {
                                        draft.set(String::new());
                                        error.set(None);
                                    }
                                    Err(e) => error.set(Some(crate::server_fns::auth::user_facing_error(&e))),
                                }
                            });
                        },
                        "Send"
                    }
                }
            }
        }
    }
}
