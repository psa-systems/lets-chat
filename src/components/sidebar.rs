use dioxus::prelude::*;

use crate::routes::Route;
use crate::server_fns::chat::list_rooms;

#[component]
pub fn Sidebar() -> Element {
    let rooms = use_server_future(list_rooms)?;

    let room_list = match rooms() {
        Some(Ok(list)) => list,
        _ => vec![],
    };

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
    }
}
