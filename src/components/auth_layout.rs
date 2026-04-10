use dioxus::prelude::*;

use crate::routes::Route;
use crate::server_fns::auth;

#[component]
pub fn AuthLayout() -> Element {
    let user_future = use_server_future(|| auth::get_current_user())?;

    let read_guard = user_future.read();
    let result = match &*read_guard {
        Some(Ok(Some(user))) => {
            use_context_provider(|| Signal::new(user.clone()));
            let ws = crate::components::use_websocket::use_websocket();
            use_context_provider(|| ws);
            rsx! {
                Outlet::<Route> {}
            }
        }
        Some(Ok(None)) => {
            // Not authenticated, redirect to login
            let nav = use_navigator();
            nav.push(Route::Login {});
            rsx! {
                div { class: "min-h-screen flex items-center justify-center bg-gray-100",
                    p { class: "text-gray-500", "Redirecting to login..." }
                }
            }
        }
        Some(Err(e)) => {
            let err_msg = e.to_string();
            rsx! {
                div { class: "min-h-screen flex items-center justify-center bg-gray-100",
                    div { class: "bg-white p-8 rounded-lg shadow-md text-center",
                        p { class: "text-red-600 mb-4", "Error: {err_msg}" }
                        Link { class: "text-blue-600 hover:underline", to: Route::Login {},
                            "Go to login"
                        }
                    }
                }
            }
        }
        None => {
            // Loading
            rsx! {
                div { class: "min-h-screen flex items-center justify-center bg-gray-100",
                    p { class: "text-gray-500", "Loading..." }
                }
            }
        }
    };
    result
}
