use dioxus::prelude::*;

use crate::models::User;
use crate::server_fns::admin::{change_user_role, delete_user, list_users};
use crate::server_fns::moderation::{
    ban_user, mute_user, unban_user, unmute_user,
};

#[component]
pub fn AdminUsersPage() -> Element {
    let current_user: Signal<Option<User>> = use_context::<Signal<Option<User>>>();
    let mut users_future = use_server_future(list_users)?;
    let mut feedback = use_signal(|| Option::<(bool, String)>::None);
    let mut confirm_delete = use_signal(|| Option::<(String, String)>::None);
    let mut mod_modal = use_signal(|| Option::<ModModalState>::None);

    let read_guard = users_future.read();
    let users = match &*read_guard {
        Some(Ok(u)) => u.clone(),
        Some(Err(e)) => {
            let err = e.to_string();
            return rsx! {
                div { class: "text-red-600 p-4", "Error loading users: {err}" }
            };
        }
        None => {
            return rsx! {
                div { class: "text-gray-500 p-4", "Loading users..." }
            };
        }
    };

    let cu = current_user().expect("user must be authenticated");
    let is_admin = cu.role == "admin";

    rsx! {
        div { class: "max-w-5xl mx-auto space-y-4",
            h2 { class: "text-lg font-semibold text-gray-800", "Users" }

            if let Some((is_ok, msg)) = feedback() {
                div {
                    class: if is_ok { "p-3 rounded-lg bg-green-50 text-green-700 text-sm" } else { "p-3 rounded-lg bg-red-50 text-red-700 text-sm" },
                    "{msg}"
                }
            }

            // Delete confirmation modal
            if let Some((user_id, username)) = confirm_delete() {
                div { class: "fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50",
                    div { class: "bg-white rounded-lg p-6 max-w-sm w-full mx-4",
                        h3 { class: "text-lg font-semibold text-gray-800 mb-2", "Delete User" }
                        p { class: "text-sm text-gray-600 mb-4",
                            "Are you sure you want to delete user "
                            span { class: "font-medium", "{username}" }
                            "? This cannot be undone."
                        }
                        div { class: "flex justify-end gap-2",
                            button {
                                class: "px-3 py-1.5 text-sm font-medium text-gray-700 bg-gray-100 rounded-md hover:bg-gray-200",
                                onclick: move |_| confirm_delete.set(None),
                                "Cancel"
                            }
                            button {
                                class: "px-3 py-1.5 text-sm font-medium text-white bg-red-600 rounded-md hover:bg-red-700",
                                onclick: move |_| {
                                    let uid = user_id.clone();
                                    spawn(async move {
                                        confirm_delete.set(None);
                                        match delete_user(uid).await {
                                            Ok(()) => {
                                                feedback.set(Some((true, "User deleted.".to_string())));
                                                users_future.restart();
                                            }
                                            Err(e) => feedback.set(Some((false, format!("Error: {}", e)))),
                                        }
                                    });
                                },
                                "Delete"
                            }
                        }
                    }
                }
            }

            // Mod action modal (ban/mute with reason + duration)
            if let Some(state) = mod_modal() {
                { render_mod_modal(state, mod_modal, feedback, users_future) }
            }

            div { class: "bg-white border border-gray-200 rounded-lg overflow-hidden",
                table { class: "w-full text-sm",
                    thead {
                        tr { class: "bg-gray-50 text-left",
                            th { class: "px-4 py-3 font-medium text-gray-500", "Username" }
                            if is_admin {
                                th { class: "px-4 py-3 font-medium text-gray-500", "Role" }
                            }
                            th { class: "px-4 py-3 font-medium text-gray-500", "Status" }
                            th { class: "px-4 py-3 font-medium text-gray-500", "Created" }
                            th { class: "px-4 py-3 font-medium text-gray-500", "Actions" }
                        }
                    }
                    tbody {
                        for user in users.iter() {
                            {
                                let user_id = user.id.clone();
                                let username = user.username.clone();
                                let role = user.role.clone();
                                let user_is_banned = user.is_banned;
                                let user_is_muted = user.is_muted;
                                let created = user.created_at.clone();

                                let status_text = if user_is_banned {
                                    "banned"
                                } else if user_is_muted {
                                    "muted"
                                } else {
                                    "active"
                                };
                                let status_class = if user_is_banned {
                                    "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-red-100 text-red-800"
                                } else if user_is_muted {
                                    "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-yellow-100 text-yellow-800"
                                } else {
                                    "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-green-100 text-green-800"
                                };

                                let uid_for_role = user_id.clone();
                                let uid_for_delete = user_id.clone();
                                let uname_for_delete = username.clone();
                                let uid_for_ban = user_id.clone();
                                let uname_for_ban = username.clone();
                                let uid_for_mute = user_id.clone();
                                let uname_for_mute = username.clone();
                                let uid_for_unban = user_id.clone();
                                let uid_for_unmute = user_id.clone();

                                rsx! {
                                    tr { key: "{user_id}", class: "border-t border-gray-100",
                                        td { class: "px-4 py-3 font-medium text-gray-800",
                                            "{username}"
                                            span { class: "ml-1 text-xs text-gray-400", "({role})" }
                                        }
                                        if is_admin {
                                            td { class: "px-4 py-3",
                                                select {
                                                    class: "border border-gray-300 rounded px-2 py-1 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                                                    value: "{role}",
                                                    oninput: move |e| {
                                                        let new_role = e.value();
                                                        let uid = uid_for_role.clone();
                                                        spawn(async move {
                                                            match change_user_role(uid, new_role).await {
                                                                Ok(()) => {
                                                                    feedback.set(Some((true, "Role updated.".to_string())));
                                                                    users_future.restart();
                                                                }
                                                                Err(e) => feedback.set(Some((false, format!("Error: {}", e)))),
                                                            }
                                                        });
                                                    },
                                                    option { value: "user", selected: role == "user", "user" }
                                                    option { value: "moderator", selected: role == "moderator", "moderator" }
                                                    option { value: "admin", selected: role == "admin", "admin" }
                                                }
                                            }
                                        }
                                        td { class: "px-4 py-3",
                                            span { class: status_class, "{status_text}" }
                                        }
                                        td { class: "px-4 py-3 text-gray-500", "{created}" }
                                        td { class: "px-4 py-3 space-x-2",
                                            if user_is_banned {
                                                button {
                                                    class: "text-xs text-green-600 hover:text-green-800 font-medium",
                                                    onclick: move |_| {
                                                        let uid = uid_for_unban.clone();
                                                        spawn(async move {
                                                            match unban_user(uid).await {
                                                                Ok(()) => {
                                                                    feedback.set(Some((true, "User unbanned.".to_string())));
                                                                    users_future.restart();
                                                                }
                                                                Err(e) => feedback.set(Some((false, format!("Error: {}", e)))),
                                                            }
                                                        });
                                                    },
                                                    "Unban"
                                                }
                                            } else {
                                                button {
                                                    class: "text-xs text-red-600 hover:text-red-800 font-medium",
                                                    onclick: move |_| {
                                                        mod_modal.set(Some(ModModalState {
                                                            action: "ban".to_string(),
                                                            user_id: uid_for_ban.clone(),
                                                            username: uname_for_ban.clone(),
                                                        }));
                                                    },
                                                    "Ban"
                                                }
                                            }
                                            if user_is_muted {
                                                button {
                                                    class: "text-xs text-green-600 hover:text-green-800 font-medium",
                                                    onclick: move |_| {
                                                        let uid = uid_for_unmute.clone();
                                                        spawn(async move {
                                                            match unmute_user(uid).await {
                                                                Ok(()) => {
                                                                    feedback.set(Some((true, "User unmuted.".to_string())));
                                                                    users_future.restart();
                                                                }
                                                                Err(e) => feedback.set(Some((false, format!("Error: {}", e)))),
                                                            }
                                                        });
                                                    },
                                                    "Unmute"
                                                }
                                            } else {
                                                button {
                                                    class: "text-xs text-yellow-600 hover:text-yellow-800 font-medium",
                                                    onclick: move |_| {
                                                        mod_modal.set(Some(ModModalState {
                                                            action: "mute".to_string(),
                                                            user_id: uid_for_mute.clone(),
                                                            username: uname_for_mute.clone(),
                                                        }));
                                                    },
                                                    "Mute"
                                                }
                                            }
                                            if is_admin {
                                                button {
                                                    class: "text-xs text-red-600 hover:text-red-800 font-medium",
                                                    onclick: move |_| {
                                                        confirm_delete.set(Some((uid_for_delete.clone(), uname_for_delete.clone())));
                                                    },
                                                    "Delete"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ModModalState {
    action: String,
    user_id: String,
    username: String,
}

fn render_mod_modal(
    state: ModModalState,
    mut mod_modal: Signal<Option<ModModalState>>,
    mut feedback: Signal<Option<(bool, String)>>,
    mut users_future: Resource<Result<Vec<User>, dioxus::prelude::ServerFnError>>,
) -> Element {
    let mut reason = use_signal(String::new);
    let mut duration = use_signal(|| "1h".to_string());

    let title = match state.action.as_str() {
        "ban" => format!("Ban {}", state.username),
        "mute" => format!("Mute {}", state.username),
        _ => "Mod Action".to_string(),
    };

    let show_duration = state.action == "mute";

    rsx! {
        div { class: "fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50",
            div { class: "bg-white rounded-lg p-6 max-w-sm w-full mx-4",
                h3 { class: "text-lg font-semibold text-gray-800 mb-4", "{title}" }
                div { class: "space-y-3",
                    div {
                        label { class: "block text-sm font-medium text-gray-700 mb-1", "Reason" }
                        input {
                            class: "w-full px-3 py-1.5 border border-gray-300 rounded text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                            r#type: "text",
                            placeholder: "Reason for action...",
                            value: "{reason}",
                            oninput: move |e| { let v = e.value(); spawn(async move { reason.set(v); }); },
                        }
                    }
                    if show_duration {
                        div {
                            label { class: "block text-sm font-medium text-gray-700 mb-1", "Duration" }
                            select {
                                class: "w-full px-3 py-1.5 border border-gray-300 rounded text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                                value: "{duration}",
                                oninput: move |e| { let v = e.value(); spawn(async move { duration.set(v); }); },
                                option { value: "1", "1 hour" }
                                option { value: "24", "24 hours" }
                                option { value: "168", "7 days" }
                                option { value: "720", "30 days" }
                                option { value: "", "Permanent" }
                            }
                        }
                    }
                }
                div { class: "flex justify-end gap-2 mt-4",
                    button {
                        class: "px-3 py-1.5 text-sm font-medium text-gray-700 bg-gray-100 rounded-md hover:bg-gray-200",
                        onclick: move |_| mod_modal.set(None),
                        "Cancel"
                    }
                    button {
                        class: "px-3 py-1.5 text-sm font-medium text-white bg-red-600 rounded-md hover:bg-red-700",
                        onclick: move |_| {
                            let action = state.action.clone();
                            let uid = state.user_id.clone();
                            let r = reason();
                            let d = duration();
                            spawn(async move {
                                mod_modal.set(None);
                                let result = match action.as_str() {
                                    "ban" => ban_user(uid, r).await,
                                    "mute" => mute_user(uid, d, r).await,
                                    _ => Ok(()),
                                };
                                match result {
                                    Ok(()) => {
                                        feedback.set(Some((true, format!("{} applied.", action))));
                                        users_future.restart();
                                    }
                                    Err(e) => feedback.set(Some((false, format!("Error: {}", e)))),
                                }
                            });
                        },
                        "Confirm"
                    }
                }
            }
        }
    }
}
