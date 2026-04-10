use dioxus::prelude::*;

use crate::routes::Route;

use super::sidebar::Sidebar;

#[component]
pub fn Layout() -> Element {
    rsx! {
        div { class: "flex h-screen bg-gray-100",
            aside { class: "w-64 flex-shrink-0 bg-white border-r border-gray-200 flex flex-col",
                Sidebar {}
            }
            main { class: "flex-1 flex flex-col overflow-hidden",
                Outlet::<Route> {}
            }
        }
    }
}
