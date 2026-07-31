// LC-185: controlled-side OS input injection for remote control.
//
// The webview cannot inject OS input (browser sandbox), so the input stream
// that arrives on the WebRTC data channel (LC-184) is bridged to native Rust
// over Tauri IPC and synthesized here with OS APIs. Windows is the first
// target (`SendInput`); other platforms are `#[cfg(...)]` no-ops until their
// slice lands.
//
// Trust boundary: the bridge is reachable only from the configured server
// origin (a runtime remote capability scoped to that one URL in main.rs), and
// every event is gated on `ControlState.active`, which is flipped on only by
// the LOCAL user's explicit "Grant control" click (lc:control-start). A peer
// who floods the data channel without a live grant injects nothing because
// `active` is false. This does NOT defend against a compromised server page -
// that is the same trust the app already places in `LETS_CHAT_SERVER_URL`.

use std::collections::HashSet;
use std::sync::Mutex;

use serde::Deserialize;
use tauri::State;

/// One decoded input event from a controller data-channel frame. Field names
/// mirror the compact wire format produced by `call.js` (LC-184):
/// `m` move, `d` button-down, `u` button-up, `w` wheel, `k`/`K` key down/up.
#[derive(Deserialize)]
struct InputFrame {
    t: String,
    #[serde(default)]
    x: f64,
    #[serde(default)]
    y: f64,
    #[serde(default)]
    b: i64,
    #[serde(default)]
    dx: f64,
    #[serde(default)]
    dy: f64,
    #[serde(default)]
    c: String,
}

/// Managed Tauri state: whether a control session is live, plus the keys and
/// mouse buttons currently held down so they can be force-released on any end
/// path (stuck-key prevention - design "Held-input cleanup").
#[derive(Default)]
pub struct ControlState(Mutex<Inner>);

#[derive(Default)]
struct Inner {
    active: bool,
    /// Held scan codes, with their extended-key flag.
    held_keys: HashSet<(u16, bool)>,
    /// Held mouse buttons, keyed by the controller's web button id (0/1/2).
    held_buttons: HashSet<i64>,
}

impl ControlState {
    /// Hard kill (LC-186 kill-switch): disarm + release everything held,
    /// independent of the webview. Called from the global-shortcut handler so
    /// a session is severed at the OS level even if the controlling peer holds
    /// the input focus or the page is unresponsive. Idempotent.
    pub fn kill(&self) {
        let mut inner = self.0.lock().unwrap();
        release_all(&mut inner);
        inner.active = false;
    }
}

/// Arm / disarm the injector. Called from the bridge on `lc:control-start`
/// (active=true, after the local user granted) and `lc:control-end`
/// (active=false: a clean revoke, a call teardown, or an abrupt channel drop).
/// Disarming releases everything still held so no key/button is left stuck
/// down at the OS level.
#[tauri::command]
pub fn rc_session(active: bool, state: State<'_, ControlState>) {
    let mut inner = state.0.lock().unwrap();
    if active {
        // Start clean: drop any held state left over from a prior session that
        // never received a disarm (e.g. a missed lc:control-end). Stale entries
        // would otherwise only be flushed on the next disarm.
        release_all(&mut inner);
        inner.active = true;
    } else {
        release_all(&mut inner);
        inner.active = false;
    }
}

/// Inject a single input frame. Drops the event unless a session is active -
/// the native re-assertion of the grant; the data channel alone is never
/// trusted.
#[tauri::command]
pub fn rc_input(frame: String, state: State<'_, ControlState>) {
    let f: InputFrame = match serde_json::from_str(&frame) {
        Ok(f) => f,
        Err(_) => return,
    };
    let mut inner = state.0.lock().unwrap();
    if !inner.active {
        return;
    }
    match f.t.as_str() {
        "m" => sys::move_mouse(f.x, f.y),
        "d" => {
            sys::mouse_button(f.x, f.y, f.b, true);
            inner.held_buttons.insert(f.b);
        }
        "u" => {
            sys::mouse_button(f.x, f.y, f.b, false);
            inner.held_buttons.remove(&f.b);
        }
        "w" => sys::wheel(f.x, f.y, f.dx, f.dy),
        "k" => {
            if let Some((scan, ext)) = scan_code(&f.c) {
                sys::key(scan, ext, true);
                inner.held_keys.insert((scan, ext));
            }
        }
        "K" => {
            if let Some((scan, ext)) = scan_code(&f.c) {
                sys::key(scan, ext, false);
                inner.held_keys.remove(&(scan, ext));
            }
        }
        _ => {}
    }
}

