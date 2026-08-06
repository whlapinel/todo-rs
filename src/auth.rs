use std::sync::Arc;

use axum::{
    Extension, Json,
    body::Body,
    extract::Query,
    http::{self, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, Scope, TokenResponse, TokenUrl, basic::BasicClient,
    reqwest::async_http_client,
};
use serde::{Deserialize, Serialize};
use tower_cookies::{Cookie, Cookies, cookie::SameSite};
use tracing::info;

use crate::storage::sqlite::{RepoError, UserRepo};

#[derive(Clone)]
pub struct AppState {
    pub oauth_client: BasicClient,
    pub jwt_secret: String,
    pub user_repo: Arc<dyn UserRepo>,
}

impl AppState {
    pub fn new(
        google_client_id: String,
        google_client_secret: String,
        base_url: String,
        jwt_secret: String,
        user_repo: Arc<dyn UserRepo>,
    ) -> Self {
        let oauth_client = BasicClient::new(
            ClientId::new(google_client_id),
            Some(ClientSecret::new(google_client_secret)),
            AuthUrl::new("https://accounts.google.com/o/oauth2/v2/auth".to_string()).unwrap(),
            Some(TokenUrl::new("https://www.googleapis.com/oauth2/v4/token".to_string()).unwrap()),
        )
        .set_redirect_uri(RedirectUrl::new(format!("{base_url}/auth/callback")).unwrap());

        Self {
            oauth_client,
            jwt_secret,
            user_repo,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: usize,
}

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: String,
}

pub async fn auth_login(
    Extension(state): Extension<Arc<AppState>>,
    cookies: Cookies,
) -> impl IntoResponse {
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let (auth_url, csrf_token) = state
        .oauth_client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new("openid".to_string()))
        .add_scope(Scope::new("email".to_string()))
        .add_scope(Scope::new("profile".to_string()))
        .set_pkce_challenge(pkce_challenge)
        .url();

    let mut state_cookie = Cookie::new("oauth_state", csrf_token.secret().clone());
    state_cookie.set_http_only(true);
    state_cookie.set_path("/");
    state_cookie.set_same_site(Some(SameSite::Lax));
    state_cookie.set_secure(Some(true));
    cookies.add(state_cookie);

    let mut pkce_cookie = Cookie::new("oauth_pkce_verifier", pkce_verifier.secret().clone());
    pkce_cookie.set_http_only(true);
    pkce_cookie.set_path("/");
    pkce_cookie.set_same_site(Some(SameSite::Lax));
    pkce_cookie.set_secure(Some(true));
    cookies.add(pkce_cookie);

    Redirect::to(auth_url.as_str())
}

#[derive(Deserialize)]
pub struct CallbackParams {
    code: String,
    state: String,
}

#[derive(Deserialize)]
struct GoogleUserInfo {
    sub: String,
    email: String,
    given_name: String,
    family_name: Option<String>,
}

pub async fn auth_callback(
    Extension(state): Extension<Arc<AppState>>,
    Query(params): Query<CallbackParams>,
    cookies: Cookies,
) -> Response {
    let stored_state = match cookies.get("oauth_state") {
        Some(c) => c.value().to_string(),
        None => {
            tracing::warn!(
                "auth_callback: oauth_state cookie missing; cookies present: oauth_pkce_verifier={}",
                cookies.get("oauth_pkce_verifier").is_some()
            );
            return (StatusCode::BAD_REQUEST, "Missing state cookie").into_response();
        }
    };
    if stored_state != params.state {
        tracing::warn!("auth_callback: CSRF state mismatch (stored != returned)");
        return (StatusCode::BAD_REQUEST, "State mismatch").into_response();
    }

    let pkce_secret = match cookies.get("oauth_pkce_verifier") {
        Some(c) => c.value().to_string(),
        None => {
            tracing::warn!("auth_callback: oauth_pkce_verifier cookie missing");
            return (StatusCode::BAD_REQUEST, "Missing PKCE cookie").into_response();
        }
    };
    let pkce_verifier = PkceCodeVerifier::new(pkce_secret);

    cookies.remove(Cookie::build("oauth_state", "").path("/").finish());
    cookies.remove(Cookie::build("oauth_pkce_verifier", "").path("/").finish());

    let token = match state
        .oauth_client
        .exchange_code(AuthorizationCode::new(params.code))
        .set_pkce_verifier(pkce_verifier)
        .request_async(async_http_client)
        .await
    {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("Token exchange failed: {e:?}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Token exchange failed").into_response();
        }
    };

    let access_token = token.access_token().secret().to_string();
    let userinfo: GoogleUserInfo = match reqwest::Client::new()
        .get("https://www.googleapis.com/oauth2/v3/userinfo")
        .bearer_auth(&access_token)
        .send()
        .await
        .and_then(|r| r.error_for_status())
    {
        Ok(r) => match r.json().await {
            Ok(u) => u,
            Err(e) => {
                tracing::error!("Failed to parse user info: {e}");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to parse user info",
                )
                    .into_response();
            }
        },
        Err(e) => {
            tracing::error!("Failed to fetch user info: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to fetch user info",
            )
                .into_response();
        }
    };

    let user = match state
        .user_repo
        .get_or_create_by_google_id(
            &userinfo.sub,
            &userinfo.email,
            &userinfo.given_name,
            userinfo.family_name.as_deref().unwrap_or(""),
        )
        .await
    {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("DB error in get_or_create_by_google_id: {e:?}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    let exp = (chrono::Utc::now() + chrono::Duration::days(7)).timestamp() as usize;
    let claims = Claims {
        sub: user.id.clone(),
        exp,
    };
    let jwt = match encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.jwt_secret.as_bytes()),
    ) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("JWT encode error: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to create session",
            )
                .into_response();
        }
    };

    let mut auth_cookie = Cookie::new("todo_auth", jwt);
    auth_cookie.set_http_only(true);
    auth_cookie.set_path("/");
    cookies.add(auth_cookie);

    Redirect::to(&format!("/users/{}", user.id)).into_response()
}

