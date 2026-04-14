# Chat Auto-Scroll Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add sticky-bottom auto-scroll, first-unseen-on-open scroll, and a "new messages" pill to room and DM chat views.

**Architecture:** A single Dioxus hook (`use_auto_scroll`) owns all DOM scroll interaction, `localStorage` last-seen tracking, and the pill-visibility signal. Both `room_view.rs` and `dm_view.rs` integrate the hook identically. All browser APIs are gated behind `#[cfg(target_arch = "wasm32")]`; server/desktop builds get no-op stubs.

**Tech Stack:** Rust 1.x, Dioxus 0.7.x, `web-sys`, `wasm-bindgen`, `js-sys`, browser `localStorage` and DOM scroll APIs.

**Spec:** `docs/superpowers/specs/2026-04-14-chat-auto-scroll-design.md`

**Verification note:** This project has no WASM/browser test harness — there is no `wasm-bindgen-test` setup and `just test` only runs `cargo test` on the native target against in-memory SQLite. We therefore verify via `just check` (compilation + clippy + fmt) and a manual browser pass in `just dev-web-local`. Do not fabricate a WASM test suite; do not skip the manual pass.

---

## File Structure

**New:**
- `src/components/use_auto_scroll.rs` — the hook. Owns DOM scroll access, `localStorage` reads/writes, pill visibility, first-unseen computation. ~180 lines.

**Modified:**
- `src/components/mod.rs` — register the new module.
- `src/components/room_view.rs` — call the hook, attach `id` to the scroll `<div>`, render the divider and pill.
- `src/components/dm_view.rs` — same integration as rooms.

No server code, migrations, or server functions change.

---

## Task 1: Scaffold the hook module

**Files:**
- Create: `src/components/use_auto_scroll.rs`
- Modify: `src/components/mod.rs`

- [ ] **Step 1: Create the hook file with the public API and no-op bodies**

Create `src/components/use_auto_scroll.rs`:

```rust
use dioxus::prelude::*;

use crate::models::Message;

/// Handle returned by [`use_auto_scroll`]. Components attach `container_id`
/// to their scrollable `<div>`, render the `↑ New messages` divider above
/// the message whose id equals `first_unseen_id()`, and render the
/// `↓ New messages` pill when `show_new_pill()` is true.
#[derive(Clone)]
pub struct AutoScroll {
    pub container_id: String,
    pub show_new_pill: Signal<bool>,
    pub first_unseen_id: Signal<Option<i64>>,
    pub scroll_to_bottom: Callback<()>,
}

/// Manage scroll position for a chat message list.
///
/// - On first non-empty render after a room change, scrolls to the first
///   unseen message (or to the bottom if nothing is unseen).
/// - On message append, stays pinned to the bottom if the user is already
///   near the bottom; otherwise shows the "new messages" pill.
/// - Tracks the highest-seen message id in `localStorage` keyed by room.
pub fn use_auto_scroll(
    room_id: Signal<i64>,
    messages: Signal<Vec<Message>>,
) -> AutoScroll {
    let show_new_pill = use_signal(|| false);
    let first_unseen_id = use_signal(|| Option::<i64>::None);

    let scroll_to_bottom = use_callback(move |_: ()| {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = room_id;
            // Filled in by a later task.
        }
    });

    let container_id = {
        let id = *room_id.peek();
        format!("chat-scroll-{}", id)
    };

    AutoScroll {
        container_id,
        show_new_pill,
        first_unseen_id,
        scroll_to_bottom,
    }
}
```

- [ ] **Step 2: Register the module**

Modify `src/components/mod.rs`. Add the `use_auto_scroll` entry alphabetically with the other `use_*` modules:

```rust
pub mod admin;
pub mod auth_layout;
pub mod dm_view;
pub mod invite;
pub mod layout;
pub mod login;
pub mod register;
pub mod room_view;
pub mod sidebar;
pub mod use_auto_scroll;
pub mod use_websocket;
pub mod welcome;
```

- [ ] **Step 3: Verify it compiles**

Run: `just check`
Expected: exits 0. No warnings from the new file (one unused-import warning on `room_id` is acceptable and will be resolved in Task 2).

- [ ] **Step 4: Commit**

```bash
git add src/components/use_auto_scroll.rs src/components/mod.rs
git commit -m "feat(chat): scaffold use_auto_scroll hook"
```

---

## Task 2: localStorage last-seen helpers

**Files:**
- Modify: `src/components/use_auto_scroll.rs`

- [ ] **Step 1: Add the localStorage helpers**

