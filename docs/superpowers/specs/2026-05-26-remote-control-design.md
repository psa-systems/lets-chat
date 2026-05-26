# Design: remote control (TeamViewer-style) - LC-181

Status: proposed. Story: LC-181. This subtask: LC-182. Sub-issues: LC-183 (handshake + consent + verified-email gate), LC-184 (controller input capture + transport), LC-185 (Windows OS injection), LC-186 (kill-switch + audit + limits).

## Problem

In a 1:1 call, user A shares their screen (already supported: `call.js` screen-share, WebRTC, signaling relayed by `relay_call_signal` in `routes/ws.rs`). We want user B to be able to *operate* A's machine - move A's mouse, type on A's keyboard - so B can fix/drive A's screen. This is remote control, not just screen view.

This is high-blast-radius: granting control hands a remote person live input to your OS. The design is therefore consent-first, verified-email-gated, instantly revocable, and audited.

## Why the controlled side must be the native app, not a browser (settled)

A browser - hardware-accelerated or not - **cannot be the controlled machine**. Hardware acceleration only governs GPU rendering/decoding; it confers no OS privilege. The barrier is the browser security **sandbox**: no web API synthesizes a real OS mouse/keyboard event into *other* applications. Every input-capable web API is page-scoped (Pointer Lock, key/pointer events), and `getDisplayMedia` is read-only *capture*, not control.

