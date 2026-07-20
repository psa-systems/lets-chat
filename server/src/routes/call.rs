use axum::extract::State;
use axum::Json;
use serde_json::Value;

use crate::auth::AuthUser;
use crate::state::AppState;

/// GET /call/config - per-session WebRTC configuration the global call UI
/// needs. The ICE-server list is set once at server start via the
/// `LETS_CHAT_ICE_SERVERS` env var and would otherwise have to be threaded
/// through every page struct that extends `layout.html`; serving it through
/// a tiny JSON endpoint keeps the field off pages that have nothing to do
/// with calls. Auth-gated because the surface only renders for logged-in
/// sessions in the first place.
pub async fn get_config(State(state): State<AppState>, AuthUser(_user): AuthUser) -> Json<Value> {
    let ice: Value = serde_json::from_str(&state.ice_servers)
        .unwrap_or_else(|_| serde_json::json!([{"urls": "stun:stun.l.google.com:19302"}]));
    // LC-393 Phase 3: tells transcribe.js which transcription engine to use -
    // when true, capture audio clips and POST them for server-side STT; when
    // false, use the in-browser Web Speech API.
    //
    // LC-610: `huddleSfu` tells voice.js which transport a huddle uses. It is a
    // pure server-config read (LiveKit configured or not), deliberately
    // decoupled from membership so voice.js can pick the transport BEFORE
    // joining - the token endpoint, which does gate on membership, would be a
    // chicken-and-egg here. When true, a huddle connects to the SFU; when
    // false, the WebRTC mesh, unchanged.
    Json(serde_json::json!({
        "iceServers": ice,
        "sttServer": state.stt_available(),
        "huddleSfu": crate::livekit::available(),
    }))
}
