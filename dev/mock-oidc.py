#!/usr/bin/env python3
"""Minimal mock OIDC OP for LOCAL DEV ONLY.

Since the LC-22 cutover, lets-chat-server refuses to start unless a Bunyip SSO
OP answers discovery + JWKS at boot (server/src/main.rs, server/src/oidc). That
makes `just dev-web-local` unbootable without the full bunyip dev-sso stack,
which blocks pure-UI work (e.g. screenshotting the theme gallery).

This server answers just enough for BunyipSsoClient::initialize() to succeed:
  GET /.well-known/openid-configuration  -> discovery doc (issuer must match)
  GET /jwks.json                         -> one Ed25519 (OKP/EdDSA) key

It CANNOT authenticate a real login (the key is a dummy 32 zero bytes and there
is no authorize/token flow), so only unauthenticated debug routes render, most
usefully GET /dev/theme-gallery. NEVER use this outside local dev.
"""
import http.server
import json

ISSUER = "http://mock-oidc:9000"

DISCOVERY = {
    "issuer": ISSUER,
    "authorization_endpoint": ISSUER + "/authorize",
    "token_endpoint": ISSUER + "/token",
    "userinfo_endpoint": ISSUER + "/userinfo",
    "jwks_uri": ISSUER + "/jwks.json",
}

# One Ed25519 JWK. x = base64url-nopad of 32 zero bytes (43 'A's) -> decodes to
# exactly 32 bytes, satisfying fetch_jwks (kty=OKP, crv=Ed25519, alg=EdDSA).
JWKS = {
    "keys": [
        {
            "kty": "OKP",
            "crv": "Ed25519",
            "alg": "EdDSA",
            "use": "sig",
            "kid": "mock-dev-1",
            "x": "A" * 43,
        }
    ]
}


class Handler(http.server.BaseHTTPRequestHandler):
    def _json(self, obj):
        body = json.dumps(obj).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path.startswith("/.well-known/openid-configuration"):
            self._json(DISCOVERY)
        elif self.path.startswith("/jwks"):
            self._json(JWKS)
        else:
            self.send_response(404)
            self.end_headers()

    def log_message(self, *args):
        pass


if __name__ == "__main__":
    http.server.HTTPServer(("0.0.0.0", 9000), Handler).serve_forever()