Add these private functions at the bottom of `src/components/use_auto_scroll.rs`. They are gated for WASM; the non-WASM stubs return `None` / do nothing so the hook compiles on desktop/server builds.

```rust
const LAST_SEEN_PREFIX: &str = "lets-chat:last-seen:";

#[cfg(target_arch = "wasm32")]
fn read_last_seen(room_id: i64) -> Option<i64> {
    let storage = web_sys::window()?.local_storage().ok().flatten()?;
    let key = format!("{LAST_SEEN_PREFIX}{room_id}");
    let raw = storage.get_item(&key).ok().flatten()?;
    raw.parse::<i64>().ok()
}

#[cfg(target_arch = "wasm32")]
fn write_last_seen(room_id: i64, message_id: i64) {
    let Some(window) = web_sys::window() else { return };
    let Ok(Some(storage)) = window.local_storage() else { return };
    let key = format!("{LAST_SEEN_PREFIX}{room_id}");
    let _ = storage.set_item(&key, &message_id.to_string());
}

#[cfg(not(target_arch = "wasm32"))]
fn read_last_seen(_room_id: i64) -> Option<i64> {
    None
}

#[cfg(not(target_arch = "wasm32"))]
fn write_last_seen(_room_id: i64, _message_id: i64) {}
```

- [ ] **Step 2: Verify web-sys features are available**

Run: `just check`
Expected: exits 0. If it fails with `no method named local_storage` or similar, `web-sys` is missing the `Storage` feature — check `Cargo.toml`, confirm `web-sys` features include `Window` and `Storage`, add whatever is missing, and re-run.

- [ ] **Step 3: Commit**

```bash
git add src/components/use_auto_scroll.rs Cargo.toml Cargo.lock
git commit -m "feat(chat): add localStorage last-seen helpers"
```

(The `Cargo.toml`/`Cargo.lock` entries only appear if Step 2 required adding a feature. If not, omit them from the `git add`.)

---

## Task 3: Scroll primitives

**Files:**
- Modify: `src/components/use_auto_scroll.rs`

- [ ] **Step 1: Add DOM scroll helpers**

Append to `src/components/use_auto_scroll.rs`:

