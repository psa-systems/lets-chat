use dioxus::prelude::*;

use crate::models::User;
use crate::routes::Route;
use crate::server_fns::auth;

#[component]
pub fn AuthLayout() -> Element {
    // These must be called unconditionally on every render (hook ordering).
    let user_signal = use_context_provider(|| Signal::new(Option::<User>::None));
    let ws = crate::components::use_websocket::use_websocket();
    use_context_provider(|| ws);

    let nav = use_navigator();
    let user_future = use_server_future(|| auth::get_current_user())?;

    match user_future() {
        Some(Ok(Some(user))) => {
            // Populate the user signal so child components can read it via use_context.
            user_signal.clone().set(Some(user));
            rsx! { Outlet::<Route> {} }
        }
        Some(Ok(None)) => {
            nav.push(Route::Login {});
            rsx! {
                div { class: "min-h-screen flex items-center justify-center bg-gray-100",
                    p { class: "text-gray-500", "Redirecting to login..." }
                }
            }
        }
        Some(Err(e)) => {
            let err_msg = auth::user_facing_error(&e);
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
        None => rsx! {
            div { class: "min-h-screen flex items-center justify-center bg-gray-100",
                p { class: "text-gray-500", "Loading..." }
            }
        },
    }
}
