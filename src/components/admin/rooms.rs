use dioxus::prelude::*;

use crate::server_fns::admin::{create_room, delete_room, update_room};
use crate::server_fns::chat::list_rooms;

#[component]
pub fn AdminRoomsPage() -> Element {
    let mut rooms_future = use_server_future(list_rooms)?;
    let mut feedback = use_signal(|| Option::<(bool, String)>::None);
    let mut new_name = use_signal(|| String::new());
    let mut new_topic = use_signal(|| String::new());
    let mut creating = use_signal(|| false);
    let mut editing = use_signal(|| Option::<i64>::None);
    let mut edit_name = use_signal(|| String::new());
    let mut edit_topic = use_signal(|| String::new());
    let mut confirm_delete = use_signal(|| Option::<(i64, String)>::None);

    let read_guard = rooms_future.read();
    let rooms = match &*read_guard {
        Some(Ok(list)) => list.clone(),
        Some(Err(e)) => {
            let err = e.to_string();
            return rsx! {
                div { class: "text-red-600 p-4", "Error loading rooms: {err}" }
            };
        }
        None => {
            return rsx! {
                div { class: "text-gray-500 p-4", "Loading rooms..." }
            };
        }
    };

    rsx! {
        div { class: "max-w-4xl mx-auto space-y-4",
            h2 { class: "text-lg font-semibold text-gray-800", "Rooms" }

            if let Some((is_ok, msg)) = feedback() {
                div {
                    class: if is_ok { "p-3 rounded-lg bg-green-50 text-green-700 text-sm" } else { "p-3 rounded-lg bg-red-50 text-red-700 text-sm" },
                    "{msg}"
                }
            }

            // Create room form
            div { class: "bg-white border border-gray-200 rounded-lg p-4",
                h3 { class: "text-sm font-semibold text-gray-700 mb-3", "Create Room" }
                div { class: "flex gap-3 items-end",
                    div { class: "flex-1",
                        label { class: "block text-sm font-medium text-gray-700 mb-1", "Name" }
                        input {
                            class: "w-full border border-gray-300 rounded-md px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                            r#type: "text",
                            placeholder: "general",
                            value: "{new_name}",
                            oninput: move |e| new_name.set(e.value()),
                        }
                    }
                    div { class: "flex-1",
                        label { class: "block text-sm font-medium text-gray-700 mb-1", "Topic" }
                        input {
                            class: "w-full border border-gray-300 rounded-md px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                            r#type: "text",
                            placeholder: "General discussion",
                            value: "{new_topic}",
                            oninput: move |e| new_topic.set(e.value()),
                        }
                    }
                    button {
                        class: "px-4 py-2 bg-blue-600 text-white text-sm font-medium rounded-md hover:bg-blue-700 disabled:opacity-50",
                        disabled: creating(),
                        onclick: move |_| {
                            let name = new_name();
                            let topic = new_topic();
                            if name.trim().is_empty() {
                                feedback.set(Some((false, "Room name is required.".to_string())));
                                return;
                            }
                            spawn(async move {
                                creating.set(true);
                                feedback.set(None);
                                match create_room(name, topic).await {
                                    Ok(room) => {
                                        feedback.set(Some((true, format!("Room '{}' created.", room.name))));
                                        new_name.set(String::new());
                                        new_topic.set(String::new());
                                        rooms_future.restart();
                                    }
                                    Err(e) => feedback.set(Some((false, format!("Error: {}", e)))),
                                }
                                creating.set(false);
                            });
                        },
                        if creating() { "Creating..." } else { "Create" }
                    }
                }
            }

            // Delete confirmation modal
            if let Some((room_id, room_name)) = confirm_delete() {
                div { class: "fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50",
                    div { class: "bg-white rounded-lg p-6 max-w-sm w-full mx-4",
                        h3 { class: "text-lg font-semibold text-gray-800 mb-2", "Delete Room" }
                        p { class: "text-sm text-gray-600 mb-4",
                            "Are you sure you want to delete room "
                            span { class: "font-medium", "{room_name}" }
                            "? All messages will be lost."
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
                                    spawn(async move {
                                        confirm_delete.set(None);
                                        match delete_room(room_id).await {
                                            Ok(()) => {
                                                feedback.set(Some((true, "Room deleted.".to_string())));
                                                rooms_future.restart();
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

            // Rooms table
            div { class: "bg-white border border-gray-200 rounded-lg overflow-hidden",
                table { class: "w-full text-sm",
                    thead {
                        tr { class: "bg-gray-50 text-left",
                            th { class: "px-4 py-3 font-medium text-gray-500", "Name" }
                            th { class: "px-4 py-3 font-medium text-gray-500", "Topic" }
                            th { class: "px-4 py-3 font-medium text-gray-500", "Created" }
                            th { class: "px-4 py-3 font-medium text-gray-500", "Actions" }
                        }
                    }
                    tbody {
                        for room in rooms.iter() {
                            {
                                let room_id = room.id;
                                let room_name = room.name.clone();
                                let room_topic = room.topic.clone().unwrap_or_default();
                                let created_at = room.created_at.clone();
                                let is_editing = editing() == Some(room_id);

                                let name_for_delete = room_name.clone();
                                let name_for_edit = room_name.clone();
                                let topic_for_edit = room_topic.clone();

                                rsx! {
                                    tr { key: "{room_id}", class: "border-t border-gray-100",
                                        if is_editing {
                                            td { class: "px-4 py-3",
                                                input {
                                                    class: "w-full border border-gray-300 rounded px-2 py-1 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                                                    r#type: "text",
                                                    value: "{edit_name}",
                                                    oninput: move |e| edit_name.set(e.value()),
                                                }
                                            }
                                            td { class: "px-4 py-3",
                                                input {
                                                    class: "w-full border border-gray-300 rounded px-2 py-1 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                                                    r#type: "text",
                                                    value: "{edit_topic}",
                                                    oninput: move |e| edit_topic.set(e.value()),
                                                }
                                            }
                                            td { class: "px-4 py-3 text-gray-500", "{created_at}" }
                                            td { class: "px-4 py-3",
                                                div { class: "flex gap-2",
                                                    button {
                                                        class: "text-sm text-blue-600 hover:text-blue-800 font-medium",
                                                        onclick: move |_| {
                                                            let name = edit_name();
                                                            let topic = edit_topic();
                                                            spawn(async move {
                                                                match update_room(room_id, name, topic).await {
                                                                    Ok(()) => {
                                                                        feedback.set(Some((true, "Room updated.".to_string())));
                                                                        editing.set(None);
                                                                        rooms_future.restart();
                                                                    }
                                                                    Err(e) => feedback.set(Some((false, format!("Error: {}", e)))),
                                                                }
                                                            });
                                                        },
                                                        "Save"
                                                    }
                                                    button {
                                                        class: "text-sm text-gray-500 hover:text-gray-700 font-medium",
                                                        onclick: move |_| editing.set(None),
                                                        "Cancel"
                                                    }
                                                }
                                            }
                                        } else {
                                            td { class: "px-4 py-3 font-medium text-gray-800", "{room_name}" }
                                            td { class: "px-4 py-3 text-gray-600",
                                                if room_topic.is_empty() { "-" } else { "{room_topic}" }
                                            }
                                            td { class: "px-4 py-3 text-gray-500", "{created_at}" }
                                            td { class: "px-4 py-3",
                                                div { class: "flex gap-2",
                                                    button {
                                                        class: "text-sm text-blue-600 hover:text-blue-800 font-medium",
                                                        onclick: move |_| {
                                                            editing.set(Some(room_id));
                                                            edit_name.set(name_for_edit.clone());
                                                            edit_topic.set(topic_for_edit.clone());
                                                        },
                                                        "Edit"
                                                    }
                                                    button {
                                                        class: "text-sm text-red-600 hover:text-red-800 font-medium",
                                                        onclick: move |_| {
                                                            confirm_delete.set(Some((room_id, name_for_delete.clone())));
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
                if rooms.is_empty() {
                    div { class: "px-4 py-8 text-center text-gray-400 text-sm", "No rooms yet." }
                }
            }
        }
    }
}
