#!/usr/bin/env python3
"""Minimal mock OIDC OP for LOCAL DEV ONLY.

Since the LC-22 cutover, lets-chat-server refuses to start unless a Bunyip SSO
OP answers discovery + JWKS at boot (server/src/main.rs, server/src/oidc). That
makes `just dev-web-local` unbootable without the full bunyip dev-sso stack,
which blocks pure-UI work.

LC-577: this now implements a real authorization-code flow, so the mock stack
can complete a login and every AUTHENTICATED page (room view, /settings, the
home dashboard, the thread and details panels) renders and can be screenshotted
without bunyip. It previously served only discovery + JWKS with a dummy key and
could not authenticate at all, which left nearly the whole product unverifiable.

Endpoints:
  GET  /.well-known/openid-configuration -> discovery doc (issuer must match)
  GET  /jwks.json                        -> the REAL Ed25519 public key
  GET  /authorize                        -> redirects straight back with a code
  POST /token                            -> code -> EdDSA-signed id_token

There is no login form and no consent: this is a stub, and it authenticates a
single fixed identity (override with MOCK_SSO_SUB / MOCK_SSO_EMAIL /
MOCK_SSO_USERNAME / MOCK_SSO_ROLE). The `sub` is stable across restarts so
repeated boots reuse the same account instead of provisioning a new one each
time, and the identity claims `bunyip_role: admin` so it lands on (and keeps)
the DEV-300 SETUP_DEFAULT_ADMIN account.

PKCE: the client sends code_challenge/code_challenge_method=S256. The mock
accepts and does NOT verify the verifier - it is a local-dev stub, not a
conformance target. The server's own OIDC verification path is left fully
intact and exercised: the id_token is really signed and really validated.

NEVER use this outside local dev.
"""

import base64
import http.server
import json
import os
import secrets
import time
import urllib.parse

import jwt
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

ISSUER = os.environ.get("MOCK_SSO_ISSUER", "http://mock-oidc:9000")
KID = "mock-dev-1"

SUB = os.environ.get("MOCK_SSO_SUB", "mock-dev-user-1")
EMAIL = os.environ.get("MOCK_SSO_EMAIL", "dev@example.test")
USERNAME = os.environ.get("MOCK_SSO_USERNAME", "devuser")
# DEV-300: the `bunyip_role` claim. mirror_bunyip_admin_role (LC-413) makes the
# OP the source of truth for the lets-chat role on EVERY login, so without an
# `admin` claim here the SETUP_DEFAULT_ADMIN seed is demoted the moment it is
# adopted. The primary identity is the dev admin; cookie-seeded extras below
# stay ordinary users so role-gated UI is still testable.
ROLE = os.environ.get("MOCK_SSO_ROLE", "admin")

DISCOVERY = {
    "issuer": ISSUER,
    "authorization_endpoint": ISSUER + "/authorize",
    "token_endpoint": ISSUER + "/token",
    "userinfo_endpoint": ISSUER + "/userinfo",
    "jwks_uri": ISSUER + "/jwks.json",
    "response_types_supported": ["code"],
    "subject_types_supported": ["public"],
    "id_token_signing_alg_values_supported": ["EdDSA"],
    "scopes_supported": ["openid", "email", "profile"],
    "token_endpoint_auth_methods_supported": [
        "client_secret_basic",
        "client_secret_post",
    ],
    "code_challenge_methods_supported": ["S256"],
}

# A REAL Ed25519 keypair, derived from a FIXED seed rather than generated fresh.
# The server fetches JWKS once at boot and caches it, so a per-process random key
# would invalidate that cache every time this container restarted and force an
# app restart in lockstep. A fixed seed keeps the published key stable, so the
# mock can be restarted on its own. It is a hardcoded dev key by design: it
# signs nothing outside this local-only stack.
MOCK_SEED = b"lets-chat-local-dev-mock-oidc-32"  # exactly 32 bytes
PRIVATE_KEY = Ed25519PrivateKey.from_private_bytes(MOCK_SEED)
PUBLIC_RAW = PRIVATE_KEY.public_key().public_bytes(
    encoding=serialization.Encoding.Raw,
    format=serialization.PublicFormat.Raw,
)
PRIVATE_PEM = PRIVATE_KEY.private_bytes(
    encoding=serialization.Encoding.PEM,
    format=serialization.PrivateFormat.PKCS8,
    encryption_algorithm=serialization.NoEncryption(),
)


def b64u(raw: bytes) -> str:
    return base64.urlsafe_b64encode(raw).rstrip(b"=").decode()


JWKS = {
    "keys": [
        {
            "kty": "OKP",
            "crv": "Ed25519",
            "alg": "EdDSA",
            "use": "sig",
            "kid": KID,
            "x": b64u(PUBLIC_RAW),
        }
    ]
}

# code -> {"nonce": str, "aud": str, "ident": dict}. In-memory and single-use.
CODES = {}
# access_token -> ident dict, so /userinfo returns the SAME identity the code
# authenticated (the callback rejects a userinfo sub that differs from the
# id_token). Lets one mock OP mint many local identities for seeding/testing.
TOKENS = {}


