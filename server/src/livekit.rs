//! LC-512: LiveKit SFU integration - the media transport for the LC-494 stage.
//!
//! The stage control plane (roles + request-to-speak) ships in LC-494; this
//! adds the actual audio by routing it through a LiveKit SFU instead of the
//! full mesh (which caps at ~4-6 peers). LiveKit is self-hostable; the operator
//! runs a LiveKit server and points the three env vars at it - same "optional
//! external service configured by env" posture as the LLM / STT / GIF features.
//!
//! Server-side this is just access-token minting: a LiveKit token is a JWT
//! (HS256, signed with the API secret) carrying a `video` grant that pins the
//! room and the participant's publish/subscribe rights. We derive those rights
//! from the stage roster - speakers (and hosts) may publish; everyone may
//! subscribe - so granting/revoking the floor maps directly onto LiveKit
//! publish permission (the client re-fetches a token when its role changes).
//! The browser uses the `livekit-client` SDK to connect; no Rust media SDK.

use serde::{Deserialize, Serialize};

/// Operator LiveKit configuration. `from_env` returns `None` when the URL /
/// key / secret are not all set, which disables stage audio (the control plane
/// still works; the panel just has no media).
#[derive(Debug, Clone)]
pub struct LiveKitConfig {
    /// Browser-facing LiveKit signaling URL, e.g. `wss://livekit.example.com`.
    pub url: String,
    pub api_key: String,
    pub api_secret: String,
}

impl LiveKitConfig {
    /// `LETS_CHAT_LIVEKIT_URL` / `_API_KEY` / `_API_SECRET` - all three required
    /// to enable stage audio.
    pub fn from_env() -> Option<Self> {
        let var = |k: &str| {
            std::env::var(k)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        Some(Self {
            url: var("LETS_CHAT_LIVEKIT_URL")?,
            api_key: var("LETS_CHAT_LIVEKIT_API_KEY")?,
            api_secret: var("LETS_CHAT_LIVEKIT_API_SECRET")?,
        })
    }
}

/// Whether stage audio is configured (`LETS_CHAT_LIVEKIT_*` all set).
pub fn available() -> bool {
    LiveKitConfig::from_env().is_some()
}

/// Token lifetime. Generous enough to outlast a long stage session; the client
/// re-fetches on role change anyway, and a stale token only means the next
/// reconnect must refresh.
const TOKEN_TTL_SECS: u64 = 6 * 60 * 60;

/// The LiveKit `video` grant claim. Field names are LiveKit's wire format
/// (camelCase) and must not be renamed.
#[derive(Debug, Serialize, Deserialize)]
struct VideoGrant {
    room: String,
    #[serde(rename = "roomJoin")]
    room_join: bool,
    #[serde(rename = "canPublish")]
    can_publish: bool,
    #[serde(rename = "canSubscribe")]
    can_subscribe: bool,
    #[serde(rename = "canPublishData")]
    can_publish_data: bool,
}

/// LiveKit access-token claims (a JWT). `iss` is the API key; `sub`/`name` the
/// participant identity/label; `video` the room + permission grant.
#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    name: String,
    exp: u64,
    nbf: u64,
    video: VideoGrant,
}

/// LC-596/LC-610: the participant count at which mesh-for-2 would hand over to
/// the SFU. NOT enforced: the huddle token endpoint issues a token to any
/// member of a configured room regardless of count.
///
/// #569 gated on this so 2-peer huddles kept the cheaper direct mesh. LC-610
/// found that model incoherent without a mid-call mesh->SFU handover: a huddle
/// grows one person at a time, so at the crossing point the earlier peers are
/// already on the mesh and would be stranded there. Until that handover exists,
/// a huddle is entirely SFU (LiveKit configured) or entirely mesh (not), and
/// the transport does not depend on count.
///
/// Kept as a documented seam, not deleted, so the handover work can reintroduce
/// mesh-for-2 without re-deriving the number. Reintroducing it means restoring
/// the `< SFU_MIN_PARTICIPANTS` refusal in `get_huddle_token` AND building the
/// handover; one without the other is the incoherent state above.
pub const SFU_MIN_PARTICIPANTS: usize = 3;

/// LC-596: which real-time surface a LiveKit room belongs to.
///
/// A lets-chat room can host a Stage and a huddle at the same time - they are
/// independent features keyed off the same `room_id`, one by `stage_enabled`
/// and the other by live mesh membership. Naming both `stage-{id}` would drop
/// their participants into a single LiveKit room, so a huddle would leak into
/// the Stage broadcast and vice versa. The surface is part of the name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Stage,
    Huddle,
}

