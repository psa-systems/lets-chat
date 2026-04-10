use dioxus::prelude::*;

use crate::server_fns::admin::{change_user_role, delete_user, list_users};

#[component]
pub fn AdminUsersPage() -> Element {
    let mut users_future = use_server_future(list_users)?;
    let mut feedback = use_signal(|| Option::<(bool, String)>::None);
    let mut confirm_delete = use_signal(|| Option::<(String, String)>::None);

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

    rsx! {
        div { class: "max-w-4xl mx-auto space-y-4",
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

            div { class: "bg-white border border-gray-200 rounded-lg overflow-hidden",
                table { class: "w-full text-sm",
                    thead {
                        tr { class: "bg-gray-50 text-left",
                            th { class: "px-4 py-3 font-medium text-gray-500", "Username" }
                            th { class: "px-4 py-3 font-medium text-gray-500", "Role" }
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
                                let is_muted = user.is_muted;
                                let created = user.created_at.clone();

                                let status = if is_muted { "muted" } else { "active" };
                                let status_class = if is_muted {
                                    "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-yellow-100 text-yellow-800"
                                } else {
                                    "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-green-100 text-green-800"
                                };

                                let uid_for_role = user_id.clone();
                                let uid_for_delete = user_id.clone();
                                let uname_for_delete = username.clone();

                                rsx! {
                                    tr { key: "{user_id}", class: "border-t border-gray-100",
                                        td { class: "px-4 py-3 font-medium text-gray-800", "{username}" }
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
                                        td { class: "px-4 py-3",
                                            span { class: status_class, "{status}" }
                                        }
                                        td { class: "px-4 py-3 text-gray-500", "{created}" }
                                        td { class: "px-4 py-3",
                                            button {
                                                class: "text-sm text-red-600 hover:text-red-800 font-medium",
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