/// Synthesize a key-up / button-up for everything currently held, then clear
/// the held sets. Runs at the start of every disarm.
fn release_all(inner: &mut Inner) {
    for &(scan, ext) in inner.held_keys.iter() {
        sys::key(scan, ext, false);
    }
    inner.held_keys.clear();
    for &b in inner.held_buttons.iter() {
        // Button-up at the current cursor position (no move).
        sys::mouse_button_current(b, false);
    }
    inner.held_buttons.clear();
}

/// Map a `KeyboardEvent.code` (physical key, layout-independent) to a PS/2
/// set-1 scan code and whether it is an extended (0xE0-prefixed) key. Covers
/// the practical typing + navigation set; unknown codes are dropped (returns
/// `None`) rather than guessed. Modifier *state* is not consulted - the
/// controller sends discrete down/up for the modifier keys themselves, so the
/// OS tracks Shift/Ctrl/Alt from those injected events.
fn scan_code(code: &str) -> Option<(u16, bool)> {
    // (scan, extended)
    let v: (u16, bool) = match code {
        // Letters
        "KeyA" => (0x1E, false),
        "KeyB" => (0x30, false),
        "KeyC" => (0x2E, false),
        "KeyD" => (0x20, false),
        "KeyE" => (0x12, false),
        "KeyF" => (0x21, false),
        "KeyG" => (0x22, false),
        "KeyH" => (0x23, false),
        "KeyI" => (0x17, false),
        "KeyJ" => (0x24, false),
        "KeyK" => (0x25, false),
        "KeyL" => (0x26, false),
        "KeyM" => (0x32, false),
        "KeyN" => (0x31, false),
        "KeyO" => (0x18, false),
        "KeyP" => (0x19, false),
        "KeyQ" => (0x10, false),
        "KeyR" => (0x13, false),
        "KeyS" => (0x1F, false),
        "KeyT" => (0x14, false),
        "KeyU" => (0x16, false),
        "KeyV" => (0x2F, false),
        "KeyW" => (0x11, false),
        "KeyX" => (0x2D, false),
        "KeyY" => (0x15, false),
        "KeyZ" => (0x2C, false),
        // Number row
        "Digit1" => (0x02, false),
        "Digit2" => (0x03, false),
        "Digit3" => (0x04, false),
        "Digit4" => (0x05, false),
        "Digit5" => (0x06, false),
        "Digit6" => (0x07, false),
        "Digit7" => (0x08, false),
        "Digit8" => (0x09, false),
        "Digit9" => (0x0A, false),
        "Digit0" => (0x0B, false),
        // Whitespace / editing
        "Enter" => (0x1C, false),
        "Escape" => (0x01, false),
        "Backspace" => (0x0E, false),
        "Tab" => (0x0F, false),
        "Space" => (0x39, false),
        "CapsLock" => (0x3A, false),
        // Punctuation
        "Minus" => (0x0C, false),
        "Equal" => (0x0D, false),
        "BracketLeft" => (0x1A, false),
        "BracketRight" => (0x1B, false),
        "Backslash" => (0x2B, false),
        "Semicolon" => (0x27, false),
        "Quote" => (0x28, false),
        "Backquote" => (0x29, false),
        "Comma" => (0x33, false),
        "Period" => (0x34, false),
        "Slash" => (0x35, false),
        // Function keys
        "F1" => (0x3B, false),
        "F2" => (0x3C, false),
        "F3" => (0x3D, false),
        "F4" => (0x3E, false),
        "F5" => (0x3F, false),
        "F6" => (0x40, false),
        "F7" => (0x41, false),
        "F8" => (0x42, false),
        "F9" => (0x43, false),
        "F10" => (0x44, false),
        "F11" => (0x57, false),
        "F12" => (0x58, false),
        // Modifiers
        "ShiftLeft" => (0x2A, false),
        "ShiftRight" => (0x36, false),
        "ControlLeft" => (0x1D, false),
        "ControlRight" => (0x1D, true),
        "AltLeft" => (0x38, false),
        "AltRight" => (0x38, true),
        "MetaLeft" => (0x5B, true),
        "MetaRight" => (0x5C, true),
        "ContextMenu" => (0x5D, true),
        // Navigation (extended)
        "ArrowUp" => (0x48, true),
        "ArrowDown" => (0x50, true),
        "ArrowLeft" => (0x4B, true),
        "ArrowRight" => (0x4D, true),
        "Home" => (0x47, true),
        "End" => (0x4F, true),
        "PageUp" => (0x49, true),
        "PageDown" => (0x51, true),
        "Insert" => (0x52, true),
        "Delete" => (0x53, true),
        // Numpad
        "Numpad0" => (0x52, false),
        "Numpad1" => (0x4F, false),
        "Numpad2" => (0x50, false),
        "Numpad3" => (0x51, false),
        "Numpad4" => (0x4B, false),
        "Numpad5" => (0x4C, false),
        "Numpad6" => (0x4D, false),
        "Numpad7" => (0x47, false),
        "Numpad8" => (0x48, false),
        "Numpad9" => (0x49, false),
        "NumpadDecimal" => (0x53, false),
        "NumpadMultiply" => (0x37, false),
        "NumpadSubtract" => (0x4A, false),
        "NumpadAdd" => (0x4E, false),
        "NumpadDivide" => (0x35, true),
        "NumpadEnter" => (0x1C, true),
        _ => return None,
    };
    Some(v)
}

