use dioxus::prelude::*;

use crate::models::SiteSettings;
use crate::server_fns::admin::{get_settings, update_settings};

#[component]
pub fn AdminSettingsPage() -> Element {
    let settings_future = use_server_future(get_settings)?;
    let mut feedback = use_signal(|| Option::<(bool, String)>::None);
    let mut saving = use_signal(|| false);

    let read_guard = settings_future.read();
    let settings = match &*read_guard {
        Some(Ok(s)) => s.clone(),
        Some(Err(e)) => {
            let err = crate::server_fns::auth::user_facing_error(e);
            return rsx! {
                div { class: "text-red-600 p-4", "Error loading settings: {err}" }
            };
        }
        None => {
            return rsx! {
                div { class: "text-gray-500 p-4", "Loading settings..." }
            };
        }
    };

    // Form state signals
    let mut site_name = use_signal(|| settings.site_name.clone());
    let mut welcome_message = use_signal(|| settings.welcome_message.clone());
    let mut motd = use_signal(|| settings.motd.clone());
    let mut registration_open = use_signal(|| settings.registration_open);
    let mut require_invite_code = use_signal(|| settings.require_invite_code);
    let mut maintenance_mode = use_signal(|| settings.maintenance_mode);
    let mut default_role = use_signal(|| settings.default_role.clone());
    let mut max_message_length = use_signal(|| settings.max_message_length.to_string());
    let mut rate_limit_messages = use_signal(|| settings.rate_limit_messages.to_string());
    let mut smtp_host = use_signal(|| settings.smtp_host.clone());
    let mut smtp_port = use_signal(|| settings.smtp_port.clone());
    let mut smtp_user = use_signal(|| settings.smtp_user.clone());
    let mut smtp_pass = use_signal(|| settings.smtp_pass.clone());

    let save_all = move |_| {
        spawn(async move {
            saving.set(true);
            feedback.set(None);

            let updated = SiteSettings {
                site_name: site_name(),
                registration_open: registration_open(),
                require_invite_code: require_invite_code(),
                default_role: default_role(),
                welcome_message: welcome_message(),
                max_message_length: max_message_length().parse().unwrap_or(4000),
                motd: motd(),
                maintenance_mode: maintenance_mode(),
                rate_limit_messages: rate_limit_messages().parse().unwrap_or(30),
                smtp_host: smtp_host(),
                smtp_port: smtp_port(),
                smtp_user: smtp_user(),
                smtp_pass: smtp_pass(),
            };

            match update_settings(updated).await {
                Ok(()) => feedback.set(Some((true, "Settings saved successfully.".to_string()))),
                Err(e) => feedback.set(Some((false, format!("Error: {}", e)))),
            }
            saving.set(false);
        });
    };

    rsx! {
        div { class: "max-w-3xl mx-auto space-y-6",
            // Feedback banner
            if let Some((is_ok, msg)) = feedback() {
                div {
                    class: if is_ok { "p-3 rounded-lg bg-green-50 text-green-700 text-sm" } else { "p-3 rounded-lg bg-red-50 text-red-700 text-sm" },
                    "{msg}"
                }
            }

            // General Settings
            div { class: "bg-white border border-gray-200 rounded-lg p-4",
                h2 { class: "text-lg font-semibold text-gray-800 mb-4", "General" }
                div { class: "space-y-4",
                    div {
                        label { class: "block text-sm font-medium text-gray-700 mb-1", "Site Name" }
                        input {
                            class: "w-full border border-gray-300 rounded-md px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                            r#type: "text",
                            value: "{site_name}",
                            oninput: move |e| { let v = e.value(); spawn(async move { site_name.set(v); }); },
                        }
                    }
                    div {
                        label { class: "block text-sm font-medium text-gray-700 mb-1", "Welcome Message" }
                        textarea {
                            class: "w-full border border-gray-300 rounded-md px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                            rows: "3",
                            value: "{welcome_message}",
                            oninput: move |e| { let v = e.value(); spawn(async move { welcome_message.set(v); }); },
                        }
                    }
                    div {
                        label { class: "block text-sm font-medium text-gray-700 mb-1", "Message of the Day" }
                        textarea {
                            class: "w-full border border-gray-300 rounded-md px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                            rows: "2",
                            value: "{motd}",
                            oninput: move |e| { let v = e.value(); spawn(async move { motd.set(v); }); },
                        }
                    }
                    div { class: "flex items-center gap-3",
                        input {
                            r#type: "checkbox",
                            class: "h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500",
                            checked: maintenance_mode(),
                            oninput: move |e| { let v = e.checked(); spawn(async move { maintenance_mode.set(v); }); },
                        }
                        label { class: "text-sm font-medium text-gray-700", "Maintenance Mode" }
                    }
                }
            }

            // Registration Settings
            div { class: "bg-white border border-gray-200 rounded-lg p-4",
                h2 { class: "text-lg font-semibold text-gray-800 mb-4", "Registration" }
                div { class: "space-y-4",
                    div { class: "flex items-center gap-3",
                        input {
                            r#type: "checkbox",
                            class: "h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500",
                            checked: registration_open(),
                            oninput: move |e| { let v = e.checked(); spawn(async move { registration_open.set(v); }); },
                        }
                        label { class: "text-sm font-medium text-gray-700", "Registration Open" }
                    }
                    div { class: "flex items-center gap-3",
                        input {
                            r#type: "checkbox",
                            class: "h-4 w-4 rounded border-gray-300 text-blue-600 focus:ring-blue-500",
                            checked: require_invite_code(),
                            oninput: move |e| { let v = e.checked(); spawn(async move { require_invite_code.set(v); }); },
                        }
                        label { class: "text-sm font-medium text-gray-700", "Require Invite Code" }
                    }
                    div {
                        label { class: "block text-sm font-medium text-gray-700 mb-1", "Default Role" }
                        select {
                            class: "w-full border border-gray-300 rounded-md px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                            value: "{default_role}",
                            oninput: move |e| { let v = e.value(); spawn(async move { default_role.set(v); }); },
                            option { value: "user", "user" }
                            option { value: "moderator", "moderator" }
                            option { value: "admin", "admin" }
                        }
                    }
                }
            }

            // Limits
            div { class: "bg-white border border-gray-200 rounded-lg p-4",
                h2 { class: "text-lg font-semibold text-gray-800 mb-4", "Limits" }
                div { class: "space-y-4",
                    div {
                        label { class: "block text-sm font-medium text-gray-700 mb-1", "Max Message Length" }
                        input {
                            class: "w-full border border-gray-300 rounded-md px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                            r#type: "number",
                            value: "{max_message_length}",
                            oninput: move |e| { let v = e.value(); spawn(async move { max_message_length.set(v); }); },
                        }
                    }
                    div {
                        label { class: "block text-sm font-medium text-gray-700 mb-1", "Rate Limit (messages per minute)" }
                        input {
                            class: "w-full border border-gray-300 rounded-md px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                            r#type: "number",
                            value: "{rate_limit_messages}",
                            oninput: move |e| { let v = e.value(); spawn(async move { rate_limit_messages.set(v); }); },
                        }
                    }
                }
            }

            // SMTP
            div { class: "bg-white border border-gray-200 rounded-lg p-4",
                h2 { class: "text-lg font-semibold text-gray-800 mb-4", "SMTP" }
                div { class: "space-y-4",
                    div {
                        label { class: "block text-sm font-medium text-gray-700 mb-1", "SMTP Host" }
                        input {
                            class: "w-full border border-gray-300 rounded-md px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                            r#type: "text",
                            value: "{smtp_host}",
                            oninput: move |e| { let v = e.value(); spawn(async move { smtp_host.set(v); }); },
                        }
                    }
                    div {
                        label { class: "block text-sm font-medium text-gray-700 mb-1", "SMTP Port" }
                        input {
                            class: "w-full border border-gray-300 rounded-md px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                            r#type: "text",
                            value: "{smtp_port}",
                            oninput: move |e| { let v = e.value(); spawn(async move { smtp_port.set(v); }); },
                        }
                    }
                    div {
                        label { class: "block text-sm font-medium text-gray-700 mb-1", "SMTP User" }
                        input {
                            class: "w-full border border-gray-300 rounded-md px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                            r#type: "text",
                            value: "{smtp_user}",
                            oninput: move |e| { let v = e.value(); spawn(async move { smtp_user.set(v); }); },
                        }
                    }
                    div {
                        label { class: "block text-sm font-medium text-gray-700 mb-1", "SMTP Password" }
                        input {
                            class: "w-full border border-gray-300 rounded-md px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                            r#type: "password",
                            value: "{smtp_pass}",
                            oninput: move |e| { let v = e.value(); spawn(async move { smtp_pass.set(v); }); },
                        }
                    }
                }
            }

            // Save button
            div { class: "flex justify-end",
                button {
                    class: "px-4 py-2 bg-blue-600 text-white text-sm font-medium rounded-md hover:bg-blue-700 disabled:opacity-50",
                    disabled: saving(),
                    onclick: save_all,
                    if saving() { "Saving..." } else { "Save All Settings" }
                }
            }
        }
    }
}
