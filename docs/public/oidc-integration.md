# OIDC integration

Lets chat has one sign-in path: Bunyip Single Sign-On through OpenID Connect. There is no local username / password, no password reset, no second factor on the chat side; those live on the Bunyip side.

## Pattern

The lets-chat server is a confidential OIDC client. At startup it fetches OpenID Connect discovery and JWKS from the configured issuer. On every sign-in it runs an authorization code flow against the OP and exchanges the code for an ID token.

## Register the client on Bunyip

Create a client on the Bunyip OP with:

- Client type: confidential, `client_secret_basic` authentication
- Allowed redirect URI: exactly `<LETS_CHAT_PUBLIC_URL>/auth/bunyip/callback`
- Allowed grant types: `authorization_code`
- Required scopes: `openid` plus any profile claims lets-chat should receive (email, name)

Bunyip issues a `client_id` and `client_secret`. Paste those into the lets-chat environment variables.

## PKCE

The authorization code flow uses PKCE. No shared secret rides in the URL; the code verifier is a random value created per sign-in and held in the session until the token exchange. Operators do not configure PKCE; it is always on.

## Tokens in the browser (web)

The browser session is a cookie-backed session on the lets-chat server. Access and refresh tokens never reach the browser. When the server needs a refreshed access token it talks to Bunyip directly.

Session storage: the cookie is `HttpOnly`, `Secure`, and `SameSite=Lax`. It carries a random session id, not the OIDC tokens themselves.

## Tokens on the desktop app

The desktop app embeds the same web UI inside a native shell. The browser-cookie model applies there too; tokens still never touch the Dioxus side. The desktop app ships with a keyring-backed credential store for the refresh path when the operator enables staying signed in.

## First administrator

The first account to sign in is promoted to administrator. There is no "install admin" env var and no bootstrap link. Decide ahead of time who signs in first.

Dev builds have a convenience path through `LETS_CHAT_BOOTSTRAP_ADMIN` that seeds an admin row before SSO completes; it is off in release builds and never set in production.

## Validating the setup

Before rolling to real users:

1. Hit `GET /healthz`. It returns 200 only when discovery and JWKS have loaded.
2. Load the sign-in page in a browser. The OP's login screen should render, not a lets-chat error page.
3. Sign in with a non-admin Bunyip account. Confirm the user lands on the home room list and that the session cookie is set on the lets-chat hostname, not the OP hostname.

A failure here is almost always one of three things: a wrong `REDIRECT_URI` (must match character-for-character), a JWKS fetch failure (issuer URL or TLS), or a stale discovery document cached by the OP.

## Rotating the client secret

Mint the new secret on the OP, update `LETS_CHAT_BUNYIP_SSO_CLIENT_SECRET`, restart the server. Active sessions are not affected (sessions are server-side; no re-sign-in needed).

## Removing a user

Removing a user from Bunyip stops them from signing in. Existing sessions stay until they expire or until an admin revokes them from `/admin/users`. There is no implicit user-delete on lets-chat; the `users` row stays with its history.