```rust
#[cfg(target_arch = "wasm32")]
const NEAR_BOTTOM_PX: f64 = 50.0;

#[cfg(target_arch = "wasm32")]
fn get_container(id: &str) -> Option<web_sys::HtmlElement> {
    use wasm_bindgen::JsCast;
    let doc = web_sys::window()?.document()?;
    let el = doc.get_element_by_id(id)?;
    el.dyn_into::<web_sys::HtmlElement>().ok()
}

#[cfg(target_arch = "wasm32")]
fn is_near_bottom(el: &web_sys::HtmlElement) -> bool {
    let scroll_top = el.scroll_top() as f64;
    let client_height = el.client_height() as f64;
    let scroll_height = el.scroll_height() as f64;
    scroll_top + client_height >= scroll_height - NEAR_BOTTOM_PX
}

#[cfg(target_arch = "wasm32")]
fn scroll_container_to_bottom(id: &str) {
    if let Some(el) = get_container(id) {
        el.set_scroll_top(el.scroll_height());
    }
}

#[cfg(target_arch = "wasm32")]
fn scroll_message_into_view(message_id: i64) {
    use wasm_bindgen::JsCast;
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let selector = format!("[data-msg-id=\"{message_id}\"]");
    let Ok(Some(el)) = doc.query_selector(&selector) else {
        return;
    };
    if let Ok(html) = el.dyn_into::<web_sys::HtmlElement>() {
        // `scroll_into_view_with_bool(true)` aligns to top of the scroll
        // parent, which is the behavior we want for the first unseen row.
        html.scroll_into_view_with_bool(true);
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `just check`
Expected: exits 0. Unused-function warnings for these helpers are acceptable — they are wired up in Task 4.

- [ ] **Step 3: Commit**

```bash
git add src/components/use_auto_scroll.rs
git commit -m "feat(chat): add DOM scroll primitives for auto-scroll hook"
```

---

## Task 4: Open-time scroll + sticky-bottom logic

**Files:**
- Modify: `src/components/use_auto_scroll.rs`

- [ ] **Step 1: Replace the `use_auto_scroll` body with the real logic**

Replace the existing `use_auto_scroll` function in `src/components/use_auto_scroll.rs`:

```rust
pub fn use_auto_scroll(
    room_id: Signal<i64>,
    messages: Signal<Vec<Message>>,
) -> AutoScroll {
    let mut show_new_pill = use_signal(|| false);
    let mut first_unseen_id = use_signal(|| Option::<i64>::None);

    // Per-room state reset + first-unseen computation on first non-empty
    // snapshot for the current room.
    let mut initialized_for_room = use_signal(|| 0i64);
    let mut last_len = use_signal(|| 0usize);
    let mut last_max_id = use_signal(|| 0i64);

    use_effect(move || {
        let rid = room_id();
        let list = messages();

        if *initialized_for_room.peek() != rid {
            // Room changed (or first mount): reset transient state.
            show_new_pill.set(false);
            first_unseen_id.set(None);
            last_len.set(0);
            last_max_id.set(0);
            initialized_for_room.set(rid);
        }

        let prev_len = *last_len.peek();
        let new_len = list.len();
        let newest_id = list.last().map(|m| m.id).unwrap_or(0);

        // First non-empty render for this room: decide initial scroll.
        if prev_len == 0 && new_len > 0 {
            let last_seen = read_last_seen(rid);
            let unseen = match last_seen {
                Some(seen) => list.iter().find(|m| m.id > seen).map(|m| m.id),
                None => None,
            };
            first_unseen_id.set(unseen);

            #[cfg(target_arch = "wasm32")]
            {
                let container_id = format!("chat-scroll-{rid}");
                match unseen {
                    Some(id) => scroll_message_into_view(id),
                    None => {
                        scroll_container_to_bottom(&container_id);
                        write_last_seen(rid, newest_id);
                    }
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = newest_id;
            }

            last_len.set(new_len);
            last_max_id.set(newest_id);
            return;
        }

        // Appended messages (new message arrived) for this room.
        if new_len > prev_len && newest_id > *last_max_id.peek() {
            #[cfg(target_arch = "wasm32")]
            {
                let container_id = format!("chat-scroll-{rid}");
                let was_near_bottom = get_container(&container_id)
                    .map(|el| is_near_bottom(&el))
                    .unwrap_or(true);

                if was_near_bottom {
                    scroll_container_to_bottom(&container_id);
                    write_last_seen(rid, newest_id);
                    show_new_pill.set(false);
                } else {
                    show_new_pill.set(true);
                }
            }
            last_max_id.set(newest_id);
        }

        // Keep length in sync even when messages are deleted.
        last_len.set(new_len);
    });

    let scroll_to_bottom = use_callback(move |_: ()| {
        let rid = *room_id.peek();
        let newest_id = messages.peek().last().map(|m| m.id).unwrap_or(0);
        #[cfg(target_arch = "wasm32")]
        {
            let container_id = format!("chat-scroll-{rid}");
            scroll_container_to_bottom(&container_id);
            if newest_id > 0 {
                write_last_seen(rid, newest_id);
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (rid, newest_id);
        }
        show_new_pill.set(false);
    });

    let container_id = {
        let id = *room_id.peek();
        format!("chat-scroll-{id}")
    };

    AutoScroll {
        container_id,
        show_new_pill,
        first_unseen_id,
        scroll_to_bottom,
    }
}
```

- [ ] **Step 2: Verify it compiles with no warnings**

Run: `just check`
Expected: exits 0, no warnings from `use_auto_scroll.rs`.

- [ ] **Step 3: Commit**

```bash
git add src/components/use_auto_scroll.rs
git commit -m "feat(chat): implement auto-scroll open + sticky-bottom logic"
```

---

## Task 5: Scroll listener for auto-dismiss + last-seen advance

**Files:**
- Modify: `src/components/use_auto_scroll.rs`

- [ ] **Step 1: Add a scroll listener effect inside `use_auto_scroll`**

Add the effect below, placed in `use_auto_scroll` AFTER the existing `use_effect(move || { ... })` block from Task 4 and BEFORE the `scroll_to_bottom` callback:

```rust
    // Attach a `scroll` listener so the pill auto-dismisses when the user
    // reaches the bottom, and so last-seen advances on manual scroll-down.
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::closure::Closure;
        use wasm_bindgen::JsCast;

        use_effect(move || {
            let rid = room_id();
            let container_id = format!("chat-scroll-{rid}");

            // The container may not exist on the very first effect run.
            // Re-query via a microtask so the DOM has rendered.
            let Some(el) = get_container(&container_id) else {
                return;
            };

            let mut show_new_pill = show_new_pill;
            let messages = messages;
            let rid_captured = rid;

            let cb = Closure::<dyn FnMut()>::new(move || {
                let Some(el) = get_container(&format!("chat-scroll-{rid_captured}"))
                else {
                    return;
                };
                if is_near_bottom(&el) {
                    let newest = messages.peek().last().map(|m| m.id).unwrap_or(0);
                    if newest > 0 {
                        write_last_seen(rid_captured, newest);
                    }
                    show_new_pill.set(false);
                }
            });

            let _ = el.add_event_listener_with_callback(
                "scroll",
                cb.as_ref().unchecked_ref(),
            );
            // Leak the closure intentionally: it lives for the lifetime of
            // this view. Dioxus will drop the element on room switch, which
            // also removes the listener.
            cb.forget();
        });
    }
```

- [ ] **Step 2: Verify compilation**

Run: `just check`
Expected: exits 0.

- [ ] **Step 3: Commit**

```bash
git add src/components/use_auto_scroll.rs
git commit -m "feat(chat): auto-dismiss pill and advance last-seen on manual scroll"
```

---

## Task 6: Wire hook into `room_view.rs`

**Files:**
- Modify: `src/components/room_view.rs`

- [ ] **Step 1: Import the hook**

In `src/components/room_view.rs`, add this import near the existing `use crate::components::use_websocket::WsHandle;` line:

```rust
use crate::components::use_auto_scroll::use_auto_scroll;
```

- [ ] **Step 2: Call the hook**

In `RoomViewPage`, immediately after the block that declares `let mut messages = use_signal(Vec::<Message>::new);` and `let mut load_error = use_signal(|| Option::<String>::None);` (around line 36), add:

```rust
    let auto = use_auto_scroll(room_id_sig, messages);
```

- [ ] **Step 3: Attach the DOM id to the scroll container**

In the same file, find the message-list `<div>` (currently `div { class: "flex-1 overflow-y-auto px-6 py-4 space-y-3",`) and add the `id` attribute:

```rust
        div {
            id: "{auto.container_id}",
            class: "flex-1 overflow-y-auto px-6 py-4 space-y-3",
```

- [ ] **Step 4: Add `data-msg-id` to each message div**

In the `for msg in message_list.iter()` loop, modify the outer `div` so it carries `data-msg-id`. Change:

```rust
                            div { key: "{msg.id}", class: "group flex flex-col",
```

to:

```rust
                            div {
                                key: "{msg.id}",
                                "data-msg-id": "{msg.id}",
                                class: "group flex flex-col",
```

- [ ] **Step 5: Render the "↑ New messages" divider**

Inside the same iteration, immediately before the existing outer `div { key: "{msg.id}", ... }` (i.e. before the change from Step 4), render a divider when this message is the first unseen. Wrap the existing `rsx!` block so the divider precedes the message div:

```rust
                        let is_first_unseen = auto.first_unseen_id() == Some(msg_id);
                        rsx! {
                            if is_first_unseen {
                                div {
                                    class: "flex items-center gap-2 my-2 text-xs font-medium text-blue-600",
                                    div { class: "flex-1 h-px bg-blue-300" }
                                    span { "New messages" }
                                    div { class: "flex-1 h-px bg-blue-300" }
                                }
                            }
                            div {
                                key: "{msg.id}",
                                "data-msg-id": "{msg.id}",
                                class: "group flex flex-col",
                                // ... existing message body unchanged ...
                            }
                        }
```

(Preserve the full existing body of the message `div` — the `// ... existing message body unchanged ...` comment is just a placeholder for what is already there. Do not delete real code.)

- [ ] **Step 6: Render the "↓ New messages" pill**

After the message-list `<div>` closes and before the typing indicator block, add:

```rust
        if auto.show_new_pill() {
            div { class: "relative",
                button {
                    r#type: "button",
                    class: "absolute right-6 -top-12 px-3 py-1.5 bg-blue-600 text-white text-sm rounded-full shadow-lg hover:bg-blue-700",
                    onclick: move |_| auto.scroll_to_bottom.call(()),
                    "↓ New messages"
                }
            }
        }
```

- [ ] **Step 7: Verify compilation**

Run: `just check`
Expected: exits 0.

- [ ] **Step 8: Commit**

```bash
git add src/components/room_view.rs
git commit -m "feat(chat): integrate auto-scroll into room view"
```

---

## Task 7: Wire hook into `dm_view.rs`

**Files:**
- Modify: `src/components/dm_view.rs`

- [ ] **Step 1: Import the hook and lift `room_id` into a signal**

In `src/components/dm_view.rs`, add near the existing WsHandle import:

```rust
use crate::components::use_auto_scroll::use_auto_scroll;
```

The hook needs `Signal<i64>`. `room_id` here is already a plain `i64` derived from `dm_room()`. Immediately after `let room_id = room.id;` (around line 40), add:

```rust
    let room_id_sig = use_signal(|| room_id);
```

(A `use_signal` initialized from a non-reactive value is fine here — DM view re-mounts on room change because the component key is driven by the URL `user_id`.)

- [ ] **Step 2: Call the hook**

After the `let mut messages = use_signal(Vec::<Message>::new);` declaration, add:

```rust
    let auto = use_auto_scroll(room_id_sig, messages);
```

- [ ] **Step 3: Attach DOM id and `data-msg-id`**

Find the message-list `<div class="flex-1 overflow-y-auto px-6 py-4 space-y-3",` and update it the same way as in Task 6 Step 3:

```rust
        div {
            id: "{auto.container_id}",
            class: "flex-1 overflow-y-auto px-6 py-4 space-y-3",
```

Inside the iteration loop, update the outer message `div` to add `"data-msg-id": "{msg.id}",` (see Task 6 Step 4 for the exact shape).

- [ ] **Step 4: Render the divider**

As in Task 6 Step 5, wrap the iteration body so the divider precedes the message `div`:

```rust
                        let is_first_unseen = auto.first_unseen_id() == Some(msg_id);
                        rsx! {
                            if is_first_unseen {
                                div {
                                    class: "flex items-center gap-2 my-2 text-xs font-medium text-blue-600",
                                    div { class: "flex-1 h-px bg-blue-300" }
                                    span { "New messages" }
                                    div { class: "flex-1 h-px bg-blue-300" }
                                }
                            }
                            div {
                                key: "{msg.id}",
                                "data-msg-id": "{msg.id}",
                                class: "group flex flex-col",
                                // ... existing message body unchanged ...
                            }
                        }
```

- [ ] **Step 5: Render the pill**

After the message-list `<div>` closes and before the typing indicator, add:

```rust
        if auto.show_new_pill() {
            div { class: "relative",
                button {
                    r#type: "button",
                    class: "absolute right-6 -top-12 px-3 py-1.5 bg-blue-600 text-white text-sm rounded-full shadow-lg hover:bg-blue-700",
                    onclick: move |_| auto.scroll_to_bottom.call(()),
                    "↓ New messages"
                }
            }
        }
```

- [ ] **Step 6: Verify compilation**

Run: `just check`
Expected: exits 0.

- [ ] **Step 7: Commit**

```bash
git add src/components/dm_view.rs
git commit -m "feat(chat): integrate auto-scroll into DM view"
```

---

## Task 8: Manual verification

**Files:** none modified.

- [ ] **Step 1: Rebuild CSS and start the local dev server**

Run: `just build-css && just dev-web-local`
Expected: server reachable at http://localhost:8080.

- [ ] **Step 2: Run the seven manual checks**

For each check, note pass/fail. All must pass.

1. **Fresh open with many messages.** Open any room that has >1 screen of messages. → Viewport starts scrolled to the bottom, newest message visible.
2. **First-unseen on return (room).** In the same room, scroll up; close the room (navigate away); have another user (or a second browser tab logged in as a different user) post 3+ messages; reopen the room. → Viewport is positioned with the first new message near the top, and a blue "New messages" divider sits above it.
3. **Sticky-bottom.** Scroll to bottom in a room. Have another user post a message. → Viewport stays pinned to the bottom; new message is visible; no pill.
4. **Pill on scrolled-up arrival.** Scroll up in a room. Have another user post a message. → Viewport does not move. A "↓ New messages" pill appears near the bottom-right.
5. **Pill click.** Click the pill from check 4. → Viewport jumps to bottom, pill disappears.
6. **Pill auto-dismiss.** Repeat check 4 to show the pill, then manually scroll to the bottom with the mouse wheel. → Pill disappears on its own.
7. **Same behavior in DMs.** Repeat checks 1-5 in a DM. → All behaviors identical to rooms.

- [ ] **Step 3: Stop the dev server**

Ctrl-C the `just dev-web-local` process.

- [ ] **Step 4: No commit**

This task records verification outcomes. If any check failed, file the failure as a bug and fix it in a follow-up task; do not mark this task complete until all seven pass.

---

## Task 9: Final check sweep

**Files:** none modified.

- [ ] **Step 1: Run all checks**

Run: `just check`
Expected: exits 0.

- [ ] **Step 2: Run native tests**

Run: `just test`
Expected: exits 0. The auto-scroll hook is not exercised by these tests (WASM-only code paths are `#[cfg]`-gated out), so test count should be unchanged from before this branch.

- [ ] **Step 3: Confirm branch state**

Run: `git status` and `git log --oneline main..HEAD`
Expected: clean working tree; six new commits matching Tasks 1, 2, 3, 4, 5, 6, 7 (spec commit from brainstorming is also on the branch).
