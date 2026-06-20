# Auth Troubleshooting

## Issue 1 — "Missing state cookie" during internal-mode OAuth callback

### Symptom

Clicking "Sign in with Google" redirects to Google, but after completing sign-in Google's callback returns a `400 Bad Request` with the body `Missing state cookie`.

### Root cause

The OAuth callback flow sets two cookies during `/auth/google` (`oauth_state` for CSRF, `oauth_pkce_verifier` for PKCE), then reads them back in `/auth/callback` after Google redirects the user home. If those cookies aren't sent with the callback request, the handler rejects it.

Two attributes were missing from both cookies:

- **`SameSite=Lax`** — not explicitly set, so browsers used their own default. Most modern browsers default to `Lax`, which allows cookies to be sent on top-level cross-site navigations (exactly what an OAuth redirect is). But not all browsers agree, and relying on the default is fragile.
- **`Secure=true`** — not set. On an HTTPS site some browsers quietly drop cookies that lack this flag, or refuse to store them for HTTPS-origin redirects.

### Fix (applied)

`src/auth.rs` — `auth_login`: both OAuth cookies now explicitly set `SameSite=Lax` and `Secure=true`.

`src/auth.rs` — `auth_callback`: added `tracing::warn!` before each early-return error path so the server logs which cookie was missing and what other cookies were present when the error fires.

---

## Issue 2 — Stuck on login page after signing in via Caddy portal (caddy mode)

### Symptom

When `TODO_AUTH_MODE=caddy`, caddy-security handles Google OAuth and sets its own session cookies. After a successful sign-in the user is redirected back to the app but the frontend shows the "Sign in with Google" login page again, with no server errors logged.

### Root cause

The frontend SPA calls `GET /auth/me` on every page load to determine whether the user is authenticated. **In caddy mode, `/auth/me` was never mounted.** The request fell through to the static-file fallback, which served `index.html` with HTTP 200. The frontend's `checkAuth()` tried to parse that as JSON, threw, caught the error, and returned `null` — so the app always showed the login page regardless of Caddy auth state.

The "Sign in with Google" link points to `/auth/google`, which also has no route in caddy mode, so clicking it served `index.html` again → infinite loop.

The caddy middleware logs (`caddy_header_middleware`) also had no logging for the "no email header" path, making it impossible to distinguish "header present, user lookup failed" from "header never arrived."

### How caddy mode is supposed to work

caddy-security sits in front of the app and injects `x-token-user-email: <google-email>` into every forwarded request after the user authenticates. The app's `caddy_header_middleware` reads that header, looks up (or creates) the user in the database, and injects an `AuthUser` extension for handlers to use.

### Fix (applied)

**`src/auth.rs`** — new `caddy_auth_me` handler: reads `x-token-user-email` (or `TODO_DEV_EMAIL` for local dev), looks up the user, and returns the same `{ userId, firstName, lastName }` JSON shape the internal `/auth/me` returns. Logs a warning if neither email source is present.

**`src/main.rs`** — caddy branch: mounts `GET /auth/me → caddy_auth_me`. The `user_repo` extension added at the outer router level is available to it.

**`src/auth.rs`** — `caddy_header_middleware`: added `tracing::warn!` (with the request path) when neither `x-token-user-email` nor `TODO_DEV_EMAIL` is set.

**`src/auth.rs`** — `jwt_auth_middleware`: added `tracing::warn!` for "no token found" and "token present but invalid/expired", both including the request path.

### Local dev in caddy mode (no Caddy in front)

Set `TODO_DEV_EMAIL=you@example.com` in the environment. Both `caddy_auth_me` and `caddy_header_middleware` check this variable first, so all requests are treated as authenticated with that email without needing the header.
