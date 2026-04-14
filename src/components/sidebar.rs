use dioxus::prelude::*;

use crate::components::use_websocket::WsHandle;
use crate::models::User;
use crate::routes::Route;
use crate::server_fns::auth::{clear_session_cookie, logout, set_read_receipts_enabled};
use crate::server_fns::chat::list_rooms;
use crate::server_fns::dm::{list_dm_unread_counts_fn, list_my_dms};
use crate::ws::events::ChatEvent;

#[component]
pub fn Sidebar() -> Element {
    let user: Signal<Option<User>> = use_context::<Signal<Option<User>>>();
    let my_id = user().map(|u| u.id).unwrap_or_default();

    let mut rooms_version = use_signal(|| 0u32);
    let rooms = use_server_future(move || {
        let _v = rooms_version();
        async move { list_rooms().await }
    })?;

    let mut dms_version = use_signal(|| 0u32);
    let dms = use_server_future(move || {
        let _v = dms_version();
        async move { list_my_dms().await }
    })?;

    let mut unread_version = use_signal(|| 0u32);
    let unread = use_server_future(move || {
        let _v = unread_version();
        async move { list_dm_unread_counts_fn().await }
    })?;

    let ws = use_context::<WsHandle>();
    use_effect(move || {
        if let Some(ref event) = *ws.latest_event.read() {
            match event {
                ChatEvent::NewMessage { is_dm: true, .. } => {
                    let v = *dms_version.peek();
                    dms_version.set(v + 1);
                    let v = *unread_version.peek();
                    unread_version.set(v + 1);
                }
                ChatEvent::DmRead { .. } => {
                    let v = *unread_version.peek();
                    unread_version.set(v + 1);
                }
                ChatEvent::RoomMemberAdded { user_id, .. }
                | ChatEvent::RoomMemberRemoved { user_id, .. }
                    if *user_id == my_id =>
                {
                    let v = *rooms_version.peek();
                    rooms_version.set(v + 1);
                }
                _ => {}
            }
        }
    });

    let nav = use_navigator();

    let room_list = match rooms() {
        Some(Ok(list)) => list,
        _ => vec![],
    };

    let dm_list = match dms() {
        Some(Ok(list)) => list,
        _ => vec![],
    };

    let unread_map: std::collections::HashMap<i64, i64> = match unread() {
        Some(Ok(list)) => list.into_iter().map(|u| (u.room_id, u.count)).collect(),
        _ => std::collections::HashMap::new(),
    };

    let u = user().expect("user must be authenticated");
    let mut show_settings = use_signal(|| false);
    let mut receipts_enabled = use_signal(|| u.read_receipts_enabled);
    let avatar_initial = u
        .display_name
        .as_deref()
        .and_then(|d| d.chars().next())
        .unwrap_or_else(|| u.username.chars().next().unwrap_or('?'))
        .to_uppercase()
        .next()
        .unwrap_or('?');

    let display = u.display_name.as_deref().unwrap_or(&u.username).to_string();

    rsx! {
        div { class: "p-4 border-b border-gray-200",
            h1 { class: "text-lg font-bold text-gray-800", "Let's Chat" }
        }

        nav { class: "flex-1 overflow-y-auto p-2",
            div { class: "px-3 py-1 text-xs font-semibold text-gray-500 uppercase tracking-wider",
                "Rooms"
            }
            if room_list.is_empty() {
                div { class: "px-3 py-2 text-sm text-gray-400", "No rooms" }
            } else {
                for room in room_list.iter() {
                    Link {
                        key: "{room.id}",
                        to: Route::Room { room_id: room.id.to_string() },
                        class: "flex items-center gap-2 px-3 py-1.5 text-sm rounded hover:bg-gray-100 text-gray-700",
                        span { class: "text-gray-400", "#" }
                        span { "{room.name}" }
                    }
                }
            }

            // Direct Messages section
            div { class: "mt-4 px-3 py-1 text-xs font-semibold text-gray-500 uppercase tracking-wider",
                "Direct Messages"
            }
            if dm_list.is_empty() {
                div { class: "px-3 py-2 text-sm text-gray-400", "No conversations" }
            } else {
                for dm in dm_list.iter() {
                    {
                        let count = unread_map.get(&dm.room_id).copied().unwrap_or(0);
                        rsx! {
                            Link {
                                key: "{dm.room_id}",
                                to: Route::Dm { user_id: dm.other_user_id.clone() },
                                class: "flex items-center gap-2 px-3 py-1.5 text-sm rounded hover:bg-gray-100 text-gray-700",
                                span { class: "text-gray-400", "@" }
                                span { class: "flex-1", "{dm.other_user_name}" }
                                if count > 0 {
                                    span {
                                        class: "ml-auto inline-flex items-center justify-center min-w-[1.25rem] h-5 px-1.5 rounded-full bg-blue-600 text-white text-xs font-semibold",
                                        "{count}"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if u.role == "admin" || u.role == "moderator" {
            div { class: "px-2 mt-2",
                Link {
                    to: if u.role == "admin" { Route::AdminSettings {} } else { Route::AdminUsers {} },
                    class: "flex items-center gap-2 px-3 py-1.5 text-sm rounded hover:bg-red-50 text-red-600 font-medium",
                    span { if u.role == "admin" { "Admin" } else { "Moderate" } }
                }
            }
        }

        // Settings popover
        if show_settings() {
            div { class: "px-3 py-2 border-t border-gray-200 bg-gray-50 text-xs",
                label { class: "flex items-center gap-2 cursor-pointer",
                    input {
                        r#type: "checkbox",
                        checked: receipts_enabled(),
                        oninput: move |e| {
                            let new_val = e.checked();
                            spawn(async move {
                                if set_read_receipts_enabled(new_val).await.is_ok() {
                                    receipts_enabled.set(new_val);
                                }
                            });
                        },
                    }
                    span { "Send and receive read receipts" }
                }
            }
        }

        // User info + logout at the bottom
        div { class: "p-3 border-t border-gray-200 flex items-center gap-2",
            // Avatar circle with initial
            div { class: "w-8 h-8 rounded-full bg-blue-500 text-white flex items-center justify-center text-sm font-semibold flex-shrink-0",
                "{avatar_initial}"
            }
            // Username + role
            div { class: "flex-1 min-w-0",
                div { class: "text-sm font-medium text-gray-800 truncate", "{display}" }
                div { class: "text-xs text-gray-400", "{u.role}" }
            }
            // Settings (gear) button
            button {
                class: "text-xs text-gray-500 hover:text-blue-600 flex-shrink-0",
                r#type: "button",
                onclick: move |_| {
                    let cur = show_settings();
                    show_settings.set(!cur);
                },
                "⚙"
            }
            // Logout button
            button {
                class: "text-xs text-gray-500 hover:text-red-600 flex-shrink-0",
                r#type: "button",
                onclick: move |_| {
                    let nav = nav.clone();
                    spawn(async move {
                        let _ = logout().await;
                        clear_session_cookie();
                        nav.push(Route::Login {});
                    });
                },
                "Logout"
            }
        }
    }
}