/// JS injected ahead of the page (runs at document start in every frame). It
/// forwards the LC-184 control DOM events to the native injector. Uses the
/// internal IPC entrypoint so we do not have to expose the whole global Tauri
/// API surface to the remote page (`withGlobalTauri` stays off).
pub const BRIDGE_JS: &str = r#"
(function () {
  // LC-640: advertise to the page that this client HAS a native injector, so
  // when it is the one being controlled it can tell the controller their input
  // will actually land. A plain browser leaves this undefined (falsy).
  try { window.__lcNativeControl = true; } catch (e) {}
  function inv(cmd, args) {
    try {
      var t = window.__TAURI_INTERNALS__;
      if (t && typeof t.invoke === 'function') { t.invoke(cmd, args); }
    } catch (e) {}
  }
  document.addEventListener('lc:control-start', function () { inv('rc_session', { active: true }); });
  document.addEventListener('lc:control-end', function () { inv('rc_session', { active: false }); });
  document.addEventListener('lc:control-input', function (e) {
    var d = e && e.detail;
    inv('rc_input', { frame: typeof d === 'string' ? d : JSON.stringify(d) });
  });
})();
"#;

// ---- platform synthesis -------------------------------------------------

#[cfg(windows)]
mod sys {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
        KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
        MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
        MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK,
        MOUSEEVENTF_WHEEL, MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
    };

    const WHEEL_DELTA: f64 = 120.0;

    fn send(input: INPUT) {
        unsafe {
            SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
        }
    }

    // Normalized [0,1] -> 0..=65535 over the whole virtual desktop. With
    // MOUSEEVENTF_VIRTUALDESK the absolute range spans all monitors, so this
    // is DPI- and resolution-independent. Single-shared-surface assumption:
    // the controller's [0,1] is taken over the virtual desktop, which matches
    // a primary/full-screen share; per-monitor surface selection is out of
    // scope (see the design doc).
    fn abs(v: f64) -> i32 {
        (v.clamp(0.0, 1.0) * 65535.0).round() as i32
    }

    fn mouse(dx: i32, dy: i32, data: i32, flags: MOUSE_EVENT_FLAGS) {
        send(INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    // mouseData is a DWORD; the wheel delta is a signed value
                    // reinterpreted as u32 (negative = scroll toward the user).
                    mouseData: data as u32,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
    }

    pub fn move_mouse(x: f64, y: f64) {
        mouse(
            abs(x),
            abs(y),
            0,
            MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
        );
    }

    fn button_flags(b: i64, down: bool) -> MOUSE_EVENT_FLAGS {
        match (b, down) {
            (0, true) => MOUSEEVENTF_LEFTDOWN,
            (0, false) => MOUSEEVENTF_LEFTUP,
            (1, true) => MOUSEEVENTF_MIDDLEDOWN,
            (1, false) => MOUSEEVENTF_MIDDLEUP,
            (2, true) => MOUSEEVENTF_RIGHTDOWN,
            (2, false) => MOUSEEVENTF_RIGHTUP,
            // Unknown button -> no flag; the combined move still positions.
            _ => MOUSE_EVENT_FLAGS(0),
        }
    }

    // Position + press/release in one event so the click lands where the
    // controller pointed.
    pub fn mouse_button(x: f64, y: f64, b: i64, down: bool) {
        mouse(
            abs(x),
            abs(y),
            0,
            MOUSEEVENTF_MOVE
                | MOUSEEVENTF_ABSOLUTE
                | MOUSEEVENTF_VIRTUALDESK
                | button_flags(b, down),
        );
    }

    // Release at the current cursor position (used by stuck-key cleanup, which
    // has no coordinate).
    pub fn mouse_button_current(b: i64, down: bool) {
        mouse(0, 0, 0, button_flags(b, down));
    }

    pub fn wheel(x: f64, y: f64, dx: f64, dy: f64) {
        // Web wheel: deltaY > 0 scrolls down; Windows wheel positive scrolls
        // up, so invert. Treat ~100px of web delta as one notch.
        if dy != 0.0 {
            let data = (-dy / 100.0 * WHEEL_DELTA).round() as i32;
            mouse(
                abs(x),
                abs(y),
                data,
                MOUSEEVENTF_MOVE
                    | MOUSEEVENTF_ABSOLUTE
                    | MOUSEEVENTF_VIRTUALDESK
                    | MOUSEEVENTF_WHEEL,
            );
        }
        if dx != 0.0 {
            let data = (dx / 100.0 * WHEEL_DELTA).round() as i32;
            mouse(
                abs(x),
                abs(y),
                data,
                MOUSEEVENTF_MOVE
                    | MOUSEEVENTF_ABSOLUTE
                    | MOUSEEVENTF_VIRTUALDESK
                    | MOUSEEVENTF_HWHEEL,
            );
        }
    }

    pub fn key(scan: u16, extended: bool, down: bool) {
        let mut flags = KEYEVENTF_SCANCODE;
        if extended {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        if !down {
            flags |= KEYEVENTF_KEYUP;
        }
        send(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: scan,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
    }
}

// Non-Windows: input synthesis is not implemented yet (Linux uinput/XTEST and
// macOS CGEvent are later subtasks). The bridge + gating still work; injection
// is a silent no-op so a non-Windows controlled peer simply does nothing.
#[cfg(not(windows))]
mod sys {
    pub fn move_mouse(_x: f64, _y: f64) {}
    pub fn mouse_button(_x: f64, _y: f64, _b: i64, _down: bool) {}
    pub fn mouse_button_current(_b: i64, _down: bool) {}
    pub fn wheel(_x: f64, _y: f64, _dx: f64, _dy: f64) {}
    pub fn key(_scan: u16, _extended: bool, _down: bool) {}
}
