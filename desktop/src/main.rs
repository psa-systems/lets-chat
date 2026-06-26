mod config;
mod inject;
mod net_guard;
mod update;
mod update_verify;
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
    let url_for_cap = url.clone();
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
        // LC-186: global kill-switch for remote control. The handler fires on
        // ANY registered shortcut press; we register only the kill hotkey
        // below, so no shortcut match is needed.
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        kill_remote_control(app);
                    }
                })
                .build(),
        )
        .manage(inject::ControlState::default())
        .invoke_handler(tauri::generate_handler![
            set_server_url,
            inject::rc_session,
            inject::rc_input
        ])
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
            let window = WebviewWindowBuilder::new(app, "main", webview_url)
                .title(&title_for_window)
                .inner_size(1100.0, 750.0)
                // LC-185: bridge the remote-control DOM events into the native
                // injector. Runs at document start, before the server's page
                // scripts, so the listeners are attached before call.js fires.
                .initialization_script(inject::BRIDGE_JS)
                .build()?;
            install_media_permission_handler(&window)?;
            grant_remote_control_ipc(app, &url_for_cap);
            // LC-186: register the kill-switch hotkey. Non-fatal if the combo
            // is already taken by another app - the on-banner button remains.
            {
                use tauri_plugin_global_shortcut::GlobalShortcutExt;
                if let Err(e) = app.global_shortcut().register(REMOTE_CONTROL_KILL_HOTKEY) {
                    eprintln!(
                        "lets-chat-desktop: could not register kill-switch hotkey \
                         {REMOTE_CONTROL_KILL_HOTKEY}: {e}"
                    );
                }
            }
            build_tray_icon(app.handle())?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error running tauri application");
}

// LC-186: kill-switch hotkey. Chosen to be unlikely to collide with common
// app/OS shortcuts; surfaced in the active-control banner so the sharer knows
// it. (Ctrl+Shift+Esc is OS-reserved on Windows, so it is deliberately avoided.)
const REMOTE_CONTROL_KILL_HOTKEY: &str = "CommandOrControl+Alt+F9";

// LC-186: sever any live remote-control session immediately. Flips the native
// injector off + releases held input (does not depend on the webview), then
// nudges the page to send the peer a `revoke` and drop its banner. Driven by
// the global kill-switch hotkey.
fn kill_remote_control(app: &tauri::AppHandle) {
    use tauri::Manager;
    if let Some(state) = app.try_state::<inject::ControlState>() {
        state.kill();
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.eval("document.dispatchEvent(new CustomEvent('lc:control-kill'))");
    }
}

// LC-185: grant the configured server origin (and only it) access to the
// remote-control IPC commands. App commands are unreachable from a remote
// (non-`tauri://`) origin unless a capability with a matching `remote.urls`
// entry allows them; we add that capability at runtime, scoped to exactly the
// configured server URL, because the URL is user-configurable and cannot be
// baked into a static capability file. The injector still gates every event
// on an active grant (inject.rs), so enabling the IPC path is no weaker than
// the trust the app already places in `LETS_CHAT_SERVER_URL`.
fn grant_remote_control_ipc(app: &tauri::App, server_url: &str) {
    use tauri::{ipc::CapabilityBuilder, Manager};
    let Some(pattern) = remote_url_pattern(server_url) else {
        eprintln!("lets-chat-desktop: remote-control IPC disabled (unparseable server URL)");
        return;
    };
    let cap = CapabilityBuilder::new("remote-control")
        .window("main")
        // Remote-only: the local welcome page never calls these commands.
        .local(false)
        .remote(pattern)
        .permission("allow-rc-session")
        .permission("allow-rc-input");
    if let Err(e) = app.add_capability(cap) {
        eprintln!("lets-chat-desktop: remote-control capability not added: {e}");
    }
}