- A browser / webview can be a **controller** (capture the human's input on the remote-video element and send it - no special API needed).
- A browser / webview **cannot be controlled** (cannot inject input into the host OS).

Incumbent proof: Chrome Remote Desktop runs the viewer in a browser but the *controlled* machine runs a separate **native host** binary; TeamViewer is fully native. Even they cannot inject OS input from pure browser JS.

Resolution for lets-chat: the webview inside our Tauri desktop app still can't inject, but the **native host process around it can**, via OS APIs (`SendInput`, etc.) over a JS->native bridge (LC-185). Hence:

- lets-chat **web** client -> controller only.
- lets-chat **desktop** app -> controllable (and controller).

No amount of browser capability (acceleration, WASM, WebGPU) changes this; do not re-litigate.

## Trust model + threat model

Granting control = trusting the controller with live keyboard+mouse on your machine for the session's duration. The platform's job is to make that grant **deliberate, scoped, observable, and instantly reversible** - not to make a malicious-grantee safe (that is impossible once you hand someone your input).

What the verified-email gate buys, and does not:
- Buys: a weak identity/accountability anchor + an abuse-cost floor (throwaway accounts can't request control); pairs with the audit log so a control session is attributable.
- Does NOT buy: protection from a verified-but-malicious controller, or from social-engineering a victim into granting. Those are mitigated by consent friction + the kill-switch + scope, not by the gate.

Primary threats + mitigations:
- **Unsolicited / surprise control** -> control never starts without an explicit per-session grant by the sharer (no "remember this peer"); a persistent banner makes an active session impossible to miss.
- **Grant that outlives intent** -> auto-revoke on call end / screen-share stop / shared-surface change / data-channel drop; a manual kill-switch (button + global hotkey) severs instantly.
- **Privilege escalation via the controlled host** -> Windows UIPI already blocks `SendInput` from driving higher-integrity (elevated/admin) windows; we honor that limit (surface "can't control this window" rather than try to defeat it). Document that an admin-elevated window is out of the controller's reach.
- **Abuse / spam / harassment** -> block-list aware (a blocked peer cannot request); rate-limit requests; audit every session.
- **Compromised controller mid-session** -> kill-switch + auto-revoke are the only real defense; documented as residual risk.

## Verified-email gate (both ways)

Neither requesting nor granting control is permitted unless BOTH peers have a verified email. Enforced **server-side** at the handshake, not merely hidden in the UI: check `db::auth::get_user_email_verified_at(...)` is `Some` for both the controller and the sharer when relaying the control-request / grant signals. A client that forges the request still gets rejected at the relay.

saas vs standalone semantics (decide + encode in LC-183):
- **standalone**: `email_verified_at` is set by the `#[cfg(standalone)]` email-verification flow. The gate is literally `email_verified_at IS NOT NULL`.
- **saas**: SaaS auth may verify emails out-of-band (the platform owns identity). Either (a) treat SaaS-authenticated accounts as verified, or (b) require the same `email_verified_at` column to be populated by the SaaS auth bridge. The design's requirement is "both peers verified by the build's definition of verified"; LC-183 picks the concrete check per build.

## Consent lifecycle

```
viewer: Request control  ──signal──▶  sharer: Grant / Deny prompt
                                            │ Grant
                                            ▼
                          ACTIVE  (sharer: persistent banner + kill-switch;
                                   viewer: input captured + sent)
                                            │
        Revoke ◀── manual (button / global hotkey)
               ◀── auto (call end | share stop | surface change | channel drop)
                                            ▼
                                        ENDED  (injector flag flipped,
                                                held keys/buttons released,
                                                audit row finalized)
```

Invariants: one active controller per shared session at a time; the "Request control" affordance only renders when both peers are verified AND a screen share is active; a blocked peer never sees it and is rejected server-side anyway.

## Transport + protocol sketch

- **Signaling** (request / grant / deny / revoke): low-rate control messages. Either extend the `CallSignal` `kind` set (`CALL_SIGNAL_KINDS` in `routes/ws.rs`, relayed by `relay_call_signal` between the two call participants) or add dedicated `ChatEvent` variants. The verified-email + block-list gate lives on this relay path.
- **Input stream** (high-rate, while active): a dedicated WebRTC **data channel** on the existing 1:1 `RTCPeerConnection` (not the WS - keep input peer-to-peer + off the server). Event shape (compact): pointer move/down/up/wheel + key down/up with modifiers and (for keys) scan codes.
- **Coordinates**: the controller sends **normalized [0,1]** surface coordinates (computed from the scaled/letterboxed `<video>`), never pixels. The controlled side maps [0,1] to absolute screen pixels using its own monitor/window geometry + DPI. This keeps the protocol resolution- and DPI-independent.
- **Rate**: coalesce pointer-move to ~60-120 Hz (rAF or a timer); send clicks/keys immediately.

## Platform matrix (controlled side, LC-185+)

| OS | API | Notes |
|----|-----|-------|
| Windows | `SendInput` | First target. Scan-code keyboard for layout robustness. UIPI blocks elevated windows. Cross-builds from the existing mingw `x86_64-pc-windows-gnu`. |
| Linux | uinput (preferred) or XTEST | Wayland blocks XTEST; uinput needs device permission. Deferred. |
| macOS | `CGEvent` | Requires the user to grant Accessibility permission (prompt + deep link to System Settings). Deferred. |

Non-Windows targets ship as `#[cfg(...)]` no-ops until their slice lands; the web controller works against any controllable peer regardless.

## Audit + limits

- Audit every control session (controller id, sharer id, start, end, duration). The existing `db::moderation::log_mod_action` is the closest precedent; decide whether to reuse it or add a `remote_control_sessions` table (a dedicated table is cleaner since these are not moderation actions).
- Rate-limit control requests per (requester, target) to blunt spam; respect `db::auth::is_blocked_either_way`.
- Consider a max session duration or periodic re-consent (decide in LC-186).

## Acceptance (story-level)

- A verified user in a call with another verified user, who is screen-sharing, can request control; the sharer sees a grant/deny prompt; on grant the controller's mouse + keyboard drive the sharer's machine (desktop app).
- Both the verified-email gate and the block-list are enforced server-side (a forged request is rejected).
- The sharer can sever control instantly (button + hotkey); control auto-revokes on call end / share stop.
- Every session is audited.

## Out of scope

- File transfer, clipboard sync, multi-monitor target selection beyond the shared surface, unattended access ("remember this peer" / always-allow). All explicitly deferred - this is attended, per-session, consent-gated control only.
- Making a verified-but-malicious controller safe (impossible by construction; mitigated by friction + kill-switch + audit, not eliminated).
