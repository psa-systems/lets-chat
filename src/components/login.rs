use dioxus::prelude::*;

use crate::routes::Route;
use crate::server_fns::auth;

#[component]
pub fn LoginPage() -> Element {
    let mut username = use_signal(|| String::new());
    let mut password = use_signal(|| String::new());
    let mut error = use_signal(|| Option::<String>::None);
    let mut loading = use_signal(|| false);
    let nav = use_navigator();

    // All signal mutations happen inside spawn so they run after the event handler
    // returns — avoids the Dioxus "RefCell already borrowed" panic.
    let do_login = move || {
        spawn(async move {
            if loading() {
                return;
            }
            error.set(None);
            loading.set(true);
            match auth::login(username(), password()).await {
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
                p { class: "text-gray-500 text-center mb-6", "Sign in" }

                if let Some(err) = error() {
                    div { class: "bg-red-50 text-red-700 p-3 rounded mb-4 text-sm",
                        "{err}"
                    }
                }

                div { class: "mb-4",
                    label { class: "block text-sm font-medium text-gray-700 mb-1",
                        r#for: "username",
                        "Username"
                    }
                    input {
                        class: "w-full px-3 py-2 border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500",
                        r#type: "text",
                        id: "username",
                        value: "{username}",
                        oninput: move |evt| username.set(evt.value()),
                        onkeydown: move |evt| {
                            if evt.key() == Key::Enter {
                                do_login();
                            }
                        },
                    }
                }

                div { class: "mb-6",
                    label { class: "block text-sm font-medium text-gray-700 mb-1",
                        r#for: "password",
                        "Password"
                    }
                    input {
                        class: "w-full px-3 py-2 border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500",
                        r#type: "password",
                        id: "password",
                        value: "{password}",
                        oninput: move |evt| password.set(evt.value()),
                        onkeydown: move |evt| {
                            if evt.key() == Key::Enter {
                                do_login();
                            }
                        },
                    }
                }

                button {
                    class: "w-full bg-blue-600 text-white py-2 rounded hover:bg-blue-700 disabled:opacity-50",
                    r#type: "button",
                    disabled: loading(),
                    onclick: move |_| do_login(),
                    if loading() { "Signing in..." } else { "Sign in" }
                }

                p { class: "mt-4 text-center text-sm text-gray-500",
                    "Don't have an account? "
                    Link { class: "text-blue-600 hover:underline", to: Route::Register {},
                        "Register"
                    }
                }
            }
        }
    }
}
