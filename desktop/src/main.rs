mod config;
mod update;
mod welcome;

use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager, WebviewUrl, WebviewWindowBuilder,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const GIT_HASH: &str = env!("GIT_HASH");
const GIT_VERSION: &str = env!("GIT_VERSION");
const BUILD_DATE: &str = env!("BUILD_DATE");

// Custom URI scheme used to serve the offline "couldn't reach the server"
// welcome page into the webview. Tauri 2 has no direct "load HTML string"
// builder method (Wry did), so the offline fallback goes through a
// registered protocol handler that returns the rendered HTML for any
// request under welcome://localhost/.
const WELCOME_SCHEME: &str = "welcome";
const WELCOME_URL: &str = "welcome://localhost/";

// JSON payload returned by the set_server_url IPC command. The webview JS
// uses `reachable` to decide whether to navigate to the new URL or to keep
// showing the welcome page with the new failure reason.
#[derive(serde::Serialize)]
struct ProbeResult {
    url: String,
    reachable: bool,
    reason: Option<String>,
}

#[tauri::command]
fn set_server_url(url: String) -> Result<ProbeResult, String> {
    let trimmed = url.trim().to_string();
    if trimmed.is_empty() {
        return Err("URL cannot be empty".into());
    }
    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        return Err("URL must start with http:// or https://".into());
    }
    config::save(&trimmed).map_err(|e| format!("config save failed: {e}"))?;
    Ok(match welcome::server_reachable(&trimmed) {
        Ok(()) => ProbeResult {
            url: trimmed,
            reachable: true,
            reason: None,
        },
        Err(reason) => ProbeResult {
            url: trimmed,
            reachable: false,
            reason: Some(reason),
        },
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!(
            "lets-chat-desktop {VERSION} ({GIT_VERSION}, commit {GIT_HASH}, built {BUILD_DATE})"
        );
        return;
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

    let url = config::initial_server_url();
    let title = format!("lets-chat v{VERSION} ({GIT_HASH})");

    // Probe the configured server before pointing the webview at it.
    // Without this, a misconfigured or down server gives the user a blank
    // window with no hint at what went wrong; the welcome page tells them
    // which URL failed and how to recover (including an inline URL editor
    // backed by the set_server_url command below).
    let probe = welcome::server_reachable(&url);
    let welcome_html = match &probe {
        Ok(()) => String::new(),
        Err(reason) => {
            eprintln!("lets-chat-desktop: server probe failed for {url}: {reason}");
            welcome::render(&url, reason, VERSION, GIT_HASH)
        }
    };
    let probe_ok = probe.is_ok();

    let url_for_window = url.clone();
    let title_for_window = title.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // Re-launch attempt: focus the existing window instead of opening
            // a second one. Silent if the window is already gone (eg. user
            // is mid-shutdown).
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![set_server_url])
        .register_uri_scheme_protocol(WELCOME_SCHEME, move |_ctx, _request| {
            let body = welcome_html.clone().into_bytes();
            tauri::http::Response::builder()
                .header("Content-Type", "text/html; charset=utf-8")
                .body(body)
                .expect("welcome response")
        })
        .setup(move |app| {
            let webview_url = if probe_ok {
                WebviewUrl::External(url_for_window.parse()?)
            } else {
                WebviewUrl::CustomProtocol(WELCOME_URL.parse()?)
            };
            WebviewWindowBuilder::new(app, "main", webview_url)
                .title(&title_for_window)
                .inner_size(1100.0, 750.0)
                .build()?;
            build_tray_icon(app.handle())?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error running tauri application");
}

// Build a system-tray icon with Show / Quit menu items. Left-click focuses
// the main window; right-click opens the menu. The window is `hide()`d
// rather than destroyed when the user closes it (handled via the tray's
// Show entry), so the app keeps running in the background.
fn build_tray_icon(app: &tauri::AppHandle) -> tauri::Result<()> {
    let icon = Image::from_bytes(include_bytes!("../icons/icon-square-128.png"))?;
    let show_item = MenuItem::with_id(app, "show", "Show window", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit lets-chat", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

    TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("lets-chat")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => focus_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // Left-click anywhere on the tray icon focuses the main
            // window, matching how Slack / Discord / Element behave on
            // Linux. The default would be "do nothing" - which is
            // confusing when the menu is right-click-only.
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                focus_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn focus_main_window(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
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