impl Surface {
    fn prefix(self) -> &'static str {
        match self {
            Surface::Stage => "stage",
            Surface::Huddle => "huddle",
        }
    }
}

/// Stable LiveKit room name for one surface of a lets-chat room.
pub fn room_name(surface: Surface, room_id: i64) -> String {
    format!("{}-{room_id}", surface.prefix())
}

/// Mint a LiveKit access token (JWT) for `identity` to join `surface` of
/// `room_id`. `can_publish` is true for anyone allowed to send media - Stage
/// speakers and hosts, every huddle participant - while everyone may subscribe.
/// `now_unix` is injected so the claims are deterministically testable.
pub fn mint_token(
    cfg: &LiveKitConfig,
    surface: Surface,
    room_id: i64,
    identity: &str,
    name: &str,
    can_publish: bool,
    now_unix: u64,
) -> Result<String, jsonwebtoken::errors::Error> {
    let claims = Claims {
        iss: cfg.api_key.clone(),
        sub: identity.to_string(),
        name: name.to_string(),
        nbf: now_unix,
        exp: now_unix + TOKEN_TTL_SECS,
        video: VideoGrant {
            room: room_name(surface, room_id),
            room_join: true,
            can_publish,
            can_subscribe: true,
            can_publish_data: false,
        },
    };
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(cfg.api_secret.as_bytes()),
    )
}

// ── LC-814 (LC-810 stage 2): dispatch the server-side transcription agent ────
//
// When transcription starts on an SFU huddle and the operator has configured a
// transcription agent, the server dispatches the sidecar agent into the LiveKit
// room so it can subscribe to every participant's audio track (see the LC-810
// design). Dispatch is a signed call to LiveKit's AgentDispatchService over
// twirp - the ONE server-side LiveKit API surface beyond token minting. This
// module builds the admin token, the endpoint URL, and the request body; the
// HTTP round-trip itself is exercised only against a live LiveKit (staging QA).

/// Reserved LiveKit identity prefix for the dispatched transcription agent. It
/// joins only to subscribe to audio and is hidden from the roster; huddle_sfu.js
/// filters the same prefix so the agent never renders a tile. Kept distinct from
/// lets-chat user ids (which never start with this).
pub const AGENT_IDENTITY_PREFIX: &str = "agent-";

/// Default registered name of the transcription agent (the `agent_name` the
/// sidecar registers under), overridable via `LETS_CHAT_TRANSCRIBE_AGENT_NAME`.
const DEFAULT_AGENT_NAME: &str = "transcriber";

/// The transcription agent's registered name for dispatch.
pub fn transcribe_agent_name() -> String {
    std::env::var("LETS_CHAT_TRANSCRIBE_AGENT_NAME")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_AGENT_NAME.to_string())
}

/// True when server-side transcription can be dispatched: LiveKit is configured
/// AND the agent callback token (LC-813) is set. The token is the trust boundary
/// for the agent's clip callbacks, so without it the agent has nowhere to post
/// and dispatch is pointless.
pub fn transcribe_dispatch_ready() -> bool {
    available()
        && std::env::var("LETS_CHAT_TRANSCRIBE_AGENT_TOKEN")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .is_some()
}

/// Admin (`roomAdmin`) grant, required to call the LiveKit server API for a
/// specific room. Field names are LiveKit's wire format (camelCase).
#[derive(Debug, Serialize, Deserialize)]
struct AdminGrant {
    room: String,
    #[serde(rename = "roomAdmin")]
    room_admin: bool,
}

/// Claims for a LiveKit server-API (admin) token. No `sub`/`name`: this is not a
/// participant, it authorizes a service call.
#[derive(Debug, Serialize, Deserialize)]
struct AdminClaims {
    iss: String,
    exp: u64,
    nbf: u64,
    video: AdminGrant,
}

/// Short-lived admin token lifetime: a dispatch call is immediate, so this only
/// needs to outlast clock skew + the request.
const ADMIN_TOKEN_TTL_SECS: u64 = 60;

/// Mint a `roomAdmin` token scoped to `room` for calling the LiveKit server API
/// (here, AgentDispatchService). `now_unix` is injected for deterministic tests.
pub fn mint_admin_token(
    cfg: &LiveKitConfig,
    room: &str,
    now_unix: u64,
) -> Result<String, jsonwebtoken::errors::Error> {
    let claims = AdminClaims {
        iss: cfg.api_key.clone(),
        nbf: now_unix,
        exp: now_unix + ADMIN_TOKEN_TTL_SECS,
        video: AdminGrant {
            room: room.to_string(),
            room_admin: true,
        },
    };
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(cfg.api_secret.as_bytes()),
    )
}

