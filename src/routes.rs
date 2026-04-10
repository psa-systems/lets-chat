use dioxus::prelude::*;

use crate::components::{layout::Layout, room_view::RoomViewPage, welcome::WelcomePage};

#[derive(Routable, Clone, PartialEq, Debug)]
pub enum Route {
    #[layout(Layout)]
    #[route("/")]
    Home {},
    #[route("/room/:room_id")]
    Room { room_id: String },
}

#[component]
fn Home() -> Element {
    rsx! {
        WelcomePage {}
    }
}

#[component]
fn Room(room_id: String) -> Element {
    rsx! {
        RoomViewPage { room_id }
    }
}
