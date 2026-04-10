use dioxus::prelude::*;

#[component]
pub fn WelcomePage() -> Element {
    rsx! {
        div { class: "flex-1 flex items-center justify-center text-gray-500",
            div { class: "text-center",
                p { class: "text-2xl mb-2", "Welcome to Let's Chat" }
                p { class: "text-sm", "Pick a room from the sidebar to start chatting" }
            }
        }
    }
}