/// Derive the LiveKit server-API base URL from the browser signaling URL: the
/// server API is HTTP(S) on the same host, so `wss://` -> `https://` and
/// `ws://` -> `http://` (a bare `http(s)://` is left as-is), trailing slash
/// trimmed.
fn server_api_base(signaling_url: &str) -> String {
    let u = signaling_url.trim().trim_end_matches('/');
    if let Some(rest) = u.strip_prefix("wss://") {
        format!("https://{rest}")
    } else if let Some(rest) = u.strip_prefix("ws://") {
        format!("http://{rest}")
    } else {
        u.to_string()
    }
}

/// Full twirp endpoint for AgentDispatchService.CreateDispatch.
pub fn dispatch_api_url(signaling_url: &str) -> String {
    format!(
        "{}/twirp/livekit.AgentDispatchService/CreateDispatch",
        server_api_base(signaling_url)
    )
}

/// CreateAgentDispatchRequest body. `metadata` is an opaque string the agent
/// reads from its job (we pass JSON identifying the transcript + callback base).
#[derive(Debug, Serialize)]
pub struct CreateDispatchRequest {
    pub agent_name: String,
    pub room: String,
    pub metadata: String,
}

/// A dispatch failure. Kept coarse: the caller treats every variant as
/// best-effort (log + fall back to per-client capture).
#[derive(Debug)]
pub enum DispatchError {
    Token(jsonwebtoken::errors::Error),
    Transport(String),
    Status(u16),
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DispatchError::Token(e) => write!(f, "admin token: {e}"),
            DispatchError::Transport(e) => write!(f, "transport: {e}"),
            DispatchError::Status(s) => write!(f, "livekit status {s}"),
        }
    }
}

