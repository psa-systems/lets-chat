use dioxus::prelude::*;

use crate::components::{
    admin::{
        invites::AdminInvitesPage,
        layout::AdminLayout,
        mod_log::AdminModLogPage,
        rooms::AdminRoomsPage,
        settings::AdminSettingsPage,
        users::AdminUsersPage,
    },
    auth_layout::AuthLayout,
    dm_view::DmViewPage,
    invite::InvitePage,
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
    #[route("/dm/:user_id")]
    Dm { user_id: String },
    #[route("/invite/:code")]
    Invite { code: String },
    #[layout(AdminLayout)]
    #[route("/admin")]
    AdminSettings {},
    #[route("/admin/users")]
    AdminUsers {},
    #[route("/admin/invites")]
    AdminInvites {},
    #[route("/admin/rooms")]
    AdminRooms {},
    #[route("/admin/modlog")]
    AdminModLog {},
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

#[component]
fn Dm(user_id: String) -> Element {
    rsx! { DmViewPage { user_id } }
}

#[component]
fn AdminSettings() -> Element {
    rsx! { AdminSettingsPage {} }
}

#[component]
fn AdminUsers() -> Element {
    rsx! { AdminUsersPage {} }
}

#[component]
fn AdminInvites() -> Element {
    rsx! { AdminInvitesPage {} }
}

#[component]
fn AdminRooms() -> Element {
    rsx! { AdminRoomsPage {} }
}

#[component]
fn AdminModLog() -> Element {
    rsx! { AdminModLogPage {} }
}

#[component]
fn Invite(code: String) -> Element {
    rsx! { InvitePage { code } }
}
