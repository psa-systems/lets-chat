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

/// LC-596: the participant count at which a huddle stops using the WebRTC mesh
/// and moves to the SFU.
///
/// The mesh is one `RTCPeerConnection` per pair, so cost is N-squared and
/// `voice.js` documents a practical cap of 4 to 6. Two peers is exactly one
/// connection and the mesh is the cheaper, lower-latency path there - no server
/// hop - so the SFU takes over from the third participant. The ticket called
/// this an assumption to revise once measured, so it is one named constant
/// rather than a number spread across client and server.
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
}