pub async fn auth_logout(cookies: Cookies) -> impl IntoResponse {
    cookies.remove(Cookie::build("todo_auth", "").path("/").finish());
    Redirect::to("/")
}

pub async fn auth_token(Extension(state): Extension<Arc<AppState>>, cookies: Cookies) -> Response {
    let cookie_token = match cookies.get("todo_auth").map(|c| c.value().to_string()) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "not authenticated"})),
            )
                .into_response();
        }
    };

    let claims = match decode::<Claims>(
        &cookie_token,
        &DecodingKey::from_secret(state.jwt_secret.as_bytes()),
        &Validation::default(),
    ) {
        Ok(d) => d.claims,
        Err(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "invalid or expired session"})),
            )
                .into_response();
        }
    };

    let exp = (chrono::Utc::now() + chrono::Duration::days(365)).timestamp() as usize;
    let long_lived = Claims {
        sub: claims.sub,
        exp,
    };
    match encode(
        &Header::default(),
        &long_lived,
        &EncodingKey::from_secret(state.jwt_secret.as_bytes()),
    ) {
        Ok(token) => Json(serde_json::json!({"token": token})).into_response(),
        Err(e) => {
            tracing::error!("JWT encode error: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "failed to create token"})),
            )
                .into_response()
        }
    }
}

