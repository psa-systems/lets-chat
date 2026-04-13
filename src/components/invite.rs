use dioxus::prelude::*;

use crate::routes::Route;
use crate::server_fns::rooms::join_room_by_invite;

#[component]
pub fn InvitePage(code: String) -> Element {
    let nav = use_navigator();

    let result = use_server_future(move || {
        let c = code.clone();
        async move { join_room_by_invite(c).await }
    })?;

    match result() {
        Some(Ok(room_id)) => {
            nav.push(Route::Room { room_id: room_id.to_string() });
            rsx! {
                div { class: "flex-1 flex items-center justify-center text-gray-500",
                    "Joining room…"
                }
            }
        }
        Some(Err(e)) => rsx! {
            div { class: "flex-1 flex items-center justify-center",
                div { class: "text-center space-y-4",
                    p { class: "text-red-600", "Could not join room: {e}" }
                    Link {
                        to: Route::Home {},
                        class: "text-blue-600 hover:underline text-sm",
                        "Go home"
                    }
                }
            }
        },
        None => rsx! {
            div { class: "flex-1 flex items-center justify-center text-gray-500",
                "Joining…"
            }
        },
    }
}
