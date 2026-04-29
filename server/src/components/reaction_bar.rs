use dioxus::prelude::*;

use crate::models::Reaction;
use crate::server_fns::reactions::toggle_reaction;

const EMOJI_PICKER: &[&str] = &[
    "👍", "👎", "❤️", "😂", "😮", "😢", "🔥", "🎉", "👀", "✅",
];

#[component]
pub fn ReactionBar(
    message_id: i64,
    room_id: i64,
    /// Reactions for this message. Owned by the parent and re-passed on every
    /// render so WS-driven updates reach all viewers, not only the user who
    /// toggled the reaction.
    initial_reactions: Vec<Reaction>,
    /// The current user's id (kept for prop compat with callers).
    my_user_id: String,
) -> Element {
    let mut show_picker = use_signal(|| false);

    rsx! {
        div { class: "flex items-center gap-1 flex-wrap mt-1",
            // Existing reactions
            for reaction in initial_reactions.iter() {
                {
                    let emoji = reaction.emoji.clone();
                    let count = reaction.count;
                    let reacted = reaction.reacted_by_me;
                    let emoji_for_click = emoji.clone();
                    rsx! {
                        button {
                            key: "{emoji}",
                            class: if reacted {
                                "inline-flex items-center gap-0.5 px-1.5 py-0.5 rounded-full text-xs border border-blue-400 bg-blue-50 text-blue-700 hover:bg-blue-100"
                            } else {
                                "inline-flex items-center gap-0.5 px-1.5 py-0.5 rounded-full text-xs border border-gray-200 bg-gray-50 text-gray-600 hover:bg-gray-100"
                            },
                            r#type: "button",
                            onclick: move |_| {
                                let e = emoji_for_click.clone();
                                let mid = message_id;
                                spawn(async move {
                                    let _ = toggle_reaction(mid, e).await;
                                });
                            },
                            span { "{emoji}" }
                            span { "{count}" }
                        }
                    }
                }
            }

            // + button to open picker
            div { class: "relative",
                button {
                    class: "opacity-0 group-hover:opacity-100 inline-flex items-center justify-center w-6 h-6 rounded-full text-xs border border-gray-200 text-gray-400 hover:bg-gray-100 hover:text-gray-600 transition-opacity",
                    r#type: "button",
                    onclick: move |_| {
                        spawn(async move {
                            show_picker.set(!show_picker());
                        });
                    },
                    "+"
                }
                if show_picker() {
                    div { class: "absolute top-full mt-1 left-0 z-20 bg-white border border-gray-200 rounded-lg shadow-lg p-2 flex flex-wrap gap-1",
                        style: "min-width: 180px;",
                        for &e in EMOJI_PICKER {
                            {
                                let emoji = e.to_string();
                                rsx! {
                                    button {
                                        key: "{e}",
                                        class: "text-lg hover:bg-gray-100 rounded p-1 w-8 h-8 flex items-center justify-center",
                                        r#type: "button",
                                        onclick: move |_| {
                                            let em = emoji.clone();
                                            let mid = message_id;
                                            spawn(async move {
                                                show_picker.set(false);
                                                let _ = toggle_reaction(mid, em).await;
                                            });
                                        },
                                        "{e}"
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
