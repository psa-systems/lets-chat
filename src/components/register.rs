use dioxus::prelude::*;

use crate::routes::Route;
use crate::server_fns::auth;

/// Read an input element's current value directly from the DOM by id.
///
/// Controlled inputs rely on `oninput` to sync the DOM value into a Rust signal,
/// but `oninput` only fires after Dioxus has hydrated the WASM event listeners.
/// On `/register` and `/login` the page is SSR'd and interactive natively before
/// hydration completes — a user who types and clicks Submit quickly races the
/// hydration and the signal stays empty. Reading from the DOM at submit time
/// recovers the real value regardless of hydration state.
#[cfg(target_arch = "wasm32")]
fn read_input_value(id: &str) -> String {
    use wasm_bindgen::JsCast;
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(id))
        .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|i| i.value())
        .unwrap_or_default()
}

#[cfg(not(target_arch = "wasm32"))]
fn read_input_value(_id: &str) -> String {
    String::new()
}

#[component]
pub fn RegisterPage() -> Element {
    let mut username = use_signal(|| String::new());
    let mut password = use_signal(|| String::new());
    let mut confirm_password = use_signal(|| String::new());
    let mut error = use_signal(|| Option::<String>::None);
    let mut loading = use_signal(|| false);
    let nav = use_navigator();

    // All signal mutations happen inside spawn so they run after the event handler
    // returns — avoids the Dioxus "RefCell already borrowed" panic.
    let do_register = move || {
        spawn(async move {
            if loading() {
                return;
            }
            let u = read_input_value("username");
            let p = read_input_value("password");
            let cp = read_input_value("confirm_password");
            username.set(u.clone());
            password.set(p.clone());
            confirm_password.set(cp.clone());
            if p != cp {
                error.set(Some("Passwords do not match".to_string()));
                return;
            }
            error.set(None);
            loading.set(true);
            match auth::register(u, p).await {
                Ok(resp) => {
                    auth::set_session_cookie(&resp.session_token);
                    nav.push(Route::Home {});
                }
                Err(e) => {
                    error.set(Some(e.to_string()));
                    loading.set(false);
                }
            }
        });
    };

    rsx! {
        div { class: "min-h-screen flex items-center justify-center bg-gray-100",
            div { class: "bg-white p-8 rounded-lg shadow-md w-full max-w-sm",
                h1 { class: "text-2xl font-bold text-center mb-1", "Let's Chat" }
                p { class: "text-gray-500 text-center mb-6", "Create an account" }

                if let Some(err) = error() {
                    div { class: "bg-red-50 text-red-700 p-3 rounded mb-4 text-sm",
                        "{err}"
                    }
                }

                div {
                    div { class: "mb-4",
                        label { class: "block text-sm font-medium text-gray-700 mb-1",
                            r#for: "username",
                            "Username"
                        }
                        input {
                            class: "w-full px-3 py-2 border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500",
                            r#type: "text",
                            id: "username",
                            oninput: move |evt| username.set(evt.value()),
                            onkeydown: move |evt| {
                                if evt.key() == Key::Enter {
                                    do_register();
                                }
                            },
                        }
                    }

                    div { class: "mb-4",
                        label { class: "block text-sm font-medium text-gray-700 mb-1",
                            r#for: "password",
                            "Password"
                        }
                        input {
                            class: "w-full px-3 py-2 border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500",
                            r#type: "password",
                            id: "password",
                            oninput: move |evt| password.set(evt.value()),
                            onkeydown: move |evt| {
                                if evt.key() == Key::Enter {
                                    do_register();
                                }
                            },
                        }
                    }

                    div { class: "mb-6",
                        label { class: "block text-sm font-medium text-gray-700 mb-1",
                            r#for: "confirm_password",
                            "Confirm password"
                        }
                        input {
                            class: "w-full px-3 py-2 border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500",
                            r#type: "password",
                            id: "confirm_password",
                            oninput: move |evt| confirm_password.set(evt.value()),
                            onkeydown: move |evt| {
                                if evt.key() == Key::Enter {
                                    do_register();
                                }
                            },
                        }
                    }

                    button {
                        class: "w-full bg-blue-600 text-white py-2 rounded hover:bg-blue-700 disabled:opacity-50",
                        r#type: "button",
                        disabled: loading(),
                        onclick: move |_| do_register(),
                        if loading() { "Creating account..." } else { "Register" }
                    }
                }

                p { class: "mt-4 text-center text-sm text-gray-500",
                    "Already have an account? "
                    Link { class: "text-blue-600 hover:underline", to: Route::Login {},
                        "Sign in"
                    }
                }
            }
        }
    }
}
