use dioxus::prelude::*;

use crate::components::use_auto_scroll::use_auto_scroll;
use crate::components::use_websocket::WsHandle;
use crate::models::{Message, User};
use crate::routes::Route;
use crate::server_fns::chat::{edit_message, get_room, list_messages, send_message};
use crate::server_fns::moderation::delete_message;
use crate::ws::events::ChatEvent;

#[component]
pub fn RoomViewPage(room_id: String) -> Element {
    // Sync room_id prop into a Signal so use_server_future tracks it reactively.
    // Without this, navigating between rooms doesn't re-run the futures.
    let mut room_id_sig = use_signal(|| room_id.parse::<i64>().unwrap_or(0));
    let new_id = room_id.parse::<i64>().unwrap_or(0);
    if *room_id_sig.peek() != new_id {
        room_id_sig.set(new_id);
    }

    let user: Signal<Option<User>> = use_context::<Signal<Option<User>>>();
    let u = user().expect("user must be authenticated");
    let is_mod = u.role == "admin" || u.role == "moderator";
    let my_user_id = u.id.clone();

    // Futures read room_id_sig() — they re-run automatically when the room changes.
    let room = use_server_future(move || async move { get_room(room_id_sig()).await })?;

    // Initial load from server — re-runs only when the room changes.
    let messages_fetch = use_server_future(move || {
        let id = room_id_sig();
        async move { list_messages(id).await }
    })?;

    // Local, WS-driven source of truth for the rendered message list.
    let mut messages = use_signal(Vec::<Message>::new);
    let mut load_error = use_signal(|| Option::<String>::None);

    let auto = use_auto_scroll(room_id_sig, messages);

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

    let ws = use_context::<WsHandle>();

    let mut typing_users = use_signal(Vec::<(String, String)>::new);
    let mut last_typing_sent = use_signal(|| 0.0f64);

    // Track which room we're subscribed to so we can cleanly unsubscribe when switching.
    let mut last_subscribed = use_signal(|| 0i64);
    let ws_sub = ws.clone();
    let ws_unsub = ws.clone();
    use_effect(move || {
        let id = room_id_sig();
        let prev = *last_subscribed.peek();
        if prev != 0 && prev != id {
            ws_unsub.unsubscribe(prev);
            typing_users.set(Vec::new());
            messages.set(Vec::new());
        }
        ws_sub.subscribe(id);
        last_subscribed.set(id);
    });

    let ws_drop = ws.clone();
    let last_sub_for_drop = last_subscribed;
    use_drop(move || {
        let id = *last_sub_for_drop.peek();
        if id != 0 {
            ws_drop.unsubscribe(id);
        }
    });

    // Apply WS events directly to the local messages signal — no re-fetching.
    use_effect(move || {
        if let Some(ref event) = *ws.latest_event.read() {
            let id = room_id_sig();
            match event {
                ChatEvent::NewMessage { message, .. } if message.room_id == id => {
                    let m = message.clone();
                    messages.with_mut(|v| {
                        if !v.iter().any(|x| x.id == m.id) {
                            v.push(m);
                        }
                    });
                }
                ChatEvent::MessageDeleted {
                    room_id,
                    message_id,
                } if *room_id == id => {
                    let mid = *message_id;
                    messages.with_mut(|v| v.retain(|m| m.id != mid));
                }
                ChatEvent::MessageEdited {
                    room_id,
                    message_id,
                    new_body,
                    edited_at,
                } if *room_id == id => {
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
                _ => {}
            }
        }
    });

    // Handle typing indicator events.
    use_effect(move || {
        if let Some(ref event) = *ws.latest_event.read() {
            let id = room_id_sig();
            match event {
                ChatEvent::UserTyping {
                    room_id,
                    user_id,
                    username,
                } if *room_id == id && *user_id != my_user_id => {
                    let uid = user_id.clone();
                    let name = username.clone();
                    typing_users.with_mut(|v| {
                        if !v.iter().any(|(id, _)| id == &uid) {
                            v.push((uid, name));
                        }
                    });
                }
                ChatEvent::UserStoppedTyping { room_id, user_id } if *room_id == id => {
                    let uid = user_id.clone();
                    typing_users.with_mut(|v| v.retain(|(id, _)| id != &uid));
                }
                ChatEvent::NewMessage { message, .. } if message.room_id == id => {
                    let uid = message.user_id.clone();
                    typing_users.with_mut(|v| v.retain(|(id, _)| id != &uid));
                }
                _ => {}
            }
        }
    });

    let mut draft = use_signal(String::new);
    let mut error = use_signal(|| Option::<String>::None);
    let mut delete_reason = use_signal(String::new);
    let mut confirm_delete_msg = use_signal(|| Option::<i64>::None);
    let mut editing_msg_id = use_signal(|| Option::<i64>::None);
    let mut edit_draft = use_signal(String::new);
    let mut edit_error = use_signal(|| Option::<String>::None);

    if new_id == 0 {
        return rsx! {
            div { class: "flex-1 flex items-center justify-center text-red-500",
                "Invalid room id"
            }
        };
    }

    let room_name = match room() {
        Some(Ok(Some(r))) => r.name,
        _ => format!("room {}", room_id_sig()),
    };
    let room_topic = match room() {
        Some(Ok(Some(r))) => r.topic,
        _ => None,
    };

    if let Some(e) = load_error() {
        return rsx! {
            div { class: "flex-1 flex items-center justify-center text-red-500",
                "Failed to load messages: {e}"
            }
        };
    }
    let message_list = messages();

    // Determine mute state for composer
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
                h2 { class: "text-lg font-semibold text-gray-800", "# {room_name}" }
                if let Some(topic) = room_topic {
                    span { class: "text-sm text-gray-500", "{topic}" }
                }
            }
        }

        // Delete message confirmation modal
        if let Some(msg_id) = confirm_delete_msg() {
            div { class: "fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50",
                div { class: "bg-white rounded-lg p-6 max-w-sm w-full mx-4",
                    h3 { class: "text-lg font-semibold text-gray-800 mb-2", "Delete Message" }
                    div { class: "space-y-3 mb-4",
                        label { class: "block text-sm font-medium text-gray-700", "Reason" }
                        input {
                            class: "w-full px-3 py-1.5 border border-gray-300 rounded text-sm",
                            r#type: "text",
                            placeholder: "Reason for deletion...",
                            value: "{delete_reason}",
                            oninput: move |e| { let v = e.value(); spawn(async move { delete_reason.set(v); }); },
                        }
                    }
                    div { class: "flex justify-end gap-2",
                        button {
                            class: "px-3 py-1.5 text-sm font-medium text-gray-700 bg-gray-100 rounded-md hover:bg-gray-200",
                            onclick: move |_| {
                                spawn(async move {
                                    confirm_delete_msg.set(None);
                                    delete_reason.set(String::new());
                                });
                            },
                            "Cancel"
                        }
                        button {
                            class: "px-3 py-1.5 text-sm font-medium text-white bg-red-600 rounded-md hover:bg-red-700",
                            onclick: move |_| {
                                let reason = delete_reason();
                                spawn(async move {
                                    confirm_delete_msg.set(None);
                                    delete_reason.set(String::new());
                                    match delete_message(msg_id, reason).await {
                                        Ok(()) => {}
                                        Err(e) => {
                                            error.set(Some(format!("Delete failed: {}", e)));
                                        }
                                    }
                                });
                            },
                            "Delete"
                        }
                    }
                }
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
                        let is_first_unseen = auto.first_unseen_id() == Some(msg_id);
                        rsx! {
                            if is_first_unseen {
                                div { class: "flex items-center gap-2 my-2 text-xs font-medium text-blue-600",
                                    div { class: "flex-1 h-px bg-blue-300" }
                                    span { "New messages" }
                                    div { class: "flex-1 h-px bg-blue-300" }
                                }
                            }
                            div {
                                key: "{msg.id}",
                                "data-msg-id": "{msg.id}",
                                class: "group flex flex-col",
                                div { class: "flex items-baseline gap-2",
                                    if msg_user_id != u.id {
                                        Link {
                                            to: Route::Dm { user_id: msg_user_id.clone() },
                                            class: "font-semibold text-gray-800 hover:underline hover:text-blue-600",
                                            "{msg.author_name}"
                                        }
                                    } else {
                                        span { class: "font-semibold text-gray-800", "{msg.author_name}" }
                                    }
                                    span { class: "text-xs text-gray-400", "{msg.created_at}" }
                                    if has_edited {
                                        span { class: "text-xs text-gray-400 italic", "(edited)" }
                                    }
                                    if is_own && !is_editing {
                                        button {
                                            class: "opacity-0 group-hover:opacity-100 text-xs text-blue-500 hover:text-blue-700 ml-2 transition-opacity",
                                            onclick: move |_| {
                                                let body = msg_body.clone();
                                                spawn(async move {
                                                    editing_msg_id.set(Some(msg_id));
                                                    edit_draft.set(body);
                                                    edit_error.set(None);
                                                });
                                            },
                                            "edit"
                                        }
                                    }
                                    if is_mod && !is_editing {
                                        button {
                                            class: "opacity-0 group-hover:opacity-100 text-xs text-red-500 hover:text-red-700 ml-2 transition-opacity",
                                            onclick: move |_| {
                                                spawn(async move {
                                                    confirm_delete_msg.set(Some(msg_id));
                                                });
                                            },
                                            "delete"
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
                                            oninput: move |e| { let v = e.value(); spawn(async move { edit_draft.set(v); }); },
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
                                                    spawn(async move {
                                                        editing_msg_id.set(None);
                                                        edit_draft.set(String::new());
                                                        edit_error.set(None);
                                                    });
                                                },
                                                "Cancel"
                                            }
                                        }
                                    }
                                } else {
                                    p { class: "text-gray-700 whitespace-pre-wrap", "{msg.body}" }
                                }
                            }
                        }
                    }
                }
            }
        }

        if auto.show_new_pill() {
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

        // Composer (or mute banner)
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
                                    ws.send_typing(room_id_sig());
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
                                    match send_message(room_id_sig(), body).await {
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
                                match send_message(room_id_sig(), body).await {
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
