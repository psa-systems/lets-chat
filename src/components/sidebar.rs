use dioxus::prelude::*;

use crate::models::User;
use crate::routes::Route;
use crate::server_fns::auth::logout;
use crate::server_fns::chat::list_rooms;

#[component]
pub fn Sidebar() -> Element {
    let rooms = use_server_future(list_rooms)?;
    let user: Signal<User> = use_context::<Signal<User>>();
    let nav = use_navigator();

    let room_list = match rooms() {
        Some(Ok(list)) => list,
        _ => vec![],
    };

    let u = user();
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
        }

        if u.role == "admin" {
            div { class: "px-2 mt-2",
                Link {
                    to: Route::Admin {},
                    class: "flex items-center gap-2 px-3 py-1.5 text-sm rounded hover:bg-red-50 text-red-600 font-medium",
                    span { "Admin" }
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
            // Logout button
            button {
                class: "text-xs text-gray-500 hover:text-red-600 flex-shrink-0",
                r#type: "button",
                onclick: move |_| {
                    let nav = nav.clone();
                    spawn(async move {
                        let _ = logout().await;
                        nav.push(Route::Login {});
                    });
                },
                "Logout"
            }
        }
    }
}
