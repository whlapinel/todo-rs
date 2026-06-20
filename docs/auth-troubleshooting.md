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

## For self-education about the modern OIDC flow

The following was provided by Gemini. I was confused about PKCE until I read this.

Here is the complete, modern OpenID Connect (OIDC) flow using the Authorization Code Flow with PKCE, which is the current security standard for all modern web and mobile applications. [1, 2] 
------------------------------
## Step 1: Pre-Flight (Local Generation)
Before any network requests happen, the Client Application (your app) generates two values in memory:

   1. Code Verifier: A high-entropy, random cryptographic string.
   2. Code Challenge: The SHA-256 hash of the verifier, URL-encoded (Base64URL(SHA256(verifier))). [3] 

------------------------------
## Step 2: The Authorization Request
The Client App redirects the user's browser to the Identity Provider (IdP) authorization endpoint. [4] 

* How it is sent: HTTP GET (Browser redirect)
* Parameters Sent:
* response_type=code (Requests an authorization code)
   * client_id=[YOUR_CLIENT_ID] (Identifies your app)
   * redirect_uri=[YOUR_CALLBACK_URL] (Where the IdP sends the user back)
   * scope=openid profile email (Must include openid to trigger OIDC; requests user data)
   * state=[RANDOM_STRING] (Anti-CSRF token generated by client)
   * code_challenge=[THE_CHALLENGE] (The hashed PKCE string)
   * code_challenge_method=S256 (Tells the IdP that SHA-256 was used) [5, 6, 7] 

------------------------------
## Step 3: User Authentication & Consent
The IdP presents the login page to the user.

   1. The user enters their credentials.
   2. The user approves the permissions (scopes) requested by the app.
   3. Note: This interaction happens entirely between the user and the IdP; the Client App cannot see these credentials. [8] 

------------------------------
## Step 4: The Authorization Response
Upon successful login, the IdP sends the browser back to the Client App’s callback URL.

* How it is sent: HTTP GET (Browser redirect to the redirect_uri)
* Parameters Sent:
* code=[TEMPORARY_AUTH_CODE] (Short-lived, single-use code)
   * state=[RANDOM_STRING] (Must match the state string sent in Step 2)

------------------------------
## Step 5: The Token Exchange Request
The Client App extracts the code from the URL. It now makes a direct, back-channel server-to-server request to the IdP's token endpoint.

* How it is sent: HTTP POST
* Content-Type: application/x-www-form-urlencoded
* Body Parameters Sent:
* grant_type=authorization_code (Specifies the flow type)
   * client_id=[YOUR_CLIENT_ID] (Identifies your app)
   * code=[TEMPORARY_AUTH_CODE] (The code received in Step 4)
   * redirect_uri=[YOUR_CALLBACK_URL] (Must match Step 2 exactly)
   * code_verifier=[THE_VERIFIER] (The raw, unhashed PKCE secret from Step 1) [9] 

------------------------------
## Step 6: Token Verification & Response
The IdP hashes the incoming code_verifier using SHA-256. It verifies that it matches the code_challenge sent in Step 2. If it matches, the IdP returns the tokens. [10, 11] 

* How it is sent: HTTP 200 OK (Direct JSON response) [12] 
* JSON Payload Received:
* id_token: A signed JWT containing user identity information (name, email, sub).
   * access_token: A token used to authenticate requests to protected APIs.
   * refresh_token (Optional): A long-lived token used to get new access tokens without forcing a re-login.
   * expires_in: Token lifetime in seconds.
   * token_type: Usually Bearer. [13, 14, 15, 16, 17] 

------------------------------
## Step 7: Client-Side Validation
The Client App receives the JSON payload and must validate the id_token (JWT) before trusting it:

   1. Validates the digital signature against the IdP's public keys (fetched via its .well-known/openid-configuration endpoint).
   2. Verifies the iss (Issuer) matches the IdP URL.
   3. Verifies the aud (Audience) matches the app’s client_id.
   4. Verifies the exp (Expiration) timestamp has not passed. [18] 

The user is now officially logged in.
------------------------------
