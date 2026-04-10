use dioxus::prelude::*;

use crate::components::{
    auth_layout::AuthLayout,
    layout::Layout,
    login::LoginPage,
    register::RegisterPage,
    room_view::RoomViewPage,
    welcome::WelcomePage,
};

#[derive(Routable, Clone, PartialEq, Debug)]
pub enum Route {
    #[route("/login")]
    Login {},
    #[route("/register")]
    Register {},
    #[layout(AuthLayout)]
    #[layout(Layout)]
    #[route("/")]
    Home {},
    #[route("/room/:room_id")]
    Room { room_id: String },
}

#[component]
fn Login() -> Element {
    rsx! { LoginPage {} }
}

#[component]
fn Register() -> Element {
    rsx! { RegisterPage {} }
}

#[component]
fn Home() -> Element {
    rsx! { WelcomePage {} }
}

#[component]
fn Room(room_id: String) -> Element {
    rsx! { RoomViewPage { room_id } }
}
