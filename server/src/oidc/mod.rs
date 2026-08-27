//! LC-22: "Log in with Bunyip" SSO client.
//!
//! Hand-rolled OIDC RP wrapper. Keeps the dependency surface narrow (just
//! `jsonwebtoken` + the existing `reqwest`) instead of pulling in the
//! `openidconnect` crate's larger tree. The shape mirrors what we need from
//! bunyip-api (the OP) and nothing else:
//!
//! - One-shot discovery fetch + JWKS fetch at startup
//! - Lazy JWKS re-fetch on unknown `kid` (key rotation)
//! - POST to the token endpoint with `client_secret_basic`
//! - GET userinfo with the access token
//! - Verify the `id_token` EdDSA signature + iss + aud + exp + nonce
//!
//! The discovery + JWKS fetches at startup MUST succeed; the binary refuses
//! to start with the SSO flag on but bunyip-api unreachable. This is the
//! all-or-nothing posture documented in
//! `docs/lets-chat/sso/bunyip-only/04-lets-chat-server-additions.md` §4.2.

mod client;
mod config;
mod pkce;
mod verify;

pub use client::{BunyipSsoClient, BunyipSsoError, TokenResponse, UserInfo};
pub use config::{dev_no_sso_opt_out, BunyipSsoConfig};
pub use pkce::{new_pkce_pair, new_random_token};
pub use verify::IdTokenClaims;
