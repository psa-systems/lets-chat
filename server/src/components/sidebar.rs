use dioxus::prelude::*;

use crate::components::use_websocket::WsHandle;
use crate::models::User;
use crate::routes::Route;
use crate::server_fns::auth::{clear_session_cookie, logout, set_read_receipts_enabled};
use crate::server_fns::chat::{list_rooms, search_messages};
use crate::server_fns::dm::{list_dm_unread_counts_fn, list_my_dms};
use crate::ws::events::ChatEvent;

#[component]
pub fn Sidebar() -> Element {
    let user: Signal<Option<User>> = use_context::<Signal<Option<User>>>();
    let my_id = user().map(|u| u.id).unwrap_or_default();

    // These futures use `use_resource` (not `use_server_future`) because they are
    // refetched in response to WebSocket events via the version signals below.
    // `use_server_future` re-suspends on every refetch via its `?` operator,
    // which propagates to the root SuspenseBoundary and unmounts the entire
    // tree (including the route Outlet) until the new data arrives — visible
    // as a full-page flash on every DM send/receive. `use_resource` keeps the
    // previous value while refetching, so siblings keep rendering smoothly.
    let mut rooms_version = use_signal(|| 0u32);
    let rooms = use_resource(move || {
        let _v = rooms_version();
        async move { list_rooms().await }
    });

    let mut dms_version = use_signal(|| 0u32);
    let dms = use_resource(move || {
        let _v = dms_version();
        async move { list_my_dms().await }
    });

    let mut unread_version = use_signal(|| 0u32);
    let unread = use_resource(move || {
        let _v = unread_version();
        async move { list_dm_unread_counts_fn().await }
    });

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

    // Search state: `search_input` tracks the live text; `search_query` is set
    // on submit and drives the server future so it only fires on Enter/click.
    let mut search_input = use_signal(String::new);
    let mut search_query = use_signal(String::new);

    let search_results = use_resource(move || {
        let q = search_query();
        async move {
            if q.trim().is_empty() {
                Ok(vec![])
            } else {
                search_messages(q, None).await
            }
        }
    });

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

    let is_searching = !search_query().trim().is_empty();

    rsx! {
        div { class: "p-4 border-b border-gray-200",
            h1 { class: "text-lg font-bold text-gray-800", "Let's Chat" }
        }

        // Search bar
        div { class: "px-3 py-2 border-b border-gray-100",
            div { class: "flex items-center gap-1",
                input {
                    class: "flex-1 px-2 py-1 text-sm border border-gray-200 rounded bg-gray-50 focus:outline-none focus:border-blue-400",
                    r#type: "text",
                    placeholder: "Search messages…",
                    value: "{search_input}",
                    oninput: move |e| {
                        let v = e.value();
                        spawn(async move { search_input.set(v); });
                    },
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            let q = search_input();
                            spawn(async move { search_query.set(q); });
                        }
                        if e.key() == Key::Escape {
                            spawn(async move {
                                search_input.set(String::new());
                                search_query.set(String::new());
                            });
                        }
                    },
                }
                if is_searching {
                    button {
                        class: "text-gray-400 hover:text-gray-600 text-sm px-1",
                        r#type: "button",
                        onclick: move |_| {
                            spawn(async move {
                                search_input.set(String::new());
                                search_query.set(String::new());
                            });
                        },
                        "×"
                    }
                } else {
                    button {
                        class: "text-gray-400 hover:text-blue-600 text-sm px-1",
                        r#type: "button",
                        onclick: move |_| {
                            let q = search_input();
                            spawn(async move { search_query.set(q); });
                        },
                        "↵"
                    }
                }
            }
        }

        if is_searching {
            // Search results panel
            div { class: "flex-1 overflow-y-auto p-2",
                match search_results() {
                    None => rsx! {
                        div { class: "px-3 py-4 text-sm text-gray-400 text-center", "Searching…" }
                    },
                    Some(Err(e)) => rsx! {
                        div { class: "px-3 py-2 text-sm text-red-500", "{e}" }
                    },
                    Some(Ok(results)) => {
                        if results.is_empty() {
                            rsx! {
                                div { class: "px-3 py-4 text-sm text-gray-400 text-center",
                                    "No results for "{search_query()}""
                                }
                            }
                        } else {
                            let count = results.len();
                            let plural = if count == 1 { "" } else { "s" };
                            rsx! {
                                div { class: "px-3 py-1 mb-1 text-xs font-semibold text-gray-500 uppercase tracking-wider",
                                    "{count} result{plural}"
                                }
                                for r in results.iter() {
                                    {
                                        let room_id_str = r.room_id.to_string();
                                        let room_name = r.room_name.clone();
                                        let author = r.author_name.clone();
                                        let body = r.body.clone();
                                        let ts = r.created_at.split(' ').next().unwrap_or("").to_string();
                                        rsx! {
                                            button {
                                                key: "{r.message_id}",
                                                class: "w-full text-left px-3 py-2 rounded hover:bg-blue-50 border border-transparent hover:border-blue-100 mb-1",
                                                r#type: "button",
                                                onclick: move |_| {
                                                    let rid = room_id_str.clone();
                                                    nav.push(Route::Room { room_id: rid });
                                                    spawn(async move {
                                                        search_input.set(String::new());
                                                        search_query.set(String::new());
                                                    });
                                                },
                                                div { class: "flex items-center gap-1 mb-0.5",
                                                    span { class: "text-xs font-semibold text-blue-700", "#{room_name}" }
                                                    span { class: "text-xs text-gray-400", "·" }
                                                    span { class: "text-xs text-gray-500", "{author}" }
                                                    span { class: "ml-auto text-xs text-gray-400", "{ts}" }
                                                }
                                                p { class: "text-xs text-gray-700 line-clamp-2 whitespace-pre-wrap",
                                                    "{body}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        } else {
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