/// Dispatch the transcription agent into `room` with `metadata`, via
/// AgentDispatchService.CreateDispatch. Best-effort: any error is returned for
/// the caller to log. `now_unix` is injected for testability.
///
/// The wire round-trip is verified at staging (needs a live LiveKit); the token,
/// URL, and body it builds are unit-tested here.
pub async fn dispatch_transcription_agent(
    cfg: &LiveKitConfig,
    room: &str,
    metadata: String,
    now_unix: u64,
) -> Result<(), DispatchError> {
    let token = mint_admin_token(cfg, room, now_unix).map_err(DispatchError::Token)?;
    let body = CreateDispatchRequest {
        agent_name: transcribe_agent_name(),
        room: room.to_string(),
        metadata,
    };
    let url = dispatch_api_url(&cfg.url);
    // LiveKit is an operator-configured, trusted host (same posture as STT), so
    // the SSRF filter is intentionally bypassed - the URL comes from operator env.
    let req = crate::http_client::outbound_trusted_post(&url)
        .await
        .map_err(|e| DispatchError::Transport(e.to_string()))?
        .bearer_auth(token)
        .json(&body);
    let resp = req
        .send()
        .await
        .map_err(|e| DispatchError::Transport(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(DispatchError::Status(resp.status().as_u16()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{DecodingKey, Validation};

    fn cfg() -> LiveKitConfig {
        LiveKitConfig {
            url: "wss://lk.example.com".into(),
            api_key: "devkey".into(),
            api_secret: "devsecretdevsecretdevsecret123456".into(),
        }
    }

    fn decode(token: &str, secret: &str) -> Claims {
        let mut v = Validation::new(jsonwebtoken::Algorithm::HS256);
        // The tests mint with a fixed 1970 `now_unix`, so skip wall-clock expiry
        // validation; we assert the claim contents (incl. exp) directly.
        v.validate_exp = false;
        // LiveKit tokens have no `aud`; don't require one.
        v.required_spec_claims.clear();
        jsonwebtoken::decode::<Claims>(token, &DecodingKey::from_secret(secret.as_bytes()), &v)
            .unwrap()
            .claims
    }

    #[test]
    fn speaker_token_can_publish() {
        let c = cfg();
        let t = mint_token(&c, Surface::Stage, 42, "user-1", "Alice", true, 1_000).unwrap();
        let claims = decode(&t, &c.api_secret);
        assert_eq!(claims.iss, "devkey");
        assert_eq!(claims.sub, "user-1");
        assert_eq!(claims.video.room, "stage-42");
        assert!(claims.video.room_join);
        assert!(claims.video.can_publish);
        assert!(claims.video.can_subscribe);
        assert_eq!(claims.exp, 1_000 + TOKEN_TTL_SECS);
    }

    #[test]
    fn listener_token_cannot_publish_but_subscribes() {
        let c = cfg();
        let t = mint_token(&c, Surface::Stage, 7, "user-2", "Bob", false, 5_000).unwrap();
        let claims = decode(&t, &c.api_secret);
        assert!(!claims.video.can_publish);
        assert!(claims.video.can_subscribe);
    }

    /// LC-596: a room can host a Stage and a huddle simultaneously - one gated
    /// by `stage_enabled`, the other by live mesh membership - so the surface
    /// has to be part of the LiveKit room name. Without this the two sets of
    /// participants share one LiveKit room and hear each other.
    #[test]
    fn stage_and_huddle_in_one_room_do_not_collide() {
        let c = cfg();
        let stage = decode(
            &mint_token(&c, Surface::Stage, 42, "u", "U", true, 1_000).unwrap(),
            &c.api_secret,
        );
        let huddle = decode(
            &mint_token(&c, Surface::Huddle, 42, "u", "U", true, 1_000).unwrap(),
            &c.api_secret,
        );
        assert_eq!(stage.video.room, "stage-42");
        assert_eq!(huddle.video.room, "huddle-42");
        assert_ne!(
            stage.video.room, huddle.video.room,
            "the same lets-chat room must map to two distinct LiveKit rooms"
        );
    }

    /// Every huddle participant is a publisher: a huddle is symmetric, unlike a
    /// Stage where the floor is granted.
    #[test]
    fn huddle_participant_publishes() {
        let c = cfg();
        let claims = decode(
            &mint_token(&c, Surface::Huddle, 9, "user-3", "Cara", true, 2_000).unwrap(),
            &c.api_secret,
        );
        assert_eq!(claims.video.room, "huddle-9");
        assert!(claims.video.can_publish);
        assert!(claims.video.can_subscribe);
    }

    #[test]
    fn token_is_rejected_under_the_wrong_secret() {
        let c = cfg();
        let t = mint_token(&c, Surface::Stage, 1, "u", "U", true, 1_000).unwrap();
        let mut v = Validation::new(jsonwebtoken::Algorithm::HS256);
        v.required_spec_claims.clear();
        assert!(
            jsonwebtoken::decode::<Claims>(&t, &DecodingKey::from_secret(b"wrong"), &v).is_err()
        );
    }

    // ── LC-814: agent dispatch ───────────────────────────────────────────────

    fn decode_admin(token: &str, secret: &str) -> AdminClaims {
        let mut v = Validation::new(jsonwebtoken::Algorithm::HS256);
        v.validate_exp = false;
        v.required_spec_claims.clear();
        jsonwebtoken::decode::<AdminClaims>(token, &DecodingKey::from_secret(secret.as_bytes()), &v)
            .unwrap()
            .claims
    }

    #[test]
    fn admin_token_carries_room_admin_for_the_target_room() {
        let c = cfg();
        let t = mint_admin_token(&c, "huddle-42", 1_000).unwrap();
        let claims = decode_admin(&t, &c.api_secret);
        assert_eq!(claims.iss, "devkey");
        assert_eq!(claims.video.room, "huddle-42");
        assert!(claims.video.room_admin);
        assert_eq!(claims.exp, 1_000 + ADMIN_TOKEN_TTL_SECS);
    }

    #[test]
    fn dispatch_url_maps_ws_scheme_to_http_and_appends_the_twirp_path() {
        assert_eq!(
            dispatch_api_url("wss://lk.example.com"),
            "https://lk.example.com/twirp/livekit.AgentDispatchService/CreateDispatch"
        );
        assert_eq!(
            dispatch_api_url("ws://lk.example.com:7880/"),
            "http://lk.example.com:7880/twirp/livekit.AgentDispatchService/CreateDispatch"
        );
        // A bare http(s) base is left as-is (only the trailing slash trimmed).
        assert_eq!(
            dispatch_api_url("https://lk.example.com/"),
            "https://lk.example.com/twirp/livekit.AgentDispatchService/CreateDispatch"
        );
    }

    #[test]
    fn dispatch_body_serializes_the_expected_fields() {
        let body = CreateDispatchRequest {
            agent_name: "transcriber".into(),
            room: "huddle-7".into(),
            metadata: "{\"transcript_id\":5}".into(),
        };
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&body).unwrap()).expect("valid json");
        assert_eq!(v["agent_name"], "transcriber");
        assert_eq!(v["room"], "huddle-7");
        assert_eq!(v["metadata"], "{\"transcript_id\":5}");
    }

    #[test]
    fn agent_name_defaults_when_unset() {
        // No env override in this process -> the documented default.
        std::env::remove_var("LETS_CHAT_TRANSCRIBE_AGENT_NAME");
        assert_eq!(transcribe_agent_name(), "transcriber");
    }
}
