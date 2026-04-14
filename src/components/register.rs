use dioxus::prelude::*;

use crate::routes::Route;
use crate::server_fns::auth;

#[component]
pub fn RegisterPage() -> Element {
    let mut hydrated = use_signal(|| false);
    #[cfg(target_arch = "wasm32")]
    use_hook(|| {
        spawn(async move {
            hydrated.set(true);
        });
    });

    if !hydrated() {
        return rsx! { LoadingCard { message: "Loading…" } };
    }

    rsx! { RegisterForm {} }
}

#[component]
pub(crate) fn LoadingCard(message: String) -> Element {
    rsx! {
        div { class: "min-h-screen flex items-center justify-center bg-gray-100",
            div { class: "bg-white p-8 rounded-lg shadow-md w-full max-w-sm text-center",
                h1 { class: "text-2xl font-bold mb-2", "Let's Chat" }
                p { class: "text-gray-500", "{message}" }
            }
        }
    }
}

#[component]
fn RegisterForm() -> Element {
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut confirm_password = use_signal(String::new);
    let mut error = use_signal(|| Option::<String>::None);
    let mut loading = use_signal(|| false);
    let nav = use_navigator();

    let mut do_register = move || {
        if loading() {
            return;
        }
        let u = username();
        let p = password();
        let cp = confirm_password();
        if p != cp {
            error.set(Some("Passwords do not match".to_string()));
            return;
        }
        error.set(None);
        loading.set(true);
        spawn(async move {
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

    let button_class = if loading() {
        "w-full bg-blue-400 text-white py-2 rounded opacity-50 cursor-not-allowed"
    } else {
        "w-full bg-blue-600 text-white py-2 rounded hover:bg-blue-700"
    };
    let button_label = if loading() { "Creating account..." } else { "Register" };

    rsx! {
        div { class: "min-h-screen flex items-center justify-center bg-gray-100",
            div { class: "bg-white p-8 rounded-lg shadow-md w-full max-w-sm",
                h1 { class: "text-2xl font-bold text-center mb-1", "Let's Chat" }
                p { class: "text-gray-500 text-center mb-6", "Create an account" }

                if let Some(err) = error() {
                    div { class: "bg-red-50 text-red-700 p-3 rounded mb-4 text-sm", "{err}" }
                }

                div {
                    div { class: "mb-4",
                        label { class: "block text-sm font-medium text-gray-700 mb-1", r#for: "username", "Username" }
                        input {
                            class: "w-full px-3 py-2 border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500",
                            r#type: "text",
                            id: "username",
                            value: "{username}",
                            oninput: move |evt| username.set(evt.value()),
                            onkeydown: move |evt| { if evt.key() == Key::Enter { do_register(); } },
                        }
                    }

                    div { class: "mb-4",
                        label { class: "block text-sm font-medium text-gray-700 mb-1", r#for: "password", "Password" }
                        input {
                            class: "w-full px-3 py-2 border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500",
                            r#type: "password",
                            id: "password",
                            value: "{password}",
                            oninput: move |evt| password.set(evt.value()),
                            onkeydown: move |evt| { if evt.key() == Key::Enter { do_register(); } },
                        }
                    }

                    div { class: "mb-6",
                        label { class: "block text-sm font-medium text-gray-700 mb-1", r#for: "confirm_password", "Confirm password" }
                        input {
                            class: "w-full px-3 py-2 border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500",
                            r#type: "password",
                            id: "confirm_password",
                            value: "{confirm_password}",
                            oninput: move |evt| confirm_password.set(evt.value()),
                            onkeydown: move |evt| { if evt.key() == Key::Enter { do_register(); } },
                        }
                    }

                    button {
                        class: button_class,
                        r#type: "button",
                        disabled: loading(),
                        onclick: move |_| do_register(),
                        "{button_label}"
                    }
                }

                p { class: "mt-4 text-center text-sm text-gray-500",
                    "Already have an account? "
                    Link { class: "text-blue-600 hover:underline", to: Route::Login {}, "Sign in" }
                }
            }
        }
    }
}
