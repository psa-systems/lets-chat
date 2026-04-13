use dioxus::prelude::*;

use crate::models::User;
use crate::routes::Route;

#[component]
pub fn AdminLayout() -> Element {
    let user: Signal<Option<User>> = use_context::<Signal<Option<User>>>();
    let u = user().expect("user must be authenticated");

    let mut tabs: Vec<(&str, Route)> = vec![];

    if u.role == "admin" {
        tabs.push(("Settings", Route::AdminSettings {}));
    }

    tabs.push(("Users", Route::AdminUsers {}));

    if u.role == "admin" {
        tabs.push(("Invite Codes", Route::AdminInvites {}));
        tabs.push(("Rooms", Route::AdminRooms {}));
    }

    tabs.push(("Mod Log", Route::AdminModLog {}));

    let current_route = use_route::<Route>();

    rsx! {
        div { class: "flex-1 flex flex-col overflow-hidden",
            div { class: "flex border-b border-gray-200 bg-white px-4",
                for (label, route) in tabs {
                    {
                        let is_active = current_route == route;
                        let class = if is_active {
                            "px-4 py-2.5 text-sm font-medium border-b-2 -mb-px border-blue-500 text-blue-600"
                        } else {
                            "px-4 py-2.5 text-sm font-medium border-b-2 -mb-px border-transparent text-gray-500 hover:text-gray-700 hover:border-gray-300"
                        };
                        rsx! {
                            Link {
                                to: route,
                                class: class,
                                "{label}"
                            }
                        }
                    }
                }
            }
            div { class: "flex-1 overflow-y-auto p-6 bg-gray-50",
                Outlet::<Route> {}
            }
        }
    }
}
