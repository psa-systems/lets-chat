use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

fn main() -> wry::Result<()> {
    set_linux_gtk_env();
    let url = std::env::var("LETS_CHAT_SERVER_URL")
        .unwrap_or_else(|_| "http://localhost:8080".to_string());

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("lets-chat")
        .with_inner_size(tao::dpi::LogicalSize::new(1100.0, 750.0))
        .build(&event_loop)
        .expect("window");

    let _webview = WebViewBuilder::new()
        .with_url(&url)
        .build(&window)?;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested, ..
        } = event
        {
            *control_flow = ControlFlow::Exit;
        }
    });
}

#[cfg(target_os = "linux")]
fn set_linux_gtk_env() {
    for (k, v) in [("GDK_BACKEND", "x11"), ("GDK_SCALE", "1"), ("GDK_DPI_SCALE", "1")] {
        if std::env::var(k).unwrap_or_default().is_empty() {
            unsafe { std::env::set_var(k, v) };
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn set_linux_gtk_env() {}
