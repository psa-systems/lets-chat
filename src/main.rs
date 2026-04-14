use dioxus::prelude::*;

mod components;
#[cfg(not(target_arch = "wasm32"))]
mod db;
mod models;
mod routes;
mod server_fns;
mod ws;

use routes::Route;

#[cfg(not(target_arch = "wasm32"))]
fn parse_data_dir() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.windows(2)
        .find(|pair| pair[0] == "--data-dir")
        .map(|pair| pair[1].clone())
}

#[cfg(all(not(target_arch = "wasm32"), feature = "server"))]
fn build_server_router() -> axum::Router {
    use axum::routing::get;
    use dioxus::server::{DioxusRouterExt, FullstackState};
    use tower_http::services::ServeFile;

    let index_html = std::env::var("DIOXUS_PUBLIC_PATH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::current_exe()
                .expect("current_exe")
                .parent()
                .expect("parent")
                .join("public")
        })
        .join("index.html");

    axum::Router::new()
        .route("/ws", get(ws::handler::ws_handler))
        .register_server_functions()
        .serve_static_assets()
        .fallback_service(ServeFile::new(index_html))
        .with_state(FullstackState::headless())
}

fn main() {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use tracing_subscriber::EnvFilter;
        tracing_subscriber::fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| EnvFilter::new("lets_chat=info")),
            )
            .init();

        let data_dir = parse_data_dir()
            .or_else(|| std::env::var("LETS_CHAT_DATA_DIR").ok())
            .unwrap_or_else(|| "/data".to_string());
        tracing::info!(data_dir = %data_dir, "starting lets-chat");
        db::set_data_dir(data_dir);
    }

    #[cfg(target_os = "linux")]
    {
        if std::env::var("GDK_BACKEND").unwrap_or_default().is_empty() {
            unsafe { std::env::set_var("GDK_BACKEND", "x11") };
        }
        if std::env::var("GDK_SCALE").unwrap_or_default().is_empty() {
            unsafe { std::env::set_var("GDK_SCALE", "1") };
        }
        if std::env::var("GDK_DPI_SCALE").unwrap_or_default().is_empty() {
            unsafe { std::env::set_var("GDK_DPI_SCALE", "1") };
        }
    }

    // Desktop: spawn an embedded Axum server on a background thread before
    // launching the UI.
    #[cfg(feature = "desktop")]
    {
        std::thread::spawn(|| {
            let rt = tokio::runtime::Runtime::new().expect("Failed to create server runtime");
            rt.block_on(async {
                let router = build_server_router();
                let listener = tokio::net::TcpListener::bind("127.0.0.1:8080")
                    .await
                    .expect("Failed to bind server to 127.0.0.1:8080");
                axum::serve(listener, router.into_make_service())
                    .await
                    .expect("Server exited unexpectedly");
            });
        });

        std::thread::sleep(std::time::Duration::from_millis(200));

        dioxus::LaunchBuilder::new()
            .with_cfg(desktop! {
                dioxus::desktop::Config::new().with_window(
                    dioxus::desktop::WindowBuilder::new()
                        .with_always_on_top(false)
                )
            })
            .launch(App);
    }

    // Web fullstack server binary built by `dx build`.
    #[cfg(all(not(target_arch = "wasm32"), not(feature = "desktop"), feature = "server"))]
    {
        tokio::runtime::Runtime::new()
            .expect("Failed to create runtime")
            .block_on(async {
                let router = build_server_router();
                let addr = dioxus::cli_config::fullstack_address_or_localhost();
                let listener = tokio::net::TcpListener::bind(addr)
                    .await
                    .unwrap_or_else(|e| panic!("Failed to bind to {}: {}", addr, e));
                tracing::info!(%addr, "listening");
                axum::serve(listener, router.into_make_service())
                    .await
                    .expect("Server exited unexpectedly");
            });
    }

    // WASM client: just launch the frontend
    #[cfg(target_arch = "wasm32")]
    {
        dioxus::LaunchBuilder::new().launch(App);
    }
}

#[component]
fn App() -> Element {
    rsx! {
        document::Stylesheet { href: asset!("/assets/tailwind-built.css") }
        document::Stylesheet { href: asset!("/assets/main.css") }
        Router::<Route> {}
    }
}
