use dioxus::prelude::*;

use crate::server_fns::moderation::list_mod_actions;

#[component]
pub fn AdminModLogPage() -> Element {
    let actions_future = use_server_future(list_mod_actions)?;

    let read_guard = actions_future.read();
    let actions = match &*read_guard {
        Some(Ok(a)) => a.clone(),
        Some(Err(e)) => {
            let err = e.to_string();
            return rsx! {
                div { class: "text-red-600 p-4", "Error loading mod log: {err}" }
            };
        }
        None => {
            return rsx! {
                div { class: "text-gray-500 p-4", "Loading..." }
            };
        }
    };

    rsx! {
        div { class: "max-w-4xl mx-auto space-y-4",
            h2 { class: "text-lg font-semibold text-gray-800", "Moderation Log" }

            if actions.is_empty() {
                div { class: "text-gray-500 text-sm", "No moderation actions recorded yet." }
            } else {
                div { class: "bg-white border border-gray-200 rounded-lg overflow-hidden",
                    table { class: "w-full text-sm",
                        thead {
                            tr { class: "bg-gray-50 text-left",
                                th { class: "px-4 py-3 font-medium text-gray-500", "Time" }
                                th { class: "px-4 py-3 font-medium text-gray-500", "Action" }
                                th { class: "px-4 py-3 font-medium text-gray-500", "Target" }
                                th { class: "px-4 py-3 font-medium text-gray-500", "By" }
                                th { class: "px-4 py-3 font-medium text-gray-500", "Reason" }
                            }
                        }
                        tbody {
                            for action in actions.iter() {
                                {
                                    let action_class = match action.action.as_str() {
                                        "ban" => "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-red-100 text-red-800",
                                        "unban" => "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-green-100 text-green-800",
                                        "suspend" => "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-orange-100 text-orange-800",
                                        "mute" => "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-yellow-100 text-yellow-800",
                                        "unmute" => "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-green-100 text-green-800",
                                        "kick" => "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-purple-100 text-purple-800",
                                        "delete_message" => "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-gray-100 text-gray-800",
                                        _ => "inline-block px-2 py-0.5 text-xs font-medium rounded-full bg-gray-100 text-gray-800",
                                    };
                                    let reason = action.reason.clone().unwrap_or_default();
                                    rsx! {
                                        tr { key: "{action.id}", class: "border-t border-gray-100",
                                            td { class: "px-4 py-3 text-gray-500 whitespace-nowrap", "{action.created_at}" }
                                            td { class: "px-4 py-3",
                                                span { class: action_class, "{action.action}" }
                                            }
                                            td { class: "px-4 py-3 font-medium text-gray-800", "{action.target_user}" }
                                            td { class: "px-4 py-3 text-gray-600", "{action.actor_user}" }
                                            td { class: "px-4 py-3 text-gray-600", "{reason}" }
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