pub async fn auth_me(Extension(state): Extension<Arc<AppState>>, cookies: Cookies) -> Response {
    let token = match cookies.get("todo_auth").map(|c| c.value().to_string()) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "not authenticated"})),
            )
                .into_response();
        }
    };

    let claims = match decode::<Claims>(
        &token,
        &DecodingKey::from_secret(state.jwt_secret.as_bytes()),
        &Validation::default(),
    ) {
        Ok(d) => d.claims,
        Err(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "invalid token"})),
            )
                .into_response();
        }
    };

    match state.user_repo.get(&claims.sub).await {
        Ok(user) => Json(serde_json::json!({
            "userId": user.id,
            "firstName": user.first_name,
            "lastName": user.last_name,
        }))
        .into_response(),
        Err(RepoError::NotFound) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "user not found"})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("{e:?}")})),
        )
            .into_response(),
    }
}

pub async fn caddy_auth_me(
    Extension(repo): Extension<Arc<dyn UserRepo>>,
    req: Request<Body>,
) -> Response {
    info!("caddy security injected headers: {:?}", req.headers());
    let dev_email = std::env::var("TODO_DEV_EMAIL").ok();
    let header_email = req
        .headers()
        .get("x-token-user-email")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let header_username = req
        .headers()
        .get("x-token-user-name")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let email = match (dev_email, header_email) {
        (Some(e), _) => e,
        (None, Some(e)) => e,
        (None, None) => {
            tracing::warn!(
                "caddy /auth/me: x-token-user-email header absent and TODO_DEV_EMAIL not set — user not authenticated"
            );
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "not authenticated"})),
            )
                .into_response();
        }
    };

    match repo.get_or_create_by_email(&email, header_username.as_deref()).await {
        Ok(user) => Json(serde_json::json!({
            "userId": user.id,
            "firstName": user.first_name,
            "lastName": user.last_name,
        }))
        .into_response(),
        Err(e) => {
            tracing::error!("caddy /auth/me: failed to resolve user for {email}: {e:?}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub async fn caddy_auth_token(
    Extension(jwt_secret): Extension<Arc<String>>,
    Extension(repo): Extension<Arc<dyn UserRepo>>,
    req: Request<Body>,
) -> Response {
    let dev_email = std::env::var("TODO_DEV_EMAIL").ok();
    let header_email = req
        .headers()
        .get("x-token-user-email")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let header_username = req
        .headers()
        .get("x-token-user-name")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let email = match (dev_email, header_email) {
        (Some(e), _) => e,
        (None, Some(e)) => e,
        (None, None) => {
            tracing::warn!(
                "caddy /auth/token: x-token-user-email header absent and TODO_DEV_EMAIL not set — user not authenticated"
            );
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "not authenticated"})),
            )
                .into_response();
        }
    };

    let user = match repo.get_or_create_by_email(&email, header_username.as_deref()).await {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("caddy /auth/token: failed to resolve user for {email}: {e:?}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let exp = (chrono::Utc::now() + chrono::Duration::days(365)).timestamp() as usize;
    let claims = Claims { sub: user.id, exp };
    match encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(jwt_secret.as_bytes()),
    ) {
        Ok(token) => Json(serde_json::json!({"token": token})).into_response(),
        Err(e) => {
            tracing::error!("JWT encode error: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "failed to create token"})),
            )
                .into_response()
        }
    }
}

