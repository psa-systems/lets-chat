use dioxus::prelude::*;

use crate::server_fns::admin::{
    create_room, delete_room, invite_user_to_room, regenerate_invite_code, update_room,
};
use crate::server_fns::chat::list_rooms;

#[component]
pub fn AdminRoomsPage() -> Element {
    let mut rooms_future = use_server_future(list_rooms)?;
    let mut feedback = use_signal(|| Option::<(bool, String)>::None);
    let mut new_name = use_signal(|| String::new());
    let mut new_topic = use_signal(|| String::new());
    let mut new_room_type = use_signal(|| "public".to_string());
    let mut creating = use_signal(|| false);
    let mut editing = use_signal(|| Option::<i64>::None);
    let mut edit_name = use_signal(|| String::new());
    let mut edit_topic = use_signal(|| String::new());
    let mut confirm_delete = use_signal(|| Option::<(i64, String)>::None);
    let mut inviting_room = use_signal(|| Option::<i64>::None);
    let mut invite_username = use_signal(|| String::new());

    let read_guard = rooms_future.read();
    let rooms = match &*read_guard {
        Some(Ok(list)) => list.clone(),
        Some(Err(e)) => {
            let err = crate::server_fns::auth::user_facing_error(e);
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
                div { class: "flex gap-3 items-end flex-wrap",
                    div { class: "flex-1 min-w-32",
                        label { class: "block text-sm font-medium text-gray-700 mb-1", "Name" }
                        input {
                            class: "w-full border border-gray-300 rounded-md px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                            r#type: "text",
                            placeholder: "general",
                            value: "{new_name}",
                            oninput: move |e| { let v = e.value(); spawn(async move { new_name.set(v); }); },
                        }
                    }
                    div { class: "flex-1 min-w-32",
                        label { class: "block text-sm font-medium text-gray-700 mb-1", "Topic" }
                        input {
                            class: "w-full border border-gray-300 rounded-md px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                            r#type: "text",
                            placeholder: "General discussion",
                            value: "{new_topic}",
                            oninput: move |e| { let v = e.value(); spawn(async move { new_topic.set(v); }); },
                        }
                    }
                    div {
                        label { class: "block text-sm font-medium text-gray-700 mb-1", "Type" }
                        select {
                            class: "border border-gray-300 rounded-md px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                            value: "{new_room_type}",
                            onchange: move |e| new_room_type.set(e.value()),
                            option { value: "public", "Public" }
                            option { value: "private", "Private" }
                        }
                    }
                    button {
                        class: "px-4 py-2 bg-blue-600 text-white text-sm font-medium rounded-md hover:bg-blue-700 disabled:opacity-50",
                        disabled: creating(),
                        onclick: move |_| {
                            let name = new_name();
                            let topic = new_topic();
                            let room_type = new_room_type();
                            if name.trim().is_empty() {
                                feedback.set(Some((false, "Room name is required.".to_string())));
                                return;
                            }
                            spawn(async move {
                                creating.set(true);
                                feedback.set(None);
                                match create_room(name, topic, room_type).await {
                                    Ok(room) => {
                                        feedback.set(Some((true, format!("Room '{}' created.", room.name))));
                                        new_name.set(String::new());
                                        new_topic.set(String::new());
                                        new_room_type.set("public".to_string());
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
                            th { class: "px-4 py-3 font-medium text-gray-500", "Type" }
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
                                let is_private = room.room_type == "private";
                                let invite_code = room.invite_code.clone();
                                let is_inviting = inviting_room() == Some(room_id);

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
                                                    oninput: move |e| { let v = e.value(); spawn(async move { edit_name.set(v); }); },
                                                }
                                            }
                                            td { class: "px-4 py-3 text-gray-500", "{room.room_type}" }
                                            td { class: "px-4 py-3",
                                                input {
                                                    class: "w-full border border-gray-300 rounded px-2 py-1 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
                                                    r#type: "text",
                                                    value: "{edit_topic}",
                                                    oninput: move |e| { let v = e.value(); spawn(async move { edit_topic.set(v); }); },
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
                                            td { class: "px-4 py-3 font-medium text-gray-800",
                                                div { "{room_name}" }
                                                if is_private {
                                                    if let Some(ref code) = invite_code {
                                                        div { class: "mt-1 space-y-1",
                                                            span { class: "text-xs font-mono text-gray-500 bg-gray-100 px-1 rounded",
                                                                "/invite/{code}"
                                                            }
                                                            button {
                                                                class: "block text-xs text-blue-500 hover:text-blue-700",
                                                                onclick: move |_| {
                                                                    spawn(async move {
                                                                        match regenerate_invite_code(room_id).await {
                                                                            Ok(new_code) => {
                                                                                feedback.set(Some((true, format!("New invite link: /invite/{}", new_code))));
                                                                                rooms_future.restart();
                                                                            }
                                                                            Err(e) => feedback.set(Some((false, format!("Error: {}", e)))),
                                                                        }
                                                                    });
                                                                },
                                                                "Regenerate link"
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            td { class: "px-4 py-3",
                                                span {
                                                    class: if is_private {
                                                        "text-xs font-medium px-1.5 py-0.5 rounded bg-purple-100 text-purple-700"
                                                    } else {
                                                        "text-xs font-medium px-1.5 py-0.5 rounded bg-green-100 text-green-700"
                                                    },
                                                    if is_private { "private" } else { "public" }
                                                }
                                            }
                                            td { class: "px-4 py-3 text-gray-600",
                                                if room_topic.is_empty() { "-" } else { "{room_topic}" }
                                            }
                                            td { class: "px-4 py-3 text-gray-500", "{created_at}" }
                                            td { class: "px-4 py-3",
                                                div { class: "flex flex-col gap-1",
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
                                                        if is_private {
                                                            button {
                                                                class: "text-sm text-purple-600 hover:text-purple-800 font-medium",
                                                                onclick: move |_| {
                                                                    if inviting_room() == Some(room_id) {
                                                                        inviting_room.set(None);
                                                                    } else {
                                                                        inviting_room.set(Some(room_id));
                                                                        invite_username.set(String::new());
                                                                    }
                                                                },
                                                                "Invite"
                                                            }
                                                        }
                                                        button {
                                                            class: "text-sm text-red-600 hover:text-red-800 font-medium",
                                                            onclick: move |_| {
                                                                confirm_delete.set(Some((room_id, name_for_delete.clone())));
                                                            },
                                                            "Delete"
                                                        }
                                                    }
                                                    if is_inviting {
                                                        div { class: "flex gap-1 mt-1",
                                                            input {
                                                                class: "border border-gray-300 rounded px-2 py-1 text-sm w-32",
                                                                r#type: "text",
                                                                placeholder: "username",
                                                                value: "{invite_username}",
                                                                oninput: move |e| { let v = e.value(); spawn(async move { invite_username.set(v); }); },
                                                            }
                                                            button {
                                                                class: "px-2 py-1 text-xs bg-purple-600 text-white rounded hover:bg-purple-700",
                                                                onclick: move |_| {
                                                                    let username = invite_username();
                                                                    if username.trim().is_empty() { return; }
                                                                    spawn(async move {
                                                                        match invite_user_to_room(room_id, username.clone()).await {
                                                                            Ok(()) => {
                                                                                feedback.set(Some((true, format!("Invited {} to room.", username))));
                                                                                inviting_room.set(None);
                                                                                invite_username.set(String::new());
                                                                            }
                                                                            Err(e) => feedback.set(Some((false, format!("Error: {}", e)))),
                                                                        }
                                                                    });
                                                                },
                                                                "Add"
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
                if rooms.is_empty() {
                    div { class: "px-4 py-8 text-center text-gray-400 text-sm", "No rooms yet." }
                }
            }
        }
    }
}
