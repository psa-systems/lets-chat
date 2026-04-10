use dioxus::prelude::*;

use crate::routes::Route;
use crate::server_fns::auth;

#[component]
pub fn RegisterPage() -> Element {
    let mut username = use_signal(|| String::new());
    let mut password = use_signal(|| String::new());
    let mut confirm_password = use_signal(|| String::new());
    let mut error = use_signal(|| Option::<String>::None);
    let mut loading = use_signal(|| false);
    let nav = use_navigator();

    let on_submit = move |evt: Event<FormData>| async move {
        evt.prevent_default();
        error.set(None);

        if password() != confirm_password() {
            error.set(Some("Passwords do not match".to_string()));
            return;
        }

        loading.set(true);

        match auth::register(username(), password()).await {
            Ok(resp) => {
                auth::set_session_cookie(&resp.session_token);
                nav.push(Route::Home {});
            }
            Err(e) => {
                error.set(Some(e.to_string()));
            }
        }
        loading.set(false);
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

                form { onsubmit: on_submit,
                    div { class: "mb-4",
                        label { class: "block text-sm font-medium text-gray-700 mb-1",
                            r#for: "username",
                            "Username"
                        }
                        input {
                            class: "w-full px-3 py-2 border border-gray-300 rounded focus:outline-none focus:ring-2 focus:ring-blue-500",
                            r#type: "text",
                            id: "username",
                            name: "username",
                            required: true,
                            value: "{username}",
                            oninput: move |evt| username.set(evt.value()),
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
                            name: "password",
                            required: true,
                            value: "{password}",
                            oninput: move |evt| password.set(evt.value()),
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
                            name: "confirm_password",
                            required: true,
                            value: "{confirm_password}",
                            oninput: move |evt| confirm_password.set(evt.value()),
                        }
                    }

                    button {
                        class: "w-full bg-blue-600 text-white py-2 rounded hover:bg-blue-700 disabled:opacity-50",
                        r#type: "submit",
                        disabled: loading(),
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