pub async fn caddy_header_middleware(
    Extension(jwt_secret): Extension<Arc<String>>,
    mut req: Request<Body>,
    next: Next<Body>,
) -> Response {
    let dev_email = std::env::var("TODO_DEV_EMAIL").ok();
    let header_email = req
        .headers()
        .get("x-token-user-email")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let header_username = req
        .headers()
        .get("x-token-user-name")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let email = match (dev_email, header_email) {
        (Some(e), _) => Some(e),
        (None, Some(e)) => Some(e),
        (None, None) => None,
    };

    // Caddy-security authenticates browser sessions and injects x-token-user-email
    // itself; it never sees CLI/MCP requests bearing our own long-lived JWTs (those
    // are exempted from the portal at the edge — see the Caddyfile). For those, we
    // fall back to verifying the token the same way jwt_auth_middleware does.
    let auth_user = if let Some(email) = email {
        let repo = match req
            .extensions()
            .get::<Arc<dyn UserRepo>>()
            .cloned()
        {
            Some(r) => r,
            None => {
                tracing::error!("UserRepo not found in extensions for caddy middleware");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };
        match repo.get_or_create_by_email(&email, header_username.as_deref()).await {
            Ok(user) => AuthUser { user_id: user.id },
            Err(e) => {
                tracing::error!("Failed to resolve user for email {email}: {e:?}");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
    } else {
        let bearer_token = req
            .headers()
            .get(http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "));

        let claims = bearer_token.and_then(|t| {
            decode::<Claims>(
                t,
                &DecodingKey::from_secret(jwt_secret.as_bytes()),
                &Validation::default(),
            )
            .ok()
        });

        match claims {
            Some(data) => AuthUser {
                user_id: data.claims.sub,
            },
            None => {
                tracing::warn!(
                    path = %req.uri().path(),
                    "caddy_header_middleware: no x-token-user-email header, TODO_DEV_EMAIL unset, and no valid Bearer token"
                );
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({"error": "authentication required"})),
                )
                    .into_response();
            }
        }
    };

    req.extensions_mut().insert(auth_user);
    next.run(req).await
}

pub async fn jwt_auth_middleware(
    Extension(state): Extension<Arc<AppState>>,
    mut req: Request<Body>,
    next: Next<Body>,
) -> Response {
    let token = req
        .headers()
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").map(|t| t.to_string()))
        .or_else(|| {
            req.headers()
                .get(http::header::COOKIE)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| {
                    s.split(';').find_map(|part| {
                        let part = part.trim();
                        part.strip_prefix("todo_auth=").map(|v| v.to_string())
                    })
                })
        });

    match token {
        Some(t) => match decode::<Claims>(
            &t,
            &DecodingKey::from_secret(state.jwt_secret.as_bytes()),
            &Validation::default(),
        ) {
            Ok(data) => {
                req.extensions_mut().insert(AuthUser {
                    user_id: data.claims.sub,
                });
                next.run(req).await
            }
            Err(e) => {
                tracing::warn!(path = %req.uri().path(), error = %e, "jwt_auth_middleware: token present but invalid or expired");
                (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({"error": "invalid or expired token"})),
                )
                    .into_response()
            }
        },
        None => {
            tracing::warn!(path = %req.uri().path(), "jwt_auth_middleware: no Bearer token and no todo_auth cookie");
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "authentication required"})),
            )
                .into_response()
        }
    }
}

/// Same identity check as `jwt_auth_middleware` (Bearer header or `todo_auth` cookie), but
/// redirects to the Google login flow instead of returning a JSON 401 — for the
/// browser-facing `/web/*` page routes, where a bare 401 body is the wrong UX for a
/// signed-out visitor. `/api/*` keeps `jwt_auth_middleware` unchanged (CLI/MCP clients want
/// a 401, not a redirect).
pub async fn web_auth_middleware(
    Extension(state): Extension<Arc<AppState>>,
    mut req: Request<Body>,
    next: Next<Body>,
) -> Response {
    let token = req
        .headers()
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").map(|t| t.to_string()))
        .or_else(|| {
            req.headers()
                .get(http::header::COOKIE)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| {
                    s.split(';').find_map(|part| {
                        let part = part.trim();
                        part.strip_prefix("todo_auth=").map(|v| v.to_string())
                    })
                })
        });

    let claims = token.and_then(|t| {
        decode::<Claims>(
            &t,
            &DecodingKey::from_secret(state.jwt_secret.as_bytes()),
            &Validation::default(),
        )
        .ok()
    });

    match claims {
        Some(data) => {
            req.extensions_mut().insert(AuthUser {
                user_id: data.claims.sub,
            });
            next.run(req).await
        }
        None => {
            tracing::info!(path = %req.uri().path(), "web_auth_middleware: no valid session, redirecting to login");
            Redirect::to("/auth/google").into_response()
        }
    }
}
