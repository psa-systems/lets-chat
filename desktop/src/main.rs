mod update;

use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const GIT_HASH: &str = env!("GIT_HASH");
const GIT_VERSION: &str = env!("GIT_VERSION");
const BUILD_DATE: &str = env!("BUILD_DATE");

fn main() -> wry::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!(
            "lets-chat-desktop {VERSION} ({GIT_VERSION}, commit {GIT_HASH}, built {BUILD_DATE})"
        );
        return Ok(());
    }

    if args.iter().any(|a| a == "--check-update") {
        std::process::exit(run_check_update());
    }

    if args.iter().any(|a| a == "--update") {
        std::process::exit(run_update());
    }

    eprintln!("lets-chat-desktop {VERSION} ({GIT_VERSION}, commit {GIT_HASH}, built {BUILD_DATE})");
    update::spawn_startup_check();
    set_linux_gtk_env();
    let url = std::env::var("LETS_CHAT_SERVER_URL")
        .unwrap_or_else(|_| "http://localhost:8080".to_string());

    let title = format!("lets-chat v{VERSION} ({GIT_HASH})");
    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title(&title)
        .with_inner_size(tao::dpi::LogicalSize::new(1100.0, 750.0))
        .build(&event_loop)
        .expect("window");

    let _webview = WebViewBuilder::new().with_url(&url).build(&window)?;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            *control_flow = ControlFlow::Exit;
        }
    });
}

#[cfg(target_os = "linux")]
fn set_linux_gtk_env() {
    for (k, v) in [
        ("GDK_BACKEND", "x11"),
        ("GDK_SCALE", "1"),
        ("GDK_DPI_SCALE", "1"),
    ] {
        if std::env::var(k).unwrap_or_default().is_empty() {
            unsafe { std::env::set_var(k, v) };
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn set_linux_gtk_env() {}

fn run_check_update() -> i32 {
    match update::check() {
        Ok(Some(v)) => {
            println!("update available: {v} (current: {VERSION})");
            0
        }
        Ok(None) => {
            println!("already at latest version: {VERSION}");
            0
        }
        Err(e) => {
            eprintln!("check-update failed: {e}");
            1
        }
    }
}

fn run_update() -> i32 {
    match update::apply() {
        Ok(update::ApplyOutcome::Updated(v)) => {
            println!("updated to {v}; restart lets-chat-desktop to use the new binary");
            0
        }
        Ok(update::ApplyOutcome::AlreadyLatest) => {
            println!("already at latest version: {VERSION}");
            0
        }
        Err(e) => {
            eprintln!("update failed: {e}");
            1
        }
    }
}
