use dioxus::prelude::*;

use crate::server_fns::admin::{create_invite_code, list_invite_codes, revoke_invite_code};

#[component]
pub fn AdminInvitesPage() -> Element {
    let mut invites_future = use_server_future(list_invite_codes)?;
    let mut feedback = use_signal(|| Option::<(bool, String)>::None);
    let mut generating = use_signal(|| false);

    let read_guard = invites_future.read();
    let invites = match &*read_guard {
        Some(Ok(list)) => list.clone(),
        Some(Err(e)) => {
            let err = e.to_string();
            return rsx! {
                div { class: "text-red-600 p-4", "Error loading invite codes: {err}" }
            };
        }
        None => {
            return rsx! {
                div { class: "text-gray-500 p-4", "Loading invite codes..." }
            };
        }
    };

    rsx! {
        div { class: "max-w-4xl mx-auto space-y-4",
            div { class: "flex items-center justify-between",
                h2 { class: "text-lg font-semibold text-gray-800", "Invite Codes" }
                button {
                    class: "px-4 py-2 bg-blue-600 text-white text-sm font-medium rounded-md hover:bg-blue-700 disabled:opacity-50",
                    disabled: generating(),
                    onclick: move |_| {
                        spawn(async move {
                            generating.set(true);
                            feedback.set(None);
                            match create_invite_code().await {
                                Ok(invite) => {
                                    feedback.set(Some((true, format!("Generated code: {}", invite.code))));
                                    invites_future.restart();
                                }
                                Err(e) => feedback.set(Some((false, format!("Error: {}", e)))),
                            }
                            generating.set(false);
                        });
                    },
                    if generating() { "Generating..." } else { "Generate Code" }
                }
            }

            if let Some((is_ok, msg)) = feedback() {
                div {
                    class: if is_ok { "p-3 rounded-lg bg-green-50 text-green-700 text-sm" } else { "p-3 rounded-lg bg-red-50 text-red-700 text-sm" },
                    "{msg}"
                }
            }

            div { class: "bg-white border border-gray-200 rounded-lg overflow-hidden",
                table { class: "w-full text-sm",
                    thead {
                        tr { class: "bg-gray-50 text-left",
                            th { class: "px-4 py-3 font-medium text-gray-500", "Code" }
                            th { class: "px-4 py-3 font-medium text-gray-500", "Created By" }
                            th { class: "px-4 py-3 font-medium text-gray-500", "Used By" }
                            th { class: "px-4 py-3 font-medium text-gray-500", "Status" }
                            th { class: "px-4 py-3 font-medium text-gray-500", "Created" }
                            th { class: "px-4 py-3 font-medium text-gray-500", "Actions" }
                        }
                    }
                    tbody {
                        for invite in invites.iter() {
                            {
                                let code = invite.code.clone();
                                let created_by = invite.created_by.clone();
                                let used_by = invite.used_by.clone().unwrap_or_default();
                                let is_used = invite.used_by.is_some();
                                let created_at = invite.created_at.clone();
                                let invite_id = invite.id;

                                let status = if is_used { "used" } else { "available" };
                                let status_class = if is_used {
                                    "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-gray-100 text-gray-600"
                                } else {
                                    "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-green-100 text-green-800"
                                };

                                rsx! {
                                    tr { key: "{invite_id}", class: "border-t border-gray-100",
                                        td { class: "px-4 py-3 font-mono text-gray-800", "{code}" }
                                        td { class: "px-4 py-3 text-gray-600", "{created_by}" }
                                        td { class: "px-4 py-3 text-gray-600",
                                            if used_by.is_empty() { "-" } else { "{used_by}" }
                                        }
                                        td { class: "px-4 py-3",
                                            span { class: status_class, "{status}" }
                                        }
                                        td { class: "px-4 py-3 text-gray-500", "{created_at}" }
                                        td { class: "px-4 py-3",
                                            if !is_used {
                                                button {
                                                    class: "text-sm text-red-600 hover:text-red-800 font-medium",
                                                    onclick: move |_| {
                                                        spawn(async move {
                                                            match revoke_invite_code(invite_id).await {
                                                                Ok(()) => {
                                                                    feedback.set(Some((true, "Code revoked.".to_string())));
                                                                    invites_future.restart();
                                                                }
                                                                Err(e) => feedback.set(Some((false, format!("Error: {}", e)))),
                                                            }
                                                        });
                                                    },
                                                    "Revoke"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if invites.is_empty() {
                    div { class: "px-4 py-8 text-center text-gray-400 text-sm", "No invite codes yet." }
                }
            }
        }
    }
}