def _ident_from_cookie(cookie_header: str) -> dict:
    """LOCAL SEEDING: pick the identity from a `mock_user` cookie so a browser
    can log in as alice/bob/carol/... without restarting the mock. No cookie ->
    the fixed default identity, so existing single-user flows are unchanged."""
    jar = {}
    for part in (cookie_header or "").split(";"):
        if "=" in part:
            k, v = part.strip().split("=", 1)
            jar[k] = v
    u = jar.get("mock_user")
    if u:
        return {
            "sub": "mock-" + u,
            "email": u + "@example.test",
            "username": u,
            "name": u.replace("-", " ").title(),
            "role": "subscriber",
        }
    return {
        "sub": SUB, "email": EMAIL, "username": USERNAME, "name": USERNAME,
        "role": ROLE,
    }


class Handler(http.server.BaseHTTPRequestHandler):
    def _json(self, obj, status=200):
        body = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        parsed = urllib.parse.urlparse(self.path)
        if parsed.path.startswith("/.well-known/openid-configuration"):
            self._json(DISCOVERY)
        elif parsed.path.startswith("/jwks"):
            self._json(JWKS)
        elif parsed.path == "/authorize":
            self._authorize(urllib.parse.parse_qs(parsed.query))
        elif parsed.path == "/userinfo":
            # The callback fetches /userinfo after the token exchange and rejects
            # the login if its `sub` differs from the id_token's, so this must
            # serve the SAME identity (see routes/bunyip_sso.rs). Resolve it from
            # the Bearer access_token bound at /token.
            auth = self.headers.get("Authorization", "")
            token = auth[7:] if auth.startswith("Bearer ") else ""
            ident = TOKENS.get(token) or {
                "sub": SUB, "email": EMAIL, "username": USERNAME, "name": USERNAME,
                "role": ROLE,
            }
            self._json(
                {
                    "sub": ident["sub"],
                    "email": ident["email"],
                    "email_verified": True,
                    "preferred_username": ident["username"],
                    "name": ident["name"],
                }
            )
        else:
            self.send_response(404)
            self.end_headers()

    def do_POST(self):
        parsed = urllib.parse.urlparse(self.path)
        if parsed.path == "/token":
            length = int(self.headers.get("Content-Length") or 0)
            form = urllib.parse.parse_qs(self.rfile.read(length).decode())
            self._token(form)
        else:
            self.send_response(404)
            self.end_headers()

    def _authorize(self, q):
        """No login form, no consent: mint a code and bounce straight back."""
        redirect_uri = (q.get("redirect_uri") or [""])[0]
        if not redirect_uri:
            self._json({"error": "invalid_request"}, status=400)
            return
        code = secrets.token_urlsafe(24)
        CODES[code] = {
            "nonce": (q.get("nonce") or [""])[0],
            "aud": (q.get("client_id") or [""])[0],
            "ident": _ident_from_cookie(self.headers.get("Cookie", "")),
        }
        params = {"code": code}
        state = (q.get("state") or [""])[0]
        if state:
            params["state"] = state
        sep = "&" if urllib.parse.urlparse(redirect_uri).query else "?"
        self.send_response(302)
        self.send_header(
            "Location", redirect_uri + sep + urllib.parse.urlencode(params)
        )
        self.send_header("Content-Length", "0")
        self.end_headers()

    def _token(self, form):
        code = (form.get("code") or [""])[0]
        # Single-use: pop so a replayed code fails, as a real OP would.
        entry = CODES.pop(code, None)
        if entry is None:
            self._json({"error": "invalid_grant"}, status=400)
            return
        # The client id may arrive in the body or via Basic auth; fall back to
        # whatever /authorize saw so the `aud` claim always matches the client.
        aud = (form.get("client_id") or [""])[0] or entry["aud"]
        ident = entry.get("ident") or {
            "sub": SUB, "email": EMAIL, "username": USERNAME, "name": USERNAME,
            "role": ROLE,
        }
        now = int(time.time())
        claims = {
            "iss": ISSUER,
            "aud": aud,
            "sub": ident["sub"],
            "email": ident["email"],
            "email_verified": True,
            "preferred_username": ident["username"],
            "name": ident["name"],
            "bunyip_role": ident.get("role", ROLE),
            "iat": now,
            "exp": now + 3600,
        }
        if entry["nonce"]:
            claims["nonce"] = entry["nonce"]
        id_token = jwt.encode(
            claims,
            PRIVATE_PEM,
            algorithm="EdDSA",
            headers={"kid": KID},
        )
        access_token = secrets.token_urlsafe(24)
        # Bind the identity to the access_token so /userinfo (called without the
        # cookie) returns the SAME sub the id_token carried.
        TOKENS[access_token] = ident
        self._json(
            {
                "access_token": access_token,
                "token_type": "Bearer",
                "expires_in": 3600,
                "id_token": id_token,
            }
        )

    def log_message(self, *args):
        pass


if __name__ == "__main__":
    http.server.HTTPServer(("0.0.0.0", 9000), Handler).serve_forever()