// Build an origin URL-pattern (`scheme://host[:port]/*`) from the configured
// server URL for the remote capability. Strips path/query/fragment and any
// userinfo so the pattern matches every page under the origin. Returns None
// for anything that is not an http(s) URL.
fn remote_url_pattern(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if host.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{host}/*"))
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

// Wire a media-stream auto-grant handler into the underlying webview so
// WebRTC camera / microphone (and screen-share on Windows) work without
// silent denials. The webview never navigates off the configured server
// URL (config.rs + set_server_url enforce this), and a compromised
// server can already exfiltrate everything via the IPC bridge anyway,
// so granting unconditionally inside this binary is no weaker than the
// trust model around `LETS_CHAT_SERVER_URL` itself.
//
// macOS (LC-134) needs no Rust-side handler: wry's WryWebViewUIDelegate
// already implements
// `webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:`
// and unconditionally grants it (WKPermissionDecision::Grant), so the
// WebKit-layer consent is auto-granted. The macOS-specific work is the
// OS-level (TCC) prompt, gated on the NSCameraUsageDescription /
// NSMicrophoneUsageDescription strings in desktop/Info.plist plus the
// camera/microphone entitlements in desktop/lets-chat.entitlements; see the
// explicit macOS arm below.
//
// iOS / Android variants live under LC-135 / LC-136 in the Apple+mobile
// tracker (LC-133); those targets need their permission flows wired through
// Tauri's per-platform hooks once the build paths for those targets land.
#[cfg(target_os = "linux")]
fn install_media_permission_handler(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    use webkit2gtk::glib::Cast;
    use webkit2gtk::{PermissionRequestExt, UserMediaPermissionRequest, WebViewExt};
    window.with_webview(|webview| {
        let wv = webview.inner();
        wv.connect_permission_request(|_view, request| {
            if request
                .downcast_ref::<UserMediaPermissionRequest>()
                .is_some()
            {
                request.allow();
                true
            } else {
                false
            }
        });
    })?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn install_media_permission_handler(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2PermissionRequestedEventArgs, COREWEBVIEW2_PERMISSION_KIND,
        COREWEBVIEW2_PERMISSION_KIND_CAMERA, COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
        COREWEBVIEW2_PERMISSION_STATE_ALLOW,
    };
    use webview2_com::PermissionRequestedEventHandler;
    window.with_webview(|webview| {
        let controller = webview.controller();
        let core = unsafe { controller.CoreWebView2() }.expect("CoreWebView2 unavailable");
        let handler = PermissionRequestedEventHandler::create(Box::new(
            |_sender, args: Option<ICoreWebView2PermissionRequestedEventArgs>| {
                if let Some(args) = args {
                    unsafe {
                        let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
                        args.PermissionKind(&mut kind)?;
                        if kind == COREWEBVIEW2_PERMISSION_KIND_CAMERA
                            || kind == COREWEBVIEW2_PERMISSION_KIND_MICROPHONE
                        {
                            args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW)?;
                        }
                    }
                }
                Ok(())
            },
        ));
        let mut token = Default::default();
        unsafe {
            core.add_PermissionRequested(&handler, &mut token)
                .expect("add_PermissionRequested failed");
        }
    })?;
    Ok(())
}

// LC-134: macOS WKWebView. The WebKit-layer getUserMedia consent is already
// auto-granted by wry's WryWebViewUIDelegate (WKPermissionDecision::Grant), so
// there is no delegate for us to install here without clobbering wry's own
// UIDelegate (which also drives file-upload panels and new-window handling).
// The OS-level prompt is provisioned declaratively via desktop/Info.plist
// (NSCameraUsageDescription / NSMicrophoneUsageDescription) and
// desktop/lets-chat.entitlements (camera / audio-input), so this arm is
// intentionally a no-op.
#[cfg(target_os = "macos")]
fn install_media_permission_handler(_window: &tauri::WebviewWindow) -> tauri::Result<()> {
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn install_media_permission_handler(_window: &tauri::WebviewWindow) -> tauri::Result<()> {
    Ok(())
}

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
