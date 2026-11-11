mod config;
mod inject;
mod net_guard;
mod oci;
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
fn set_server_url(url: String, app: tauri::AppHandle) -> Result<ProbeResult, String> {
    let trimmed = url.trim().to_string();
    if trimmed.is_empty() {
        return Err("URL cannot be empty".into());
    }
    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        return Err("URL must start with http:// or https://".into());
    }
    config::save(&trimmed).map_err(|e| format!("config save failed: {e}"))?;
    // LC-933: re-grant the remote-control (and update-token) capability to the
    // newly-configured origin. Without this, the grant from setup() still
    // names the OLD url and the new origin the window is about to navigate to
    // has no allow-rc-* permission for the rest of the session.
    grant_remote_control_ipc(&app, &trimmed);
    grant_update_token_ipc(&app, &trimmed);
    grant_native_notify_ipc(&app, &trimmed);
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
    set_linux_gtk_env();

    let url = config::initial_server_url();
    let title = format!("Let's Chat v{VERSION} ({GIT_HASH})");

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
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            // Re-launch attempt: focus the existing window instead of opening
            // a second one. Silent if the window is already gone (eg. user
            // is mid-shutdown).
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
            // Deep links (audit Phase 2): on Windows/Linux a `lets-chat://` link
            // launches a second process with the URL in argv; single-instance
            // forwards that argv here, so this is the WARM deep-link path -
            // focus (above) + navigate the already-running window.
            if let Some(link) = first_deep_link(&argv) {
                open_deep_link(app, &link);
            }
        }))
        // Deep links (audit Phase 2): registers the `lets-chat://` scheme (the
        // Windows registry entry + the Linux `.desktop` MimeType are generated
        // from tauri.conf.json's plugins.deep-link.schemes). Navigation is driven
        // from argv (cold: setup(); warm: the single-instance handler above),
        // which is the reliable path on Windows + Linux; macOS delivers URLs via
        // on_open_url instead (wired in setup() below, LC-1157).
        .plugin(tauri_plugin_deep_link::init())
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
            inject::rc_input,
            inject::set_registry_token,
            inject::desktop_notify,
            inject::desktop_set_badge
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
            // LC-1159: where an "open in new tab" link (target=_blank /
            // window.open) is allowed to navigate the app shell in-place. A
            // wrapped webview otherwise just drops the new-window request, so a
            // portal "Community" link to Let's Chat appeared dead. First-party
            // links navigate the existing window; everything else goes to the
            // system browser (see route_new_window). Derived from the configured
            // server origin (plus our product domains), not one hardcoded host.
            let first_party = first_party_suffixes(&config::initial_server_url());
            let new_window_app = app.handle().clone();
            let window = WebviewWindowBuilder::new(app, "main", webview_url)
                .title(&title_for_window)
                .inner_size(1100.0, 750.0)
                // LC-185: bridge the remote-control DOM events into the native
                // injector. Runs at document start, before the server's page
                // scripts, so the listeners are attached before call.js fires.
                .initialization_script(inject::BRIDGE_JS)
                // LC-1159: intercept new-window / target=_blank / window.open
                // requests (wry wires this to the WKWebView UI delegate, the
                // WebView2 NewWindowRequested event and the WebKitGTK `create`
                // signal, so one handler covers macOS / Windows / Linux).
                .on_new_window(move |url, _features| {
                    route_new_window(&new_window_app, &url, &first_party)
                })
                .build()?;
            install_media_permission_handler(&window)?;
            grant_remote_control_ipc(app.handle(), &url_for_cap);
            grant_update_token_ipc(app.handle(), &url_for_cap);
            grant_native_notify_ipc(app.handle(), &url_for_cap);
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
            // Deep links (audit Phase 2): register the scheme at runtime (covers
            // `cargo run` / dev; installed bundles register from tauri.conf), then
            // handle the COLD-start path - a `lets-chat://` URL on this process's
            // own argv navigates the just-built window to the target.
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                if let Err(e) = app.deep_link().register_all() {
                    eprintln!("lets-chat-desktop: deep-link runtime registration skipped: {e}");
                }
            }
            if let Some(link) = first_deep_link(&std::env::args().skip(1).collect::<Vec<_>>()) {
                open_deep_link(app.handle(), &link);
            }
            // Deep links on macOS (LC-1157): macOS does not carry the URL in argv
            // the way Windows/Linux do - it delivers it through on_open_url, for
            // both a cold launch (the plugin buffers the launch URL until a
            // handler is registered here) and a warm re-open. Route both through
            // the same open_deep_link -> navigate_to_path sink the argv path uses,
            // so every platform lands on one navigation path.
            #[cfg(target_os = "macos")]
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                let handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        open_deep_link(&handle, url.as_str());
                    }
                });
            }
            // LC-710: started here, not before the builder, because the check
            // needs an AppHandle to raise its native "update available"
            // notification.
            update::spawn_startup_check(app.handle().clone());
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
//
// LC-933: called both from setup() (the AppHandle behind app.handle()) and
// from set_server_url() (the AppHandle Tauri injects into the command), so
// the grant always tracks whichever origin the window is currently showing,
// not just the one configured at startup. `add_capability` is additive
// (Tauri 2), so a stale capability from a prior URL is simply left in place:
// it names an origin the window is no longer on and therefore grants nothing.
fn grant_remote_control_ipc(app: &tauri::AppHandle, server_url: &str) {
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

// LC-733: let the configured server origin (and only it) hand the native side
// the update-registry token it minted for the signed-in user. Same shape and
// same trust argument as `grant_remote_control_ipc`, kept as its own capability
// so the two grants stay independently reviewable.
fn grant_update_token_ipc(app: &tauri::AppHandle, server_url: &str) {
    use tauri::{ipc::CapabilityBuilder, Manager};
    let Some(pattern) = remote_url_pattern(server_url) else {
        eprintln!("lets-chat-desktop: update-token bridge disabled (unparseable server URL)");
        return;
    };
    let cap = CapabilityBuilder::new("update-token")
        .window("main")
        // Remote-only: the local welcome page has no session to mint against.
        .local(false)
        .remote(pattern)
        .permission("allow-set-registry-token");
    if let Err(e) = app.add_capability(cap) {
        eprintln!("lets-chat-desktop: update-token capability not added: {e}");
    }
}

// Desktop notification + badge bridge (audit Phase 2): let the configured
// server origin (and only it) call `desktop_notify` and `desktop_set_badge`, so
// the web UI can raise OS-native notifications (mentions / DMs / calls) and set
// the dock/taskbar unread badge. Same shape and trust argument as
// `grant_remote_control_ipc` / `grant_update_token_ipc`; kept its own capability
// so the grant stays independently reviewable. Both commands only drive the app
// icon / a notification, so they are strictly less sensitive than the rc grant.
fn grant_native_notify_ipc(app: &tauri::AppHandle, server_url: &str) {
    use tauri::{ipc::CapabilityBuilder, Manager};
    let Some(pattern) = remote_url_pattern(server_url) else {
        eprintln!("lets-chat-desktop: native notifications disabled (unparseable server URL)");
        return;
    };
    let cap = CapabilityBuilder::new("native-notify")
        .window("main")
        // Remote-only: the local welcome page raises no notifications / badge.
        .local(false)
        .remote(pattern)
        .permission("allow-desktop-notify")
        // Dock/taskbar unread badge (audit Phase 2): same bridge, same origin.
        .permission("allow-desktop-set-badge");
    if let Err(e) = app.add_capability(cap) {
        eprintln!("lets-chat-desktop: native-notify capability not added: {e}");
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

// Deep links (audit Phase 2) ------------------------------------------------
//
// The first `lets-chat://...` argument, if any. The OS passes the clicked link
// as a plain argv entry on Windows + Linux (cold start to a fresh process,
// warm start forwarded by single-instance).
fn first_deep_link(args: &[String]) -> Option<String> {
    args.iter().find(|a| a.starts_with("lets-chat://")).cloned()
}

// Map a `lets-chat://` URL onto an existing in-app route. The host + path are
// concatenated so `lets-chat://room/15` -> `/room/15` and `lets-chat://dm/abc`
// -> `/dm/abc`; a `#msg-7` fragment (message anchor) and any query are kept.
// Returns None for anything that is not a `lets-chat://` URL, so a stray argv
// entry is ignored. No new server route is invented - these are the routes the
// web app already serves.
fn deep_link_to_path(raw: &str) -> Option<String> {
    let u = url::Url::parse(raw.trim()).ok()?;
    if u.scheme() != "lets-chat" {
        return None;
    }
    let host = u.host_str().unwrap_or("");
    let mut out = String::from("/");
    out.push_str(host);
    let path = u.path();
    if !path.is_empty() && path != "/" {
        out.push_str(path);
    }
    // Collapse a leading `//` (empty host + absolute path, e.g. `lets-chat:///x`).
    while out.starts_with("//") {
        out.remove(0);
    }
    if let Some(q) = u.query() {
        out.push('?');
        out.push_str(q);
    }
    if let Some(f) = u.fragment() {
        out.push('#');
        out.push_str(f);
    }
    Some(out)
}

// The ONE navigation sink, shared by every "open to this room" trigger (deep
// links today; a click-capable notification once one exists - see the note in
// inject::desktop_notify). Focuses the window and navigates the webview to the
// configured server origin + `path`, reusing the app's existing routes. Using
// the absolute server URL means it also recovers from the offline welcome page.
fn navigate_to_path(app: &tauri::AppHandle, path: &str) {
    use tauri::Manager;
    let Some(w) = app.get_webview_window("main") else {
        return;
    };
    let _ = w.unminimize();
    let _ = w.show();
    let _ = w.set_focus();
    let base = config::initial_server_url();
    let full = format!("{}{}", base.trim_end_matches('/'), path);
    let js = format!(
        "location.assign({})",
        serde_json::to_string(&full).unwrap_or_else(|_| "\"/\"".to_string())
    );
    let _ = w.eval(&js);
}

// Open a `lets-chat://` URL: map it to an in-app route and navigate there.
fn open_deep_link(app: &tauri::AppHandle, raw: &str) {
    match deep_link_to_path(raw) {
        Some(path) => navigate_to_path(app, &path),
        None => eprintln!("lets-chat-desktop: ignoring non-deep-link argument {raw:?}"),
    }
}

// LC-1159: decide what happens when the page asks to open a URL in a NEW window
// (a `target=_blank` link or `window.open`). A wrapped single-window webview has
// no second window to put it in and silently drops the request, which is why the
// portal's "Community" link to Let's Chat looked dead. We never let the webview
// spawn its own window (always `Deny`); instead:
//   * a first-party URL (the configured server's own domain, our product
//     domains, sibling apps) navigates the EXISTING window in-app, and
//   * anything else opens in the user's default browser.
// This keeps untrusted web content out of the app shell while making the
// in-product "jump to Let's Chat" link work. wry delivers the request on the UI
// thread on every platform, so navigating the main window here is safe.
fn route_new_window(
    app: &tauri::AppHandle,
    url: &url::Url,
    first_party: &[String],
) -> tauri::webview::NewWindowResponse<tauri::Wry> {
    use tauri::Manager;
    // Only web links are routed; ignore anything exotic (data:, blob:, custom
    // schemes) rather than hand it to the OS.
    if !matches!(url.scheme(), "http" | "https") {
        return tauri::webview::NewWindowResponse::Deny;
    }
    if is_first_party_host(url.host_str().unwrap_or_default(), first_party) {
        if let Some(w) = app.get_webview_window("main") {
            if let Err(e) = w.navigate(url.clone()) {
                eprintln!("lets-chat-desktop: in-app navigate to {url} failed: {e}");
            }
        }
    } else if let Err(e) = open::that(url.as_str()) {
        eprintln!("lets-chat-desktop: could not open {url} in the browser: {e}");
    }
    tauri::webview::NewWindowResponse::Deny
}

// First-party host suffixes a new-window link may navigate to IN-APP. Built from
// the configured server origin's registrable domain (so a self-hosted / custom
// deployment and its sibling subdomains are covered without hardcoding one host)
// plus the known product domains. Any host not matching one of these opens in
// the system browser instead.
fn first_party_suffixes(server_url: &str) -> Vec<String> {
    let mut out = vec!["a8n.systems".to_string(), "psa.systems".to_string()];
    if let Some(domain) = url::Url::parse(server_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .and_then(|h| registrable_domain(&h))
    {
        if !out.iter().any(|s| s.eq_ignore_ascii_case(&domain)) {
            out.push(domain);
        }
    }
    out
}

// Approximate registrable domain as the last two dot-labels (e.g.
// `chat.a8n.systems` -> `a8n.systems`). Correct for the single-label TLDs in use
// (.systems, .com, .net, ...); for a multi-label public suffix like `.co.uk` it
// over-widens to the second level, which at worst routes one more first-party-
// looking link in-app for a self-hoster on such a TLD. Never used as a security
// boundary for the native bridge (that is origin-scoped separately).
fn registrable_domain(host: &str) -> Option<String> {
    let labels: Vec<&str> = host.split('.').filter(|l| !l.is_empty()).collect();
    if labels.len() < 2 {
        return None;
    }
    Some(labels[labels.len() - 2..].join("."))
}

// True when `host` equals, or is a dot-boundary subdomain of, one of the
// first-party suffixes. The leading-dot check stops a look-alike host like
// `nota8n.systems` or `a8n.systems.evil.com` from matching `a8n.systems`.
fn is_first_party_host(host: &str, suffixes: &[String]) -> bool {
    if host.is_empty() {
        return false;
    }
    let host = host.to_ascii_lowercase();
    suffixes.iter().any(|s| {
        let s = s.to_ascii_lowercase();
        host == s || host.ends_with(&format!(".{s}"))
    })
}

#[cfg(test)]
mod new_window_tests {
    use super::{first_party_suffixes, is_first_party_host, registrable_domain};

    #[test]
    fn registrable_domain_takes_last_two_labels() {
        assert_eq!(
            registrable_domain("chat.a8n.systems").as_deref(),
            Some("a8n.systems")
        );
        assert_eq!(
            registrable_domain("a8n.systems").as_deref(),
            Some("a8n.systems")
        );
        assert_eq!(registrable_domain("localhost"), None);
    }

    #[test]
    fn first_party_matches_product_and_configured_domains() {
        // Configured server on a self-hosted domain: its registrable domain and
        // the product domains are all first-party.
        let suffixes = first_party_suffixes("https://chat.example.org:8443/");
        assert!(is_first_party_host("chat.a8n.systems", &suffixes)); // product
        assert!(is_first_party_host("dashboard.a8n.systems", &suffixes)); // sibling
        assert!(is_first_party_host("chat.psa.systems", &suffixes)); // product
        assert!(is_first_party_host("example.org", &suffixes)); // configured
        assert!(is_first_party_host("chat.example.org", &suffixes)); // configured sub
    }

    #[test]
    fn look_alikes_and_third_parties_are_not_first_party() {
        let suffixes = first_party_suffixes("https://chat.a8n.systems/");
        assert!(!is_first_party_host("nota8n.systems", &suffixes));
        assert!(!is_first_party_host("a8n.systems.evil.com", &suffixes));
        assert!(!is_first_party_host("github.com", &suffixes));
        assert!(!is_first_party_host("", &suffixes));
    }
}

#[cfg(test)]
mod deep_link_tests {
    use super::deep_link_to_path;

    #[test]
    fn maps_host_and_path_onto_app_routes() {
        assert_eq!(
            deep_link_to_path("lets-chat://room/15").as_deref(),
            Some("/room/15")
        );
        assert_eq!(
            deep_link_to_path("lets-chat://dm/abc").as_deref(),
            Some("/dm/abc")
        );
        // message anchor + query are preserved
        assert_eq!(
            deep_link_to_path("lets-chat://room/15#msg-7").as_deref(),
            Some("/room/15#msg-7")
        );
        assert_eq!(
            deep_link_to_path("lets-chat://room/15?focus=1").as_deref(),
            Some("/room/15?focus=1")
        );
        // empty-host absolute form collapses to a single leading slash
        assert_eq!(
            deep_link_to_path("lets-chat:///room/15").as_deref(),
            Some("/room/15")
        );
        // not our scheme -> ignored
        assert_eq!(deep_link_to_path("https://evil.example/room/15"), None);
        assert_eq!(deep_link_to_path("not a url"), None);
    }
}

// Build a system-tray icon with Show / Quit menu items. Left-click focuses
// the main window; right-click opens the menu. The window is `hide()`d
// rather than destroyed when the user closes it (handled via the tray's
// Show entry), so the app keeps running in the background.
fn build_tray_icon(app: &tauri::AppHandle) -> tauri::Result<()> {
    let icon = Image::from_bytes(include_bytes!("../icons/icon-square-128.png"))?;
    let show_item = MenuItem::with_id(app, "show", "Show window", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit Let's Chat", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

    TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("Let's Chat")
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
// macOS (LC-134) needs no Rust-side grant handler: wry's WryWebViewUIDelegate
// already implements
// `webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:`
// and unconditionally grants it (WKPermissionDecision::Grant), so the
// WebKit-layer consent is auto-granted. The macOS-specific work is the
// OS-level (TCC) prompt, gated on the NSCameraUsageDescription /
// NSMicrophoneUsageDescription strings in desktop/Info.plist plus the
// camera/microphone entitlements in desktop/lets-chat.entitlements; see the
// explicit macOS arm below.
//
// iOS (LC-136) likewise needs no code-side hook: WKWebView routes getUserMedia
// consent through the OS prompt, which iOS shows automatically once the same
// usage-description strings are declared in desktop/Info.ios.plist (merged into
// the generated iOS Info.plist by the Tauri CLI); the no-op fallback below
// covers the iOS target. Android (LC-135) still needs its permission flow wired
// through Tauri's per-platform hooks once that build path lands (LC-133 tracker).
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

#[cfg(test)]
mod tests {
    use super::*;

    // LC-933: the remote-control capability is granted once at startup for
    // whichever URL is configured then, and again from set_server_url for
    // whichever URL the user just switched to. Both call sites feed the same
    // URL into `remote_url_pattern`, so the granted pattern always equals the
    // pattern for the currently loaded URL: this pins that invariant across a
    // URL change, not just for a single fixed URL.
    #[test]
    fn remote_url_pattern_tracks_a_url_change() {
        let pre_change = "https://old.example.com/some/path?x=1";
        let post_change = "https://new.example.com:8443/";

        let pre_pattern = remote_url_pattern(pre_change);
        let post_pattern = remote_url_pattern(post_change);

        assert_eq!(pre_pattern, Some("https://old.example.com/*".to_string()));
        assert_eq!(
            post_pattern,
            Some("https://new.example.com:8443/*".to_string())
        );
        assert_ne!(pre_pattern, post_pattern);

        // The pattern granted for a given URL is exactly the pattern computed
        // for that same URL when it is the currently loaded one.
        assert_eq!(remote_url_pattern(post_change), post_pattern);
    }
}
